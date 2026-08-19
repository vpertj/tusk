// Tusk — 应用共享状态（连接池）
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
}
