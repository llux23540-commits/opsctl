mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn audit_export_csv_and_json() {
    let app = spawn().await;
    let admin = app.admin().await;
    // generate an auditable event
    app.post("/jobs/sql", &admin, "admin-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;

    // CSV export: header row + comma-separated rows
    let r = app
        .client
        .get(app.url("/audit/export?format=csv&action=sql.exec"))
        .header("Authorization", format!("Bearer {admin}"))
        .header("x-device-id", "admin-dev")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let ct = r.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.contains("text/csv"));
    let body = r.text().await.unwrap();
    assert!(body.starts_with("ts,operator_email,action,targets,payload,result"));
    assert!(body.contains("sql.exec"));

    // JSON export: an array
    let r = app
        .client
        .get(app.url("/audit/export?format=json"))
        .header("Authorization", format!("Bearer {admin}"))
        .header("x-device-id", "admin-dev")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v.is_array());

    // non-admin forbidden
    let op = app.operator().await;
    let (s, _) = app.get("/audit/export?format=csv", &op, "op-dev").await;
    assert_eq!(s, 403);
}
