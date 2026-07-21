mod common;
use common::{spawn, spawn_sealed, TEST_PASSPHRASE};
use serde_json::json;

#[tokio::test]
async fn secrets_encrypted_at_rest_and_hidden_from_api() {
    let app = spawn().await; // unsealed
    let admin = app.admin().await;

    // create an account with a secret
    let (s, v) = app.post("/accounts", &admin, "admin-dev", json!({
        "name":"vaulted","kind":"ssh_pw","username":"u","secret":"s3cr3t-value"
    })).await;
    assert_eq!(s, 200);
    let id = v["id"].as_str().unwrap().to_string();

    // at rest: stored ciphertext is v1: prefixed and does NOT contain the plaintext
    let stored = app.stored_secret(&id).await;
    assert!(stored.starts_with("v1:"), "secret should be encrypted, got: {stored}");
    assert!(!stored.contains("s3cr3t-value"));

    // seeded fixture secret (su-webssh = "pw") was migrated to ciphertext too
    let seeded = app.stored_secret("su-webssh").await;
    assert!(seeded.starts_with("v1:"));

    // API never returns the secret
    let (_s, list) = app.get("/accounts", &admin, "admin-dev").await;
    let acc = list.as_array().unwrap().iter().find(|a| a["id"] == id.as_str()).unwrap();
    assert!(acc.get("secret").map(|x| x.is_null()).unwrap_or(true));
}

#[tokio::test]
async fn exec_decrypts_transparently() {
    let app = spawn().await;
    let op = app.operator().await;
    // operator ssh on web-01 whose account secret is now encrypted; the gate
    // passes and exec fails only on the (absent) sshd connection — never on the
    // vault ("金库已封存") or authorization ("未授权").
    let (_s, v) = app.post("/jobs/ssh", &op, "op-dev", json!({"targets":["web-01"],"command":"id"})).await;
    let err = v["results"][0]["error"].as_str().unwrap_or("");
    assert!(!err.contains("未授权"), "gate should pass, got {err}");
    assert!(!err.contains("金库"), "vault should be unsealed, got {err}");
}

#[tokio::test]
async fn sealed_blocks_secret_writes_until_unseal() {
    let app = spawn_sealed().await;
    let admin = app.admin().await;

    // status: sealed
    let (s, st) = app.get("/vault/status", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(st["sealed"], true);

    // creating an account WITH a secret is blocked (503)
    let (s, _) = app.post("/accounts", &admin, "admin-dev", json!({"name":"x","kind":"ssh_pw","username":"u","secret":"hunter2"})).await;
    assert_eq!(s, 503);

    // creating one with an EMPTY secret still works
    let (s, _) = app.post("/accounts", &admin, "admin-dev", json!({"name":"nopw","kind":"db_pw","username":"u","secret":""})).await;
    assert_eq!(s, 200);

    // unseal with the passphrase → sealed=false, migrates seeded plaintext
    let (s, un) = app.post("/vault/unseal", &admin, "admin-dev", json!({"passphrase": TEST_PASSPHRASE})).await;
    assert_eq!(s, 200);
    assert_eq!(un["sealed"], false);

    // now the secret write works and is encrypted
    let (s, v) = app.post("/accounts", &admin, "admin-dev", json!({"name":"y","kind":"ssh_pw","username":"u","secret":"hunter2"})).await;
    assert_eq!(s, 200);
    let id = v["id"].as_str().unwrap().to_string();
    assert!(app.stored_secret(&id).await.starts_with("v1:"));
}

#[tokio::test]
async fn unseal_wrong_passphrase_rejected() {
    let app = spawn_sealed().await;
    let admin = app.admin().await;
    // establish the passphrase
    app.post("/vault/unseal", &admin, "admin-dev", json!({"passphrase": TEST_PASSPHRASE})).await;
    // re-seal, then try a wrong passphrase
    app.post("/vault/seal", &admin, "admin-dev", json!({})).await;
    let (s, _) = app.post("/vault/unseal", &admin, "admin-dev", json!({"passphrase":"wrong-pass"})).await;
    assert_eq!(s, 400);
    // status still sealed
    assert_eq!(app.get("/vault/status", &admin, "admin-dev").await.1["sealed"], true);
}
