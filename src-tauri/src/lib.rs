// Tusk — PostgreSQL 客户端核心
// 核心逻辑（连接、查询、类型转换）与 Tauri command 解耦，便于测试复用

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::Mutex;
use tokio_postgres::{Client, Row};

#[derive(Clone)]
struct ConnEntry {
    client: Arc<Client>,
    cfg: ConnConfig,
}

struct AppState {
    conns: Mutex<HashMap<String, ConnEntry>>,
}

#[derive(Serialize, Debug)]
struct ColumnInfo {
    name: String,
    type_name: String,
}

#[derive(Serialize, Debug)]
struct QueryResult {
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<serde_json::Value>>,
    rows_affected: Option<u64>,
    message: Option<String>,
}

#[derive(Serialize)]
struct ConnectionInfo {
    id: String,
    version: String,
}

/// 连接配置（与 UI 表单字段一一对应）
#[derive(Debug, Clone)]
struct ConnConfig {
    host: String,
    port: u16,
    user: String,
    password: String,
    dbname: String,
}

/// 核心：建立 PostgreSQL 连接，返回可跨任务共享的 client 与服务器版本
async fn open_connection(cfg: &ConnConfig) -> Result<(Arc<Client>, String), String> {
    // libpq conninfo：空 password 字段会破坏解析，为空时省略，非空时加引号
    let mut conn_str = format!(
        "host={} port={} user={} dbname={}",
        cfg.host, cfg.port, cfg.user, cfg.dbname
    );
    if !cfg.password.is_empty() {
        let pw = cfg.password.replace('\'', "\\'");
        conn_str = format!("{conn_str} password='{pw}'");
    }
    let (client, connection) =
        tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
            .await
            .map_err(|e| format!("连接失败: {e}"))?;

    // connection task 必须在后台驱动，否则查询不工作
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("[tusk] postgres connection error: {e}");
        }
    });

    let version: String = client
        .query_one("SELECT version()", &[])
        .await
        .map_err(|e| format!("查询版本失败: {e}"))?
        .get(0);

    Ok((Arc::new(client), version))
}

/// 核心：执行 SQL。查询语句返回行列数据，非查询语句返回影响行数。
async fn run_query(client: &Client, sql: &str) -> Result<QueryResult, String> {
    let stmt = client
        .prepare(sql)
        .await
        .map_err(|e| format!("SQL 预编译失败: {e}"))?;

    if stmt.columns().is_empty() {
        // DDL / DML：返回影响行数
        let affected = client
            .execute(&stmt, &[])
            .await
            .map_err(|e| format!("执行失败: {e}"))?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            rows_affected: Some(affected),
            message: None,
        })
    } else {
        let rows = client
            .query(&stmt, &[])
            .await
            .map_err(|e| format!("查询失败: {e}"))?;
        let columns: Vec<ColumnInfo> = stmt
            .columns()
            .iter()
            .map(|c| ColumnInfo {
                name: c.name().to_string(),
                type_name: c.type_().name().to_string(),
            })
            .collect();
        let rows: Vec<Vec<serde_json::Value>> =
            rows.iter().map(row_to_json).collect();
        Ok(QueryResult {
            columns,
            rows,
            rows_affected: None,
            message: None,
        })
    }
}

/// 建立连接并缓存，返回连接 id 与服务器版本（Tauri command 入口）
#[tauri::command]
async fn connect(
    state: State<'_, AppState>,
    host: String,
    port: u16,
    user: String,
    password: String,
    dbname: String,
) -> Result<ConnectionInfo, String> {
    let cfg = ConnConfig {
        host,
        port,
        user,
        password,
        dbname,
    };
    let (client, version) = open_connection(&cfg).await?;

    let id = format!(
        "conn-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    state.conns.lock().await.insert(
        id.clone(),
        ConnEntry {
            client,
            cfg,
        },
    );
    Ok(ConnectionInfo { id, version })
}

/// 断开连接（Tauri command 入口）
#[tauri::command]
async fn disconnect(state: State<'_, AppState>, conn_id: String) -> Result<(), String> {
    state
        .conns
        .lock()
        .await
        .remove(&conn_id)
        .ok_or("连接不存在")?;
    Ok(())
}

/// 拆分 SQL 为多条语句。
/// 支持：单引号/双引号字符串（含 '' 转义）、$tag$ 美元引用、-- 行注释、/* */ 块注释
/// 顶层分号处拆分，空语句忽略。拆出的语句保留原始内容（含注释），trim 后返回。
fn split_statements(sql: &str) -> Vec<String> {
    let b: Vec<char> = sql.chars().collect();
    let n = b.len();
    let mut stmts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;

    while i < n {
        let c = b[i];

        // 行注释：-- 到行尾（含分号）
        if c == '-' && i + 1 < n && b[i + 1] == '-' {
            cur.push(c);
            cur.push('-');
            i += 2;
            while i < n && b[i] != '\n' {
                cur.push(b[i]);
                i += 1;
            }
            continue;
        }

        // 块注释：/* ... */
        if c == '/' && i + 1 < n && b[i + 1] == '*' {
            cur.push(c);
            cur.push('*');
            i += 2;
            while i + 1 < n && !(b[i] == '*' && b[i + 1] == '/') {
                cur.push(b[i]);
                i += 1;
            }
            if i + 1 < n {
                cur.push('*');
                cur.push('/');
                i += 2;
            }
            continue;
        }

        // 字符串引用（含 '' 转义）
        if c == '\'' || c == '"' {
            let q = c;
            cur.push(q);
            i += 1;
            while i < n {
                if b[i] == q {
                    if i + 1 < n && b[i + 1] == q {
                        cur.push(q);
                        cur.push(q);
                        i += 2;
                        continue;
                    }
                    cur.push(q);
                    i += 1;
                    break;
                }
                cur.push(b[i]);
                i += 1;
            }
            continue;
        }

        // 美元引用：$tag$ ... $tag$
        if c == '$' {
            let mut j = i + 1;
            while j < n && (b[j].is_alphanumeric() || b[j] == '_') {
                j += 1;
            }
            if j < n && b[j] == '$' {
                let close: String = b[i..=j].iter().collect(); // $tag$
                // 从开始标记之后查找结束标记（k 从 i + close.len() 起步）
                let mut k = i + close.len();
                let mut found = false;
                while k + close.len() <= n {
                    let seg: String = b[k..k + close.len()].iter().collect();
                    if seg == close {
                        cur.push_str(&b[i..k + close.len()].iter().collect::<String>());
                        i = k + close.len();
                        found = true;
                        break;
                    }
                    k += 1;
                }
                if !found {
                    // 未闭合的美元引用：原样推入剩余部分，交给 PostgreSQL 报错
                    cur.push_str(&b[i..].iter().collect::<String>());
                    i = n;
                }
                continue;
            }
            cur.push(c);
            i += 1;
            continue;
        }

        // 语句分隔
        if c == ';' {
            let t = cur.trim();
            if !t.is_empty() {
                stmts.push(t.to_string());
            }
            cur.clear();
            i += 1;
            continue;
        }

        cur.push(c);
        i += 1;
    }

    let t = cur.trim();
    if !t.is_empty() {
        stmts.push(t.to_string());
    }
    stmts
}

#[derive(Serialize, Debug)]
struct MultiResult {
    results: Vec<QueryResult>,
}

