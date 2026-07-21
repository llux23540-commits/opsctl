mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn manual_backup_writes_snapshot_and_status() {
    let app = spawn().await;
    let admin = app.admin().await;

    let (s, v) = app.post("/backup/run", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200, "backup run: {v}");
    assert_eq!(v["ok"], true);
    let file = v["file"].as_str().unwrap().to_string();
    let meta = std::fs::metadata(&file).expect("snapshot file exists");
    assert!(meta.len() > 0, "snapshot not empty");

    let (s, st) = app.get("/backup/status", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert!(st["last_at"].as_i64().unwrap() > 0);
    assert!(st["count"].as_i64().unwrap() >= 1);
    assert_eq!(st["retention_days"], 30);
    assert!(st["next_at"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn backup_run_admin_only_status_any_role() {
    let app = spawn().await;
    let op = app.operator().await;

    let (s, _v) = app.post("/backup/run", &op, "op-dev", json!({})).await;
    assert_eq!(s, 403);

    let (s, v) = app.get("/backup/status", &op, "op-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["enabled"], true);
}
