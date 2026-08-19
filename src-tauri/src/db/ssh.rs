//! SSH 隧道：跳板机 direct-tcpip 通道 + 本地端口转发
//! 本地监听 127.0.0.1 随机端口，首个连接经 SSH channel 双向拷贝到目标 PG
//! SshTunnel 保持存活期间隧道可用；drop 后监听关闭、会话断开

use crate::models::ConnConfig;
use russh::client;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg};
use std::sync::Arc;
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;

/// 认证处理器：接受任何服务器密钥（首版；host-key 校验可后续加固）
#[derive(Clone)]
struct SshHandler;

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// 隧道句柄：持有本地监听 + 代理任务（会话 handle 归代理任务所有，连接关闭即断开）
/// 必须保活（随 PG connection task drop），否则隧道关闭
pub struct SshTunnel {
    pub local_port: u16,
    _listener: Arc<TcpListener>,
    _proxy: tokio::task::JoinHandle<()>,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        // 通知代理与转发任务退出 → SSH 会话断开 → 连接关闭
        let _ = self.shutdown.send(true);
    }
}

/// 建立隧道：认证（私钥自动探测优先，其次密码）→ 本地监听 → 单连接转发代理
pub async fn open_tunnel(
    cfg: &ConnConfig,
    ssh_password: &str,
) -> Result<SshTunnel, String> {
    let config = Arc::new(client::Config::default());
    let mut session = client::connect(
        config,
        (cfg.ssh_host.as_str(), cfg.ssh_port),
        SshHandler,
    )
    .await
    .map_err(|e| format!("SSH 连接失败（{}:{}）: {e}", cfg.ssh_host, cfg.ssh_port))?;

    // 认证顺序：私钥自动探测 → 密码
    let mut authed = false;
    if let Some(key) = probe_private_key() {
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), Some(HashAlg::Sha256));
        if let Ok(a) = session
            .authenticate_publickey(cfg.ssh_user.as_str(), key)
            .await
        {
            if a.success() {
                authed = true;
            }
        }
    }
    if !authed {
        let a = session
            .authenticate_password(cfg.ssh_user.as_str(), ssh_password)
            .await
            .map_err(|e| format!("SSH 认证失败: {e}"))?;
        if !a.success() {
            return Err("SSH 认证失败：用户或密码/密钥不正确".into());
        }
    }

    // 本地监听随机端口（Arc 共享：proxy 与句柄各持一份，drop 时关监听）
    let listener = Arc::new(
        TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| format!("本地端口监听失败: {e}"))?,
    );
    let local_port = listener
        .local_addr()
        .map_err(|e| format!("获取本地端口失败: {e}"))?
        .port();

    // 转发代理：accept 首个连接 → SSH channel → 双向拷贝（handle 在此 task 内保活）
    // 通过 watch 通道感知句柄 drop，立即退出并断开会话
    let target_host = cfg.host.clone();
    let target_port = cfg.port;
    let proxy_listener = listener.clone();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let proxy = tokio::spawn(async move {
        tokio::select! {
            _ = shutdown_rx.changed() => return,
            accepted = proxy_listener.accept() => {
                let Ok((mut local, _)) = accepted else { return };
                let channel = match session
                    .channel_open_direct_tcpip(target_host, target_port as u32, "127.0.0.1".to_string(), 0)
                    .await
                {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let mut remote = channel.into_stream();
                let mut gone = shutdown_rx.clone();
                tokio::select! {
                    _ = gone.changed() => return,
                    r = copy_bidirectional(&mut local, &mut remote) => { let _ = r; }
                }
            }
        }
    });

    Ok(SshTunnel {
        local_port,
        _listener: listener,
        _proxy: proxy,
        shutdown: shutdown_tx,
    })
}

