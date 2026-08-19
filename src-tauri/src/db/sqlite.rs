// Tusk — SQLite 驱动实现
// 嵌入式数据库：文件即库，支持 连接/查询/浏览/编辑/导出 核心子集
use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags};
use tauri::State;
use tokio::sync::Mutex;

use crate::models::*;
use crate::state::{AppState, ConnEntry};

fn conn_of<'a>(entry: &'a ConnEntry) -> Result<&'a Arc<Mutex<Connection>>, String> {
    entry
        .sqlite
        .as_ref()
        .ok_or_else(|| "连接不是 SQLite".to_string())
}

/// 打开 SQLite 文件连接并缓存（Tauri command 入口由 pg::connect 分发调用）
pub async fn connect(state: State<'_, AppState>, cfg: ConnConfig) -> Result<ConnectionInfo, String> {
    if cfg.path.trim().is_empty() {
        return Err("请选择 SQLite 数据库文件".into());
    }
    let path_str = cfg.path.clone();
    let conn = Connection::open_with_flags(
        &path_str,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开 SQLite 失败: {e}"))?;
    let version: String = conn
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .map_err(|e| format!("查询版本失败: {e}"))?;
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
            client: None,
            cfg: cfg.clone(),
            sqlite: Some(Arc::new(Mutex::new(conn))),
        },
    );
    let fname = Path::new(&path_str)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path_str.clone());
    Ok(ConnectionInfo {
        id,
        version: format!("SQLite {version}"),
        user: String::new(),
        host: fname,
        port: 0,
    })
}

/// 执行 SQL，返回列与行
pub async fn query(entry: &ConnEntry, sql: &str) -> Result<QueryResult, String> {
    let conn = conn_of(entry)?.lock().await;
    let mut stmt = conn.prepare(sql).map_err(|e| format!("SQL 错误: {e}"))?;
    let col_names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|c| c.to_string())
        .collect();
    let columns: Vec<ColumnInfo> = col_names
        .iter()
        .map(|c| ColumnInfo {
            name: c.clone(),
            type_name: String::new(),
        })
        .collect();
    let mut rows = Vec::new();
    let mut row_iter = stmt
        .query([])
        .map_err(|e| format!("执行失败: {e}"))?;
    let mut rows_affected = None;
    while let Some(row) = row_iter.next().map_err(|e| format!("读取失败: {e}"))? {
        let mut vals = Vec::new();
        for i in 0..row.as_ref().column_count() {
            vals.push(row_val(row, i));
        }
        rows.push(vals);
    }
    if rows.is_empty() && columns.is_empty() {
        // DDL / DML：报告影响行数
        rows_affected = Some(conn.changes() as u64);
    }
    Ok(QueryResult {
        columns,
        rows,
        rows_affected,
        message: None,
    })
}


/// 行内取一列值转 JSON（处理 ValueRef 借用形式）
fn row_val(row: &rusqlite::Row, i: usize) -> serde_json::Value {
    match row.get_ref(i) {
        Ok(rusqlite::types::ValueRef::Null) => serde_json::Value::Null,
        Ok(rusqlite::types::ValueRef::Integer(v)) => serde_json::Value::from(v),
        Ok(rusqlite::types::ValueRef::Real(v)) => serde_json::Value::from(v),
        Ok(rusqlite::types::ValueRef::Text(v)) => {
            serde_json::Value::String(String::from_utf8_lossy(v).to_string())
        }
        Ok(rusqlite::types::ValueRef::Blob(v)) => {
            serde_json::Value::String(format!("<blob {} bytes>", v.len()))
        }
        Err(_) => serde_json::Value::Null,
    }
}

/// SQLite 单库（文件即库），返回一个虚拟库条目
pub async fn list_databases(entry: &ConnEntry) -> Result<Vec<DatabaseInfo>, String> {
    let path = Path::new(&entry.cfg.path);
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "main".to_string());
    Ok(vec![DatabaseInfo { name }])
}

/// 列出表
pub async fn list_tables(entry: &ConnEntry, _dbname: &str) -> Result<Vec<TableInfo>, String> {
    let conn = conn_of(entry)?.lock().await;
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(|e| format!("SQL 错误: {e}"))?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("查询失败: {e}"))?;
    let mut tables = Vec::new();
    for n in names {
        tables.push(TableInfo {
            name: n.map_err(|e| e.to_string())?,
            kind: "table".into(),
        });
    }
    Ok(tables)
}