/// 核心：逐条执行多条 SQL（拆分后循环调用 run_query）
async fn run_query_multi(client: &Client, sql: &str) -> Result<MultiResult, String> {
    let stmts = split_statements(sql);
    if stmts.is_empty() {
        return Err("没有可执行的 SQL 语句".into());
    }
    let mut results = Vec::with_capacity(stmts.len());
    for s in &stmts {
        results.push(run_query(client, s).await?);
    }
    Ok(MultiResult { results })
}

/// 执行 SQL（Tauri command 入口，支持多语句）
#[tauri::command]
async fn query(
    state: State<'_, AppState>,
    conn_id: String,
    sql: String,
) -> Result<MultiResult, String> {
    let entry = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).cloned().ok_or("连接不存在或已断开")?
    };
    run_query_multi(&entry.client, &sql).await
}

/// 列出连接对应集群的数据库（Tauri command 入口）
#[tauri::command]
async fn list_databases(state: State<'_, AppState>, conn_id: String) -> Result<Vec<DatabaseInfo>, String> {
    let entry = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).cloned().ok_or("连接不存在或已断开")?
    };
    list_databases_core(&entry.client).await
}

/// 列出目标库的表（Tauri command 入口，跨库查询按目标库开临时连接）
#[tauri::command]
async fn list_tables(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
) -> Result<Vec<TableInfo>, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    list_tables_core(&cfg, &dbname).await
}

/// 列出表的字段（Tauri command 入口）
#[tauri::command]
async fn list_columns(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
) -> Result<Vec<SchemaColumn>, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    list_columns_core(&cfg, &dbname, &table).await
}

#[derive(Serialize, Debug)]
struct DatabaseInfo {
    name: String,
}

#[derive(Serialize, Debug)]
struct TableInfo {
    name: String,
}

#[derive(Serialize, Debug)]
struct SchemaColumn {
    name: String,
    type_name: String,
    is_nullable: String,
    default: Option<String>,
    is_pk: bool,
}

/// 核心：列出集群内数据库（pg_database 集群级，当前连接即可查）
async fn list_databases_core(client: &Client) -> Result<Vec<DatabaseInfo>, String> {
    let rows = client
        .query(
            "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname",
            &[],
        )
        .await
        .map_err(|e| format!("查询数据库列表失败: {e}"))?;
    Ok(rows
        .iter()
        .map(|r| DatabaseInfo { name: r.get(0) })
        .collect())
}

/// 核心：列出目标库 public schema 下的表
/// （PostgreSQL 连接绑定单库，需按目标库临时开连接）
async fn list_tables_core(cfg: &ConnConfig, dbname: &str) -> Result<Vec<TableInfo>, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let rows = client
        .query(
            "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename",
            &[],
        )
        .await
        .map_err(|e| format!("查询表列表失败: {e}"))?;
    Ok(rows
        .iter()
        .map(|r| TableInfo { name: r.get(0) })
        .collect())
}

/// 核心：列出表的字段信息（类型/可空/默认值/主键标记）
async fn list_columns_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
) -> Result<Vec<SchemaColumn>, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;

    let rows = client
        .query(
            "SELECT column_name, data_type, is_nullable, column_default
             FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1
             ORDER BY ordinal_position",
            &[&table],
        )
        .await
        .map_err(|e| format!("查询字段失败: {e}"))?;

    let pk_rows = client
        .query(
            "SELECT kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             WHERE tc.constraint_type = 'PRIMARY KEY'
               AND tc.table_schema = 'public' AND tc.table_name = $1",
            &[&table],
        )
        .await
        .map_err(|e| format!("查询主键失败: {e}"))?;
    let pks: std::collections::HashSet<String> =
        pk_rows.iter().map(|r| r.get::<_, String>(0)).collect();

    Ok(rows
        .iter()
        .map(|r| SchemaColumn {
            name: r.get(0),
            type_name: r.get(1),
            is_nullable: r.get(2),
            default: r.get(3),
            is_pk: pks.contains(&r.get::<_, String>(0)),
        })
        .collect())
}

#[derive(Serialize, Debug)]
struct TablePage {
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<serde_json::Value>>,
    total: Option<i64>,
}

/// 核心：分页读取表数据（SELECT * + LIMIT/OFFSET + 总行数）
/// 表名来自对象树（合法标识符），做双引号转义防注入
async fn paginate_table_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
    limit: u32,
    offset: u32,
) -> Result<TablePage, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let qtable = format!("\"{}\"", table.replace('"', "\"\""));

    // 按主键排序（无主键表按物理位置 ctid，保证分页稳定）
    // '"表名"'::regclass：单引号内嵌双引号保留大小写（'Test'::regclass 会被折叠成小写）
    let tbl_lit = format!("\"{}\"", table.replace('"', "\"\""));
    let pk_sql = format!(
        "SELECT a.attname FROM pg_constraint c \
         JOIN LATERAL unnest(c.conkey) WITH ORDINALITY AS k(attnum, ord) ON true \
         JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum \
         WHERE c.conrelid = '{tbl_lit}'::regclass AND c.contype = 'p' \
         ORDER BY k.ord"
    );
    let pk_rows = client
        .query(&pk_sql, &[])
        .await
        .map_err(|e| format!("主键查询失败: {e}"))?;
    let pk_cols: Vec<String> = pk_rows.iter().map(|r| r.get::<_, String>(0)).collect();
    let order_by = if pk_cols.is_empty() {
        " ORDER BY ctid".to_string()
    } else {
        let cols: Vec<String> = pk_cols.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect();
        format!(" ORDER BY {}", cols.join(", "))
    };

    let sql = format!("SELECT * FROM {qtable}{order_by} LIMIT $1 OFFSET $2");
    let stmt = client
        .prepare(&sql)
        .await
        .map_err(|e| format!("SQL 预编译失败: {e}"))?;
    let rows = client
        .query(&stmt, &[&(limit as i64), &(offset as i64)])
        .await
        .map_err(|e| format!("查询失败: {e}"))?;
    let columns: Vec<ColumnInfo> = stmt
        .columns()
        .iter()
        .map(|c| ColumnInfo {
            name: c.name().to_string(),
            type_name: c.type_().name().to_string(),
        })
        .collect();
    let rows: Vec<Vec<serde_json::Value>> = rows.iter().map(row_to_json).collect();
    let total: i64 = client
        .query_one(&format!("SELECT count(*) FROM {qtable}"), &[])
        .await
        .map_err(|e| format!("查询总行数失败: {e}"))?
        .get(0);
    Ok(TablePage {
        columns,
        rows,
        total: Some(total),
    })
}

/// 分页读取表数据（Tauri command 入口）
#[tauri::command]
async fn paginate_table(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
    limit: u32,
    offset: u32,
) -> Result<TablePage, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    let res = paginate_table_core(&cfg, &dbname, &table, limit, offset).await;
    match &res {
        Ok(p) => eprintln!(
            "[tusk] paginate_table {dbname}.{table} limit={limit} offset={offset} -> {} 行, total={:?}",
            p.rows.len(),
            p.total
        ),
        Err(e) => eprintln!("[tusk] paginate_table 错误 {dbname}.{table}: {e}"),
    }
    res
}

// ================= 连接管理（配置 JSON + Keychain 密码） =================

