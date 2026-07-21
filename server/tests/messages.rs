mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn login_produces_notification_and_read_flow() {
    let app = spawn().await;
    // logging in as admin (via admin()) already produced a login notification
    let admin = app.admin().await;

    let (s, list) = app.get("/messages", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert!(list.as_array().unwrap().iter().any(|m| m["kind"] == "login"), "expected a login notification");

    let unread0 = app.get("/messages/unread-count", &admin, "admin-dev").await.1["count"].as_i64().unwrap();
    assert!(unread0 >= 1);

    // mark one read → unread decreases
    let id = list.as_array().unwrap().iter().find(|m| m["read"] == 0).unwrap()["id"].as_str().unwrap().to_string();
    let (s, _) = app.post(&format!("/messages/{id}/read"), &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200);
    let unread1 = app.get("/messages/unread-count", &admin, "admin-dev").await.1["count"].as_i64().unwrap();
    assert_eq!(unread1, unread0 - 1);

    // read-all → zero
    app.post("/messages/read-all", &admin, "admin-dev", json!({})).await;
    let unread2 = app.get("/messages/unread-count", &admin, "admin-dev").await.1["count"].as_i64().unwrap();
    assert_eq!(unread2, 0);

    // delete one → removed from list
    let before = app.get("/messages", &admin, "admin-dev").await.1.as_array().unwrap().len();
    let (s, _) = app.delete(&format!("/messages/{id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let after = app.get("/messages", &admin, "admin-dev").await.1.as_array().unwrap().len();
    assert_eq!(after, before - 1);
}

#[tokio::test]
async fn decision_notifies_requester() {
    let app = spawn().await;
    let admin = app.admin().await;
    // enable approval on sql rule
    let r = app.get("/rules", &admin, "admin-dev").await.1;
    let sub = r.as_array().unwrap().iter().find(|x| x["id"] == "rule-op-sql").unwrap()["subject_user_id"].clone();
    app.put("/rules/rule-op-sql", &admin, "admin-dev", json!({
        "name":"op sql","subject_user_id": sub,"selector_kind":"assets","selector":"db-demo",
        "system_user_id":"su-demodb","actions":["sql"],"needs_approval":true
    })).await;

    let op = app.operator().await;
    let op_unread_before = app.get("/messages/unread-count", &op, "op-dev").await.1["count"].as_i64().unwrap();
    app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;

    let pending = app.get("/approvals", &admin, "admin-dev").await.1;
    let id = pending.as_array().unwrap().iter().find(|a| a["state"] == "pending").unwrap()["id"].as_str().unwrap().to_string();
    app.post(&format!("/approvals/{id}/decide"), &admin, "admin-dev", json!({"verdict":"approve"})).await;

    // requester (operator) received an approval notification
    let op_unread_after = app.get("/messages/unread-count", &op, "op-dev").await.1["count"].as_i64().unwrap();
    assert!(op_unread_after > op_unread_before, "requester should be notified of the decision");
}