/// 列出列（PRAGMA table_info）
pub async fn list_columns(entry: &ConnEntry, _dbname: &str, table: &str) -> Result<Vec<SchemaColumn>, String> {
    let conn = conn_of(entry)?.lock().await;
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{}\")", table.replace('"', "\"\"")))
        .map_err(|e| format!("SQL 错误: {e}"))?;
    let cols = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| format!("查询失败: {e}"))?;
    let mut out = Vec::new();
    for c in cols {
        let (name, ty, notnull, dflt, pk) = c.map_err(|e| e.to_string())?;
        out.push(SchemaColumn {
            name,
            type_name: ty,
            is_nullable: if notnull == 0 { "YES".into() } else { "NO".into() },
            default: dflt,
            is_pk: pk > 0,
            comment: None,
        });
    }
    Ok(out)
}

/// 分页浏览
pub async fn paginate_table(
    entry: &ConnEntry,
    _dbname: &str,
    table: &str,
    page: u64,
    page_size: u64,
    filter: Option<FilterCond>,
) -> Result<TablePage, String> {
    let conn = conn_of(entry)?.lock().await;
    let t = table.replace('"', "\"\"");
    let mut where_sql = String::new();
    if let Some(f) = &filter {
        if !f.value.as_deref().unwrap_or("").trim().is_empty() {
            let col = f.column.replace('"', "\"\"");
            let v = f.value.clone().unwrap_or_default();
            where_sql = format!(
                " WHERE \"{col}\" {} \"{v}\"",
                if f.op == "LIKE" { "LIKE" } else { "=" }
            );
        }
    }
    let count_sql = format!("SELECT count(*) FROM \"{t}\"{where_sql}");
    let total: i64 = conn
        .query_row(&count_sql, [], |r| r.get(0))
        .map_err(|e| format!("统计失败: {e}"))?;
    let offset = page * page_size;
    let data_sql = format!("SELECT * FROM \"{t}\"{where_sql} LIMIT {page_size} OFFSET {offset}");
    let mut stmt = conn.prepare(&data_sql).map_err(|e| format!("SQL 错误: {e}"))?;
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let columns: Vec<ColumnInfo> = col_names
        .iter()
        .map(|c| ColumnInfo {
            name: c.clone(),
            type_name: String::new(),
        })
        .collect();
    let mut rows = Vec::new();
    let mut iter = stmt.query([]).map_err(|e| format!("执行失败: {e}"))?;
    while let Some(row) = iter.next().map_err(|e| format!("读取失败: {e}"))? {
        let mut vals = Vec::new();
        for i in 0..row.as_ref().column_count() {
            vals.push(row_val(row, i));
        }
        rows.push(vals);
    }
    Ok(TablePage {
        columns,
        rows,
        total: Some(total),
    })
}

/// 更新单元格（按主键定位；无主键表用 rowid）
pub async fn update_cell(
    entry: &ConnEntry,
    _dbname: &str,
    table: &str,
    rowid: i64,
    column: &str,
    value: Option<String>,
) -> Result<(), String> {
    let conn = conn_of(entry)?.lock().await;
    let t = table.replace('"', "\"\"");
    let c = column.replace('"', "\"\"");
    let sql = match &value {
        Some(_v) => format!("UPDATE \"{t}\" SET \"{c}\" = ?1 WHERE rowid = ?2"),
        None => format!("UPDATE \"{t}\" SET \"{c}\" = NULL WHERE rowid = ?2"),
    };
    match &value {
        Some(v) => {
            conn.execute(&sql, rusqlite::params![v, rowid])
                .map_err(|e| format!("更新失败: {e}"))?;
        }
        None => {
            conn.execute(&sql, rusqlite::params![rowid])
                .map_err(|e| format!("更新失败: {e}"))?;
        }
    }
    Ok(())
}

