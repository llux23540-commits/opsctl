mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn ssh_node_file_view() {
    let app = spawn().await;
    let admin = app.admin().await;
    // web-01 is a seeded server node bound to su-webssh
    let (s, f) = app.get("/assets/web-01/file", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(f["filename"], "web-01.md");
    assert_eq!(f["path"], "ssh/web-01.md");
    assert_eq!(f["encrypted_in_git"], true);
    let c = f["content"].as_str().unwrap();
    assert!(c.contains("opsctl-node: web-01"));
    assert!(c.contains("root"));        // username readable in the view
    assert!(c.contains("***"));         // secret masked, never shown
    assert!(!c.contains("\"secret\": \"pw\""));

    // operator cannot read SSH node config
    let op = app.operator().await;
    let (s, _) = app.get("/assets/web-01/file", &op, "op-dev").await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn asset_crud_and_admin_only() {
    let app = spawn().await;
    let admin = app.admin().await;

    // create
    let (s, v) = app.post("/assets", &admin, "admin-dev", json!({
        "name":"db-01","kind":"database","parent_id":"site-east","host":"data/x.db","port":0
    })).await;
    assert_eq!(s, 200);
    let id = v["id"].as_str().unwrap().to_string();

    // detail carries tag_ids/account_ids
    let (s, d) = app.get(&format!("/assets/{id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(d["asset"]["name"], "db-01");
    assert!(d["tag_ids"].is_array() && d["account_ids"].is_array());

    // update
    let (s, _) = app.put(&format!("/assets/{id}"), &admin, "admin-dev", json!({
        "name":"db-01b","kind":"database","parent_id":"site-east","host":"data/x.db","port":0,"status":"enabled"
    })).await;
    assert_eq!(s, 200);

    // delete
    let (s, _) = app.delete(&format!("/assets/{id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);

    // non-admin cannot create
    let op = app.operator().await;
    let (s, _) = app.post("/assets", &op, "op-dev", json!({"name":"x","kind":"server"})).await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn delete_protections() {
    let app = spawn().await;
    let admin = app.admin().await;

    // tag referenced by a rule → 400
    let (s, v) = app.delete("/tags/tag-web", &admin, "admin-dev").await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("规则"));

    // site with children → 400
    let (s, v) = app.delete("/assets/site-east", &admin, "admin-dev").await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("节点"));

    // account referenced by a rule → 400
    let (s, v) = app.delete("/accounts/su-webssh", &admin, "admin-dev").await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("规则"));
}

#[tokio::test]
async fn account_update_keeps_secret_when_blank() {
    let app = spawn().await;
    let admin = app.admin().await;
    // create an account with a secret
    let (_s, v) = app.post("/accounts", &admin, "admin-dev", json!({"name":"acc1","kind":"ssh_pw","username":"u","secret":"topsecret"})).await;
    let id = v["id"].as_str().unwrap().to_string();
    // update with blank secret + new username → should not error
    let (s, _) = app.put(&format!("/accounts/{id}"), &admin, "admin-dev", json!({"name":"acc1","kind":"ssh_pw","username":"u2","secret":""})).await;
    assert_eq!(s, 200);
    // list shows updated username (secret is never serialized)
    let (_s, list) = app.get("/accounts", &admin, "admin-dev").await;
    let acc = list.as_array().unwrap().iter().find(|a| a["id"] == id.as_str()).unwrap();
    assert_eq!(acc["username"], "u2");
    assert!(acc.get("secret").map(|x| x.is_null()).unwrap_or(true) || acc["secret"] == "");
}

#[tokio::test]
async fn tag_crud() {
    let app = spawn().await;
    let admin = app.admin().await;
    let (_s, v) = app.post("/tags", &admin, "admin-dev", json!({"name":"db","color":"#f00"})).await;
    let id = v["id"].as_str().unwrap().to_string();
    let (s, _) = app.put(&format!("/tags/{id}"), &admin, "admin-dev", json!({"name":"db2","color":"#0f0"})).await;
    assert_eq!(s, 200);
    let (s, _) = app.delete(&format!("/tags/{id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
}
