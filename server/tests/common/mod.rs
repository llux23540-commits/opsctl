//! Shared integration-test harness: spins the real router on an ephemeral port
//! backed by a throwaway sqlite DB seeded with the deterministic fixture.

#![allow(dead_code)]

use std::sync::Arc;

use opsctl_server::state::AppState;
use opsctl_server::vault::Vault;
use opsctl_server::{build_router, connect_and_init, seed_fixture};
use reqwest::Client;
use serde_json::{json, Value};

pub const TEST_PASSPHRASE: &str = "test-unseal-pass";

pub struct TestApp {
    pub base: String,
    pub client: Client,
    /// sqlite URL of this instance's DB, for direct at-rest assertions.
    pub db_url: String,
    /// Isolated backup snapshot directory for this instance.
    pub backup_dir: String,
}

/// Unique-ish relative path under `target/test-dbs` (avoids Windows drive-letter
/// URL issues and keeps forward slashes for the sqlite URL).
fn tmp_db(tag: &str) -> String {
    // uuid gives uniqueness without Math.random / time.
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::create_dir_all("target/test-dbs").ok();
    format!("target/test-dbs/{tag}-{id}.db")
}

/// Boot a fresh, unsealed app instance (default for most tests).
pub async fn spawn() -> TestApp {
    spawn_inner(true).await
}

/// Boot a fresh app with the vault left SEALED (no passphrase, secrets not
/// migrated to ciphertext).
pub async fn spawn_sealed() -> TestApp {
    spawn_inner(false).await
}

async fn spawn_inner(unseal: bool) -> TestApp {
    let main_db = tmp_db("main");
    let demo_db = tmp_db("demo");
    let db_url = format!("sqlite://{main_db}?mode=rwc");
    let store = connect_and_init(&db_url).await.expect("init store");
    seed_fixture(&store, &demo_db).await.expect("seed fixture");

    let vault = Arc::new(Vault::new());
    if unseal {
        vault.unseal(TEST_PASSPHRASE, &store).await.expect("unseal");
        vault.migrate_plaintext(&store).await.expect("migrate");
    }

    let backup_dir = format!(
        "target/test-backups/{}",
        uuid::Uuid::new_v4().simple()
    );
    let state = AppState {
        store,
        jwt_secret: Arc::new("test-secret".to_string()),
        default_ttl_secs: 7 * 24 * 3600,
        vault,
        backup: Arc::new(opsctl_server::config::BackupCfg {
            enabled: true,
            retention_days: 30,
            dir: backup_dir.clone(),
        }),
        ws: Arc::new(opsctl_server::ws::WsHub::new()),
    };
    tokio::spawn(opsctl_server::ws::run_dispatcher(state.clone()));
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });

    TestApp {
        base: format!("http://{addr}"),
        client: Client::new(),
        db_url,
        backup_dir,
    }
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("{}/api{path}", self.base)
    }

    /// Log in and return the bearer token. Panics if login didn't return a token
    /// (use `login_raw` for the OTP / failure paths).
    pub async fn login(&self, user: &str, pass: &str, device: &str) -> String {
        let v = self.login_raw(user, pass, device).await;
        v["token"].as_str().expect("login token").to_string()
    }

    pub async fn login_raw(&self, user: &str, pass: &str, device: &str) -> Value {
        self.client
            .post(self.url("/login"))
            .json(&json!({ "username": user, "password": pass, "device_id": device }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    pub async fn status_login(&self, user: &str, pass: &str, device: &str) -> u16 {
        self.client
            .post(self.url("/login"))
            .json(&json!({ "username": user, "password": pass, "device_id": device }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    /// Authenticated GET → (status, json).
    pub async fn get(&self, path: &str, token: &str, device: &str) -> (u16, Value) {
        let r = self
            .client
            .get(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .header("x-device-id", device)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn post(&self, path: &str, token: &str, device: &str, body: Value) -> (u16, Value) {
        let r = self
            .client
            .post(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .header("x-device-id", device)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn put(&self, path: &str, token: &str, device: &str, body: Value) -> (u16, Value) {
        let r = self
            .client
            .put(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .header("x-device-id", device)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn delete(&self, path: &str, token: &str, device: &str) -> (u16, Value) {
        let r = self
            .client
            .delete(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .header("x-device-id", device)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    /// Read the raw (at-rest) secret of a system-user directly from this
    /// instance's DB — for asserting encryption at rest.
    pub async fn stored_secret(&self, id: &str) -> String {
        let store = opsctl_server::store::Store::connect(&self.db_url).await.unwrap();
        store.get_system_user(id).await.unwrap().map(|s| s.secret).unwrap_or_default()
    }

    /// Convenience: admin token bound to device "admin-dev".
    pub async fn admin(&self) -> String {
        self.login("admin", "admin", "admin-dev").await
    }
    pub async fn operator(&self) -> String {
        self.login("operator", "operator", "op-dev").await
    }
    /// Second admin (for multi-approver quorum tests).
    pub async fn admin2(&self) -> String {
        self.login("admin2", "admin2", "admin2-dev").await
    }
}
