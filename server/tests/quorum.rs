mod common;
use common::spawn;
use serde_json::{json, Value};

async fn set_sql_rule(app: &common::TestApp, admin: &str, min: i64) {
    let rules = app.get("/rules", admin, "admin-dev").await.1;
    let sub = rules.as_array().unwrap().iter()
        .find(|r| r["id"] == "rule-op-sql").unwrap()["subject_user_id"].clone();
    app.put("/rules/rule-op-sql", admin, "admin-dev", json!({
        "name":"op sql","subject_user_id": sub,"selector_kind":"assets","selector":"db-demo",
        "system_user_id":"su-demodb","actions":["sql"],"needs_approval":true,"min_approvals": min
    })).await;
}

async fn first_pending(app: &common::TestApp, admin: &str) -> Value {
    let list = app.get("/approvals", admin, "admin-dev").await.1;
    list.as_array().unwrap().iter().find(|a| a["state"] == "pending").unwrap().clone()
}

#[tokio::test]
async fn two_approvers_required() {
    let app = spawn().await;
    let admin = app.admin().await;
    set_sql_rule(&app, &admin, 2).await;

    // operator submits → held pending, needs 2 approvals
    let op = app.operator().await;
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let ap = first_pending(&app, &admin).await;
    assert_eq!(ap["min_approvals"], 2);
    let id = ap["id"].as_str().unwrap().to_string();

    // admin #1 approves → still pending (1/2), NOT executed
    let (s, v) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["state"], "pending");
    assert_eq!(v["votes"], 1);
    assert_eq!(v["need"], 2);

    // same admin approving again does NOT double-count (idempotent)
    let (_s, v) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(v["state"], "pending");
    assert_eq!(v["votes"], 1);

    // admin #2 approves → quorum reached → approved + executed
    let admin2 = app.admin2().await;
    let (s, v) = app.post(&format!("/approvals/{id}/decide"), &admin2, "admin2-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["state"], "approved");
    assert_eq!(v["votes"], 2);
    assert_eq!(v["result"]["ok"], true); // sqlite SELECT 1 really ran
}

#[tokio::test]
async fn single_reject_vetoes_even_with_quorum() {
    let app = spawn().await;
    let admin = app.admin().await;
    set_sql_rule(&app, &admin, 2).await;
    let op = app.operator().await;
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let id = first_pending(&app, &admin).await["id"].as_str().unwrap().to_string();

    // one reject rejects immediately, regardless of quorum
    let (s, v) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"reject","reason":"no"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["state"], "rejected");
}

#[tokio::test]
async fn default_min_one_approves_immediately() {
    // rules default to min_approvals=1 → single approve executes (unchanged behavior)
    let app = spawn().await;
    let admin = app.admin().await;
    set_sql_rule(&app, &admin, 1).await;
    let op = app.operator().await;
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let id = first_pending(&app, &admin).await["id"].as_str().unwrap().to_string();
    let (_s, v) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(v["state"], "approved");
}
