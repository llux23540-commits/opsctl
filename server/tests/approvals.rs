mod common;
use common::spawn;
use serde_json::{json, Value};

/// Turn on approval for the operator SQL rule (sqlite exec really succeeds on
/// release, which lets us assert the dispatch-by-action path).
async fn enable_sql_approval(app: &common::TestApp, admin: &str) {
    let rules = app.get("/rules", admin, "admin-dev").await.1;
    let r = rules.as_array().unwrap().iter().find(|r| r["id"] == "rule-op-sql").unwrap().clone();
    app.put("/rules/rule-op-sql", admin, "admin-dev", json!({
        "name":"op sql","subject_user_id": r["subject_user_id"],
        "selector_kind":"assets","selector":"db-demo","system_user_id":"su-demodb",
        "actions":["sql"],"needs_approval":true
    })).await;
}

async fn pending_ids(app: &common::TestApp, admin: &str) -> Vec<String> {
    let list: Value = app.get("/approvals", admin, "admin-dev").await.1;
    list.as_array().unwrap().iter()
        .filter(|a| a["state"] == "pending")
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn approve_executes_and_rejects_require_reason() {
    let app = spawn().await;
    let admin = app.admin().await;
    enable_sql_approval(&app, &admin).await;
    let op = app.operator().await;

    // two pending sql requests
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let (_s, list) = app.get("/approvals", &admin, "admin-dev").await;
    let pending = list.as_array().unwrap().iter().find(|a| a["state"] == "pending").unwrap();
    assert_eq!(pending["action"], "sql");
    assert_eq!(pending["target_name"], "demo-sqlite");
    let id = pending["id"].as_str().unwrap().to_string();

    // approve → executes sql, returns rows
    let (s, v) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["state"], "approved");
    assert_eq!(v["result"]["ok"], true);

    // double decide → 400
    let (s, _) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 400);

    // new pending → reject empty reason 400, with reason ok
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 2"})).await;
    let id2 = pending_ids(&app, &admin).await[0].clone();
    let (s, _) = app.post(&format!("/approvals/{id2}/decide"), &admin, "admin-dev", json!({"verdict":"reject","reason":"  "})).await;
    assert_eq!(s, 400);
    let (s, v) = app.post(&format!("/approvals/{id2}/decide"), &admin, "admin-dev", json!({"verdict":"reject","reason":"maintenance window"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["state"], "rejected");
}

#[tokio::test]
async fn batch_and_admin_only() {
    let app = spawn().await;
    let admin = app.admin().await;
    enable_sql_approval(&app, &admin).await;
    let op = app.operator().await;

    // operator cannot decide
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let id = pending_ids(&app, &admin).await[0].clone();
    let (s, _) = app.post(&format!("/approvals/{id}/decide"), &op, "op-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 403);

    // batch reject empty reason → 400
    let ids = pending_ids(&app, &admin).await;
    let (s, _) = app.post("/approvals/decide-batch", &admin, "admin-dev", json!({"ids": ids, "verdict":"reject","reason":""})).await;
    assert_eq!(s, 400);

    // batch approve → all approved
    let ids = pending_ids(&app, &admin).await;
    let n = ids.len();
    let (s, v) = app.post("/approvals/decide-batch", &admin, "admin-dev", json!({"ids": ids, "verdict":"approve"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], n as i64);
    assert_eq!(v["failed"], 0);
    assert!(pending_ids(&app, &admin).await.is_empty());
}

#[tokio::test]
async fn rule_quick_roundtrip_and_approval_view() {
    let app = spawn().await;
    let admin = app.admin().await;

    // switch the seeded operator-ssh rule to a tg-quick approval gate (a new
    // rule would be shadowed by the fixture rule that matches first)
    let (s, v) = app.put("/rules/rule-op-web", &admin, "admin-dev", json!({
        "name":"op web tg","subject_user_id":"u-op",
        "selector_kind":"tag","selector":"tag-web","system_user_id":"su-webssh",
        "actions":["ssh"],"needs_approval":true,"quick":"tg"
    })).await;
    assert_eq!(s, 200, "update rule: {v}");

    // roundtrip on the list
    let (_s, rules) = app.get("/rules", &admin, "admin-dev").await;
    let r = rules.as_array().unwrap().iter().find(|r| r["id"] == "rule-op-web").unwrap();
    assert_eq!(r["quick"], "tg");

    // default is console
    let (_s, rules) = app.get("/rules", &admin, "admin-dev").await;
    let seeded = rules.as_array().unwrap().iter().find(|r| r["id"] == "rule-op-sql").unwrap();
    assert_eq!(seeded["quick"], "console");

    // invalid value → 400
    let (s, _v) = app.post("/rules", &admin, "admin-dev", json!({
        "name":"bad","subject_user_id":"u-op","selector_kind":"tag","selector":"tag-web",
        "actions":["ssh"],"needs_approval":true,"quick":"bogus"
    })).await;
    assert_eq!(s, 400);

    // operator hits the tg rule → approval view carries quick
    let op = app.operator().await;
    let (_s, v) = app.post("/jobs/ssh", &op, "op-dev", json!({"targets":["web-01"],"command":"uptime"})).await;
    assert_eq!(v["results"][0]["pending"], true);
    let (_s, approvals) = app.get("/approvals", &admin, "admin-dev").await;
    let ap = approvals.as_array().unwrap().iter().find(|a| a["state"] == "pending").unwrap();
    assert_eq!(ap["quick"], "tg");
}
