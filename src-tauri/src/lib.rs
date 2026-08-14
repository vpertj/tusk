// Tusk — PostgreSQL 客户端核心
// 核心逻辑（连接、查询、类型转换）与 Tauri command 解耦，便于测试复用

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::Mutex;
use tokio_postgres::{Client, Row};
use tokio_postgres::types::ToSql;

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
    kind: String, // "table" | "view"
}

#[derive(Serialize, Debug)]
struct SchemaColumn {
    name: String,
    type_name: String,
    is_nullable: String,
    default: Option<String>,
    is_pk: bool,
    comment: Option<String>,
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
            "SELECT c.relname,
               CASE WHEN c.relkind = 'v' THEN 'view' ELSE 'table' END
             FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v')
             ORDER BY c.relname",
            &[],
        )
        .await
        .map_err(|e| format!("查询表列表失败: {e}"))?;
    Ok(rows
        .iter()
        .map(|r| TableInfo {
            name: r.get(0),
            kind: r.get(1),
        })
        .collect())
}

/// 核心：列出表的索引（排除主键索引，主键已在结构里显示）
#[derive(Serialize, Debug)]
struct IndexInfo {
    name: String,
    columns: String,
    is_unique: bool,
}

async fn list_indexes_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
) -> Result<Vec<IndexInfo>, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    // relname 是裸表名（不需要引号包裹），转义单引号防注入
    let tbl_lit = table.replace('\'', "''");
    let sql = format!(
        "SELECT i.relname,
                COALESCE((SELECT string_agg(a.attname, ', ' ORDER BY k.ord)
                          FROM unnest(ix.indkey) WITH ORDINALITY AS k(attnum, ord)
                          JOIN pg_attribute a ON a.attrelid = ix.indrelid AND a.attnum = k.attnum
                          WHERE k.attnum > 0), ''),
                ix.indisunique
         FROM pg_index ix
         JOIN pg_class i ON i.oid = ix.indexrelid
         JOIN pg_class t ON t.oid = ix.indrelid
         JOIN pg_namespace n ON n.oid = t.relnamespace
         WHERE t.relname = '{tbl_lit}' AND n.nspname = 'public' AND NOT ix.indisprimary
         ORDER BY i.relname"
    );
    let rows = client
        .query(&sql, &[])
        .await
        .map_err(|e| format!("查询索引失败: {e}"))?;
    Ok(rows
        .iter()
        .map(|r| IndexInfo {
            name: r.get(0),
            columns: r.get(1),
            is_unique: r.get(2),
        })
        .collect())
}

#[tauri::command]
async fn list_indexes(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
) -> Result<Vec<IndexInfo>, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    list_indexes_core(&cfg, &dbname, &table).await
}

/// 生成 COMMENT ON 语句（列 + 表），单引号转义
fn comment_statements(
    qtable: &str,
    table_comment: Option<&str>,
    cols: &[ColumnDef],
) -> Vec<String> {
    let mut stmts: Vec<String> = Vec::new();
    if let Some(c) = table_comment {
        if !c.trim().is_empty() {
            stmts.push(format!(
                "COMMENT ON TABLE {qtable} IS '{}'",
                c.replace('\'', "''")
            ));
        }
    }
    for col in cols {
        if let Some(c) = &col.comment {
            if !c.trim().is_empty() {
                let qcol = quote_ident(&col.name);
                stmts.push(format!(
                    "COMMENT ON COLUMN {qtable}.{qcol} IS '{}'",
                    c.replace('\'', "''")
                ));
            }
        }
    }
    stmts
}

/// 核心：创建视图（只允许 SELECT，防注入）
async fn create_view_core(
    cfg: &ConnConfig,
    dbname: &str,
    view_name: &str,
    select_sql: &str,
) -> Result<(), String> {
    if view_name.trim().is_empty() {
        return Err("视图名不能为空".into());
    }
    let head = strip_leading_comments(select_sql).to_ascii_uppercase();
    if !head.starts_with("SELECT") {
        return Err("视图必须是 SELECT 语句".into());
    }
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let sql = format!("CREATE VIEW {} AS {}", quote_ident(view_name), select_sql);
    client
        .execute(&sql, &[])
        .await
        .map_err(|e| format!("创建视图失败: {e}"))?;
    Ok(())
}

/// 核心：删除视图
async fn drop_view_core(cfg: &ConnConfig, dbname: &str, view_name: &str) -> Result<(), String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let sql = format!("DROP VIEW {}", quote_ident(view_name));
    client
        .execute(&sql, &[])
        .await
        .map_err(|e| format!("删除视图失败: {e}"))?;
    Ok(())
}

#[tauri::command]
async fn create_view(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    view_name: String,
    select_sql: String,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    create_view_core(&cfg, &dbname, &view_name, &select_sql).await
}

