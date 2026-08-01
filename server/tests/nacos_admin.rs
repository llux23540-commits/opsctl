//! Nacos 管理面:命名空间 / 账号 / 角色 / 权限。
//!
//! mock 按 alibaba/nacos@2.3.2 源码核对过的形状实现,重点复刻三处「不统一」——
//! 列表是裸 `Page<T>`、写操作是 `RestResult{code:200}`、命名空间增删改是裸布尔,
//! 失败则是 HTTP 400 + 纯文本。这些正是最容易解析错的地方。

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Form, Json, Router};
use common::spawn;
use serde_json::{json, Value};

#[derive(Default)]
struct Mock {
    users: Vec<String>,
    roles: Vec<(String, String)>,       // (role, username)
    perms: Vec<(String, String, String)>, // (role, resource, action)
    namespaces: Vec<(String, String, String)>, // (id, name, desc)
}

type Shared = Arc<Mutex<Mock>>;

struct MockNacos {
    addr: String,
    state: Shared,
}

async fn mock() -> MockNacos {
    let state: Shared = Arc::new(Mutex::new(Mock {
        users: vec!["nacos".into()],
        roles: vec![("ROLE_ADMIN".into(), "nacos".into())],
        perms: Vec::new(),
        namespaces: vec![(String::new(), "public".into(), String::new())],
    }));
    let app = Router::new()
        .route("/nacos/v1/auth/login", axum::routing::post(login))
        .route("/nacos/v1/console/health/readiness", get(|| async { "ok" }))
        // v3 一律 404 → opsctl 的版本探测应落到 v1
        .route("/nacos/v3/auth/user/list", get(not_found))
        .route(
            "/nacos/v1/auth/users",
            get(list_users).post(create_user).put(reset_user).delete(delete_user),
        )
        .route("/nacos/v1/auth/roles", get(list_roles).post(bind_role).delete(unbind_role))
        .route(
            "/nacos/v1/auth/permissions",
            get(list_perms).post(grant_perm).delete(revoke_perm),
        )
        .route(
            "/nacos/v1/console/namespaces",
            get(list_ns).post(create_ns).put(update_ns).delete(delete_ns),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    MockNacos { addr: addr.to_string(), state }
}

async fn not_found() -> axum::response::Response {
    use axum::response::IntoResponse;
    (axum::http::StatusCode::NOT_FOUND, "no v3").into_response()
}

async fn login(Form(f): Form<HashMap<String, String>>) -> axum::response::Response {
    use axum::response::IntoResponse;
    if f.get("username").map(String::as_str) == Some("nacos")
        && f.get("password").map(String::as_str) == Some("nacos-pass")
    {
        Json(json!({ "accessToken": "tok", "tokenTtl": 18000, "globalAdmin": true })).into_response()
    } else {
        (axum::http::StatusCode::FORBIDDEN, "unknown user!").into_response()
    }
}

fn authed(q: &HashMap<String, String>) -> bool {
    q.get("accessToken").map(String::as_str) == Some("tok")
}

/// 纯文本 400 —— 真实 Nacos 抛 IllegalArgumentException 时就是这个形状。
fn bad(msg: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (axum::http::StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

fn rest_ok(data: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    Json(json!({ "code": 200, "message": null, "data": data })).into_response()
}

/// 裸 Page<T>,没有 {code,message,data} 信封。
fn page(items: Vec<Value>) -> axum::response::Response {
    use axum::response::IntoResponse;
    Json(json!({
        "totalCount": items.len(), "pageNumber": 1, "pagesAvailable": 1, "pageItems": items
    }))
    .into_response()
}

// ---- users ----

async fn list_users(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    // search 是 mapping 谓词:真实服务端缺了它根本匹配不到 handler
    if !q.contains_key("search") {
        return bad("Parameter conditions \"search=accurate\" not met");
    }
    let g = st.lock().unwrap();
    page(
        g.users
            .iter()
            // 真实响应会带 bcrypt 哈希,opsctl 必须过滤掉
            .map(|u| json!({ "username": u, "password": "$2a$10$abcdefghijklmnopqrstuv" }))
            .collect(),
    )
}

async fn create_user(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    let name = f.get("username").cloned().unwrap_or_default();
    let mut g = st.lock().unwrap();
    if g.users.contains(&name) {
        return bad(&format!("user '{name}' already exist!"));
    }
    g.users.push(name);
    rest_ok("create user ok!")
}

async fn reset_user(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    let name = f.get("username").cloned().unwrap_or_default();
    if !st.lock().unwrap().users.contains(&name) {
        return bad("user not found!");
    }
    if f.get("newPassword").map(String::as_str).unwrap_or_default().is_empty() {
        return bad("newPassword is blank");
    }
    rest_ok("update user ok!")
}

async fn delete_user(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    let name = q.get("username").cloned().unwrap_or_default();
    let mut g = st.lock().unwrap();
    if g.roles.iter().any(|(r, u)| r == "ROLE_ADMIN" && *u == name) {
        return bad(&format!("cannot delete admin: {name}"));
    }
    g.users.retain(|u| *u != name);
    rest_ok("delete user ok!")
}

// ---- roles ----

async fn list_roles(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    if !q.contains_key("search") {
        return bad("Parameter conditions \"search=accurate\" not met");
    }
    let g = st.lock().unwrap();
    page(g.roles.iter().map(|(r, u)| json!({ "role": r, "username": u })).collect())
}

async fn bind_role(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    let role = f.get("role").cloned().unwrap_or_default();
    if role == "ROLE_ADMIN" {
        return bad("role 'ROLE_ADMIN' is not permitted to create!");
    }
    st.lock().unwrap().roles.push((role, f.get("username").cloned().unwrap_or_default()));
    rest_ok("add role ok!")
}

async fn unbind_role(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    let role = q.get("role").cloned().unwrap_or_default();
    let username = q.get("username").cloned().unwrap_or_default();
    let mut g = st.lock().unwrap();
    // username 留空 = 对所有用户删除该角色
    g.roles.retain(|(r, u)| !(*r == role && (username.is_empty() || *u == username)));
    rest_ok("delete role of user ok!")
}

// ---- permissions ----

async fn list_perms(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    if !q.contains_key("search") {
        return bad("Parameter conditions \"search=accurate\" not met");
    }
    let filter = q.get("role").cloned().unwrap_or_default();
    let g = st.lock().unwrap();
    page(
        g.perms
            .iter()
            .filter(|(r, _, _)| filter.is_empty() || *r == filter)
            .map(|(r, res, a)| json!({ "role": r, "resource": res, "action": a }))
            .collect(),
    )
}

async fn grant_perm(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    let role = f.get("role").cloned().unwrap_or_default();
    let mut g = st.lock().unwrap();
    // 真实服务端的顺序陷阱:角色不存在就拒绝
    if !g.roles.iter().any(|(r, _)| *r == role) {
        return bad(&format!("role {role} not found!"));
    }
    g.perms.push((
        role,
        f.get("resource").cloned().unwrap_or_default(),
        f.get("action").cloned().unwrap_or_default(),
    ));
    rest_ok("add permission ok!")
}

async fn revoke_perm(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !authed(&q) {
        return bad("no token");
    }
    let (role, res, act) = (
        q.get("role").cloned().unwrap_or_default(),
        q.get("resource").cloned().unwrap_or_default(),
        q.get("action").cloned().unwrap_or_default(),
    );
    st.lock().unwrap().perms.retain(|(r, s, a)| !(*r == role && *s == res && *a == act));
    rest_ok("delete permission ok!")
}

// ---- namespaces (裸布尔) ----

async fn list_ns(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return bad("no token");
    }
    let g = st.lock().unwrap();
    let data: Vec<Value> = g
        .namespaces
        .iter()
        .map(|(id, name, desc)| {
            json!({
                "namespace": id, "namespaceShowName": name, "namespaceDesc": desc,
                "quota": 200, "configCount": 0, "type": if id.is_empty() { 0 } else { 2 }
            })
        })
        .collect();
    Json(json!({ "code": 200, "message": null, "data": data })).into_response()
}

async fn create_ns(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return bad("no token");
    }
    // v1 create 用的是 customNamespaceId,不是 namespaceId
    let id = f.get("customNamespaceId").cloned().unwrap_or_default();
    let name = f.get("namespaceName").cloned().unwrap_or_default();
    let mut g = st.lock().unwrap();
    if g.namespaces.iter().any(|(i, _, _)| *i == id) {
        return Json(false).into_response(); // 真实行为:HTTP 200 + 裸 false
    }
    g.namespaces.push((id, name, f.get("namespaceDesc").cloned().unwrap_or_default()));
    Json(true).into_response()
}

async fn update_ns(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return bad("no token");
    }
    // v1 edit 的参数名又不一样:namespace + namespaceShowName
    let id = f.get("namespace").cloned().unwrap_or_default();
    let name = f.get("namespaceShowName").cloned().unwrap_or_default();
    let mut g = st.lock().unwrap();
    match g.namespaces.iter_mut().find(|(i, _, _)| *i == id) {
        Some(row) => {
            row.1 = name;
            row.2 = f.get("namespaceDesc").cloned().unwrap_or_default();
            Json(true).into_response()
        }
        None => Json(false).into_response(),
    }
}

