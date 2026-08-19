// Tusk — 共享数据模型（DTO）
// 与具体数据库驱动解耦，PG/SQLite 等驱动共用这些类型
use serde::{Deserialize, Serialize};

#[derive(Serialize, Debug)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
}


#[derive(Serialize, Debug)]
pub struct QueryResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub rows_affected: Option<u64>,
    pub message: Option<String>,
}


#[derive(Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub version: String,
    pub user: String,
    pub host: String,
    pub port: u16,
}


/// 连接配置（与 UI 表单字段一一对应）
#[derive(Debug, Clone)]
pub struct ConnConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub dbname: String,
}


#[derive(Serialize, Debug)]
pub struct MultiResult {
    pub results: Vec<QueryResult>,
}


#[derive(Serialize, Debug)]
pub struct DatabaseInfo {
    pub name: String,
}


#[derive(Serialize, Debug)]
pub struct TableInfo {
    pub name: String,
    pub kind: String, // "table" | "view"
}


#[derive(Serialize, Debug)]
pub struct SchemaColumn {
    pub name: String,
    pub type_name: String,
    pub is_nullable: String,
    pub default: Option<String>,
    pub is_pk: bool,
    pub comment: Option<String>,
}


/// 核心：列出表的索引（排除主键索引，主键已在结构里显示）
#[derive(Serialize, Debug)]
pub struct IndexInfo {
    pub name: String,
    pub columns: String,
    pub is_unique: bool,
}


#[derive(Serialize, Debug)]
pub struct TablePage {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub total: Option<i64>,
}


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedConn {
    #[serde(default = "default_db_type")]
    pub db_type: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub dbname: String,
}


#[derive(Deserialize, Debug, Clone)]
pub struct FilterCond {
    pub column: String,
    pub op: String,
    pub value: Option<String>,
}


#[derive(Deserialize, Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_pk: bool,
    pub is_serial: bool,
    #[serde(default)]
    pub comment: Option<String>,
}


/// 结构同步差异项
#[derive(Serialize, Debug)]
pub struct SchemaDiff {
    pub table: String,
    pub action: String, // "create" | "alter" | "drop"
    pub sql: String,
}


#[derive(Deserialize, Debug, Clone)]
pub struct ColValue {
    pub name: String,
    pub value: Option<String>,
}


/// numeric 列的字符串包装：postgres-types 未实现 numeric 的 FromSql，'''
/// 这里自定义解码（PostgreSQL wire format，精度无损）
pub struct NumericString(pub String);

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


pub fn default_db_type() -> String {
    "postgres".into()
}


/// 解码 PostgreSQL numeric 二进制格式为十进制字符串
/// 格式：int16 ndigits, int16 weight, uint16 sign, int16 dscale, int16 digits[ndigits]
/// 值 = Σ digits[i] × 10000^(weight-i)，sign: 0x0000 正 / 0x4000 负 / 0xC000 NaN
pub fn numeric_to_string(raw: &[u8]) -> Result<String, Box<dyn std::error::Error + Sync + Send>> {
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