#[tauri::command]
async fn drop_view(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    view_name: String,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    drop_view_core(&cfg, &dbname, &view_name).await
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

    let tbl_lit = format!("\"{}\"", table.replace('"', "\"\""));
    let col_sql = format!(
        "SELECT a.attname,
           CASE WHEN a.atttypmod > 0 THEN
             CASE t.typname
               WHEN 'varchar' THEN 'varchar(' || (a.atttypmod - 4) || ')'
               WHEN 'numeric' THEN 'numeric(' || ((a.atttypmod - 4) >> 16) || ',' || ((a.atttypmod - 4) & 65535) || ')'
               ELSE t.typname
             END
           ELSE t.typname END,
           CASE WHEN NOT a.attnotnull THEN 'YES' ELSE 'NO' END,
           pg_get_expr(d.adbin, d.adrelid),
           col_description(a.attrelid, a.attnum)
           FROM pg_attribute a
         JOIN pg_type t ON t.oid = a.atttypid
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE a.attrelid = '{tbl_lit}'::regclass AND a.attnum > 0 AND NOT a.attisdropped
         ORDER BY a.attnum"
    );
    let rows = client
        .query(&col_sql, &[])
        .await
        .map_err(|e| format!("查询字段失败: {e}"))?;

    let pk_sql = format!(
        "SELECT a.attname FROM pg_constraint c
         JOIN LATERAL unnest(c.conkey) WITH ORDINALITY AS k(attnum, ord) ON true
         JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum
         WHERE c.conrelid = '{tbl_lit}'::regclass AND c.contype = 'p'
         ORDER BY k.ord"
    );
    let pk_rows = client
        .query(&pk_sql, &[])
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
            comment: r.get(4),
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
    filters: Vec<FilterCond>,
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
    // 视图没有 ctid 物理列，无主键时不能 ORDER BY ctid
    let relkind: String = client
        .query_one(
            &format!("SELECT relkind::text FROM pg_class WHERE oid = '{tbl_lit}'::regclass"),
            &[],
        )
        .await
        .map_err(|e| format!("查询对象类型失败: {e}"))?
        .get(0);
    let order_by = if !pk_cols.is_empty() {
        let cols: Vec<String> = pk_cols.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect();
        format!(" ORDER BY {}", cols.join(", "))
    } else if relkind == "v" {
        String::new() // 视图：不排序
    } else {
        " ORDER BY ctid".to_string()
    };

    // 筛选条件（列名 quote_ident 防注入、运算符白名单、值参数化 + 类型强转）
    let mut where_sql = String::new();
    let mut binds: Vec<Option<String>> = Vec::new();
    if !filters.is_empty() {
        let cols = list_columns_core(cfg, dbname, table).await?;
        let mut wheres: Vec<String> = Vec::new();
        let mut idx = 1;
        for f in &filters {
            let col = cols
                .iter()
                .find(|c| c.name == f.column)
                .ok_or_else(|| format!("筛选列「{}」不存在", f.column))?;
            let op = f.op.trim().to_ascii_uppercase();
            let qcol = quote_ident(&f.column);
            match op.as_str() {
                "IS NULL" | "IS NOT NULL" => wheres.push(format!("{qcol} {op}")),
                "=" | "!=" | "<>" | ">" | "<" | ">=" | "<=" | "LIKE" | "ILIKE" | "NOT LIKE"
                | "NOT ILIKE" => {
                    let v = f.value.as_deref().unwrap_or("").trim();
                    if v.is_empty() {
                        return Err(format!("筛选条件「{qcol} {op}」缺少值"));
                    }
                    wheres.push(format!("{qcol} {op} ${idx}::text::{}", col.type_name));
                    binds.push(Some(v.to_string()));
                    idx += 1;
                }
                _ => return Err(format!("不支持的运算符: {}", f.op)),
            }
        }
        where_sql = format!(" WHERE {}", wheres.join(" AND "));
    }
    let binds_ref: Vec<&(dyn ToSql + Sync)> = binds.iter().map(|b| b as &(dyn ToSql + Sync)).collect();

    let sql = format!("SELECT * FROM {qtable}{where_sql}{order_by} LIMIT {limit} OFFSET {offset}");
    let rows = client
        .query(&sql, &binds_ref)
        .await
        .map_err(|e| format!("查询失败: {e}"))?;
    let columns: Vec<ColumnInfo> = rows
        .first()
        .map(|r| {
            r.columns()
                .iter()
                .map(|c| ColumnInfo {
                    name: c.name().to_string(),
                    type_name: c.type_().name().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let rows: Vec<Vec<serde_json::Value>> = rows.iter().map(row_to_json).collect();
    let total: i64 = client
        .query_one(&format!("SELECT count(*) FROM {qtable}{where_sql}"), &binds_ref)
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
    filters: Vec<FilterCond>,
) -> Result<TablePage, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    let res = paginate_table_core(&cfg, &dbname, &table, limit, offset, filters).await;
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
/// 跳过 SQL 开头的注释（-- 行注释 / /* */ 块注释），返回第一个语句起点
fn strip_leading_comments(mut s: &str) -> &str {
    loop {
        let t = s.trim_start();
        if let Some(_) = t.strip_prefix("--") {
            match t.find('\n') {
                Some(i) => s = &t[i + 1..],
                None => return "",
            }
        } else if let Some(_) = t.strip_prefix("/*") {
            match t.find("*/") {
                Some(i) => s = &t[i + 2..],
                None => return "",
            }
        } else {
            return t;
        }
    }
}

async fn explain_query_core(client: &Client, sql: &str) -> Result<String, String> {
    let stmts = split_statements(sql);
    if stmts.len() != 1 {
        return Err("Explain 仅支持单条语句".into());
    }
    let head = strip_leading_comments(&stmts[0]).to_ascii_uppercase();
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
struct FilterCond {
    column: String,
    op: String,
    value: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct ColumnDef {
    name: String,
    col_type: String,
    nullable: bool,
    default: Option<String>,
    is_pk: bool,
    is_serial: bool,
    #[serde(default)]
    comment: Option<String>,
}

/// 核心：CREATE TABLE（Navicat 表设计器语义：字段网格 → DDL）
async fn create_table_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
    columns: Vec<ColumnDef>,
    table_comment: Option<String>,
) -> Result<(), String> {
    if table.trim().is_empty() {
        return Err("表名不能为空".into());
    }
    if columns.is_empty() {
        return Err("至少需要一个字段".into());
    }
    let sql = build_create_table_sql(table, &columns)?;

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
    // 建表后写注释（列 + 表）
    let qtable = quote_ident(table);
    for stmt in comment_statements(&qtable, table_comment.as_deref(), &columns) {
        client
            .execute(&stmt, &[])
            .await
            .map_err(|e| format!("添加注释失败: {e}"))?;
    }
    Ok(())
}

/// 纯函数：构建 CREATE TABLE SQL（表设计器语义）
fn build_create_table_sql(table: &str, columns: &[ColumnDef]) -> Result<String, String> {
    if table.trim().is_empty() {
        return Err("表名不能为空".into());
    }
    if columns.is_empty() {
        return Err("至少需要一个字段".into());
    }
    let mut parts: Vec<String> = Vec::new();
    let mut pk_cols: Vec<String> = Vec::new();
    for c in columns {
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
    Ok(format!("CREATE TABLE {} ({})", quote_ident(table), parts.join(", ")))
}

/// 结构同步差异项
#[derive(Serialize, Debug)]
struct SchemaDiff {
    table: String,
    action: String, // "create" | "alter" | "drop"
    sql: String,
}

/// SchemaColumn → ColumnDef（结构同步用）
fn schema_cols_to_defs(cols: &[SchemaColumn]) -> Vec<ColumnDef> {
    cols.iter()
        .map(|c| {
            let is_serial = c.default.as_deref().map(|d| d.contains("nextval(")).unwrap_or(false);
            ColumnDef {
                name: c.name.clone(),
                col_type: c.type_name.clone(),
                nullable: c.is_nullable == "YES",
                default: if is_serial { None } else { c.default.clone() },
                is_pk: c.is_pk,
                is_serial,
                comment: c.comment.clone(),
            }
        })
        .collect()
}

/// 生成 ALTER TABLE 子句（列级 diff：类型/可空/默认/注释/增删列）
fn diff_columns_sql(table: &str, src: &[SchemaColumn], dst: &[SchemaColumn]) -> Vec<String> {
    let q = quote_ident(table);
    let mut out: Vec<String> = Vec::new();
    // 新增列（src 有 dst 无）
    for s in src {
        if !dst.iter().any(|d| d.name == s.name) {
            let mut def = format!("ADD COLUMN {} {}", quote_ident(&s.name), s.type_name);
            if s.is_nullable == "NO" && !s.default.as_deref().unwrap_or("").contains("nextval(") {
                def.push_str(" NOT NULL");
            }
            if let Some(d) = &s.default {
                let d = d.trim();
                if !d.is_empty() && !d.contains("nextval(") {
                    def.push_str(&format!(" DEFAULT {d}"));
                }
            }
            out.push(format!("ALTER TABLE {q} {def}"));
        }
    }
    // 删除列（dst 有 src 无）
    for d in dst {
        if !src.iter().any(|s| s.name == d.name) {
            out.push(format!("ALTER TABLE {q} DROP COLUMN {}", quote_ident(&d.name)));
        }
    }
    // 修改列（类型/可空/默认/注释）
    for s in src {
        if let Some(d) = dst.iter().find(|d| d.name == s.name) {
            let s_serial = s.default.as_deref().unwrap_or("").contains("nextval(");
            let d_serial = d.default.as_deref().unwrap_or("").contains("nextval(");
            let norm = |t: &str| {
                if t.contains("nextval(") {
                    "serial".to_string()
                } else {
                    t.trim().to_string()
                }
            };
            let s_t = norm(&s.type_name);
            let d_t = norm(&d.type_name);
            if s_t != d_t {
                out.push(format!(
                    "ALTER TABLE {q} ALTER COLUMN {} TYPE {}",
                    quote_ident(&s.name),
                    s.type_name
                ));
            }
            if s.is_nullable != d.is_nullable {
                let action = if s.is_nullable == "NO" { "SET NOT NULL" } else { "DROP NOT NULL" };
                out.push(format!("ALTER TABLE {q} ALTER COLUMN {} {action}", quote_ident(&s.name)));
            }
            let s_def = if s_serial { "" } else { s.default.as_deref().unwrap_or("").trim() };
            let d_def = if d_serial { "" } else { d.default.as_deref().unwrap_or("").trim() };
            if s_def != d_def {
                if s_def.is_empty() {
                    out.push(format!(
                        "ALTER TABLE {q} ALTER COLUMN {} DROP DEFAULT",
                        quote_ident(&s.name)
                    ));
                } else {
                    out.push(format!(
                        "ALTER TABLE {q} ALTER COLUMN {} SET DEFAULT {}",
                        quote_ident(&s.name),
                        s_def
                    ));
                }
            }
            if s.comment.as_deref().unwrap_or("") != d.comment.as_deref().unwrap_or("") {
                let c = s.comment.as_deref().unwrap_or("").replace('\'', "''");
                out.push(format!(
                    "COMMENT ON COLUMN {q}.{} IS '{}'",
                    quote_ident(&s.name),
                    c
                ));
            }
        }
    }
    // 主键变化：先删旧约束（pg_constraint 查名），再加新
    let src_pk: Vec<&str> = src.iter().filter(|c| c.is_pk).map(|c| c.name.as_str()).collect();
    let dst_pk: Vec<&str> = dst.iter().filter(|c| c.is_pk).map(|c| c.name.as_str()).collect();
    if src_pk != dst_pk {
        // 主键约束名由调用方解析（需要连接查询），这里只生成 ADD；DROP 由核心处理
        if !src_pk.is_empty() {
            let cols: Vec<String> = src_pk.iter().map(|c| quote_ident(c)).collect();
            out.push(format!("ADD PRIMARY KEY ({})", cols.join(", ")));
        }
    }
    out
}

/// 核心：比较 src_db 与 dst_db 的结构差异（Navicat 结构同步）
async fn compare_schemas_core(
    cfg: &ConnConfig,
    src_db: &str,
    dst_db: &str,
) -> Result<Vec<SchemaDiff>, String> {
    if src_db == dst_db {
        return Err("源库与目标库不能相同".into());
    }
    let src_tables = list_tables_core(cfg, src_db).await?;
    let dst_tables = list_tables_core(cfg, dst_db).await?;
    let mut diffs: Vec<SchemaDiff> = Vec::new();

    for st in &src_tables {
        if st.kind != "table" {
            continue; // 只同步表（视图暂不同步）
        }
        match dst_tables.iter().find(|t| t.name == st.name) {
            None => {
                // 新建表
                let cols = list_columns_core(cfg, src_db, &st.name).await?;
                let col_defs = schema_cols_to_defs(&cols);
                let mut sql = build_create_table_sql(&st.name, &col_defs)?;
                let comment_stmts = comment_statements(&quote_ident(&st.name), None, &col_defs);
                if !comment_stmts.is_empty() {
                    sql.push('\n');
                    sql.push_str(&comment_stmts.join("\n"));
                }
                diffs.push(SchemaDiff {
                    table: st.name.clone(),
                    action: "create".into(),
                    sql,
                });
            }
            Some(dt) => {
                // 修改表：列级 diff + 主键
                let src_cols = list_columns_core(cfg, src_db, &st.name).await?;
                let dst_cols = list_columns_core(cfg, dst_db, &dt.name).await?;
                let mut substmts = diff_columns_sql(&st.name, &src_cols, &dst_cols);
                // 主键 drop（需要查 dst 的约束名）
                let src_pk: Vec<&str> = src_cols.iter().filter(|c| c.is_pk).map(|c| c.name.as_str()).collect();
                let dst_pk: Vec<&str> = dst_cols.iter().filter(|c| c.is_pk).map(|c| c.name.as_str()).collect();
                if src_pk != dst_pk && !dst_pk.is_empty() {
                    let mut c = cfg.clone();
                    c.dbname = dst_db.to_string();
                    let (client, _) = open_connection(&c).await?;
                    let tbl_lit = format!("\"{}\"", dt.name.replace('"', "\"\""));
                    let rows = client
                        .query(
                            &format!(
                                "SELECT conname FROM pg_constraint WHERE conrelid = '{tbl_lit}'::regclass AND contype = 'p'"
                            ),
                            &[],
                        )
                        .await
                        .map_err(|e| format!("查询主键约束失败: {e}"))?;
                    for r in rows {
                        let conname: String = r.get(0);
                        substmts.insert(
                            0,
                            format!("ALTER TABLE {} DROP CONSTRAINT {}", quote_ident(&st.name), quote_ident(&conname)),
                        );
                    }
                }
                if !substmts.is_empty() {
                    diffs.push(SchemaDiff {
                        table: st.name.clone(),
                        action: "alter".into(),
                        sql: format!("{};", substmts.join(";\n")),
                    });
                }
            }
        }
    }
    for dt in &dst_tables {
        if dt.kind != "table" {
            continue;
        }
        if !src_tables.iter().any(|t| t.name == dt.name) {
            diffs.push(SchemaDiff {
                table: dt.name.clone(),
                action: "drop".into(),
                sql: format!("DROP TABLE {}", quote_ident(&dt.name)),
            });
        }
    }
    Ok(diffs)
}

#[tauri::command]
async fn execute_sql(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    sql: String,
) -> Result<String, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    let mut c = cfg;
    c.dbname = dbname;
    let (client, _) = open_connection(&c).await?;
    client
        .batch_execute(&sql)
        .await
        .map_err(|e| format!("执行失败: {e}"))?;
    Ok("执行成功".to_string())
}

#[tauri::command]
async fn check_update() -> Result<serde_json::Value, String> {
    // 走 Rust 网络栈（ureq），避免 WebView 网络限制；GitHub API 匿名可读
    let url = "https://api.github.com/repos/vpertj/tusk/releases/latest";
    let resp = tokio::task::spawn_blocking(move || {
        ureq::get(url)
            .set("User-Agent", "tusk-desktop")
            .set("Accept", "application/vnd.github+json")
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map_err(|e| format!("检查更新失败: {e}"))?
            .into_string()
            .map_err(|e| format!("读取响应失败: {e}"))
    })
    .await
    .map_err(|e| format!("检查更新失败: {e}"))??;
    let v: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("解析响应失败: {e}"))?;
    Ok(serde_json::json!({
        "tag_name": v.get("tag_name").and_then(|t| t.as_str()).unwrap_or(""),
        "body": v.get("body").and_then(|b| b.as_str()).unwrap_or(""),
        "html_url": v.get("html_url").and_then(|u| u.as_str()).unwrap_or(""),
    }))
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    // macOS 系统默认浏览器打开（零依赖）
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("打开链接失败: {e}"))?;
    Ok(())
}

/// 核心：新建数据库（CREATE DATABASE 不能参数化 → 严格标识符白名单防注入）
async fn create_database_core(
    cfg: &ConnConfig,
    name: &str,
    owner: Option<&str>,
    encoding: Option<&str>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("数据库名不能为空".into());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err("数据库名只能包含字母、数字和下划线".into());
    }
    let mut sql = format!("CREATE DATABASE \"{name}\"");
    if let Some(o) = owner {
        let o = o.trim();
        if !o.is_empty() {
            if !o.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Err("所有者只能包含字母、数字和下划线".into());
            }
            sql.push_str(&format!(" OWNER \"{o}\""));
        }
    }
    if let Some(e) = encoding {
        let e = e.trim();
        if !e.is_empty() {
            sql.push_str(&format!(" ENCODING '{e}'"));
        }
    }
    let (client, _) = open_connection(cfg).await?;
    client
        .execute(&sql, &[])
        .await
        .map_err(|e| format!("创建数据库失败: {e:?}"))?;
    Ok(())
}

#[tauri::command]
async fn create_database(
    state: State<'_, AppState>,
    conn_id: String,
    name: String,
    owner: Option<String>,
    encoding: Option<String>,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    create_database_core(&cfg, &name, owner.as_deref(), encoding.as_deref()).await
}

/// 核心：删除数据库（DROP DATABASE ... WITH (FORCE)，标识符白名单）
async fn drop_database_core(cfg: &ConnConfig, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("数据库名不能为空".into());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err("数据库名只能包含字母、数字和下划线".into());
    }
    let sql = format!("DROP DATABASE \"{name}\" WITH (FORCE)");
    let (client, _) = open_connection(cfg).await?;
    client
        .execute(&sql, &[])
        .await
        .map_err(|e| format!("删除数据库失败: {e:?}"))?;
    Ok(())
}