async fn delete_ns(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return bad("no token");
    }
    let id = q.get("namespaceId").cloned().unwrap_or_default();
    let mut g = st.lock().unwrap();
    let before = g.namespaces.len();
    g.namespaces.retain(|(i, _, _)| *i != id);
    Json(g.namespaces.len() < before).into_response()
}

// ---- helpers ----

async fn cluster(app: &common::TestApp, admin: &str, addr: &str) -> String {
    let (s, v) = app
        .post(
            "/nacos/clusters",
            admin,
            "admin-dev",
            json!({ "name": "admin-cluster", "server_addr": addr, "context_path": "/nacos",
                    "username": "nacos", "password": "nacos-pass" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v["id"].as_str().unwrap().to_string()
}

// ---- tests ----

#[tokio::test]
async fn namespace_crud_over_v1_bare_boolean() {
    let app = spawn().await;
    let admin = app.admin().await;
    let n = mock().await;
    let id = cluster(&app, &admin, &n.addr).await;

    // list: public 的 id 是空串,类型 0
    let (s, v) = app.get(&format!("/nacos/clusters/{id}/namespaces"), &admin, "admin-dev").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["flavor"], "v1", "v3 探测应回退到 v1");
    assert_eq!(v["items"][0]["namespace_id"], "");
    assert_eq!(v["items"][0]["name"], "public");
    assert_eq!(v["items"][0]["type"], 0);

    // create
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/namespaces"),
            &admin,
            "admin-dev",
            json!({ "namespace_id": "dev-ns", "name": "开发环境", "desc": "dev" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["namespace_id"], "dev-ns");
    assert_eq!(n.state.lock().unwrap().namespaces.len(), 2);

    // 重复创建 → 服务端裸 false,必须转成可读错误而不是当成功
    let (s, e) = app
        .post(
            &format!("/nacos/clusters/{id}/namespaces"),
            &admin,
            "admin-dev",
            json!({ "namespace_id": "dev-ns", "name": "开发环境" }),
        )
        .await;
    assert_eq!(s, 400, "{e}");
    assert!(e["error"].as_str().unwrap().contains("false"), "{e}");

    // 非法 id / 名称在本地就挡掉,不打远端
    let (s, e) = app
        .post(
            &format!("/nacos/clusters/{id}/namespaces"),
            &admin,
            "admin-dev",
            json!({ "namespace_id": "bad id!", "name": "x" }),
        )
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("只能包含"), "{e}");
    let (s, e) = app
        .post(
            &format!("/nacos/clusters/{id}/namespaces"),
            &admin,
            "admin-dev",
            json!({ "namespace_id": "ok-id", "name": "bad*name" }),
        )
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("不能包含"), "{e}");

    // update(v1 用 namespace + namespaceShowName 这组参数名)
    let (s, _) = app
        .put(
            &format!("/nacos/clusters/{id}/namespaces"),
            &admin,
            "admin-dev",
            json!({ "namespace_id": "dev-ns", "name": "开发环境二", "desc": "d2" }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(n.state.lock().unwrap().namespaces[1].1, "开发环境二");

    // public 不允许删除(列表里 id 是空串;字面量 "public" 也挡掉。空串本身匹配不到
    // 路由,会落到 SPA fallback,所以 UI 侧对 public 行禁用删除按钮)
    let (s, e) = app
        .delete(&format!("/nacos/clusters/{id}/namespaces/public"), &admin, "admin-dev")
        .await;
    assert_eq!(s, 400, "{e}");
    assert!(e["error"].as_str().unwrap().contains("不可删除"), "{e}");

    let (s, _) = app.delete(&format!("/nacos/clusters/{id}/namespaces/dev-ns"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(n.state.lock().unwrap().namespaces.len(), 1);
}

#[tokio::test]
async fn user_crud_never_leaks_password_hash() {
    let app = spawn().await;
    let admin = app.admin().await;
    let n = mock().await;
    let id = cluster(&app, &admin, &n.addr).await;

    let (s, v) = app.get(&format!("/nacos/clusters/{id}/users"), &admin, "admin-dev").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["username"], "nacos");
    assert!(v["items"][0].get("password").is_none(), "bcrypt 哈希不得下发:{v}");
    assert!(!v.to_string().contains("$2a$10$"), "响应里不应出现哈希:{v}");

    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/users"),
            &admin,
            "admin-dev",
            json!({ "username": "dev1", "password": "dev1-pass" }),
        )
        .await;
    assert_eq!(s, 200);
    assert!(n.state.lock().unwrap().users.contains(&"dev1".to_string()));

    // 重名 → 远端 400 纯文本,必须原样带回
    let (s, e) = app
        .post(
            &format!("/nacos/clusters/{id}/users"),
            &admin,
            "admin-dev",
            json!({ "username": "dev1", "password": "x" }),
        )
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("already exist"), "{e}");

    // 重置密码
    let (s, _) = app
        .put(
            &format!("/nacos/clusters/{id}/users"),
            &admin,
            "admin-dev",
            json!({ "username": "dev1", "new_password": "new-pass" }),
        )
        .await;
    assert_eq!(s, 200);

    // 删除管理员被服务端拒绝
    let (s, e) = app.delete(&format!("/nacos/clusters/{id}/users/nacos"), &admin, "admin-dev").await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("cannot delete admin"), "{e}");

    let (s, _) = app.delete(&format!("/nacos/clusters/{id}/users/dev1"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert!(!n.state.lock().unwrap().users.contains(&"dev1".to_string()));
}

#[tokio::test]
async fn role_bind_and_permission_grant_respect_nacos_rules() {
    let app = spawn().await;
    let admin = app.admin().await;
    let n = mock().await;
    let id = cluster(&app, &admin, &n.addr).await;

    // 角色不存在时赋权:远端会拒,错误要原样透出
    let (s, e) = app
        .post(
            &format!("/nacos/clusters/{id}/permissions"),
            &admin,
            "admin-dev",
            json!({ "role": "dev", "resource": "dev-ns:*:*", "action": "rw" }),
        )
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("not found"), "{e}");

    // ROLE_ADMIN 不允许创建,本地就拦掉(不打远端)
    let (s, e) = app
        .post(
            &format!("/nacos/clusters/{id}/roles"),
            &admin,
            "admin-dev",
            json!({ "role": "ROLE_ADMIN", "username": "dev1" }),
        )
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("ROLE_ADMIN"), "{e}");

    // 先建角色,再赋权
    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/roles"),
            &admin,
            "admin-dev",
            json!({ "role": "dev", "username": "nacos" }),
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = app.get(&format!("/nacos/clusters/{id}/roles"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["total"], 2);
    assert!(v["items"].as_array().unwrap().iter().any(|r| r["role"] == "dev"));

    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/permissions"),
            &admin,
            "admin-dev",
            json!({ "role": "dev", "resource": "dev-ns:*:*", "action": "rw" }),
        )
        .await;
    assert_eq!(s, 200);

    // 动作只认 r/w/rw
    let (s, e) = app
        .post(
            &format!("/nacos/clusters/{id}/permissions"),
            &admin,
            "admin-dev",
            json!({ "role": "dev", "resource": "dev-ns:*:*", "action": "delete" }),
        )
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("r / w / rw"), "{e}");

    let (s, v) = app
        .get(&format!("/nacos/clusters/{id}/permissions?role=dev"), &admin, "admin-dev")
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["resource"], "dev-ns:*:*");
    assert_eq!(v["items"][0]["action"], "rw");

    // 收回
    let (s, _) = app
        .delete(
            &format!("/nacos/clusters/{id}/permissions?role=dev&resource=dev-ns:*:*&action=rw"),
            &admin,
            "admin-dev",
        )
        .await;
    assert_eq!(s, 200);
    assert!(n.state.lock().unwrap().perms.is_empty());

    // 解绑角色
    let (s, _) = app
        .delete(&format!("/nacos/clusters/{id}/roles?role=dev&username=nacos"), &admin, "admin-dev")
        .await;
    assert_eq!(s, 200);
    assert!(!n.state.lock().unwrap().roles.iter().any(|(r, _)| r == "dev"));
}

