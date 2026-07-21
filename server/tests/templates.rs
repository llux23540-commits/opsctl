mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn seeded_templates_and_readable_by_operator() {
    let app = spawn().await;
    let op = app.operator().await; // non-admin can read (console needs it)
    let (s, list) = app.get("/templates", &op, "op-dev").await;
    assert_eq!(s, 200);
    let names: Vec<String> = list.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"restart".to_string()));
    assert!(names.contains(&"count".to_string()));
    // command carries {{var}} placeholders
    assert!(list.as_array().unwrap().iter().any(|t| t["command"].as_str().unwrap().contains("{{")));
}

#[tokio::test]
async fn template_rendered_as_file() {
    let app = spawn().await;
    let admin = app.admin().await;
    // seeded "restart" (ssh) → restart.md; view is readable (plaintext body),
    // frontmatter carries the kind marker
    let tid = app.get("/templates", &admin, "admin-dev").await.1
        .as_array().unwrap().iter().find(|t| t["name"] == "restart").unwrap()["id"].as_str().unwrap().to_string();
    let (s, f) = app.get(&format!("/templates/{tid}/file"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(f["filename"], "restart.md");
    assert_eq!(f["encrypted_in_git"], false);   // templates are raw
    let c = f["content"].as_str().unwrap();
    assert!(c.starts_with("---\n") && c.contains("kind: ssh"));
    assert!(c.contains("systemctl restart"));   // raw command

    // a doc-kind template also renders as .md
    let (_s, v) = app.post("/templates", &admin, "admin-dev", json!({
        "name":"runbook","kind":"doc","command":"# 重启流程\n1. 先看监控\n2. 再重启"
    })).await;
    let did = v["id"].as_str().unwrap().to_string();
    let (_s, f) = app.get(&format!("/templates/{did}/file"), &admin, "admin-dev").await;
    assert_eq!(f["filename"], "runbook.md");
    assert!(f["content"].as_str().unwrap().contains("kind: doc"));
}

#[tokio::test]
async fn template_crud_admin_only() {
    let app = spawn().await;
    let admin = app.admin().await;

    // create
    let (s, v) = app.post("/templates", &admin, "admin-dev", json!({
        "name":"deploy","kind":"ssh","command":"deploy {{app}}",
        "variables":[{"name":"app","default":"web"}]
    })).await;
    assert_eq!(s, 200);
    let id = v["id"].as_str().unwrap().to_string();

    // empty name → 400
    let (s, _) = app.post("/templates", &admin, "admin-dev", json!({"name":"","kind":"ssh","command":"x"})).await;
    assert_eq!(s, 400);

    // operator cannot create/delete
    let op = app.operator().await;
    let (s, _) = app.post("/templates", &op, "op-dev", json!({"name":"y","kind":"ssh","command":"x"})).await;
    assert_eq!(s, 403);
    let (s, _) = app.delete(&format!("/templates/{id}"), &op, "op-dev").await;
    assert_eq!(s, 403);

    // admin delete
    let (s, _) = app.delete(&format!("/templates/{id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
}