#[tauri::command]
async fn drop_database(
    state: State<'_, AppState>,
    conn_id: String,
    name: String,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    drop_database_core(&cfg, &name).await
}

#[tauri::command]
async fn compare_schemas(
    state: State<'_, AppState>,
    conn_id: String,
    src_db: String,
    dst_db: String,
) -> Result<Vec<SchemaDiff>, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    compare_schemas_core(&cfg, &src_db, &dst_db).await
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
    table_comment: Option<String>,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    create_table_core(&cfg, &dbname, &table, columns, table_comment).await
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


/// 类型名归一化：serial/bigserial → int4/int8（alter 对比时忽略自增语义差异）
fn norm_type(t: &str) -> String {
    match t {
        "serial" => "int4".to_string(),
        "bigserial" => "int8".to_string(),
        other => other.to_string(),
    }
}

/// 核心：ALTER TABLE（对比当前结构生成变更子句，事务执行）
async fn alter_table_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
    columns: Vec<ColumnDef>,
    table_comment: Option<String>,
) -> Result<(), String> {
    if table.trim().is_empty() {
        return Err("表名不能为空".into());
    }
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;

    // 当前结构
    let current = list_columns_core(cfg, dbname, table).await?;
    let qtable = quote_ident(table);

    let mut stmts: Vec<String> = Vec::new();
    let mut comment_stmts: Vec<String> = Vec::new();

    // ---- 主键变化 ----
    let cur_pk: Vec<String> = current.iter().filter(|x| x.is_pk).map(|x| x.name.clone()).collect();
    let new_pk: Vec<String> = columns.iter().filter(|x| x.is_pk).map(|x| x.name.clone()).collect();
    if cur_pk != new_pk {
        if !cur_pk.is_empty() {
            // 查主键约束名
            let rows = client
                .query(
                    &format!("SELECT conname FROM pg_constraint WHERE conrelid = '\"{table}\"'::regclass AND contype = 'p' LIMIT 1"),
                    &[],
                )
                .await
                .map_err(|e| format!("查询主键约束失败: {e}"))?;
            if let Some(r) = rows.first() {
                let conname: String = r.get(0);
                stmts.push(format!("DROP CONSTRAINT {}", quote_ident(&conname)));
            }
        }
        if !new_pk.is_empty() {
            let pks: Vec<String> = new_pk.iter().map(|p| quote_ident(p)).collect();
            stmts.push(format!("ADD PRIMARY KEY ({})", pks.join(", ")));
        }
    }

    // ---- 列对比 ----
    for col in &columns {
        let is_serial = col.is_serial || col.col_type == "serial" || col.col_type == "bigserial";
        match current.iter().find(|x| x.name == col.name) {
            Some(cur) => {
                // 类型变化（serial ↔ int4 视为相同）
                let cur_t = norm_type(&cur.type_name);
                let new_t = norm_type(&col.col_type);
                if cur_t != new_t {
                    stmts.push(format!(
                        "ALTER COLUMN {} TYPE {}",
                        quote_ident(&col.name),
                        col.col_type
                    ));
                }
                // 可空变化
                let cur_nullable = cur.is_nullable == "YES";
                if cur_nullable != col.nullable {
                    if col.nullable {
                        stmts.push(format!("ALTER COLUMN {} DROP NOT NULL", quote_ident(&col.name)));
                    } else if !is_serial {
                        stmts.push(format!("ALTER COLUMN {} SET NOT NULL", quote_ident(&col.name)));
                    }
                }
                // 默认值变化（serial 列的 nextval 默认值忽略）
                let cur_def = cur.default.as_deref().unwrap_or("").trim();
                let new_def = col.default.as_deref().unwrap_or("").trim();
                let is_nextval = cur_def.contains("nextval(");
                if is_serial && is_nextval {
                    // serial 默认值由系统管理，忽略
                } else if cur_def != new_def {
                    if new_def.is_empty() {
                        stmts.push(format!("ALTER COLUMN {} DROP DEFAULT", quote_ident(&col.name)));
                    } else {
                        stmts.push(format!(
                            "ALTER COLUMN {} SET DEFAULT {new_def}",
                            quote_ident(&col.name)
                        ));
                    }
                }
                // 注释变化
                let cur_cmt = cur.comment.as_deref().unwrap_or("").trim();
                let new_cmt = col.comment.as_deref().unwrap_or("").trim();
                if cur_cmt != new_cmt {
                    let qcol = quote_ident(&col.name);
                    if new_cmt.is_empty() {
                        comment_stmts.push(format!("COMMENT ON COLUMN {qtable}.{qcol} IS NULL"));
                    } else {
                        comment_stmts.push(format!(
                            "COMMENT ON COLUMN {qtable}.{qcol} IS '{}'",
                            new_cmt.replace('\'', "''")
                        ));
                    }
                }
            }
            None => {
                // 新增列
                let mut def = format!(
                    "ADD COLUMN {} {}",
                    quote_ident(&col.name),
                    col.col_type.trim()
                );
                if !col.nullable && !is_serial {
                    def.push_str(" NOT NULL");
                }
                let d = col.default.as_deref().unwrap_or("").trim();
                if !d.is_empty() {
                    def.push_str(&format!(" DEFAULT {d}"));
                }
                stmts.push(def);
                // 新列注释
                if let Some(c) = &col.comment {
                    if !c.trim().is_empty() {
                        let qcol = quote_ident(&col.name);
                        comment_stmts.push(format!(
                            "COMMENT ON COLUMN {qtable}.{qcol} IS '{}'",
                            c.replace('\'', "''")
                        ));
                    }
                }
            }
        }
    }

    // ---- 删除的列 ----
    for cur in &current {
        if !columns.iter().any(|x| x.name == cur.name) {
            stmts.push(format!("DROP COLUMN {}", quote_ident(&cur.name)));
        }
    }

    // ---- 表注释 ----
    if let Some(c) = &table_comment {
        if !c.trim().is_empty() {
            comment_stmts.push(format!(
                "COMMENT ON TABLE {qtable} IS '{}'",
                c.replace('\'', "''")
            ));
        }
    }

    if stmts.is_empty() && comment_stmts.is_empty() {
        return Ok(()); // 无变化
    }

    // 批量执行 ALTER（Arc<Client> 无法开事务；逐条在单批次内发送，出错即停）
    if !stmts.is_empty() {
        let sql_all: String = stmts
            .iter()
            .map(|s| format!("ALTER TABLE {qtable} {s};"))
            .collect::<Vec<_>>()
            .join("\n");
        client
            .batch_execute(&sql_all)
            .await
            .map_err(|e| format!("ALTER 失败: {e}"))?;
    }
    // COMMENT ON 是独立语句，逐条执行
    for cs in &comment_stmts {
        client
            .execute(cs, &[])
            .await
            .map_err(|e| format!("注释更新失败: {e}"))?;
    }
    Ok(())
}

