mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn admin_sees_all_operator_sees_authorized() {
    let app = spawn().await;
    let admin = app.admin().await;
    let (s, assets) = app.get("/assets", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let ids: Vec<String> = assets.as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
    for want in ["site-east", "web-01", "web-02", "db-demo"] {
        assert!(ids.contains(&want.to_string()), "admin should see {want}");
    }
    // every asset carries tag_ids
    assert!(assets.as_array().unwrap().iter().all(|a| a["tag_ids"].is_array()));

    let op = app.operator().await;
    let (_s, oassets) = app.get("/assets", &op, "op-dev").await;
    let oids: Vec<String> = oassets.as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
    // operator sees rule-matched assets + ancestor site
    assert!(oids.contains(&"web-01".to_string()));
    assert!(oids.contains(&"web-02".to_string()));
    assert!(oids.contains(&"site-east".to_string()));
    // db-demo is tag-web too (rule-op-web is ssh on tag-web) so it's visible;
    // the key negative: operator never sees more than admin.
    assert!(oids.len() <= ids.len());
}

#[tokio::test]
async fn ssh_authorized_vs_unauthorized() {
    let app = spawn().await;
    let op = app.operator().await;

    // authorized target: not "未授权"; exec fails (no sshd) but the gate passed
    let (s, v) = app.post("/jobs/ssh", &op, "op-dev", json!({"targets":["web-01"],"command":"uptime"})).await;
    assert_eq!(s, 200);
    let r = &v["results"][0];
    let err = r["error"].as_str().unwrap_or("");
    assert!(!err.contains("未授权"), "authorized target should pass the gate, got {err}");

    // unauthorized target (site-east is a container / not granted for exec)
    let (_s, v2) = app.post("/jobs/ssh", &op, "op-dev", json!({"targets":["site-east"],"command":"uptime"})).await;
    assert!(v2["results"][0]["error"].as_str().unwrap_or("").contains("未授权"));
}

#[tokio::test]
async fn sql_action_requires_sql_rule() {
    let app = spawn().await;
    let op = app.operator().await;
    // operator has rule-op-sql (sql on db-demo) → authorized
    let (s, v) = app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    assert_eq!(s, 200);
    assert!(!v["results"][0]["error"].as_str().unwrap_or("").contains("未授权"));
    assert_eq!(v["results"][0]["ok"], true);
}
