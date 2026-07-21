mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn login_success_and_wrong_password() {
    let app = spawn().await;
    let v = app.login_raw("admin", "admin", "d1").await;
    assert!(v["token"].is_string());
    assert_eq!(v["user"]["role"], "admin");

    assert_eq!(app.status_login("admin", "nope", "d1").await, 401);
    assert_eq!(app.status_login("ghost", "x", "d1").await, 401);
}

#[tokio::test]
async fn login_requires_device_id() {
    let app = spawn().await;
    let r = app
        .client
        .post(app.url("/login"))
        .json(&json!({ "username": "admin", "password": "admin", "device_id": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
}

#[tokio::test]
async fn me_requires_matching_device() {
    let app = spawn().await;
    let token = app.login("admin", "admin", "devA").await;

    // matching device → 200
    let (s, v) = app.get("/me", &token, "devA").await;
    assert_eq!(s, 200);
    assert_eq!(v["role"], "admin");

    // wrong device id (did mismatch) → 401
    let (s, _) = app.get("/me", &token, "devB").await;
    assert_eq!(s, 401);

    // no token → 401
    let r = app.client.get(app.url("/me")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
}

#[tokio::test]
async fn session_revoke_kicks_device() {
    let app = spawn().await;
    let ta = app.login("admin", "admin", "devA").await;
    let tb = app.login("admin", "admin", "devB").await;

    // devB works before revoke
    let (s, _) = app.get("/me", &tb, "devB").await;
    assert_eq!(s, 200);

    // both sessions listed, devA marked current
    let (s, list) = app.get("/sessions", &ta, "devA").await;
    assert_eq!(s, 200);
    let arr = list.as_array().unwrap();
    assert!(arr.iter().any(|x| x["device_id"] == "devA" && x["current"] == true));
    assert!(arr.iter().any(|x| x["device_id"] == "devB"));

    // revoke devB's session from devA
    let sid_b = arr
        .iter()
        .find(|x| x["device_id"] == "devB")
        .unwrap()["sid"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, _) = app.post(&format!("/sessions/{sid_b}/revoke"), &ta, "devA", json!({})).await;
    assert_eq!(s, 200);

    // devB's original token is now rejected
    let (s, _) = app.get("/me", &tb, "devB").await;
    assert_eq!(s, 401);
}

#[tokio::test]
async fn totp_two_step_login() {
    use opsctl_server::{state::now_secs, totp};
    let app = spawn().await;
    let admin = app.admin().await;

    // enroll TOTP for admin
    let (s, start) = app.post("/profile/totp/start", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200);
    let secret = start["secret"].as_str().unwrap().to_string();
    assert!(start["otpauth_uri"].as_str().unwrap().starts_with("otpauth://totp/"));

    // confirm with the current code
    let code = totp::code_at(&secret, now_secs()).unwrap();
    let (s, conf) = app.post("/profile/totp/confirm", &admin, "admin-dev", json!({"code": code})).await;
    assert_eq!(s, 200);
    assert_eq!(conf["totp_enabled"], true);
    assert_eq!(app.get("/profile", &admin, "admin-dev").await.1["totp_enabled"], true);

    // login now requires the second step — and never leaks a code
    let v = app.login_raw("admin", "admin", "d2").await;
    assert_eq!(v["need_otp"], true);
    assert!(v.get("demo_code").is_none());
    let pending = v["pending_id"].as_str().unwrap().to_string();

    // wrong code → 400 (pending survives)
    let bad = app.client.post(app.url("/login/otp"))
        .json(&json!({"pending_id": pending, "code": "000001"})).send().await.unwrap();
    assert_eq!(bad.status().as_u16(), 400);

    // correct TOTP code → token
    let good = totp::code_at(&secret, now_secs()).unwrap();
    let ok: serde_json::Value = app.client.post(app.url("/login/otp"))
        .json(&json!({"pending_id": pending, "code": good})).send().await.unwrap().json().await.unwrap();
    assert!(ok["token"].is_string());

    // disable → login no longer requires OTP
    let (s, d) = app.post("/profile/totp/disable", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(d["totp_enabled"], false);
    assert!(app.login_raw("admin", "admin", "d3").await["token"].is_string());
}

#[tokio::test]
async fn legacy_jobs_ssh_bypass_route_removed() {
    let app = spawn().await;
    // the RBAC-bypassing top-level /jobs/ssh (non-/api) must be gone: it now
    // falls through to the SPA fallback and never runs the job handler, so the
    // response carries no job result (no "job_id").
    let body = app
        .client
        .post(format!("{}/jobs/ssh", app.base))
        .json(&json!({"targets":["web-01"],"command":"id"}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!body.contains("job_id"), "legacy /jobs/ssh must not execute jobs, got: {body}");
}

#[tokio::test]
async fn session_last_seen_present() {
    let app = spawn().await;
    let token = app.login("admin", "admin", "devA").await;
    // activity
    app.get("/me", &token, "devA").await;
    let (_s, list) = app.get("/sessions", &token, "devA").await;
    let cur = list.as_array().unwrap().iter().find(|x| x["current"] == true).unwrap();
    let last = cur["last_seen"].as_i64().unwrap();
    let created = cur["created_at"].as_i64().unwrap();
    assert!(last >= created); // refreshed on activity, never before creation
}

#[tokio::test]
async fn register_gated_by_flag() {
    let app = spawn().await;
    // closed by default → 403
    let closed = app
        .client
        .post(app.url("/register"))
        .json(&json!({ "username": "newbie", "password": "secret1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(closed.status().as_u16(), 403);

    // open it
    let admin = app.admin().await;
    app.put("/flags", &admin, "admin-dev", json!({"register_open": true, "otp_enabled": false})).await;

    // register + short password + duplicate
    let ok = app.client.post(app.url("/register")).json(&json!({"username":"newbie","password":"secret1","email":"n@x.io"})).send().await.unwrap();
    assert_eq!(ok.status().as_u16(), 200);
    let short = app.client.post(app.url("/register")).json(&json!({"username":"tiny","password":"123"})).send().await.unwrap();
    assert_eq!(short.status().as_u16(), 400);
    let dup = app.client.post(app.url("/register")).json(&json!({"username":"newbie","password":"secret1"})).send().await.unwrap();
    assert_eq!(dup.status().as_u16(), 400);

    // the new viewer can log in
    let v = app.login_raw("newbie", "secret1", "nd").await;
    assert_eq!(v["user"]["role"], "viewer");
}

#[tokio::test]
async fn login_records_session_ip() {
    let app = spawn().await;
    let admin = app.admin().await;
    let (s, v) = app.get("/sessions", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let sess = v.as_array().unwrap().iter()
        .find(|r| r["device_id"] == "admin-dev")
        .expect("session for admin-dev");
    assert_eq!(sess["ip"], "127.0.0.1", "session ip should be the loopback peer addr");
}