/// 删除行（按 rowid）
pub async fn delete_row(entry: &ConnEntry, _dbname: &str, table: &str, rowid: i64) -> Result<(), String> {
    let conn = conn_of(entry)?.lock().await;
    let t = table.replace('"', "\"\"");
    conn.execute(&format!("DELETE FROM \"{t}\" WHERE rowid = ?1"), rusqlite::params![rowid])
        .map_err(|e| format!("删除失败: {e}"))?;
    Ok(())
}

/// 新增行（所有列默认值）
pub async fn insert_row(entry: &ConnEntry, _dbname: &str, table: &str) -> Result<i32, String> {
    let conn = conn_of(entry)?.lock().await;
    let t = table.replace('"', "\"\"");
    conn.execute(&format!("INSERT INTO \"{t}\" DEFAULT VALUES"), [])
        .map_err(|e| format!("插入失败: {e}"))?;
    Ok(conn.last_insert_rowid() as i32)
}

/// 导出 CSV（写盘）
pub async fn export_csv(entry: &ConnEntry, _dbname: &str, table: &str) -> Result<String, String> {
    let conn = conn_of(entry)?.lock().await;
    let t = table.replace('"', "\"\"");
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM \"{t}\""))
        .map_err(|e| format!("SQL 错误: {e}"))?;
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let mut out = String::from("sep=,\n");
    out.push_str(&col_names.join(","));
    out.push('\n');
    let mut iter = stmt.query([]).map_err(|e| format!("执行失败: {e}"))?;
    while let Some(row) = iter.next().map_err(|e| format!("读取失败: {e}"))? {
        let mut cells = Vec::new();
        for i in 0..row.as_ref().column_count() {
            let s = match row_val(row, i) {
                serde_json::Value::Null => String::new(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            cells.push(format!("\"{}\"", s.replace('"', "\"\"")));
        }
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    let fname = format!("tusk-sqlite-{}-{}.csv", t, crate::db::pg::now_str());
    let path = std::env::var("HOME")
        .map(|h| format!("{h}/Downloads/{fname}"))
        .unwrap_or_else(|_| format!("/tmp/{fname}"));
    std::fs::write(&path, out).map_err(|e| format!("写文件失败: {e}"))?;
    Ok(path)
}

/// 导入 CSV（表头匹配列，逐行 INSERT）
pub async fn import_csv(entry: &ConnEntry, _dbname: &str, table: &str, path: &str) -> Result<u64, String> {
    let conn = conn_of(entry)?.lock().await;
    let mut rdr = csv::Reader::from_path(path).map_err(|e| format!("读取 CSV 失败: {e}"))?;
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| format!("解析表头失败: {e}"))?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    // 目标列
    let mut col_stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{}\")", table.replace('"', "\"\"")))
        .map_err(|e| format!("SQL 错误: {e}"))?;
    let cols = col_stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("查询失败: {e}"))?;
    let target_cols: Vec<String> = cols.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    // 匹配
    let mut matched: Vec<usize> = Vec::new();
    for (i, h) in headers.iter().enumerate() {
        if target_cols.iter().any(|c| c.eq_ignore_ascii_case(h)) {
            matched.push(i);
        }
    }
    if matched.is_empty() {
        return Err(format!("CSV 表头与目标表列无匹配（目标列: {}）", target_cols.join(", ")));
    }
    let t = table.replace('"', "\"\"");
    let qcols: Vec<String> = matched
        .iter()
        .map(|&i| format!("\"{}\"", headers[i].replace('"', "\"\"")))
        .collect();
    let placeholders = vec!["?1".to_string(); matched.len()].join(", ");
    let insert_sql = format!("INSERT INTO \"{t}\" ({}) VALUES ({placeholders})", qcols.join(", "));
    let tx = conn.unchecked_transaction().map_err(|e| format!("事务失败: {e}"))?;
    let mut count: u64 = 0;
    {
        let mut stmt = tx.prepare(&insert_sql).map_err(|e| format!("SQL 错误: {e}"))?;
        for rec in rdr.records() {
            let rec = rec.map_err(|e| format!("解析第 {} 行失败: {e}", count + 2))?;
            let vals: Vec<String> = matched
                .iter()
                .map(|&i| rec.get(i).unwrap_or("").to_string())
                .collect();
            let params: Vec<&str> = vals.iter().map(|s| s.as_str()).collect();
            stmt.execute(rusqlite::params_from_iter(params.iter().copied()))
                .map_err(|e| format!("插入第 {} 行失败: {e}", count + 2))?;
            count += 1;
        }
    }
    tx.commit().map_err(|e| format!("提交失败: {e}"))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    fn test_entry(path: &str) -> ConnEntry {
        let conn = Connection::open(path).expect("打开 sqlite 失败");
        ConnEntry {
            client: None,
            cfg: ConnConfig {
                db_type: "sqlite".into(),
                host: String::new(),
                port: 0,
                user: String::new(),
                password: String::new(),
                dbname: String::new(),
                path: path.into(),
            },
            sqlite: Some(Arc::new(TokioMutex::new(conn))),
        }
    }

    #[test]
    fn test_sqlite_core_chain() {
        let dir = std::env::temp_dir().join(format!("tusk_sqlite_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        let _ = std::fs::remove_file(&db_path);

        let entry = test_entry(db_path.to_str().unwrap());
        let rt = tokio::runtime::Runtime::new().unwrap();

        // 1. 建表 + 插数据
        rt.block_on(async {
            let r = query(&entry, "CREATE TABLE t1 (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, price REAL)").await.expect("建表失败");
            assert!(r.rows_affected.is_some(), "DDL 应报告影响行数");
            let r = query(&entry, "INSERT INTO t1 (name, price) VALUES ('apple', 1.5), ('banana', 2.75), ('cherry', NULL)").await.expect("插入失败");
            assert_eq!(r.rows_affected, Some(3), "应插入 3 行");

            // 2. 查询
            let r = query(&entry, "SELECT name, price FROM t1 ORDER BY id").await.expect("查询失败");
            assert_eq!(r.columns.len(), 2);
            assert_eq!(r.rows.len(), 3);
            assert_eq!(r.rows[0][0], serde_json::Value::String("apple".into()));
            assert_eq!(r.rows[0][1], serde_json::Value::from(1.5f64));
            assert_eq!(r.rows[2][1], serde_json::Value::Null, "NULL 应保留");

            // 3. 库/表列表
            let dbs = list_databases(&entry).await.expect("库列表失败");
            assert_eq!(dbs.len(), 1, "SQLite 单库");
            let tables = list_tables(&entry, "main").await.expect("表列表失败");
            assert_eq!(tables.len(), 1);
            assert_eq!(tables[0].name, "t1");

            // 4. 列
            let cols = list_columns(&entry, "main", "t1").await.expect("列失败");
            assert_eq!(cols.len(), 3);
            assert!(cols[0].is_pk, "id 应是主键");

            // 5. 分页（每页 2 行）
            let page = paginate_table(&entry, "main", "t1", 0, 2, None).await.expect("分页失败");
            assert_eq!(page.rows.len(), 2);
            assert_eq!(page.total, Some(3));
            let page2 = paginate_table(&entry, "main", "t1", 1, 2, None).await.expect("第二页失败");
            assert_eq!(page2.rows.len(), 1);

            // 6. 更新（rowid=2）
            update_cell(&entry, "main", "t1", 2, "name", Some("orange".into())).await.expect("更新失败");
            let r = query(&entry, "SELECT name FROM t1 WHERE rowid = 2").await.expect("查询失败");
            assert_eq!(r.rows[0][0], serde_json::Value::String("orange".into()));

            // 7. 删除（rowid=3）
            delete_row(&entry, "main", "t1", 3).await.expect("删除失败");
            let r = query(&entry, "SELECT count(*) FROM t1").await.expect("查询失败");
            assert_eq!(r.rows[0][0], serde_json::Value::from(2i64), "删后剩 2 行");

            // 8. 新增行
            insert_row(&entry, "main", "t1").await.expect("新增失败");
            let r = query(&entry, "SELECT count(*) FROM t1").await.expect("查询失败");
            assert_eq!(r.rows[0][0], serde_json::Value::from(3i64));

            // 9. 导出 CSV
            let path = export_csv(&entry, "main", "t1").await.expect("导出失败");
            let csv = std::fs::read_to_string(&path).expect("读导出文件失败");
            assert!(csv.contains("name"), "CSV 应含表头");
            assert!(csv.contains("orange"), "CSV 应含更新后的数据");
            let _ = std::fs::remove_file(&path);
        });

        // 清理
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