/// 编辑表结构（Tauri command 入口）
#[tauri::command]
async fn alter_table(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
    columns: Vec<ColumnDef>,
    table_comment: Option<String>,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    alter_table_core(&cfg, &dbname, &table, columns, table_comment).await
}


/// 核心：复制表（结构 or 结构+数据）。serial 列自动重建独立序列
async fn duplicate_table_core(
    cfg: &ConnConfig,
    dbname: &str,
    src_table: &str,
    new_table: &str,
    with_data: bool,
) -> Result<(), String> {
    if new_table.trim().is_empty() {
        return Err("新表名不能为空".into());
    }
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let qsrc = quote_ident(src_table);
    let qnew = quote_ident(new_table);

    // 结构复制（含主键/索引/约束/默认值）
    let sql = format!("CREATE TABLE {qnew} (LIKE {qsrc} INCLUDING ALL)");
    client
        .execute(&sql, &[])
        .await
        .map_err(|e| format!("创建新表失败: {e}"))?;

    // serial 列：默认值仍指向旧序列，需重建独立序列
    let tbl_lit_src = format!("\"{}\"", src_table.replace('"', "\"\""));
    let seq_sql = format!(
        "SELECT a.attname, pg_get_expr(d.adbin, d.adrelid)
         FROM pg_attribute a
         JOIN pg_type t ON t.oid = a.atttypid
         JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE a.attrelid = '{tbl_lit_src}'::regclass AND a.attnum > 0 AND NOT a.attisdropped
           AND pg_get_expr(d.adbin, d.adrelid) LIKE 'nextval(%'"
    );
    let seq_rows = client
        .query(&seq_sql, &[])
        .await
        .map_err(|e| format!("查询序列列失败: {e}"))?;

    let tbl_lit = format!("\"{new_table}\"");
    for r in &seq_rows {
        let col: String = r.get(0);
        let seq_name = format!("{new_table}_{col}_seq");
        let qseq = quote_ident(&seq_name);
        let qcol = quote_ident(&col);
        // 新序列（从源表当前值之后开始）
        let start_sql = if with_data {
            format!("SELECT COALESCE(MAX({qcol})::bigint, 0) + 1 FROM {qsrc}")
        } else {
            "SELECT 1::bigint".to_string()
        };
        let start_val: i64 = client
            .query_one(&start_sql, &[])
            .await
            .map_err(|e| format!("计算序列起点失败: {e}"))?
            .get(0);
        let create_seq = format!(
            "CREATE SEQUENCE {qseq} START WITH {start_val} OWNED BY {qnew}.{qcol}"
        );
        client
            .execute(&create_seq, &[])
            .await
            .map_err(|e| format!("创建序列失败: {e}"))?;
        let set_def = format!(
            "ALTER TABLE {qnew} ALTER COLUMN {qcol} SET DEFAULT nextval('{seq_name}'::regclass)"
        );
        client
            .execute(&set_def, &[])
            .await
            .map_err(|e| format!("设置默认值失败: {e}"))?;
        let _ = tbl_lit;
    }

    // 数据复制
    if with_data {
        let copy_sql = format!("INSERT INTO {qnew} SELECT * FROM {qsrc}");
        client
            .execute(&copy_sql, &[])
            .await
            .map_err(|e| format!("复制数据失败: {e}"))?;
    }
    Ok(())
}

