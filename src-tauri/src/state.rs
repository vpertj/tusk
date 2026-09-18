// Tusk — 应用共享状态（连接池 + 性能缓存）
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_postgres::Client;

use crate::models::ConnConfig;

#[derive(Clone)]
pub struct ConnEntry {
    /// PostgreSQL 连接（db_type == "postgres" 时 Some）
    pub client: Option<Arc<Client>>,
    pub cfg: ConnConfig,
    /// SQLite 连接（db_type == "sqlite" 时 Some）
    pub sqlite: Option<Arc<Mutex<rusqlite::Connection>>>,
}

impl ConnEntry {
    pub fn pg_client(&self) -> Result<Arc<Client>, String> {
        self.client
            .clone()
            .ok_or_else(|| "该连接不是 PostgreSQL".to_string())
    }
}

pub struct AppState {
    pub conns: Mutex<HashMap<String, ConnEntry>>,
    /// 按目标库复用的客户端：key = (conn_id, dbname)
    /// 避免每次翻页/列表都重新走一遍 TCP+TLS+认证握手
    pub db_clients: Mutex<HashMap<(String, String), Arc<Client>>>,
    /// count(*) 缓存：key = conn_id/库/表/筛选，value = (总行数, 缓存时间)
    /// 大表上 count 是全表扫描，翻一次页就算一次会把交互拖到秒级
    pub count_cache: Mutex<HashMap<String, (i64, std::time::Instant)>>,
}
