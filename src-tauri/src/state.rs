// Tusk — 应用共享状态（连接池）
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_postgres::Client;

use crate::models::ConnConfig;

#[derive(Clone)]
pub struct ConnEntry {
    pub client: Arc<Client>,
    pub cfg: ConnConfig,
}

pub struct AppState {
    pub conns: Mutex<HashMap<String, ConnEntry>>,
}