#[tokio::test]
async fn admin_writes_are_audited_and_operator_is_denied() {
    let app = spawn().await;
    let admin = app.admin().await;
    let n = mock().await;
    let id = cluster(&app, &admin, &n.addr).await;

    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/namespaces"),
            &admin,
            "admin-dev",
            json!({ "namespace_id": "audit-ns", "name": "审计" }),
        )
        .await;
    assert_eq!(s, 200);
    let (s, rows) = app.get("/audit?action=nacos_ns_create", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["result"], "ok");
    assert_eq!(rows[0]["targets"], "admin-cluster");
    assert!(rows[0]["payload"].as_str().unwrap().contains("audit-ns"));

    // 失败也要留痕
    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/namespaces"),
            &admin,
            "admin-dev",
            json!({ "namespace_id": "audit-ns", "name": "审计" }),
        )
        .await;
    assert_eq!(s, 400);
    let (_, rows) = app.get("/audit?action=nacos_ns_create", &admin, "admin-dev").await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r["result"] == "fail"));

    // 非 admin 一律 403
    let op = app.operator().await;
    for path in [
        format!("/nacos/clusters/{id}/namespaces"),
        format!("/nacos/clusters/{id}/users"),
        format!("/nacos/clusters/{id}/roles"),
        format!("/nacos/clusters/{id}/permissions"),
    ] {
        let (s, _) = app.get(&path, &op, "op-dev").await;
        assert_eq!(s, 403, "{path} 应拒绝 operator");
    }
}