/// 复制表（Tauri command 入口）
#[tauri::command]
async fn duplicate_table(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    src_table: String,
    new_table: String,
    with_data: bool,
) -> Result<(), String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    duplicate_table_core(&cfg, &dbname, &src_table, &new_table, with_data).await
}


// ================= 新增行（填值插入） =================

#[derive(Deserialize, Debug, Clone)]
struct ColValue {
    name: String,
    value: Option<String>,
}

/// 核心：带值插入一行。values 只含用户填写的列（None 不传该列，走 DB 默认）；
/// NOT NULL 且无默认值（非 serial）的列必须填写，否则给出明确中文错误
async fn insert_row_vals_core(
    cfg: &ConnConfig,
    dbname: &str,
    table: &str,
    values: Vec<ColValue>,
) -> Result<i32, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;

    // 列信息：name / 短类型名 / 可空 / 默认值
    let cols = list_columns_core(cfg, dbname, table).await?;
    let qtable = quote_ident(table);

    let filled: Vec<&ColValue> = values
        .iter()
        .filter(|v| v.value.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false))
        .collect();

    // 必填检查：NOT NULL 无默认（非 nextval）且未填写的列
    for col in &cols {
        let has_default = col
            .default
            .as_deref()
            .map(|d| !d.trim().is_empty() && !d.contains("nextval("))
            .unwrap_or(false);
        let is_serial = col.default.as_deref().map(|d| d.contains("nextval(")).unwrap_or(false);
        if col.is_nullable != "YES" && !has_default && !is_serial && !filled.iter().any(|v| v.name == col.name) {
            return Err(format!(
                "字段「{}」不能为空：NOT NULL 且无默认值，请填写后再插入",
                col.name
            ));
        }
    }

    // 显式 NULL 处理：用户输入 NULL → 存 NULL（仅可空列）
    let mut names: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut bind: Vec<Option<String>> = Vec::new();
    let mut idx = 1;
    for v in &filled {
        let col = cols
            .iter()
            .find(|x| x.name == v.name)
            .ok_or_else(|| format!("字段「{}」不存在", v.name))?;
        let val = v.value.as_deref().unwrap_or("").trim();
        names.push(quote_ident(&v.name));
        if val.eq_ignore_ascii_case("NULL") {
            params.push(format!("${idx}::text::{}", col.type_name));
            bind.push(None);
        } else {
            params.push(format!("${idx}::text::{}", col.type_name));
            bind.push(Some(val.to_string()));
        }
        idx += 1;
    }

    if names.is_empty() {
        // 什么都没填：直接 DEFAULT VALUES
        let sql = format!("INSERT INTO {qtable} DEFAULT VALUES RETURNING 1");
        let row = client
            .query_one(&sql, &[])
            .await
            .map_err(|e| format!("插入失败: {e}"))?;
        return Ok(row.get(0));
    }

    let sql = format!(
        "INSERT INTO {qtable} ({}) VALUES ({}) RETURNING 1",
        names.join(", "),
        params.join(", ")
    );
    let params_ref: Vec<&(dyn ToSql + Sync)> = bind.iter().map(|b| b as &(dyn ToSql + Sync)).collect();
    let row = client
        .query_one(&sql, &params_ref)
        .await
        .map_err(|e| format!("插入失败: {e}"))?;
    Ok(row.get(0))
}

/// 新增行（Tauri command 入口）
#[tauri::command]
async fn insert_row_vals(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
    values: Vec<ColValue>,
) -> Result<i32, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    insert_row_vals_core(&cfg, &dbname, &table, values).await
}

/// 导出表数据为 SQL（INSERT 语句），值统一转义为字符串字面量
async fn export_sql_core(cfg: &ConnConfig, dbname: &str, table: &str) -> Result<String, String> {
    let mut c = cfg.clone();
    c.dbname = dbname.to_string();
    let (client, _) = open_connection(&c).await?;
    let qtable = quote_ident(table);

    // 列名（用于 INSERT 列清单）
    let cols = list_columns_core(cfg, dbname, table).await?;
    let col_names: Vec<String> = cols.iter().map(|x| quote_ident(&x.name)).collect();

    // 全部数据
    let rows = client
        .query(&format!("SELECT * FROM {qtable}"), &[])
        .await
        .map_err(|e| format!("查询失败: {e}"))?;

    let mut out = String::new();
    out.push_str(&format!("-- 表数据导出: {table}\\n-- 生成时间: {}\\n\\n", now_str()));
    out.push_str(&format!("TRUNCATE {qtable};\\n\\n"));

    let mut buf: Vec<String> = Vec::new();
    for r in &rows {
        let row_vals = row_to_json(r);
        let vals: Vec<String> = row_vals
            .iter()
            .map(|jv| {
                if jv.is_null() {
                    "NULL".to_string()
                } else {
                    let s = match jv {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => jv.to_string(),
                    };
                    format!("'{}'", s.replace('\'', "''"))
                }
            })
            .collect();
        buf.push(format!("({})", vals.join(", ")));
        if buf.len() >= 500 {
            out.push_str(&format!(
                "INSERT INTO {qtable} ({}) VALUES\\n{};\\n",
                col_names.join(", "),
                buf.join(",\\n")
            ));
            buf.clear();
        }
    }
    if !buf.is_empty() {
        out.push_str(&format!(
            "INSERT INTO {qtable} ({}) VALUES\\n{};\\n",
            col_names.join(", "),
            buf.join(",\\n")
        ));
    }
    Ok(out)
}

/// 通用：写入文本文件（用于导出下载；路径需以 ~/ 开头）
async fn write_text_file_core(path: &str, content: &str) -> Result<(), String> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest)
    } else {
        path.to_string()
    };
    std::fs::write(&expanded, content).map_err(|e| format!("写入文件失败: {e}"))
}

fn now_str() -> String {
    // 简单本地时间戳（无 chrono 依赖）
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", secs)
}

#[tauri::command]
async fn export_sql(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
    table: String,
) -> Result<String, String> {
    let cfg = {
        let conns = state.conns.lock().await;
        conns.get(&conn_id).map(|e| e.cfg.clone()).ok_or("连接不存在或已断开")?
    };
    export_sql_core(&cfg, &dbname, &table).await
}

#[tauri::command]
async fn write_text_file(path: String, content: String) -> Result<(), String> {
    write_text_file_core(&path, &content).await
}