fn default_db_type() -> String {
    "postgres".into()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SavedConn {
    #[serde(default = "default_db_type")]
    db_type: String,
    name: String,
    host: String,
    port: u16,
    user: String,
    dbname: String,
}

/// 配置文件路径（测试可用 TUSK_CONNS_DIR 注入目录）
fn conns_file_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("TUSK_CONNS_DIR") {
        return std::path::PathBuf::from(dir).join("connections.json");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join("Library/Application Support/com.tusk.app")
        .join("connections.json")
}

fn load_conns() -> Vec<SavedConn> {
    let path = conns_file_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save_conns(conns: &[SavedConn]) -> Result<(), String> {
    let path = conns_file_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let json = serde_json::to_string_pretty(conns).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写入配置失败: {e}"))
}

/// Keychain：写入密码（service=tusk, account=连接名）
fn keychain_set(account: &str, password: &str) -> Result<(), String> {
    let out = std::process::Command::new("security")
        .args(["add-generic-password", "-U", "-a", "tusk", "-s", account, "-w", password])
        .output()
        .map_err(|e| format!("调用 security 失败: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Keychain 写入失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Keychain：读取密码（无记录返回 None）
fn keychain_get(account: &str) -> Result<Option<String>, String> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-a", "tusk", "-s", account, "-w"])
        .output()
        .map_err(|e| format!("调用 security 失败: {e}"))?;
    if out.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&out.stdout)
                .trim_end_matches('\n')
                .to_string(),
        ))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("could not be found") || err.contains("could not be found in keychain") {
            Ok(None)
        } else {
            Err(format!("Keychain 读取失败: {err}"))
        }
    }
}

/// Keychain：删除密码（不存在时静默成功）
fn keychain_delete(account: &str) -> Result<(), String> {
    let _out = std::process::Command::new("security")
        .args(["delete-generic-password", "-a", "tusk", "-s", account])
        .output()
        .map_err(|e| format!("调用 security 失败: {e}"))?;
    Ok(())
}

/// 核心：保存连接（同名覆盖）。password 为 Some 时写入 Keychain
async fn save_connection_core(conn: &SavedConn, password: Option<&str>) -> Result<(), String> {
    let mut conns = load_conns();
    if let Some(existing) = conns.iter_mut().find(|c| c.name == conn.name) {
        *existing = conn.clone();
    } else {
        conns.push(conn.clone());
    }
    save_conns(&conns)?;
    if let Some(pw) = password {
        keychain_set(&conn.name, pw)?;
    }
    Ok(())
}

/// 核心：列出已保存连接
async fn list_connections_core() -> Result<Vec<SavedConn>, String> {
    Ok(load_conns())
}

/// 核心：删除连接（配置 + Keychain 密码）
async fn delete_connection_core(name: &str) -> Result<(), String> {
    let mut conns = load_conns();
    conns.retain(|c| c.name != name);
    save_conns(&conns)?;
    let _ = keychain_delete(name);
    Ok(())
}

/// 核心：按名称取出连接配置（含 Keychain 密码）
async fn connect_saved_core(name: &str) -> Result<(ConnConfig, String), String> {
    let conns = load_conns();
    let conn = conns
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| format!("连接「{name}」不存在"))?;
    if conn.db_type != "postgres" {
        return Err(format!("暂不支持数据库类型：{}", conn.db_type));
    }
    let password = keychain_get(name)?.unwrap_or_default();
    Ok((
        ConnConfig {
            host: conn.host.clone(),
            port: conn.port,
            user: conn.user.clone(),
            password: password.clone(),
            dbname: conn.dbname.clone(),
        },
        password,
    ))
}

/// 保存连接（Tauri command 入口）
#[tauri::command]
async fn save_connection(
    db_type: String,
    name: String,
    host: String,
    port: u16,
    user: String,
    password: String,
    dbname: String,
) -> Result<(), String> {
    let conn = SavedConn {
        db_type,
        name,
        host,
        port,
        user,
        dbname,
    };
    let pw = if password.trim().is_empty() {
        None
    } else {
        Some(password)
    };
    save_connection_core(&conn, pw.as_deref()).await
}

/// 列出已保存连接（Tauri command 入口）
#[tauri::command]
async fn list_connections() -> Result<Vec<SavedConn>, String> {
    list_connections_core().await
}

/// 删除连接（Tauri command 入口）
#[tauri::command]
async fn delete_connection(name: String) -> Result<(), String> {
    delete_connection_core(&name).await
}

/// 按名称连接（Tauri command 入口，含 Keychain 取密码）
#[tauri::command]
async fn connect_saved(state: State<'_, AppState>, name: String) -> Result<ConnectionInfo, String> {
    let (cfg, _pw) = connect_saved_core(&name).await?;
    let (client, version) = open_connection(&cfg).await?;
    let id = format!(
        "conn-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    state.conns.lock().await.insert(
        id.clone(),
        ConnEntry {
            client,
            cfg,
        },
    );
    Ok(ConnectionInfo { id, version })
}

// ================= 数据编辑（行操作 + CSV 导出） =================

/// 标识符双引号转义（防注入）
fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// 核心：更新单元格。值以文本传输，SQL 侧 CAST 到目标类型；
/// 主键用 ::text 比较（主键值即单元格文本）。
/// value = None 表示置 NULL。
async fn update_cell_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
    pk_cols: Vec<String>,
    pk_vals: Vec<String>,
    col: String,
    col_type: String,
    value: Option<String>,
) -> Result<u64, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;

    let mut sql = format!(
        "UPDATE {} SET {} = ",
        quote_ident(table),
        quote_ident(&col)
    );
    let mut params: Vec<String> = Vec::new();
    match &value {
        Some(v) => {
            // ($1)::text::<type>：强制 $1 按 text 传输，服务器端再转目标类型
            sql.push_str(&format!("($1)::text::{col_type}"));
            params.push(v.clone());
        }
        None => sql.push_str("NULL"),
    }
    let param_offset = if value.is_some() { 1 } else { 0 }; // value 占 $1（NULL 时无参数）
    sql.push_str(" WHERE ");
    for (i, pk) in pk_cols.iter().enumerate() {
        if i > 0 {
            sql.push_str(" AND ");
        }
        sql.push_str(&format!("{}::text = ${}", quote_ident(pk), i + 1 + param_offset));
        params.push(pk_vals[i].clone());
    }

    let params_ref: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|s| s as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
    let stmt = client
        .prepare(&sql)
        .await
        .map_err(|e| format!("SQL 预编译失败: {e}"))?;
    client
        .execute(&stmt, &params_ref)
        .await
        .map_err(|e| format!("更新失败: {e}"))
}

/// 核心：删除行（按主键定位）
async fn delete_row_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
    pk_cols: Vec<String>,
    pk_vals: Vec<String>,
) -> Result<u64, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;

    let mut sql = format!("DELETE FROM {} WHERE ", quote_ident(table));
    let mut params: Vec<String> = Vec::new();
    for (i, pk) in pk_cols.iter().enumerate() {
        if i > 0 {
            sql.push_str(" AND ");
        }
        sql.push_str(&format!("{}::text = ${}", quote_ident(pk), i + 1));
        params.push(pk_vals[i].clone());
    }
    let params_ref: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|s| s as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
    let stmt = client
        .prepare(&sql)
        .await
        .map_err(|e| format!("SQL 预编译失败: {e}"))?;
    client
        .execute(&stmt, &params_ref)
        .await
        .map_err(|e| format!("删除失败: {e}"))
}

