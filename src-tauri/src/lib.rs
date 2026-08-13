// Tusk — PostgreSQL 客户端核心
// 核心逻辑（连接、查询、类型转换）与 Tauri command 解耦，便于测试复用

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
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

#[derive(Serialize)]
struct ColumnInfo {
    name: String,
    type_name: String,
}

#[derive(Serialize)]
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

/// 执行 SQL（Tauri command 入口）
#[tauri::command]
async fn query(
    state: State<'_, AppState>,
    conn_id: String,
    sql: String,
) -> Result<QueryResult, String> {
    let entry = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).cloned().ok_or("连接不存在或已断开")?
    };
    run_query(&entry.client, &sql).await
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
            list_columns
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
