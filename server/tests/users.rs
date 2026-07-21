mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn user_crud_and_login() {
    let app = spawn().await;
    let admin = app.admin().await;

    // list includes seeded users with email + role
    let (s, list) = app.get("/users", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert!(list.as_array().unwrap().iter().any(|u| u["name"] == "operator" && u["role"] == "operator"));

    // create
    let (s, v) = app.post("/users", &admin, "admin-dev", json!({"name":"carol","email":"c@x.io","role":"operator","password":"carolpw"})).await;
    assert_eq!(s, 200);
    let id = v["id"].as_str().unwrap().to_string();

    // the new user can log in
    assert!(app.login_raw("carol", "carolpw", "cd").await["token"].is_string());

    // update role
    let (s, _) = app.put(&format!("/users/{id}"), &admin, "admin-dev", json!({"name":"carol","email":"c@x.io","role":"viewer"})).await;
    assert_eq!(s, 200);

    // admin resets carol's password → old fails, new works
    let (s, _) = app.post(&format!("/users/{id}/reset-password"), &admin, "admin-dev", json!({"password":"newpass1"})).await;
    assert_eq!(s, 200);
    assert_eq!(app.status_login("carol", "carolpw", "cd").await, 401);
    assert!(app.login_raw("carol", "newpass1", "cd").await["token"].is_string());

    // delete
    let (s, _) = app.delete(&format!("/users/{id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(app.status_login("carol", "newpass1", "cd").await, 401);
}

#[tokio::test]
async fn user_guards() {
    let app = spawn().await;
    let admin = app.admin().await;

    // non-admin cannot manage users
    let op = app.operator().await;
    let (s, _) = app.post("/users", &op, "op-dev", json!({"name":"x","role":"viewer","password":"secret1"})).await;
    assert_eq!(s, 403);

    // duplicate name / bad role / short password
    assert_eq!(app.post("/users", &admin, "admin-dev", json!({"name":"operator","role":"viewer","password":"secret1"})).await.0, 400);
    assert_eq!(app.post("/users", &admin, "admin-dev", json!({"name":"z","role":"king","password":"secret1"})).await.0, 400);
    assert_eq!(app.post("/users", &admin, "admin-dev", json!({"name":"z","role":"viewer","password":"12"})).await.0, 400);

    // can't delete self; can't delete a user who owns rules; can't delete last admin
    let users = app.get("/users", &admin, "admin-dev").await.1;
    let self_id = users.as_array().unwrap().iter().find(|u| u["name"] == "admin").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(app.delete(&format!("/users/{self_id}"), &admin, "admin-dev").await.0, 400);
    let op_id = users.as_array().unwrap().iter().find(|u| u["name"] == "operator").unwrap()["id"].as_str().unwrap().to_string();
    // operator owns rule-op-web/rule-op-sql → blocked
    assert_eq!(app.delete(&format!("/users/{op_id}"), &admin, "admin-dev").await.0, 400);

    // demoting the last... there are 2 admins in the fixture (admin, admin2), so
    // demoting one is allowed; demote admin2 then admin's demotion is blocked.
    let a2 = users.as_array().unwrap().iter().find(|u| u["name"] == "admin2").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(app.put(&format!("/users/{a2}"), &admin, "admin-dev", json!({"name":"admin2","role":"viewer"})).await.0, 200);
    // now admin is the last admin → can't self-demote
    assert_eq!(app.put(&format!("/users/{self_id}"), &admin, "admin-dev", json!({"name":"admin","role":"viewer"})).await.0, 400);
}

#[tokio::test]
async fn self_password_change() {
    let app = spawn().await;
    // create a disposable user to change their own password
    let admin = app.admin().await;
    app.post("/users", &admin, "admin-dev", json!({"name":"dave","role":"viewer","password":"davepw1"})).await;
    let tok = app.login("dave", "davepw1", "dd").await;

    // wrong old password → 400
    assert_eq!(app.put("/profile/password", &tok, "dd", json!({"old_password":"nope","new_password":"davepw2"})).await.0, 400);
    // correct → 200, old fails, new works
    assert_eq!(app.put("/profile/password", &tok, "dd", json!({"old_password":"davepw1","new_password":"davepw2"})).await.0, 200);
    assert_eq!(app.status_login("dave", "davepw1", "dd").await, 401);
    assert!(app.login_raw("dave", "davepw2", "dd").await["token"].is_string());
}