/// 核心：插入一行（默认值）。有 id 列时返回新 id，否则返回 0
async fn insert_row_core(cfg: &ConnConfig, dbname: &str, table: &str) -> Result<i32, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let qtable = quote_ident(table);
    let sql = format!("INSERT INTO {qtable} DEFAULT VALUES RETURNING id");
    match client.query_one(&sql, &[]).await {
        Ok(row) => Ok(row.get::<_, i32>(0)),
        Err(_) => {
            // 表没有 id 返回列：退化为无返回插入
            let sql2 = format!("INSERT INTO {qtable} DEFAULT VALUES");
            client
                .execute(&sql2, &[])
                .await
                .map_err(|e| format!("插入失败: {e}"))?;
            Ok(0)
        }
    }
}

/// CSV 字段转义：含逗号/引号/换行时用双引号包裹，引号翻倍
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 核心：导出整表为 CSV 到 ~/Downloads，返回文件路径
async fn export_csv_core(cfg: &ConnConfig, dbname: &str, table: &str) -> Result<String, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let qtable = quote_ident(table);
    let stmt = client
        .prepare(&format!("SELECT * FROM {qtable}"))
        .await
        .map_err(|e| format!("SQL 预编译失败: {e}"))?;
    let rows = client
        .query(&stmt, &[])
        .await
        .map_err(|e| format!("查询失败: {e}"))?;

    let mut csv = String::new();
    // 表头
    let header: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| csv_escape(c.name()))
        .collect();
    csv.push_str(&header.join(","));
    csv.push('\n');
    // 数据行
    for row in &rows {
        let vals: Vec<String> = (0..row.len())
            .map(|i| {
                let v = cell_to_json(row, i);
                match v {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => csv_escape(&s),
                    other => csv_escape(&other.to_string()),
                }
            })
            .collect();
        csv.push_str(&vals.join(","));
        csv.push('\n');
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = format!("{home}/Downloads/tusk-{table}-{ts}.csv");
    std::fs::write(&path, csv).map_err(|e| format!("写文件失败: {e}"))?;
    Ok(path)
}

/// 更新单元格（Tauri command 入口）
#[tauri::command]
async fn update_cell(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
    pk_cols: Vec<String>,
    pk_vals: Vec<String>,
    col: String,
    col_type: String,
    value: Option<String>,
) -> Result<u64, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    let res = update_cell_core(&cfg, &dbname, &table, pk_cols.clone(), pk_vals.clone(), col.clone(), col_type.clone(), value.clone()).await;
    match &res {
        Ok(n) => eprintln!("[tusk] update_cell {dbname}.{table} SET {col} pk={pk_cols:?}{pk_vals:?} value={value:?} -> {n} 行"),
        Err(e) => eprintln!("[tusk] update_cell ERROR {dbname}.{table} SET {col} value={value:?}: {e}"),
    }
    res
}

/// 删除行（Tauri command 入口）
#[tauri::command]
async fn delete_row(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
    pk_cols: Vec<String>,
    pk_vals: Vec<String>,
) -> Result<u64, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    let res = delete_row_core(&cfg, &dbname, &table, pk_cols.clone(), pk_vals.clone()).await;
    match &res {
        Ok(n) => eprintln!("[tusk] delete_row {dbname}.{table} pk={pk_cols:?}{pk_vals:?} -> {n} 行"),
        Err(e) => eprintln!("[tusk] delete_row ERROR {dbname}.{table}: {e}"),
    }
    res
}

/// 插入行（Tauri command 入口）
#[tauri::command]
async fn insert_row(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
) -> Result<i32, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    insert_row_core(&cfg, &dbname, &table).await
}

/// 导出 CSV（Tauri command 入口）
#[tauri::command]
async fn export_csv(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
) -> Result<String, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    export_csv_core(&cfg, &dbname, &table).await
}

/// 核心：EXPLAIN (ANALYZE, BUFFERS)。仅允许 SELECT（ANALYZE 会真实执行，DML 有副作用）
async fn explain_query_core(client: &Client, sql: &str) -> Result<String, String> {
    let stmts = split_statements(sql);
    if stmts.len() != 1 {
        return Err("Explain 仅支持单条语句".into());
    }
    let head = stmts[0].trim_start().to_ascii_uppercase();
    if !head.starts_with("SELECT") {
        return Err("Explain 仅支持 SELECT（ANALYZE 会真实执行，DML 有副作用）".into());
    }
    let explain_sql = format!("EXPLAIN (ANALYZE, BUFFERS) {}", stmts[0]);
    let rows = client
        .query(&explain_sql, &[])
        .await
        .map_err(|e| format!("Explain 失败: {e}"))?;
    let text: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    Ok(text.join("\n"))
}

/// Explain（Tauri command 入口）
#[tauri::command]
async fn explain_query(
    state: State<'_, AppState>,
    conn_id: String,
    sql: String,
) -> Result<String, String> {
    let entry = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).cloned().ok_or("连接不存在或已断开")?
    };
    explain_query_core(&entry.client, &sql).await
}


// ================= 表结构管理（DDL） =================

#[derive(Deserialize, Debug, Clone)]
struct ColumnDef {
    name: String,
    col_type: String,
    nullable: bool,
    default: Option<String>,
    is_pk: bool,
    is_serial: bool,
}

/// 核心：CREATE TABLE（Navicat 表设计器语义：字段网格 → DDL）
async fn create_table_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
    columns: Vec<ColumnDef>,
) -> Result<(), String> {
    if table.trim().is_empty() {
        return Err("表名不能为空".into());
    }
    if columns.is_empty() {
        return Err("至少需要一个字段".into());
    }
    let mut parts: Vec<String> = Vec::new();
    let mut pk_cols: Vec<String> = Vec::new();
    for c in &columns {
        if c.name.trim().is_empty() {
            return Err("字段名不能为空".into());
        }
        if c.col_type.trim().is_empty() {
            return Err(format!("字段「{}」未选择类型", c.name));
        }
        let mut def = format!("{} {}", quote_ident(&c.name), c.col_type.trim());
        if c.is_pk {
            pk_cols.push(c.name.clone());
        }
        // serial 主键隐含 NOT NULL，无需重复
        if !c.nullable && !c.is_serial {
            def.push_str(" NOT NULL");
        }
        if let Some(d) = &c.default {
            let d = d.trim();
            if !d.is_empty() {
                def.push_str(&format!(" DEFAULT {d}"));
            }
        }
        parts.push(def);
    }
    if !pk_cols.is_empty() {
        let pks: Vec<String> = pk_cols.iter().map(|p| quote_ident(p)).collect();
        parts.push(format!("PRIMARY KEY ({})", pks.join(", ")));
    }
    let sql = format!("CREATE TABLE {} ({})", quote_ident(table), parts.join(", "));

    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    client
        .execute(&sql, &[])
        .await
        .map_err(|e| {
            let mut msg = format!("建表失败: {e}");
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                msg.push_str(&format!(" <- {s}"));
                src = s.source();
            }
            eprintln!("[tusk-ddl] SQL: {sql}");
            msg
        })?;
    Ok(())
}

