mod common;
use common::spawn;
use serde_json::json;

/// An `ssh_key` account routes through public-key auth: a bad private key fails
/// at parse time with a distinctive error (proving the kind-branch is taken and
/// the key is decoded), while a password account reaches the network instead.
#[tokio::test]
async fn ssh_key_kind_branches_to_publickey() {
    let app = spawn().await;
    let admin = app.admin().await;

    // ssh_key account whose secret is not a valid key
    let (_s, v) = app.post("/accounts", &admin, "admin-dev", json!({
        "name":"keyacct","kind":"ssh_key","username":"root","secret":"-----BEGIN OPENSSH PRIVATE KEY-----\nnot-a-real-key\n-----END OPENSSH PRIVATE KEY-----"
    })).await;
    let acc = v["id"].as_str().unwrap().to_string();

    // a fresh server asset bound to that account
    let (_s, v) = app.post("/assets", &admin, "admin-dev", json!({
        "name":"keyhost","kind":"server","parent_id":"site-east","host":"127.0.0.1","port":22,"account_id": acc
    })).await;
    let asset = v["id"].as_str().unwrap().to_string();

    // admin runs SSH → public-key branch → key parse fails
    let (_s, r) = app.post("/jobs/ssh", &admin, "admin-dev", json!({"targets":[asset],"command":"id"})).await;
    let err = r["results"][0]["error"].as_str().unwrap_or("");
    assert!(err.contains("私钥解析"), "expected key-parse error (publickey branch), got: {err}");
}

#[tokio::test]
async fn ssh_pw_account_reaches_network_not_parse() {
    let app = spawn().await;
    let admin = app.admin().await;
    // web-01 uses su-webssh (ssh_pw). Error must NOT be a key-parse error.
    let (_s, r) = app.post("/jobs/ssh", &admin, "admin-dev", json!({"targets":["web-01"],"command":"id"})).await;
    let err = r["results"][0]["error"].as_str().unwrap_or("");
    assert!(!err.contains("私钥解析"), "password account must not go through key parsing, got: {err}");
}
