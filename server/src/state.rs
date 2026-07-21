//! Shared application state passed to all handlers.

use std::sync::Arc;

use crate::config::BackupCfg;
use crate::store::Store;
use crate::vault::Vault;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub jwt_secret: Arc<String>,
    pub default_ttl_secs: i64,
    pub vault: Arc<Vault>,
    pub backup: Arc<BackupCfg>,
}

/// Current unix time in seconds.
pub fn now_secs() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