/// 核心：DROP TABLE
async fn drop_table_core(cfg: &ConnConfig, dbname: &str, table: &str) -> Result<(), String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let sql = format!("DROP TABLE {}", quote_ident(table));
    client
        .execute(&sql, &[])
        .await
        .map_err(|e| format!("删除失败: {e}"))?;
    Ok(())
}

/// 建表（Tauri command 入口）
#[tauri::command]
async fn create_table(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
    columns: Vec<ColumnDef>,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    create_table_core(&cfg, &dbname, &table, columns).await
}

/// 删表（Tauri command 入口）
#[tauri::command]
async fn drop_table(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    let res = drop_table_core(&cfg, &dbname, &table).await;
    match &res {
        Ok(()) => eprintln!("[tusk] drop_table {dbname}.{table} 成功"),
        Err(e) => eprintln!("[tusk] drop_table ERROR {dbname}.{table}: {e}"),
    }
    res
}

/// numeric 列的字符串包装：postgres-types 未实现 numeric 的 FromSql，
/// 这里自定义解码（PostgreSQL wire format，精度无损）
struct NumericString(String);

impl<'a> tokio_postgres::types::FromSql<'a> for NumericString {
    fn from_sql(
        ty: &tokio_postgres::types::Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        if *ty != tokio_postgres::types::Type::NUMERIC {
            return Err(format!("类型不是 numeric: {ty}").into());
        }
        Ok(NumericString(numeric_to_string(raw)?))
    }

    fn accepts(ty: &tokio_postgres::types::Type) -> bool {
        *ty == tokio_postgres::types::Type::NUMERIC
    }
}

/// 解码 PostgreSQL numeric 二进制格式为十进制字符串
/// 格式：int16 ndigits, int16 weight, uint16 sign, int16 dscale, int16 digits[ndigits]
/// 值 = Σ digits[i] × 10000^(weight-i)，sign: 0x0000 正 / 0x4000 负 / 0xC000 NaN
fn numeric_to_string(raw: &[u8]) -> Result<String, Box<dyn std::error::Error + Sync + Send>> {
    fn rd_i16(b: &[u8], off: usize) -> i32 {
        i16::from_be_bytes([b[off], b[off + 1]]) as i32
    }
    fn rd_u16(b: &[u8], off: usize) -> u16 {
        u16::from_be_bytes([b[off], b[off + 1]])
    }

    if raw.len() < 8 {
        return Err("numeric 数据过短".into());
    }
    let ndigits = rd_i16(raw, 0);
    let weight = rd_i16(raw, 2);
    let sign = rd_u16(raw, 4);
    let dscale = rd_i16(raw, 6);

    if sign == 0xC000 {
        return Ok("NaN".into());
    }
    if ndigits < 0 || raw.len() < 8 + ndigits as usize * 2 {
        return Err("numeric 数据长度不符".into());
    }

    let digits: Vec<u16> = (0..ndigits).map(|i| rd_u16(raw, 8 + i as usize * 2)).collect();

    // ---- 整数部分 ----
    let mut int_part = String::new();
    if weight >= 0 {
        let int_groups = (weight + 1).min(ndigits) as usize;
        for i in 0..int_groups {
            if i == 0 {
                int_part.push_str(&digits[i].to_string());
            } else {
                int_part.push_str(&format!("{:04}", digits[i]));
            }
        }
        // 整数组不够时补零（超大整数）
        for _ in 0..((weight + 1 - ndigits).max(0) as usize) {
            int_part.push_str("0000");
        }
        let trimmed = int_part.trim_start_matches('0');
        int_part = if trimmed.is_empty() { "0".into() } else { trimmed.into() };
    } else {
        int_part.push('0');
    }

    // ---- 小数部分 ----
    let mut frac_part = String::new();
    let frac_start = (weight + 1).max(0) as usize;
    if frac_start < digits.len() {
        // weight < 0 时，第一个 digit 组在小数点后更远的位置，需要补零组
        if weight < 0 {
            for _ in 0..(-weight - 1) as usize {
                frac_part.push_str("0000");
            }
        }
        for i in frac_start..digits.len() {
            frac_part.push_str(&format!("{:04}", digits[i]));
        }
    }
    if frac_part.len() > dscale.max(0) as usize {
        frac_part.truncate(dscale.max(0) as usize);
    }
    while frac_part.len() < dscale.max(0) as usize {
        frac_part.push('0');
    }

    let mut s = int_part;
    if !frac_part.is_empty() {
        s.push('.');
        s.push_str(&frac_part);
    }
    if sign == 0x4000 && s != "0" {
        s.insert(0, '-');
    }
    Ok(s)
}

/// 把 PostgreSQL 行转成 JSON 数组（按列类型做基础转换）
fn row_to_json(row: &Row) -> Vec<serde_json::Value> {
    (0..row.len())
        .map(|i| cell_to_json(row, i))
        .collect()
}