/// numeric 列的字符串包装：postgres-types 未实现 numeric 的 FromSql，'''
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
        let page1 = paginate_table_core(&cfg, "tusk_demo", "products", 2, 0, vec![])
            .await
            .expect("第一页失败");
        assert_eq!(page1.rows.len(), 2, "limit=2 应返回 2 行");
        let total = page1.total.expect("应有总数");
        assert!(total >= 4, "products 应至少 4 行（用户可能已手动新增）: {total}");

        let page2 = paginate_table_core(&cfg, "tusk_demo", "products", 2, 2, vec![])
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
        let page_after = paginate_table_core(&cfg, "tusk_demo", "products", 2, 0, vec![])
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
        let err = create_table_core(&cfg, "tusk_demo", "empty_table", vec![], None)
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
            comment: None,
            },
            ColumnDef {
                name: "name".into(),
                col_type: "varchar(100)".into(),
                nullable: false,
                default: None,
                is_pk: false,
                is_serial: false,
            comment: None,
            },
            ColumnDef {
                name: "price".into(),
                col_type: "numeric(10,2)".into(),
                nullable: true,
                default: Some("0".into()),
                is_pk: false,
                is_serial: false,
            comment: None,
            },
            ColumnDef {
                name: "created_at".into(),
                col_type: "timestamptz".into(),
                nullable: true,
                default: Some("now()".into()),
                is_pk: false,
                is_serial: false,
            comment: None,
            },
        ];
        create_table_core(&cfg, "tusk_demo", &tname, cols, None)
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
            comment: None,
            }],
            None
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
            comment: None,
            }],
            None
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
            ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
            ColumnDef { name: "title".into(), col_type: "varchar(255)".into(), nullable: false, default: None, is_pk: false, is_serial: false, comment: None },
            ColumnDef { name: "amount".into(), col_type: "numeric(12,2)".into(), nullable: true, default: Some("0.00".into()), is_pk: false, is_serial: false, comment: None },
            ColumnDef { name: "enabled".into(), col_type: "bool".into(), nullable: false, default: Some("true".into()), is_pk: false, is_serial: false, comment: None },
            ColumnDef { name: "tags".into(), col_type: "jsonb".into(), nullable: true, default: Some("'[]'::jsonb".into()), is_pk: false, is_serial: false, comment: None },
            ColumnDef { name: "created_at".into(), col_type: "timestamptz".into(), nullable: true, default: Some("now()".into()), is_pk: false, is_serial: false, comment: None },
        ];
        create_table_core(&cfg, "tusk_demo", &tname, cols, None).await.expect("建表失败");

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
        let page = paginate_table_core(&cfg, "tusk_demo", &tname, 10, 0, vec![]).await.expect("分页失败");
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
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建大写表失败");

        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(&format!("INSERT INTO \"{tname}\" (name) VALUES ('x')"), &[])
            .await
            .expect("插入失败");

        // 分页读取（之前主键查询用单引号 regclass，大写表名会失败）
        let page = paginate_table_core(&cfg, "tusk_demo", tname, 10, 0, vec![])
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

    #[tokio::test]
    async fn test_alter_table() {
        let cfg = test_cfg();
        let tname = format!("tusk_alter_test_{}", std::process::id());
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: false, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "price".into(), col_type: "numeric(10,2)".into(), nullable: true, default: Some("0".into()), is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(&format!("INSERT INTO \"{tname}\" (name) VALUES ('A')"), &[])
            .await
            .expect("插入失败");

        // 1) 加列（有数据时 NOT NULL 需带 DEFAULT）+ 改类型 + 改可空 + 改默认
        alter_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "varchar(50)".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "price".into(), col_type: "numeric(12,2)".into(), nullable: true, default: Some("1.5".into()), is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "qty".into(), col_type: "int4".into(), nullable: false, default: Some("1".into()), is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("alter 失败");
        let cols = list_columns_core(&cfg, "tusk_demo", &tname).await.expect("查字段失败");
        assert_eq!(cols.len(), 4, "应新增 qty");
        let name = cols.iter().find(|c| c.name == "name").unwrap();
        assert_eq!(name.type_name, "varchar(50)", "类型应改为 varchar(50): {}", name.type_name);
        assert!(name.is_nullable == "YES", "name 应改为可空");
        let price = cols.iter().find(|c| c.name == "price").unwrap();
        assert_eq!(price.type_name, "numeric(12,2)");
        assert!(price.default.as_deref().unwrap_or("").contains("1.5"), "默认值应改: {:?}", price.default);
        let qty = cols.iter().find(|c| c.name == "qty").unwrap();
        assert!(qty.is_nullable == "NO", "qty 应 NOT NULL");
        // 旧行 qty 被默认值填充
        let q: i32 = client.query_one(&format!("SELECT qty FROM \"{tname}\""), &[]).await.expect("q").get(0);
        assert_eq!(q, 1);

        // 2) 删列 + 主键变化（id+name 组合主键）
        alter_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "varchar(50)".into(), nullable: true, default: None, is_pk: true, is_serial: false, comment: None },
                ColumnDef { name: "price".into(), col_type: "numeric(12,2)".into(), nullable: true, default: Some("1.5".into()), is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("alter2 失败");
        let cols2 = list_columns_core(&cfg, "tusk_demo", &tname).await.expect("查字段失败");
        assert_eq!(cols2.len(), 3, "qty 应被删除");
        assert_eq!(cols2.iter().filter(|c| c.is_pk).count(), 2, "主键应为 id+name");

        // 3) 无变化调用应成功（幂等；name 已是主键，PG 主键隐含 NOT NULL）
        alter_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "varchar(50)".into(), nullable: false, default: None, is_pk: true, is_serial: false, comment: None },
                ColumnDef { name: "price".into(), col_type: "numeric(12,2)".into(), nullable: true, default: Some("1.5".into()), is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("无变化应成功");

        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
    }

    #[tokio::test]
    async fn test_duplicate_table() {
        let cfg = test_cfg();
        let src = format!("tusk_dup_src_{}", std::process::id());
        let dst_struct = format!("tusk_dup_struct_{}", std::process::id());
        let dst_full = format!("tusk_dup_full_{}", std::process::id());

        create_table_core(
            &cfg,
            "tusk_demo",
            &src,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: false, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "price".into(), col_type: "numeric(10,2)".into(), nullable: true, default: Some("0".into()), is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建源表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(&format!("INSERT INTO \"{src}\" (name, price) VALUES ('A', 1.5), ('B', 2.5)"), &[])
            .await
            .expect("插入失败");

        // 1) 仅结构复制
        duplicate_table_core(&cfg, "tusk_demo", &src, &dst_struct, false)
            .await
            .expect("结构复制失败");
        let cnt: i64 = client
            .query_one(&format!("SELECT count(*) FROM \"{dst_struct}\""), &[])
            .await
            .expect("q")
            .get(0);
        assert_eq!(cnt, 0, "仅结构复制不应有数据");
        let pk: i64 = client
            .query_one(
                "SELECT count(*) FROM information_schema.table_constraints WHERE table_name = $1 AND constraint_type = 'PRIMARY KEY'",
                &[&dst_struct],
            )
            .await
            .expect("q")
            .get(0);
        assert_eq!(pk, 1, "主键应复制");
        // 序列独立：新表插入自增从 1 开始
        client
            .execute(&format!("INSERT INTO \"{dst_struct}\" (name) VALUES ('X')"), &[])
            .await
            .expect("新表插入失败");
        let new_id: i32 = client
            .query_one(&format!("SELECT id FROM \"{dst_struct}\""), &[])
            .await
            .expect("q")
            .get(0);
        assert_eq!(new_id, 1, "新表序列应从 1 开始");

        // 2) 结构+数据复制
        duplicate_table_core(&cfg, "tusk_demo", &src, &dst_full, true)
            .await
            .expect("全量复制失败");
        let cnt2: i64 = client
            .query_one(&format!("SELECT count(*) FROM \"{dst_full}\""), &[])
            .await
            .expect("q")
            .get(0);
        assert_eq!(cnt2, 2, "数据应复制 2 行");
        let first_name: String = client
            .query_one(&format!("SELECT name FROM \"{dst_full}\" WHERE id = 1"), &[])
            .await
            .expect("q")
            .get(0);
        assert_eq!(first_name, "A");

        // 3) 重名冲突报错
        let err = duplicate_table_core(&cfg, "tusk_demo", &src, &src, false)
            .await
            .expect_err("重名应报错");
        assert!(!err.is_empty());

        // 清理
        for t in [&src, &dst_struct, &dst_full] {
            drop_table_core(&cfg, "tusk_demo", t).await.expect("清理失败");
        }
    }

    #[tokio::test]
    async fn test_insert_row_vals() {
        let cfg = test_cfg();
        let tname = format!("tusk_insvals_{}", std::process::id());
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: false, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "price".into(), col_type: "numeric(10,2)".into(), nullable: false, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "qty".into(), col_type: "int4".into(), nullable: true, default: Some("1".into()), is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "note".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");

        // 1) 完整填写 → 成功，serial 自动生成
        let id1 = insert_row_vals_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColValue { name: "name".into(), value: Some("苹果".into()) },
                ColValue { name: "price".into(), value: Some("12.5".into()) },
            ],
        )
        .await
        .expect("插入失败");
        assert_eq!(id1, 1);
        let row = client
            .query_one(&format!("SELECT name, price::float8, qty, note FROM \"{tname}\" WHERE id = 1"), &[])
            .await
            .expect("q");
        let name: String = row.get(0);
        assert_eq!(name, "苹果");
        let price: f64 = row.get(1);
        assert_eq!(price, 12.5);
        let qty: i32 = row.get(2);
        assert_eq!(qty, 1, "默认值应生效");
        let note: Option<String> = row.get(3);
        assert!(note.is_none(), "可空列留空应为 NULL");

        // 2) 漏填 NOT NULL 无默认列 → 明确中文报错
        let err = insert_row_vals_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![ColValue { name: "price".into(), value: Some("5".into()) }],
        )
        .await
        .expect_err("应报错");
        assert!(err.contains("name"), "报错应指明字段: {err}");
        assert!(err.contains("不能为空"), "报错应中文提示: {err}");

        // 3) 显式填 NULL → 可空列存 NULL
        insert_row_vals_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColValue { name: "name".into(), value: Some("香蕉".into()) },
                ColValue { name: "price".into(), value: Some("3.0".into()) },
                ColValue { name: "note".into(), value: Some("NULL".into()) },
            ],
        )
        .await
        .expect("插入2失败");
        let n: i64 = client
            .query_one(&format!("SELECT count(*) FROM \"{tname}\" WHERE note IS NULL"), &[])
            .await
            .expect("q")
            .get(0);
        assert_eq!(n, 2, "note 应为 NULL");

        // 4) 非法数值 → 报错
        let err2 = insert_row_vals_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColValue { name: "name".into(), value: Some("x".into()) },
                ColValue { name: "price".into(), value: Some("abc".into()) },
            ],
        )
        .await
        .expect_err("非法数值应报错");
        assert!(!err2.is_empty());

        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
    }

    #[tokio::test]
    async fn test_pagination_filter() {
        let cfg = test_cfg();
        let tname = format!("tusk_filter_{}", std::process::id());
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "qty".into(), col_type: "int4".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "tag".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(
                &format!(
                    "INSERT INTO \"{tname}\" (name, qty, tag) VALUES ('a',1,'x'),('b',2,'x'),('c',3,'y'),('d',4,'y'),('e',5,NULL)"
                ),
                &[],
            )
            .await
            .expect("插入失败");

        let f = |column: &str, op: &str, value: Option<&str>| FilterCond {
            column: column.to_string(),
            op: op.to_string(),
            value: value.map(|v| v.to_string()),
        };

        // 等值
        let r = paginate_table_core(&cfg, "tusk_demo", &tname, 50, 0, vec![f("name", "=", Some("b"))])
            .await
            .expect("筛选失败");
        assert_eq!(r.total, Some(1), "name=b 应 1 行");

        // 大于等于 + 类型强转（int4）
        let r = paginate_table_core(&cfg, "tusk_demo", &tname, 50, 0, vec![f("qty", ">=", Some("3"))])
            .await
            .expect("筛选失败");
        assert_eq!(r.total, Some(3), "qty>=3 应 3 行");

        // IS NULL
        let r = paginate_table_core(&cfg, "tusk_demo", &tname, 50, 0, vec![f("tag", "IS NULL", None)])
            .await
            .expect("筛选失败");
        assert_eq!(r.total, Some(1), "tag IS NULL 应 1 行");

        // LIKE
        let r = paginate_table_core(&cfg, "tusk_demo", &tname, 50, 0, vec![f("name", "LIKE", Some("%a%"))])
            .await
            .expect("筛选失败");
        assert_eq!(r.total, Some(1), "name LIKE %a% 应 1 行");

        // 多条件 AND
        let r = paginate_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            50,
            0,
            vec![f("qty", ">", Some("1")), f("tag", "=", Some("y"))],
        )
        .await
        .expect("筛选失败");
        assert_eq!(r.total, Some(2), "qty>1 AND tag=y 应 2 行");

        // 非法列名
        let err = paginate_table_core(&cfg, "tusk_demo", &tname, 50, 0, vec![f("nope", "=", Some("1"))])
            .await
            .expect_err("非法列应报错");
        assert!(err.contains("nope"), "{err}");

        // 非法运算符
        let err = paginate_table_core(&cfg, "tusk_demo", &tname, 50, 0, vec![f("qty", "XOR", Some("1"))])
            .await
            .expect_err("非法 op 应报错");
        assert!(!err.is_empty());

        // 空筛选 = 全量
        let r = paginate_table_core(&cfg, "tusk_demo", &tname, 50, 0, vec![])
            .await
            .expect("空筛选失败");
        assert_eq!(r.total, Some(5));

        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
    }

    #[tokio::test]
    async fn test_export_sql() {
        let cfg = test_cfg();
        let tname = format!("tusk_sqlexport_{}", std::process::id());
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "price".into(), col_type: "numeric(10,2)".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(
                &format!(
                    "INSERT INTO \"{tname}\" (name, price) VALUES ('苹果', 12.5), ('O''Brien', NULL), (NULL, 3.14)"
                ),
                &[],
            )
            .await
            .expect("插入失败");

        let sql = export_sql_core(&cfg, "tusk_demo", &tname).await.expect("导出失败");
        assert!(sql.contains(&format!("INSERT INTO \"{tname}\"")), "应有 INSERT: {sql}");
        assert!(sql.contains("O''Brien"), "引号应转义: {sql}");
        assert!(sql.contains("NULL"), "NULL 应保留: {sql}");
        assert!(sql.contains("苹果"), "中文应保留: {sql}");
        assert!(sql.contains(";"), "应以分号结尾");
        // 行数：3 行数据（id 自动生成）
        assert!(sql.matches("INSERT INTO").count() >= 1, "应有 INSERT 语句");

        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
    }

    #[tokio::test]
    async fn test_write_text_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("tusk_write_test_{}.txt", std::process::id()));
        let p = path.to_str().unwrap().to_string();
        write_text_file_core(&p, "你好\n第二行").await.expect("写入失败");
        let content = std::fs::read_to_string(&p).expect("读取失败");
        assert!(content.contains("你好"), "内容应正确: {content}");
        std::fs::remove_file(&p).ok();
    }

    #[tokio::test]
    async fn test_views() {
        let cfg = test_cfg();
        let tname = format!("tusk_view_src_{}", std::process::id());
        let vname = format!("tusk_view_{}", std::process::id());
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(&format!("INSERT INTO \"{tname}\" (name) VALUES ('A'),('B')"), &[])
            .await
            .expect("插入失败");

        create_view_core(
            &cfg,
            "tusk_demo",
            &vname,
            &format!("SELECT id, name FROM \"{tname}\" WHERE name = 'A'"),
        )
        .await
        .expect("创建视图失败");

        let tables = list_tables_core(&cfg, "tusk_demo").await.expect("列表失败");
        let v = tables.iter().find(|t| t.name == vname).expect("视图应在列表");
        assert_eq!(v.kind, "view", "kind 应为 view: {}", v.kind);

        let page = paginate_table_core(&cfg, "tusk_demo", &vname, 50, 0, vec![])
            .await
            .expect("视图分页失败");
        assert_eq!(page.total, Some(1), "视图应返回 1 行");

        let err = create_view_core(&cfg, "tusk_demo", "tusk_bad_view", "DELETE FROM x")
            .await
            .expect_err("非 SELECT 应拒绝");
        assert!(err.contains("SELECT"), "{err}");

        drop_view_core(&cfg, "tusk_demo", &vname).await.expect("删除视图失败");
        let tables2 = list_tables_core(&cfg, "tusk_demo").await.expect("列表2失败");
        assert!(!tables2.iter().any(|t| t.name == vname), "删除后视图应消失");

        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
    }

    #[tokio::test]
    async fn test_indexes() {
        let cfg = test_cfg();
        let tname = format!("tusk_idx_{}", std::process::id());
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
                ColumnDef { name: "email".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
                ColumnDef { name: "tag".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
            ],
            None
        )
        .await
        .expect("建表失败");
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        client
            .execute(&format!("CREATE UNIQUE INDEX idx_{tname}_email ON \"{tname}\" (email)"), &[])
            .await
            .expect("建索引失败");
        client
            .execute(&format!("CREATE INDEX idx_{tname}_tag ON \"{tname}\" (tag)"), &[])
            .await
            .expect("建索引2失败");

        let idxs = list_indexes_core(&cfg, "tusk_demo", &tname).await.expect("索引列表失败");
        assert_eq!(idxs.len(), 2, "应有 2 个索引（主键索引排除）: {:?}", idxs.iter().map(|i| &i.name).collect::<Vec<_>>());
        let email = idxs.iter().find(|i| i.name.contains("email")).expect("email 索引");
        assert!(email.is_unique, "email 索引应唯一");
        assert!(email.columns.contains("email"), "列应为 email: {}", email.columns);

        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
    }

    #[tokio::test]
    async fn test_comments() {
        let cfg = test_cfg();
        let tname = format!("tusk_cmt_{}", std::process::id());
        create_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: Some("主键".into()) },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: Some("名称".into()) },
            ],
            Some("测试表注释".to_string()),
        )
        .await
        .expect("建表失败");

        // 列注释存在
        let cols = list_columns_core(&cfg, "tusk_demo", &tname).await.expect("列失败");
        let id = cols.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id.comment.as_deref(), Some("主键"), "id 注释: {:?}", id.comment);

        // 表注释存在
        let (client, _) = open_connection(&cfg).await.expect("连接失败");
        let tcmt: Option<String> = client
            .query_one(
                "SELECT obj_description(c.oid, 'pg_class') FROM pg_class c WHERE c.relname = $1",
                &[&tname],
            )
            .await
            .expect("q")
            .get(0);
        assert_eq!(tcmt.as_deref(), Some("测试表注释"), "表注释: {tcmt:?}");

        // 修改注释
        alter_table_core(
            &cfg,
            "tusk_demo",
            &tname,
            vec![
                ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: Some("主键ID".into()) },
                ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
            ],
            Some("新表注释".to_string()),
        )
        .await
        .expect("改注释失败");
        let cols2 = list_columns_core(&cfg, "tusk_demo", &tname).await.expect("列2失败");
        let id2 = cols2.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id2.comment.as_deref(), Some("主键ID"));
        let tcmt2: Option<String> = client
            .query_one(
                "SELECT obj_description(c.oid, 'pg_class') FROM pg_class c WHERE c.relname = $1",
                &[&tname],
            )
            .await
            .expect("q")
            .get(0);
        assert_eq!(tcmt2.as_deref(), Some("新表注释"));

        drop_table_core(&cfg, "tusk_demo", &tname).await.expect("清理失败");
    }

    #[tokio::test]
    async fn test_schema_sync() {
        let cfg = test_cfg();
        let suffix = std::process::id();
        let a = format!("tusk_sync_a_{suffix}");
        let b = format!("tusk_sync_b_{suffix}");
        // 建两个临时库（CREATE DATABASE 不能与多语句批量，须单条）
        let (admin, _) = open_connection(&test_cfg()).await.expect("连接失败");
        admin.execute(&format!("DROP DATABASE IF EXISTS \"{a}\" WITH (FORCE)"), &[]).await.ok();
        admin.execute(&format!("DROP DATABASE IF EXISTS \"{b}\" WITH (FORCE)"), &[]).await.ok();
        admin.execute(&format!("CREATE DATABASE \"{a}\""), &[]).await.expect("建库A失败");
        admin.execute(&format!("CREATE DATABASE \"{b}\""), &[]).await.expect("建库B失败");

        // A 库：t1（2 列）+ t2；B 库：t1（3 列，改类型）+ t3
        let cfg_a = { let mut c = cfg.clone(); c.dbname = a.clone(); c };
        let cfg_b = { let mut c = cfg.clone(); c.dbname = b.clone(); c };
        create_table_core(&cfg_a, &a, "t1", vec![
            ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
            ColumnDef { name: "name".into(), col_type: "text".into(), nullable: true, default: None, is_pk: false, is_serial: false, comment: None },
        ], None).await.expect("A.t1 建表失败");
        create_table_core(&cfg_a, &a, "t2", vec![
            ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
        ], None).await.expect("A.t2 建表失败");
        create_table_core(&cfg_b, &b, "t1", vec![
            ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
            ColumnDef { name: "name".into(), col_type: "varchar(20)".into(), nullable: false, default: None, is_pk: false, is_serial: false, comment: None },
            ColumnDef { name: "qty".into(), col_type: "int4".into(), nullable: true, default: Some("0".into()), is_pk: false, is_serial: false, comment: None },
        ], None).await.expect("B.t1 建表失败");
        create_table_core(&cfg_b, &b, "t3", vec![
            ColumnDef { name: "id".into(), col_type: "serial".into(), nullable: false, default: None, is_pk: true, is_serial: true, comment: None },
        ], None).await.expect("B.t3 建表失败");

        // 比较 A → B
        let diffs = compare_schemas_core(&cfg, &a, &b).await.expect("比较失败");
        // t2 新建、t1 修改、t3 删除
        let names: Vec<String> = diffs.iter().map(|d| d.table.clone()).collect();
        assert!(diffs.iter().any(|d| d.table == "t2" && d.action == "create"), "t2 应新建: {names:?}");
        assert!(diffs.iter().any(|d| d.table == "t1" && d.action == "alter"), "t1 应修改: {names:?}");
        assert!(diffs.iter().any(|d| d.table == "t3" && d.action == "drop"), "t3 应删除: {names:?}");

        // 执行同步 SQL
        for d in &diffs {
            let mut c = cfg.clone();
            c.dbname = b.clone();
            let (client, _) = open_connection(&c).await.expect("连接失败");
            client.batch_execute(&d.sql).await.expect(&format!("执行 {} 失败: {}", d.table, d.sql));
        }

        // 再次比较应为空（结构一致）
        let diffs2 = compare_schemas_core(&cfg, &a, &b).await.expect("比较2失败");
        assert!(diffs2.is_empty(), "同步后应无差异: {:?}", diffs2.iter().map(|d| (&d.table, &d.action)).collect::<Vec<_>>());

        admin.execute(&format!("DROP DATABASE IF EXISTS \"{a}\" WITH (FORCE)"), &[]).await.ok();
        admin.execute(&format!("DROP DATABASE IF EXISTS \"{b}\" WITH (FORCE)"), &[]).await.ok();
    }

    #[tokio::test]
    async fn test_create_database() {
        let cfg = test_cfg();
        let name = format!("tusk_db_{}", std::process::id());
        let admin = open_connection(&cfg).await.expect("连接失败").0;
        admin.execute(&format!("DROP DATABASE IF EXISTS \"{name}\""), &[]).await.ok();

        // 正常创建
        create_database_core(&cfg, &name, None, None).await.expect("创建失败");
        let dbs = list_databases_core(&admin).await.expect("列库失败");
        assert!(dbs.iter().any(|d| d.name == name), "新库应出现在列表中");

        // 重名应报错
        let err = create_database_core(&cfg, &name, None, None).await.expect_err("重名应报错");
        assert!(err.contains("已存在") || err.to_lowercase().contains("exist"), "错误: {err}");

        // 非法名字符应拒绝（防注入）
        let err2 = create_database_core(&cfg, "bad; DROP DATABASE x", None, None).await.expect_err("非法名应拒绝");
        assert!(err2.contains("只能包含"), "错误: {err2}");

        admin.execute(&format!("DROP DATABASE IF EXISTS \"{name}\""), &[]).await.ok();
    }

    #[tokio::test]
    async fn test_drop_database() {
        let cfg = test_cfg();
        let name = format!("tusk_drop_{}", std::process::id());
        let admin = open_connection(&cfg).await.expect("连接失败").0;
        admin.execute(&format!("DROP DATABASE IF EXISTS \"{name}\""), &[]).await.ok();

        create_database_core(&cfg, &name, None, None).await.expect("创建失败");
        // 删除
        drop_database_core(&cfg, &name).await.expect("删除失败");
        let dbs = list_databases_core(&admin).await.expect("列库失败");
        assert!(!dbs.iter().any(|d| d.name == name), "删除后不应存在");
        // 删除不存在的库应报错
        let err = drop_database_core(&cfg, &name).await.expect_err("删不存在的库应报错");
        assert!(err.contains("不存在") || err.to_lowercase().contains("exist"), "错误: {err}");
        // 非法名拒绝
        let err2 = drop_database_core(&cfg, "x; DROP DATABASE y").await.expect_err("非法名应拒绝");
        assert!(err2.contains("只能包含"), "错误: {err2}");
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
            drop_table,
            alter_table,
            duplicate_table,
            insert_row_vals,
            export_sql,
            write_text_file,
            create_view,
            drop_view,
            list_indexes,
            compare_schemas,
            execute_sql,
            open_url,
            check_update,
            create_database,
            drop_database
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
