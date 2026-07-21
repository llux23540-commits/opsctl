mod common;
use common::spawn;
use serde_json::{json, Value};

/// Configure rule-op-sql: needs approval, quorum `min`, designated `approvers`.
async fn set_rule(app: &common::TestApp, admin: &str, min: i64, approvers: Vec<&str>) {
    let rules = app.get("/rules", admin, "admin-dev").await.1;
    let sub = rules.as_array().unwrap().iter()
        .find(|r| r["id"] == "rule-op-sql").unwrap()["subject_user_id"].clone();
    app.put("/rules/rule-op-sql", admin, "admin-dev", json!({
        "name":"op sql","subject_user_id": sub,"selector_kind":"assets","selector":"db-demo",
        "system_user_id":"su-demodb","actions":["sql"],"needs_approval":true,
        "min_approvals": min, "approver_ids": approvers
    })).await;
}

async fn first_pending(app: &common::TestApp, admin: &str) -> Value {
    let list = app.get("/approvals", admin, "admin-dev").await.1;
    list.as_array().unwrap().iter().find(|a| a["state"] == "pending").unwrap().clone()
}

#[tokio::test]
async fn only_designated_approver_can_decide() {
    let app = spawn().await;
    let admin = app.admin().await;      // u-admin
    set_rule(&app, &admin, 1, vec!["u-admin2"]).await; // only admin2 may approve

    let op = app.operator().await;
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let ap = first_pending(&app, &admin).await;
    let id = ap["id"].as_str().unwrap().to_string();
    // the approval carries the designated approver name
    assert!(ap["approvers"].as_array().unwrap().iter().any(|n| n == "admin2"));

    // admin (not designated) is rejected with 403
    let (s, _) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 403);
    // still pending
    assert_eq!(first_pending(&app, &admin).await["id"], id.as_str());

    // admin2 (designated) approves → executed
    let admin2 = app.admin2().await;
    let (s, v) = app.post(&format!("/approvals/{id}/decide"), &admin2, "admin2-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["state"], "approved");
    assert_eq!(v["result"]["ok"], true);
}

#[tokio::test]
async fn designated_quorum_needs_both() {
    let app = spawn().await;
    let admin = app.admin().await;
    set_rule(&app, &admin, 2, vec!["u-admin", "u-admin2"]).await; // both must approve

    let op = app.operator().await;
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let id = first_pending(&app, &admin).await["id"].as_str().unwrap().to_string();

    // admin approves → still pending 1/2
    let (_s, v) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(v["state"], "pending");
    assert_eq!(v["votes"], 1);
    assert_eq!(v["need"], 2);

    // admin2 approves → quorum → approved
    let admin2 = app.admin2().await;
    let (_s, v) = app.post(&format!("/approvals/{id}/decide"), &admin2, "admin2-dev", json!({"verdict":"approve"})).await;
    assert_eq!(v["state"], "approved");
}

#[tokio::test]
async fn empty_approvers_means_any_admin() {
    // approver_ids empty → any admin can approve (regression for the plain quorum path)
    let app = spawn().await;
    let admin = app.admin().await;
    set_rule(&app, &admin, 1, vec![]).await;
    let op = app.operator().await;
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let id = first_pending(&app, &admin).await["id"].as_str().unwrap().to_string();
    let (s, v) = app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["state"], "approved");
}