fn cell_to_json(row: &Row, i: usize) -> serde_json::Value {
    use serde_json::{json, Value};
    let ty = row.columns()[i].type_().name();

    // 尝试读取为 Option 值；NULL 统一返回 null
    macro_rules! get {
        ($t:ty) => {
            row.try_get::<_, Option<$t>>(i).ok().flatten()
        };
    }

    match ty {
        "int2" => get!(i16).map(|v| json!(v)).unwrap_or(Value::Null),
        "int4" => get!(i32).map(|v| json!(v)).unwrap_or(Value::Null),
        "int8" => get!(i64).map(|v| json!(v)).unwrap_or(Value::Null),
        "float4" => get!(f32).map(|v| json!(v)).unwrap_or(Value::Null),
        "float8" => get!(f64).map(|v| json!(v)).unwrap_or(Value::Null),
        "numeric" => get!(NumericString)
            .map(|v| json!(v.0))
            .unwrap_or(Value::Null),
        "bool" => get!(bool).map(|v| json!(v)).unwrap_or(Value::Null),
        "json" | "jsonb" => get!(serde_json::Value).unwrap_or(Value::Null),
        "bytea" => get!(Vec<u8>)
            .map(|b| json!(format!("<bytea {} bytes>", b.len())))
            .unwrap_or(Value::Null),
        "timestamp" => get!(chrono::NaiveDateTime)
            .map(|v| json!(v.format("%Y-%m-%d %H:%M:%S").to_string()))
            .unwrap_or(Value::Null),
        "timestamptz" => get!(chrono::DateTime<chrono::Utc>)
            .map(|v| json!(v.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string()))
            .unwrap_or(Value::Null),
        "date" => get!(chrono::NaiveDate)
            .map(|v| json!(v.to_string()))
            .unwrap_or(Value::Null),
        "time" => get!(chrono::NaiveTime)
            .map(|v| json!(v.to_string()))
            .unwrap_or(Value::Null),
        // 兜底：优先按字符串读，失败再试数字
        _ => {
            if let Some(s) = get!(String) {
                json!(s)
            } else if let Some(n) = get!(i64) {
                json!(n)
            } else if let Some(f) = get!(f64) {
                json!(f)
            } else {
                Value::Null
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg() -> ConnConfig {
        ConnConfig {
            host: "localhost".into(),
            port: 5432,
            user: whoami().into(),
            password: String::new(),
            dbname: "tusk_demo".into(),
        }
    }

    fn whoami() -> String {
        std::env::var("USER").unwrap_or_else(|_| "tianjun".into())
    }

    #[tokio::test]
    async fn test_connect_and_query() {
        let (client, version) = open_connection(&test_cfg()).await.expect("连接失败");
        assert!(version.contains("PostgreSQL"), "版本信息: {version}");

        // 查询：覆盖 int/text/numeric/bool/timestamptz/jsonb/bytea 类型
        let res = run_query(
            &client,
            "SELECT id, name, price, in_stock, created_at, tags, note FROM products ORDER BY id LIMIT 2",
        )
        .await
        .expect("查询失败");
        assert_eq!(res.columns.len(), 7);
        assert_eq!(res.rows.len(), 2);

        // 类型断言
        let r0 = &res.rows[0];
        assert!(matches!(r0[0], serde_json::Value::Number(_)), "id 应为数字: {:?}", r0[0]);
        assert!(matches!(r0[1], serde_json::Value::String(_)), "name 应为字符串");
        assert_eq!(r0[2], serde_json::Value::String("399.00".into()), "numeric 应精度无损转字符串");
        // numeric 更多形态：小数、负数、纯小数
        let nums = run_query(
            &client,
            "SELECT 3.99::numeric, -12345.6789::numeric, 0.00123::numeric, 10000::numeric",
        )
        .await
        .expect("numeric 查询失败");
        assert_eq!(nums.rows[0][0], serde_json::Value::String("3.99".into()));
        assert_eq!(nums.rows[0][1], serde_json::Value::String("-12345.6789".into()));
        assert_eq!(nums.rows[0][2], serde_json::Value::String("0.00123".into()));
        assert_eq!(nums.rows[0][3], serde_json::Value::String("10000".into()));
        assert!(matches!(r0[3], serde_json::Value::Bool(_)), "bool 应为布尔");
        let ts = r0[4].as_str().expect("timestamptz 应为字符串");
        assert_eq!(ts.len(), 19, "timestamptz 应为紧凑格式 YYYY-MM-DD HH:MM:SS: {ts}");
        assert_eq!(ts.chars().nth(10), Some(' '), "时间分隔应为空格: {ts}");
        assert!(r0[5].is_array(), "jsonb 应为数组/对象");
        assert!(r0[6].is_null(), "NULL bytea 应为 null");

        // DML：影响行数
        let upd = run_query(&client, "UPDATE products SET stock = stock + 1 WHERE id = 1")
            .await
            .expect("UPDATE 失败");
        assert_eq!(upd.rows_affected, Some(1));

        // 错误路径：SQL 错误要能返回 Err
        assert!(run_query(&client, "SELECT * FROM no_such_table").await.is_err());
    }

    #[tokio::test]
    async fn test_schema_queries() {
        let cfg = test_cfg();
        let (client, _) = open_connection(&cfg).await.expect("连接失败");

        // 数据库列表（集群级，当前连接可查）
        let dbs = list_databases_core(&client).await.expect("list_databases 失败");
        assert!(dbs.iter().any(|d| d.name == "tusk_demo"), "应列出 tusk_demo");

        // 表列表（连接绑定单库，需要按目标库开新连接）
        let tables = list_tables_core(&cfg, "tusk_demo").await.expect("list_tables 失败");
        assert!(
            tables.iter().any(|t| t.name == "products"),
            "tusk_demo 应含 products 表: {:?}",
            tables
        );

        // 列信息（含主键标记）
        let cols = list_columns_core(&cfg, "tusk_demo", "products")
            .await
            .expect("list_columns 失败");
        let price = cols.iter().find(|c| c.name == "price").expect("应有 price 列");
        assert!(price.type_name.contains("numeric"), "price 类型: {}", price.type_name);
        let id = cols.iter().find(|c| c.name == "id").expect("应有 id 列");
        assert!(id.is_pk, "id 应标记为主键");
        assert!(
            cols.iter().all(|c| !c.is_nullable.is_empty()),
            "每个字段都应有可空标记"
        );
    }

    #[tokio::test]
    async fn test_pagination() {
        let cfg = test_cfg();
        let page1 = paginate_table_core(&cfg, "tusk_demo", "products", 2, 0)
            .await
            .expect("第一页失败");
        assert_eq!(page1.rows.len(), 2, "limit=2 应返回 2 行");
        assert_eq!(page1.total, Some(4), "products 共 4 行");

        let page2 = paginate_table_core(&cfg, "tusk_demo", "products", 2, 2)
            .await
            .expect("第二页失败");
        assert_eq!(page2.rows.len(), 2);

        // 两页数据不重叠（按 id 判断）
        let id1: Vec<i64> = page1.rows.iter().map(|r| r[0].as_i64().unwrap()).collect();
        let id2: Vec<i64> = page2.rows.iter().map(|r| r[0].as_i64().unwrap()).collect();
        assert!(
            id1.iter().all(|x| !id2.contains(x)),
            "两页 id 不应重叠: {id1:?} vs {id2:?}"
        );
        assert_eq!(page1.columns[0].name, "id", "首列应为 id");

        // 每页按主键 id 升序
        let ids: Vec<i64> = page1.rows.iter().map(|r| r[0].as_i64().unwrap()).collect();
        assert!(
            ids.windows(2).all(|w| w[0] < w[1]),
            "第一页应按主键升序，实际: {ids:?}"
        );

        // 模拟用户改值后刷新：更新过的行不应破坏分页顺序
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute("UPDATE products SET name = name WHERE id = 1", &[])
            .await
            .expect("更新失败");
        let page_after = paginate_table_core(&cfg, "tusk_demo", "products", 2, 0)
            .await
            .expect("更新后第一页失败");
        let ids_after: Vec<i64> = page_after
            .rows
            .iter()
            .map(|r| r[0].as_i64().unwrap())
            .collect();
        assert!(
            ids_after.windows(2).all(|w| w[0] < w[1]),
            "更新后仍应按主键升序，实际: {ids_after:?}"
        );
    }

    #[test]
    fn test_split_statements() {
        // 基本多语句
        assert_eq!(
            split_statements("SELECT 1; SELECT 2;"),
            vec!["SELECT 1", "SELECT 2"]
        );
        // 字符串字面量内的分号不拆分
        assert_eq!(
            split_statements("SELECT 'a;b';"),
            vec!["SELECT 'a;b'"]
        );
        // 双引号标识符内的分号
        assert_eq!(
            split_statements("SELECT \"a;b\" FROM t;"),
            vec!["SELECT \"a;b\" FROM t"]
        );
        // 美元引用内的分号
        assert_eq!(
            split_statements("SELECT $$a;b$$;"),
            vec!["SELECT $$a;b$$"]
        );
        assert_eq!(
            split_statements("SELECT $tag$a;b$tag$;"),
            vec!["SELECT $tag$a;b$tag$"]
        );
        // 行注释内的分号
        assert_eq!(
            split_statements("-- c;\nSELECT 1;"),
            vec!["-- c;\nSELECT 1"]
        );
        // 块注释内的分号
        assert_eq!(
            split_statements("/* a; b */ SELECT 1;"),
            vec!["/* a; b */ SELECT 1"]
        );
        // 空语句忽略
        assert_eq!(split_statements(";; SELECT 1 ;;"), vec!["SELECT 1"]);
        // 单引号转义 '' 内的分号
        assert_eq!(
            split_statements("SELECT 'it''s; ok';"),
            vec!["SELECT 'it''s; ok'"]
        );
    }

    #[tokio::test]
    async fn test_multi_statement() {
        let (client, _) = open_connection(&test_cfg()).await.expect("连接失败");

        // 查询 + 字符串内分号
        let multi = run_query_multi(&client, "SELECT 1 AS a; SELECT 'x;y' AS b;")
            .await
            .expect("多语句失败");
        assert_eq!(multi.results.len(), 2);
        assert_eq!(multi.results[0].columns[0].name, "a");
        assert_eq!(
            multi.results[1].rows[0][0],
            serde_json::Value::String("x;y".into())
        );

        // DML + 查询混合
        let multi2 = run_query_multi(
            &client,
            "UPDATE products SET stock = stock WHERE id = 1; SELECT count(*) FROM products;",
        )
        .await
        .expect("混合多语句失败");
        assert_eq!(multi2.results.len(), 2);
        assert_eq!(multi2.results[0].rows_affected, Some(1));
        assert!(multi2.results[1].rows[0][0].is_number());
    }

    #[tokio::test]
    async fn test_connection_store() {
        // 用临时目录隔离配置，避免污染真实配置
        let dir = std::env::temp_dir().join(format!("tusk-conn-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        std::env::set_var("TUSK_CONNS_DIR", &dir);

        let conn = SavedConn {
            db_type: "postgres".into(),
            name: "本地测试库".into(),
            host: "localhost".into(),
            port: 5432,
            user: whoami().into(),
            dbname: "tusk_demo".into(),
        };

        // 保存 → 列表
        save_connection_core(&conn, None).await.expect("保存失败");
        let list = list_connections_core().await.expect("列表失败");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "本地测试库");

        // 同名更新
        let mut conn2 = conn.clone();
        conn2.dbname = "postgres".into();
        save_connection_core(&conn2, None).await.expect("更新失败");
        let list = list_connections_core().await.expect("列表失败");
        assert_eq!(list.len(), 1, "同名应覆盖而非追加");
        assert_eq!(list[0].dbname, "postgres");

        // 从保存的连接取配置并真正连接（本地 trust 无密码）
        let (cfg, _pw) = connect_saved_core("本地测试库").await.expect("取配置失败");
        assert_eq!(cfg.dbname, "postgres");
        let (client, _) = open_connection(&cfg).await.expect("用保存的配置连接失败");
        let one: i32 = client
            .query_one("SELECT 1", &[])
            .await
            .expect("查询失败")
            .get(0);
        assert_eq!(one, 1);

        // 删除 → 列表空
        delete_connection_core("本地测试库").await.expect("删除失败");
        assert!(list_connections_core().await.expect("列表失败").is_empty());

        std::fs::remove_dir_all(&dir).ok();
        std::env::remove_var("TUSK_CONNS_DIR");
    }

    #[tokio::test]
    async fn test_row_operations() {
        let cfg = test_cfg();
        let (client, _) = open_connection(&cfg).await.expect("连接失败");

        // 测试专用表：所有列都有默认值，DEFAULT VALUES 可成功插入
        client
            .execute(
                "CREATE TABLE IF NOT EXISTS tusk_edit_test (
                    id serial PRIMARY KEY,
                    name text NOT NULL DEFAULT 'x',
                    price numeric(10,2) NOT NULL DEFAULT 0,
                    note bytea
                )",
                &[],
            )
            .await
            .expect("建测试表失败");

        // 插入一行（DEFAULT VALUES，serial 主键自动生成）
        let new_id = insert_row_core(&cfg, "tusk_demo", "tusk_edit_test")
            .await
            .expect("插入失败");
        assert!(new_id > 0, "应返回新行 id");

        // 更新该行 name（text 列）
        update_cell_core(
            &cfg,
            "tusk_demo",
            "tusk_edit_test",
            vec!["id".into()],
            vec![new_id.to_string()],
            "name".into(),
            "text".into(),
            Some("测试商品".into()),
        )
        .await
        .expect("更新失败");
        let name: String = client
            .query_one(
                "SELECT name FROM tusk_edit_test WHERE id = $1",
                &[&new_id],
            )
            .await
            .expect("查询失败")
            .get(0);
        assert_eq!(name, "测试商品");

        // 更新 price（numeric 列，text 强转）
        update_cell_core(
            &cfg,
            "tusk_demo",
            "tusk_edit_test",
            vec!["id".into()],
            vec![new_id.to_string()],
            "price".into(),
            "numeric".into(),
            Some("88.50".into()),
        )
        .await
        .expect("numeric 更新失败");
        let price: f64 = client
            .query_one(
                "SELECT price::float8 FROM tusk_edit_test WHERE id = $1",
                &[&new_id],
            )
            .await
            .expect("查询失败")
            .get(0);
        assert_eq!(price, 88.5);

        // 更新为 NULL
        update_cell_core(
            &cfg,
            "tusk_demo",
            "tusk_edit_test",
            vec!["id".into()],
            vec![new_id.to_string()],
            "note".into(),
            "bytea".into(),
            None,
        )
        .await
        .expect("置空失败");
        let note: Option<Vec<u8>> = client
            .query_one(
                "SELECT note FROM tusk_edit_test WHERE id = $1",
                &[&new_id],
            )
            .await
            .expect("查询失败")
            .get(0);
        assert!(note.is_none());

        // 删除该行
        delete_row_core(
            &cfg,
            "tusk_demo",
            "tusk_edit_test",
            vec!["id".into()],
            vec![new_id.to_string()],
        )
        .await
        .expect("删除失败");
        let cnt: i64 = client
            .query_one(
                "SELECT count(*) FROM tusk_edit_test WHERE id = $1",
                &[&new_id],
            )
            .await
            .expect("查询失败")
            .get(0);
        assert_eq!(cnt, 0, "删除后应无此行");
    }

    #[tokio::test]
    async fn test_export_csv() {
        let cfg = test_cfg();
        let path = export_csv_core(&cfg, "tusk_demo", "products")
            .await
            .expect("导出失败");
        assert!(std::path::Path::new(&path).exists(), "CSV 文件应存在: {path}");
        let content = std::fs::read_to_string(&path).expect("读文件失败");
        assert!(
            content.starts_with("id,name,price"),
            "表头应为 id,name,price... 实际: {}",
            content.lines().next().unwrap_or("")
        );
        assert!(content.contains("机械键盘"), "应包含已有数据");
        assert!(content.contains("测试商品") == false, "不应包含已删除数据");
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_explain() {
        let (client, _) = open_connection(&test_cfg()).await.expect("连接失败");
        let plan = explain_query_core(&client, "SELECT * FROM products WHERE id = 1")
            .await
            .expect("Explain 失败");
        assert!(
            plan.contains("Seq Scan") || plan.contains("Index Scan") || plan.contains("Planning"),
            "应有执行计划: {plan}"
        );
        // 非 SELECT 拒绝（EXPLAIN ANALYZE 会真实执行 DML，不做）
        let err = explain_query_core(&client, "UPDATE products SET name = name")
            .await
            .expect_err("应拒绝非 SELECT");
        assert!(err.contains("SELECT"), "错误信息应说明仅支持 SELECT: {err}");
    }

    #[tokio::test]
    async fn test_create_table() {
        let cfg = test_cfg();
        let tname = format!("tusk_ddl_test_{}", std::process::id());

        // 空字段拒绝
        let err = create_table_core(&cfg, "tusk_demo", "empty_table", vec![])
            .await
            .expect_err("空字段应拒绝");
        assert!(err.contains("至少"), "错误提示: {err}");

        // 标准建表：serial 主键 + varchar + numeric + 默认值
        let cols = vec![
            ColumnDef {
                name: "id".into(),
                col_type: "serial".into(),
                nullable: false,
                default: None,
                is_pk: true,
                is_serial: true,
            },
            ColumnDef {
                name: "name".into(),
                col_type: "varchar(100)".into(),
                nullable: false,
                default: None,
                is_pk: false,
                is_serial: false,
            },
            ColumnDef {
                name: "price".into(),
                col_type: "numeric(10,2)".into(),
                nullable: true,
                default: Some("0".into()),
                is_pk: false,
                is_serial: false,
            },
            ColumnDef {
                name: "created_at".into(),
                col_type: "timestamptz".into(),
                nullable: true,
                default: Some("now()".into()),
                is_pk: false,
                is_serial: false,
            },
        ];
        create_table_core(&cfg, "tusk_demo", &tname, cols)
            .await
            .expect("建表失败");

        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        // 4 个字段
        let cnt: i64 = client
            .query_one(
                "SELECT count(*) FROM information_schema.columns WHERE table_name = $1",
                &[&tname],
            )
            .await
            .expect("查字段失败")
            .get(0);
        assert_eq!(cnt, 4, "应有 4 个字段");
        // 主键存在
        let pk: i64 = client
            .query_one(
                "SELECT count(*) FROM information_schema.table_constraints                  WHERE table_name = $1 AND constraint_type = 'PRIMARY KEY'",
                &[&tname],
            )
            .await
            .expect("查主键失败")
            .get(0);
        assert_eq!(pk, 1, "应有主键约束");
        // 自增 + 默认值可插入
        client
            .execute(&format!("INSERT INTO \"{tname}\" (name) VALUES ('hello')"), &[])
            .await
            .expect("插入失败");
        let n: i64 = client
            .query_one(&format!("SELECT count(*) FROM \"{tname}\""), &[])
            .await
            .expect("查数失败")
            .get(0);
        assert_eq!(n, 1, "应能插入 1 行");
        // 重名建表报错
        let err2 = create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![ColumnDef {
                name: "id".into(),
                col_type: "serial".into(),
                nullable: false,
                default: None,
                is_pk: true,
                is_serial: true,
            }],
        )
        .await
        .expect_err("重名应报错");
        assert!(err2.contains("已存在") || err2.to_lowercase().contains("exist"), "错误: {err2}");
        // 清理
        client
            .execute(&format!("DROP TABLE \"{tname}\""), &[])
            .await
            .expect("清理失败");
    }

    #[tokio::test]
    async fn test_drop_table() {
        let cfg = test_cfg();
        let tname = format!("tusk_drop_test_{}", std::process::id());
        // 建表后删除
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![ColumnDef {
                name: "id".into(),
                col_type: "serial".into(),
                nullable: false,
                default: None,
                is_pk: true,
                is_serial: true,
            }],
        )
        .await
        .expect("建表失败");
        drop_table_core(&cfg, "tusk_demo", &tname)
            .await
            .expect("删表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        let exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = $1)",
                &[&tname],
            )
            .await
            .expect("查表失败")
            .get(0);
        assert!(!exists, "表应已删除");
        // 删除不存在的表报错
        let err = drop_table_core(&cfg, "tusk_demo", "no_such_table_xyz")
            .await
            .expect_err("删除不存在表应报错");
        assert!(!err.is_empty());
    }

    #[tokio::test]
    async fn test_create_table_e2e_frontend_shape() {
        // 模拟前端设计器完整传参（含 varchar 长度/numeric 精度/默认值/自增主键）
        let cfg = test_cfg();
        let tname = format!("tusk_e2e_{}", std::process::id());
        let cols = vec![
            ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true },
            ColumnDef { name: "title".into(), col_type: "varchar(255)".into(), nullable: false, default: None, is_pk: false, is_serial: false },
            ColumnDef { name: "amount".into(), col_type: "numeric(12,2)".into(), nullable: true, default: Some("0.00".into()), is_pk: false, is_serial: false },
            ColumnDef { name: "enabled".into(), col_type: "bool".into(), nullable: false, default: Some("true".into()), is_pk: false, is_serial: false },
            ColumnDef { name: "tags".into(), col_type: "jsonb".into(), nullable: true, default: Some("'[]'::jsonb".into()), is_pk: false, is_serial: false },
            ColumnDef { name: "created_at".into(), col_type: "timestamptz".into(), nullable: true, default: Some("now()".into()), is_pk: false, is_serial: false },
        ];
        create_table_core(&cfg, "tusk_demo", &tname, cols).await.expect("建表失败");

        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        // 插入完整数据验证所有默认值/类型
        client
            .execute(
                &format!("INSERT INTO \"{tname}\" (title, amount) VALUES ('订单A', 99.50)"),
                &[],
            )
            .await
            .expect("插入失败");
        let row = client
            .query_one(
                &format!("SELECT title, amount::float8, enabled FROM \"{tname}\""),
                &[],
            )
            .await
            .expect("查询失败");
        let title: String = row.get(0);
        let amount: f64 = row.get::<_, f64>(1);
        let enabled: bool = row.get(2);
        assert_eq!(title, "订单A");
        assert_eq!(amount, 99.5);
        assert!(enabled, "bool 默认值 true 应生效");

        // 分页读取（前端表页签依赖）
        let page = paginate_table_core(&cfg, "tusk_demo", &tname, 10, 0).await.expect("分页失败");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.columns[0].name, "id");
        let id: i64 = page.rows[0][0].as_i64().expect("id 数字");
        assert_eq!(id, 1, "自增主键应从 1 开始");

        // 清理
        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
        let exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = $1)",
                &[&tname],
            )
            .await
            .expect("q")
            .get(0);
        assert!(!exists, "清理后表不应存在");
    }

    #[tokio::test]
    async fn test_pagination_uppercase_table() {
        // 大小写敏感表名（PG 折叠规则：'Test'::regclass 会被折叠成小写）
        let cfg = test_cfg();
        let tname = "TestOrder";
        create_table_core(
            &cfg,
            "tusk_demo",
            tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false },
            ],
        )
        .await
        .expect("建大写表失败");

        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(&format!("INSERT INTO \"{tname}\" (name) VALUES ('x')"), &[])
            .await
            .expect("插入失败");

        // 分页读取（之前主键查询用单引号 regclass，大写表名会失败）
        let page = paginate_table_core(&cfg, "tusk_demo", tname, 10, 0)
            .await
            .expect("大写表分页失败");
        assert_eq!(page.rows.len(), 1);

        // 删除大写表
        drop_table_core(&cfg, "tusk_demo", tname).await.expect("删大写表失败");
        let exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = $1)",
                &[&tname],
            )
            .await
            .expect("q")
            .get(0);
        assert!(!exists, "大写表应已删除");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            conns: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            query,
            list_databases,
            list_tables,
            list_columns,
            paginate_table,
            save_connection,
            list_connections,
            delete_connection,
            connect_saved,
            update_cell,
            delete_row,
            insert_row,
            export_csv,
            explain_query,
            create_table,
            drop_table
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