/// 自动探测 ~/.ssh 下的常用私钥
fn probe_private_key() -> Option<PrivateKey> {
    let home = std::env::var("HOME").ok()?;
    let candidates = [
        "id_ed25519",
        "id_rsa",
        "id_ecdsa",
        "id_ecdsa_sk",
        "id_ed25519_sk",
    ];
    for name in candidates {
        let path = format!("{home}/.ssh/{name}");
        if std::path::Path::new(&path).exists() {
            if let Ok(key) = PrivateKey::read_openssh_file(&path) {
                return Some(key);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::server;
    use russh::server::{Auth, Config as ServerConfig, Session};
    use russh::ChannelId;
    
    use russh::keys::ssh_key::Algorithm;
    use std::collections::HashMap;
    use tokio::sync::Mutex;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::net::tcp::OwnedWriteHalf;

    /// 测试 SSH 服务器：密码 "pw"，direct-tcpip 转发到目标 host:port
    #[derive(Clone)]
    struct TestSshServer {
        targets: Arc<Mutex<HashMap<ChannelId, OwnedWriteHalf>>>,
    }

    impl server::Handler for TestSshServer {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            password: &str,
        ) -> Result<Auth, Self::Error> {
            Ok(if password == "pw" {
                Auth::Accept
            } else {
                Auth::Reject {
                    proceed_with_methods: None,
                    partial_success: false,
                }
            })
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: russh::Channel<russh::server::Msg>,
            host: &str,
            port: u32,
            _origin_addr: &str,
            _origin_port: u32,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            let stream = TcpStream::connect((host, port as u16))
                .await
                .map_err(russh::Error::from)?;
            let (mut target_rx, target_tx) = stream.into_split();
            self.targets.lock().await.insert(channel.id(), target_tx);
            reply.accept().await;
            // 目标 → 客户端：目标流数据灌给 channel 写半
            let chan_tx = channel.split().1;
            tokio::spawn(async move {
                let _ = chan_tx.data(&mut target_rx).await;
            });
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            // 客户端 → 目标
            if let Some(tx) = self.targets.lock().await.get_mut(&channel) {
                let _ = tx.write_all(data).await;
            }
            Ok(())
        }
    }

    /// 起本地测试 SSH 服务器，返回端口
    async fn spawn_test_ssh_server() -> u16 {
        let mut cfg = ServerConfig::default();
        cfg.keys = vec![PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()];
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut srv = TestSshServer {
            targets: Arc::new(Mutex::new(HashMap::new())),
        };
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = russh::server::run_stream(Arc::new(cfg), stream, srv).await;
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        port
    }

    #[tokio::test]
    async fn test_ssh_tunnel_pg_query() {
        let ssh_port = spawn_test_ssh_server().await;
        let cfg = ConnConfig {
            db_type: "postgres".into(),
            host: "localhost".into(),
            port: 5432,
            user: std::env::var("USER").unwrap_or_else(|_| "tianjun".into()),
            password: String::new(),
            dbname: "tusk_demo".into(),
            path: String::new(),
            ssh_enabled: true,
            ssh_host: "127.0.0.1".into(),
            ssh_port,
            ssh_user: "tester".into(),
            ssh_pass: "pw".into(),
        };
        let tunnel = open_tunnel(&cfg, &cfg.ssh_pass).await.expect("隧道建立失败");
        let conn_str = format!(
            "host=127.0.0.1 port={} user={} dbname={}",
            tunnel.local_port, cfg.user, cfg.dbname
        );
        let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
            .await
            .expect("经隧道连接 PG 失败");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let row = client
            .query_one("SELECT 1 AS n", &[])
            .await
            .expect("经隧道查询失败");
        assert_eq!(row.get::<_, i32>(0), 1, "经 SSH 隧道应能正常查询 PG");
        // 生命周期：tunnel drop 后连接应断开
        drop(tunnel);
        let err = client.query_one("SELECT 1", &[]).await;
        assert!(err.is_err(), "隧道关闭后连接应断开");
    }

    #[tokio::test]
    async fn test_ssh_tunnel_bad_password() {
        let ssh_port = spawn_test_ssh_server().await;
        let cfg = ConnConfig {
            db_type: "postgres".into(),
            host: "localhost".into(),
            port: 5432,
            user: "tester".into(),
            password: String::new(),
            dbname: "tusk_demo".into(),
            path: String::new(),
            ssh_enabled: true,
            ssh_host: "127.0.0.1".into(),
            ssh_port,
            ssh_user: "tester".into(),
            ssh_pass: "wrong".into(),
        };
        let err = open_tunnel(&cfg, &cfg.ssh_pass).await;
        assert!(
            matches!(err, Err(ref m) if m.contains("认证失败")),
            "错误密码应报认证失败"
        );
    }
}
