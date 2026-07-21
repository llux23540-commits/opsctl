mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn profile_update_and_ttl_clamp() {
    let app = spawn().await;
    let admin = app.admin().await;

    let (s, p) = app.get("/profile", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(p["name"], "admin");

    // valid update
    let (s, _) = app.put("/profile", &admin, "admin-dev", json!({"name":"boss","email":"b@x.io","login_ttl_secs":86400,"login_alert":false})).await;
    assert_eq!(s, 200);
    let (_s, p2) = app.get("/profile", &admin, "admin-dev").await;
    assert_eq!(p2["name"], "boss");
    assert_eq!(p2["email"], "b@x.io");
    assert_eq!(p2["login_alert"], false);

    // ttl clamped to <= 30 days
    let (_s, v) = app.put("/profile", &admin, "admin-dev", json!({"name":"boss","login_ttl_secs": 999999999})).await;
    assert!(v["login_ttl_secs"].as_i64().unwrap() <= 30 * 24 * 3600);

    // empty name → 400
    let (s, _) = app.put("/profile", &admin, "admin-dev", json!({"name":"  ","login_ttl_secs":86400})).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn flags_public_get_admin_put() {
    let app = spawn().await;
    let admin = app.admin().await;

    // GET is public (no auth needed)
    let r = app.client.get(app.url("/flags")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);

    // operator cannot PUT
    let op = app.operator().await;
    let (s, _) = app.put("/flags", &op, "op-dev", json!({"register_open":true,"otp_enabled":false})).await;
    assert_eq!(s, 403);

    // admin can
    let (s, _) = app.put("/flags", &admin, "admin-dev", json!({"register_open":true,"otp_enabled":false})).await;
    assert_eq!(s, 200);
    let flags: serde_json::Value = app.client.get(app.url("/flags")).send().await.unwrap().json().await.unwrap();
    assert_eq!(flags["register_open"], true);
}

#[tokio::test]
async fn telegram_bind_flow() {
    let app = spawn().await;
    let admin = app.admin().await;

    let (s, start) = app.post("/telegram/bind/start", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200);
    let code = start["code"].as_str().unwrap().to_string();

    // wrong code → 400
    let (s, _) = app.post("/telegram/bind/confirm", &admin, "admin-dev", json!({"code":"ZZZZZZ"})).await;
    assert_eq!(s, 400);

    // correct code → bound
    let (s, _) = app.post("/telegram/bind/confirm", &admin, "admin-dev", json!({"code": code})).await;
    assert_eq!(s, 200);
    assert_eq!(app.get("/profile", &admin, "admin-dev").await.1["telegram_bound"], true);

    // unbind
    app.post("/telegram/unbind", &admin, "admin-dev", json!({})).await;
    assert_eq!(app.get("/profile", &admin, "admin-dev").await.1["telegram_bound"], false);
}

#[tokio::test]
async fn git_config_admin_only() {
    let app = spawn().await;
    let admin = app.admin().await;

    // default config nested under "config"; git status included
    let (s, cfg) = app.get("/settings/git", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(cfg["config"]["mode"], "folder");
    assert_eq!(cfg["git_installed"], true);

    // operator forbidden
    let op = app.operator().await;
    let (s, _) = app.get("/settings/git", &op, "op-dev").await;
    assert_eq!(s, 403);

    // save remote config (with credential) → persisted; credential never echoed back
    let (s, _) = app.put("/settings/git", &admin, "admin-dev", json!({"mode":"remote","url":"https://x/y.git","branch":"main","username":"u","credential":"tok","auto_push":true})).await;
    assert_eq!(s, 200);
    let (_s, saved) = app.get("/settings/git", &admin, "admin-dev").await;
    assert_eq!(saved["config"]["mode"], "remote");
    assert_eq!(saved["config"]["credential"], "");        // write-only
    assert_eq!(saved["config"]["credential_set"], true);

    // empty credential on save keeps the stored one
    app.put("/settings/git", &admin, "admin-dev", json!({"mode":"remote","url":"https://x/y.git","branch":"main","username":"u","credential":"","auto_push":true})).await;
    assert_eq!(app.get("/settings/git", &admin, "admin-dev").await.1["config"]["credential_set"], true);

    // local test works
    app.put("/settings/git", &admin, "admin-dev", json!({"mode":"folder"})).await;
    let (s, act) = app.post("/settings/git/test", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(act["ok"], true);
}
