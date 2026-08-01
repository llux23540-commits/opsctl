//! Nacos 管理:集群注册表 + 节点总览 + 配置初始化。
//!
//! 远端用一个内置的 mock Nacos(按 v1/v2 Open API 的文档响应形状实现)驱动,
//! 覆盖鉴权、节点列举、配置读/写、幂等与试运行。

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Form, Json, Router};
use common::{spawn, spawn_sealed};
use serde_json::{json, Value};

// ---- mock Nacos ----

#[derive(Default)]
struct MockState {
    /// `tenant|group|dataId` → (content, type)
    configs: HashMap<String, (String, String)>,
    logins: usize,
    publishes: usize,
    /// 列表接口是否隐藏正文(复现只给元数据的版本)
    hide_list_content: bool,
}

type Shared = Arc<Mutex<MockState>>;

struct MockNacos {
    /// `host:port` of the mock, for `server_addr`.
    addr: String,
    state: Shared,
}

impl MockNacos {
    fn config(&self, tenant: &str, group: &str, data_id: &str) -> Option<String> {
        let st = self.state.lock().unwrap();
        st.configs.get(&format!("{tenant}|{group}|{data_id}")).map(|(c, _)| c.clone())
    }
    fn publishes(&self) -> usize {
        self.state.lock().unwrap().publishes
    }
    fn logins(&self) -> usize {
        self.state.lock().unwrap().logins
    }
}

/// Nacos with the auth plugin ON: every data-plane call needs `accessToken`.
async fn mock_nacos() -> MockNacos {
    let state: Shared = Arc::new(Mutex::new(MockState::default()));
    let app = Router::new()
        .route("/nacos/v1/auth/login", axum::routing::post(login))
        .route("/nacos/v1/console/health/readiness", get(readiness))
        .route("/nacos/v2/core/cluster/nodes", get(nodes))
        .route(
            "/nacos/v1/cs/configs",
            get(get_configs).post(post_config).delete(delete_config),
        )
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    MockNacos { addr: addr.to_string(), state }
}

async fn login(
    State(st): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    st.lock().unwrap().logins += 1;
    if f.get("username").map(String::as_str) == Some("nacos")
        && f.get("password").map(String::as_str) == Some("nacos-pass")
    {
        Json(json!({ "accessToken": "mock-token", "tokenTtl": 18000, "globalAdmin": true }))
            .into_response()
    } else {
        (axum::http::StatusCode::FORBIDDEN, "unknown user!").into_response()
    }
}

async fn readiness() -> &'static str {
    "ok"
}

fn authed(q: &HashMap<String, String>) -> bool {
    q.get("accessToken").map(String::as_str) == Some("mock-token")
}

async fn nodes(Query(q): Query<HashMap<String, String>>) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return (axum::http::StatusCode::FORBIDDEN, "no token").into_response();
    }
    Json(json!({
        "code": 0, "message": "success",
        "data": [
            { "ip": "127.0.0.1", "port": 8848, "state": "UP", "address": "127.0.0.1:8848",
              "extendInfo": { "version": "2.3.2" } },
            { "ip": "127.0.0.2", "port": 8848, "state": "DOWN", "address": "127.0.0.2:8848",
              "extendInfo": { "version": "2.3.2" } }
        ]
    }))
    .into_response()
}

/// One path serves both "read one config" and "search the config list"
/// (Nacos distinguishes them by the `search` parameter).
async fn get_configs(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return (axum::http::StatusCode::FORBIDDEN, "no token").into_response();
    }
    let tenant = q.get("tenant").cloned().unwrap_or_default();
    let guard = st.lock().unwrap();
    if q.contains_key("search") {
        let items: Vec<Value> = guard
            .configs
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{tenant}|")))
            .map(|(k, (content, kind))| {
                let mut parts = k.split('|');
                let _ = parts.next();
                let group = parts.next().unwrap_or_default();
                let data_id = parts.next().unwrap_or_default();
                // 有的 Nacos 版本列表接口只给元数据不给正文,用这个开关复现,
                // 验证 opsctl 会逐条回查补齐
                let body = if guard.hide_list_content { String::new() } else { content.clone() };
                json!({ "id": "1", "dataId": data_id, "group": group,
                        "content": body, "type": kind, "appName": "" })
            })
            .collect();
        return Json(json!({
            "totalCount": items.len(), "pageNumber": 1, "pagesAvailable": 1, "pageItems": items
        }))
        .into_response();
    }
    let key = format!(
        "{tenant}|{}|{}",
        q.get("group").cloned().unwrap_or_default(),
        q.get("dataId").cloned().unwrap_or_default()
    );
    match guard.configs.get(&key) {
        Some((content, _)) => content.clone().into_response(),
        None => (axum::http::StatusCode::NOT_FOUND, "config data not exist").into_response(),
    }
}

async fn post_config(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    Form(f): Form<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return (axum::http::StatusCode::FORBIDDEN, "no token").into_response();
    }
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let key = format!("{}|{}|{}", get("tenant"), get("group"), get("dataId"));
    let mut guard = st.lock().unwrap();
    guard.configs.insert(key, (get("content"), get("type")));
    guard.publishes += 1;
    "true".into_response()
}

async fn delete_config(
    State(st): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !authed(&q) {
        return (axum::http::StatusCode::FORBIDDEN, "no token").into_response();
    }
    let key = format!(
        "{}|{}|{}",
        q.get("tenant").cloned().unwrap_or_default(),
        q.get("group").cloned().unwrap_or_default(),
        q.get("dataId").cloned().unwrap_or_default()
    );
    let removed = st.lock().unwrap().configs.remove(&key).is_some();
    if removed { "true" } else { "false" }.into_response()
}

// ---- helpers ----

fn cluster_body(addr: &str) -> Value {
    json!({
        "name": "订单中心 Nacos",
        "env": "test",
        "server_addr": addr,
        "context_path": "/nacos",
        "namespace": "",
        "username": "nacos",
        "password": "nacos-pass",
        "note": "集成测试"
    })
}

async fn register_cluster(app: &common::TestApp, admin: &str, addr: &str) -> String {
    let (s, v) = app.post("/nacos/clusters", admin, "admin-dev", cluster_body(addr)).await;
    assert_eq!(s, 200, "create cluster: {v}");
    v["id"].as_str().unwrap().to_string()
}

// ---- tests ----

#[tokio::test]
async fn cluster_crud_secret_encrypted_and_admin_only() {
    let app = spawn().await;
    let admin = app.admin().await;
    let id = register_cluster(&app, &admin, "10.0.0.1,10.0.0.2:8849").await;

    // list: no secret leaks, endpoints normalized, no init yet
    let (s, list) = app.get("/nacos/clusters", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let c = &list.as_array().unwrap()[0];
    assert_eq!(c["name"], "订单中心 Nacos");
    assert!(c.get("secret").is_none(), "密码不得下发:{c}");
    assert_eq!(c["has_secret"], true);
    assert_eq!(
        c["endpoints"],
        json!(["http://10.0.0.1:8848/nacos", "http://10.0.0.2:8849/nacos"])
    );
    assert!(c["last_init"].is_null());

    // password is vault ciphertext at rest
    let store = opsctl_server::store::Store::connect(&app.db_url).await.unwrap();
    let row = store.get_nacos_cluster(&id).await.unwrap().unwrap();
    assert!(row.secret.starts_with("v1:"), "at-rest secret: {}", row.secret);
    assert!(!row.secret.contains("nacos-pass"));

    // update with a blank password keeps the stored ciphertext
    let (s, _) = app
        .put(
            &format!("/nacos/clusters/{id}"),
            &admin,
            "admin-dev",
            json!({ "name": "订单中心 Nacos", "env": "prod", "server_addr": "10.0.0.1",
                    "context_path": "/nacos", "username": "nacos", "password": "" }),
        )
        .await;
    assert_eq!(s, 200);
    let row2 = store.get_nacos_cluster(&id).await.unwrap().unwrap();
    assert_eq!(row2.secret, row.secret);
    assert_eq!(row2.env, "prod");

    // an address-less cluster is rejected
    let (s, e) = app
        .post("/nacos/clusters", &admin, "admin-dev", json!({ "name": "空", "server_addr": " " }))
        .await;
    assert_eq!(s, 400, "{e}");

    // operators cannot see or touch the module
    let op = app.operator().await;
    let (s, _) = app.get("/nacos/clusters", &op, "op-dev").await;
    assert_eq!(s, 403);
    let (s, _) = app.post("/nacos/clusters", &op, "op-dev", cluster_body("x:8848")).await;
    assert_eq!(s, 403);

    let (s, _) = app.delete(&format!("/nacos/clusters/{id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let (s, list) = app.get("/nacos/clusters", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert!(list.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn nodes_list_cluster_members_and_degrade_to_probe() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;

    let (s, v) = app.get(&format!("/nacos/clusters/{id}/nodes"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["source"], "v2", "{v}");
    assert_eq!(v["ok"], true);
    let nodes = v["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["address"], "127.0.0.1:8848");
    assert_eq!(nodes[0]["state"], "UP");
    assert_eq!(nodes[0]["version"], "2.3.2");
    assert_eq!(nodes[0]["ok"], true);
    assert_eq!(nodes[1]["ok"], false);
    assert!(nacos.logins() >= 1, "节点查询应先鉴权");

    // an unreachable cluster still answers, degraded to address probing
    let dead = register_cluster(&app, &admin, "127.0.0.1:9").await;
    let (s, v) = app.get(&format!("/nacos/clusters/{dead}/nodes"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["source"], "probe", "{v}");
    assert_eq!(v["ok"], false);
    assert_eq!(v["nodes"][0]["state"], "unreachable");
    assert!(v["message"].as_str().unwrap().contains("不可达"));
}

#[tokio::test]
async fn init_publishes_then_skips_then_overwrites() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;
    let items = json!([
        { "data_id": "order.properties", "group": "DEFAULT_GROUP", "type": "properties",
          "content": "server.port=8080" },
        { "data_id": "order-db.yaml", "group": "DB", "type": "yaml", "content": "url: jdbc" }
    ]);

    // 1) first run creates both
    let (s, v) = app
        .post(&format!("/nacos/clusters/{id}/init"), &admin, "admin-dev", json!({ "items": items }))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["total"], 2);
    assert_eq!(v["ok_count"], 2);
    assert_eq!(v["items"][0]["status"], "created");
    assert_eq!(v["items"][1]["status"], "created");
    assert_eq!(nacos.config("", "DEFAULT_GROUP", "order.properties").unwrap(), "server.port=8080");
    assert_eq!(nacos.config("", "DB", "order-db.yaml").unwrap(), "url: jdbc");

    // 2) re-run without overwrite leaves them alone
    let published = nacos.publishes();
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "items": [{ "data_id": "order.properties", "group": "DEFAULT_GROUP",
                                "content": "server.port=9999" }] }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["status"], "skipped");
    assert!(v["items"][0]["message"].as_str().unwrap().contains("未覆盖"));
    assert_eq!(nacos.publishes(), published, "skip 不应写入");
    assert_eq!(nacos.config("", "DEFAULT_GROUP", "order.properties").unwrap(), "server.port=8080");

    // 3) overwrite rewrites changed content, and no-ops identical content
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "overwrite": true,
                    "items": [{ "data_id": "order.properties", "group": "DEFAULT_GROUP",
                                "content": "server.port=9999" },
                              { "data_id": "order-db.yaml", "group": "DB", "content": "url: jdbc" }] }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["status"], "updated");
    assert_eq!(v["items"][1]["status"], "skipped");
    assert_eq!(v["items"][1]["message"], "内容一致,无需变更");
    assert_eq!(nacos.config("", "DEFAULT_GROUP", "order.properties").unwrap(), "server.port=9999");
}

#[tokio::test]
async fn dry_run_reports_without_writing() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;

    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "dry_run": true,
                    "items": [{ "data_id": "a.properties", "content": "k=v" }] }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["items"][0]["status"], "would_created");
    assert_eq!(v["items"][0]["group"], "DEFAULT_GROUP", "缺省 group 应补 DEFAULT_GROUP");
    assert_eq!(nacos.publishes(), 0);
    assert!(nacos.config("", "DEFAULT_GROUP", "a.properties").is_none());

    // a dry run must not count as "已初始化"
    let (_, list) = app.get("/nacos/clusters", &admin, "admin-dev").await;
    assert!(list[0]["last_init"].is_null(), "{list}");
}

#[tokio::test]
async fn template_init_substitutes_vars_and_fails_on_missing() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;

    let (s, t) = app
        .post(
            "/nacos/templates",
            &admin,
            "admin-dev",
            json!({ "name": "微服务基线", "note": "上线初始化",
                    "items": [{ "data_id": "${app}.properties", "type": "properties",
                                "content": "env=${env}\nport=${port}" }] }),
        )
        .await;
    assert_eq!(s, 200, "{t}");
    let tpl = t["id"].as_str().unwrap().to_string();
    let (s, list) = app.get("/nacos/templates", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(list[0]["name"], "微服务基线");

    // missing var → the item fails and nothing is published
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "template_id": tpl, "vars": { "app": "order", "env": "test" } }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "fail");
    assert_eq!(v["items"][0]["status"], "fail");
    assert!(v["items"][0]["message"].as_str().unwrap().contains("port"));
    assert_eq!(nacos.publishes(), 0);

    // all vars supplied → dataId and content are substituted
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "template_id": tpl,
                    "vars": { "app": "order", "env": "test", "port": "8080" } }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["items"][0]["data_id"], "order.properties");
    assert_eq!(
        nacos.config("", "DEFAULT_GROUP", "order.properties").unwrap(),
        "env=test\nport=8080"
    );

    // the applied run shows up as the cluster's last init + in the history + audit
    let (_, list) = app.get("/nacos/clusters", &admin, "admin-dev").await;
    assert_eq!(list[0]["last_init"]["status"], "ok");
    assert_eq!(list[0]["last_init"]["template_name"], "微服务基线");

    let (s, runs) = app.get(&format!("/nacos/runs?cluster_id={id}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 2, "失败与成功各记一次");
    assert_eq!(runs[0]["status"], "ok");
    assert_eq!(runs[0]["template_name"], "微服务基线");
    assert_eq!(runs[0]["items"][0]["data_id"], "order.properties");
    assert_eq!(runs[1]["status"], "fail");

    let (s, audit) = app.get("/audit?action=nacos_init", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let rows = audit.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r["targets"].as_str().unwrap().contains("订单中心 Nacos")));
    let results: Vec<&str> = rows.iter().map(|r| r["result"].as_str().unwrap()).collect();
    assert!(results.contains(&"ok") && results.contains(&"fail"), "{audit}");

    // template delete
    let (s, _) = app.delete(&format!("/nacos/templates/{tpl}"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    let (_, list) = app.get("/nacos/templates", &admin, "admin-dev").await;
    assert!(list.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn existing_configs_are_listed_for_review() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;
    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "items": [{ "data_id": "seen.yaml", "group": "G1", "type": "yaml",
                                "content": "a: 1" }] }),
        )
        .await;
    assert_eq!(s, 200);

    let (s, v) = app.get(&format!("/nacos/clusters/{id}/configs"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["data_id"], "seen.yaml");
    assert_eq!(v["items"][0]["group"], "G1");
    assert_eq!(v["items"][0]["type"], "yaml");
}

#[tokio::test]
async fn bad_credentials_and_unknown_cluster_are_reported() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;

    // wrong password → every item fails with the auth error, nothing published
    let (s, v) = app
        .post(
            "/nacos/clusters",
            &admin,
            "admin-dev",
            json!({ "name": "错口令", "server_addr": nacos.addr, "context_path": "/nacos",
                    "username": "nacos", "password": "wrong" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let bad = v["id"].as_str().unwrap().to_string();
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{bad}/init"),
            &admin,
            "admin-dev",
            json!({ "items": [{ "data_id": "x.properties", "content": "k=v" }] }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["status"], "fail");
    assert!(v["items"][0]["message"].as_str().unwrap().contains("鉴权失败"));
    assert_eq!(nacos.publishes(), 0);

    // unknown cluster / empty payload
    let (s, _) = app
        .post("/nacos/clusters/nope/init", &admin, "admin-dev", json!({ "items": [] }))
        .await;
    assert_eq!(s, 400);
    let (s, e) = app
        .post(&format!("/nacos/clusters/{bad}/init"), &admin, "admin-dev", json!({}))
        .await;
    assert_eq!(s, 400, "{e}");
    assert_eq!(e["error"], "没有要初始化的配置项");
}

#[tokio::test]
async fn sealed_vault_blocks_credentialed_clusters() {
    let app = spawn_sealed().await;
    let admin = app.admin().await;

    // storing a password requires the vault to be open
    let (s, _) = app.post("/nacos/clusters", &admin, "admin-dev", cluster_body("10.0.0.1")).await;
    assert_eq!(s, 503);

    // a credential-less cluster still works while sealed
    let (s, v) = app
        .post(
            "/nacos/clusters",
            &admin,
            "admin-dev",
            json!({ "name": "无鉴权集群", "server_addr": "127.0.0.1:9" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let (s, _) = app.get(&format!("/nacos/clusters/{id}/nodes"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn sync_pulls_namespace_into_a_replayable_template() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;

    // 先在远端放三条配置(经由初始化写入,顺带保证两条链路一致)
    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "items": [
                { "data_id": "a.properties", "group": "G1", "type": "properties", "content": "k=1" },
                { "data_id": "b.yaml", "group": "G1", "type": "yaml", "content": "k: 2" },
                { "data_id": "c.json", "group": "G2", "type": "json", "content": "{\"k\":3}" }
            ] }),
        )
        .await;
    assert_eq!(s, 200);

    // 试运行:只报告,不落库
    let (s, v) = app
        .post(&format!("/nacos/clusters/{id}/sync"), &admin, "admin-dev", json!({ "dry_run": true }))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["total"], 3);
    assert!(v["template_id"].is_null());
    assert!(v["items"].as_array().unwrap().iter().all(|i| i["empty"] == false));
    let (_, tpls) = app.get("/nacos/templates", &admin, "admin-dev").await;
    assert!(tpls.as_array().unwrap().is_empty(), "试运行不应产生模板:{tpls}");

    // 真同步:落成模板
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/sync"),
            &admin,
            "admin-dev",
            json!({ "template_name": "生产基线快照" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["total"], 3);
    assert_eq!(v["template_name"], "生产基线快照");
    let tpl_id = v["template_id"].as_str().unwrap().to_string();

    // 模板内容必须是完整可回放的(带正文),而不是空壳
    let (_, tpls) = app.get("/nacos/templates", &admin, "admin-dev").await;
    let t = tpls.as_array().unwrap().iter().find(|t| t["id"] == tpl_id.as_str()).unwrap();
    let items: Vec<serde_json::Value> = serde_json::from_str(t["items"].as_str().unwrap()).unwrap();
    assert_eq!(items.len(), 3);
    let a = items.iter().find(|i| i["data_id"] == "a.properties").unwrap();
    assert_eq!(a["group"], "G1");
    assert_eq!(a["type"], "properties");
    assert_eq!(a["content"], "k=1");

    // 把这份快照回放到「另一个集群」——真正的 dev→test 克隆路径
    let other = mock_nacos().await;
    let other_id = register_cluster(&app, &admin, &other.addr).await;
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{other_id}/init"),
            &admin,
            "admin-dev",
            json!({ "template_id": tpl_id }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["total"], 3);
    assert_eq!(other.config("", "G1", "a.properties").unwrap(), "k=1");
    assert_eq!(other.config("", "G2", "c.json").unwrap(), "{\"k\":3}");

    // 审计留痕
    let (_, rows) = app.get("/audit?action=nacos_sync", &admin, "admin-dev").await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["payload"].as_str().unwrap().contains("生产基线快照"));
}

#[tokio::test]
async fn sync_backfills_content_when_list_omits_it() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;
    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "items": [{ "data_id": "only.yaml", "group": "G", "content": "deep: value" }] }),
        )
        .await;
    assert_eq!(s, 200);

    // 让列表接口只回元数据 —— opsctl 必须逐条回查把正文补齐
    nacos.state.lock().unwrap().hide_list_content = true;
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/sync"),
            &admin,
            "admin-dev",
            json!({ "template_name": "补齐验证" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["empty"], false, "正文应已补齐:{v}");
    assert_eq!(v["items"][0]["bytes"], 11);

    let (_, tpls) = app.get("/nacos/templates", &admin, "admin-dev").await;
    let items: Vec<serde_json::Value> =
        serde_json::from_str(tpls[0]["items"].as_str().unwrap()).unwrap();
    assert_eq!(items[0]["content"], "deep: value");
}

#[tokio::test]
async fn config_detail_and_delete_round_trip() {
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;
    let (s, _) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "items": [{ "data_id": "app.yaml", "group": "G", "content": "port: 8080" }] }),
        )
        .await;
    assert_eq!(s, 200);

    let (s, v) = app
        .get(
            &format!("/nacos/clusters/{id}/configs/detail?data_id=app.yaml&group=G"),
            &admin,
            "admin-dev",
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["content"], "port: 8080");
    assert_eq!(v["bytes"], 10);

    // 不存在的配置要如实说,而不是给空串
    let (s, v) = app
        .get(
            &format!("/nacos/clusters/{id}/configs/detail?data_id=nope.yaml&group=G"),
            &admin,
            "admin-dev",
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], false);
    assert_eq!(v["message"], "配置不存在");

    let (s, _) = app
        .delete(
            &format!("/nacos/clusters/{id}/configs?data_id=app.yaml&group=G"),
            &admin,
            "admin-dev",
        )
        .await;
    assert_eq!(s, 200);
    assert!(nacos.config("", "G", "app.yaml").is_none(), "远端应已删除");

    let (_, rows) = app.get("/audit?action=nacos_config_delete", &admin, "admin-dev").await;
    assert_eq!(rows.as_array().unwrap().len(), 1);

    // operator 不得触碰
    let op = app.operator().await;
    let (s, _) = app
        .delete(&format!("/nacos/clusters/{id}/configs?data_id=x&group=G"), &op, "op-dev")
        .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn synced_template_replays_app_placeholders_verbatim() {
    // 真机数据暴露过的坑:线上配置里本来就有 ${mysql8.jdbc.url} 这类 **应用自己的**
    // 占位符。同步回来的模板若还按 opsctl 模板变量去代入,就会要求填值并整批失败。
    let app = spawn().await;
    let admin = app.admin().await;
    let src = mock_nacos().await;
    let src_id = register_cluster(&app, &admin, &src.addr).await;

    let raw = "spring:\n  datasource:\n    url: ${mysql8.jdbc.url}\n    name: ${spring.application.name}\n";
    // 即席条目默认会做变量代入,所以铺底数据要显式声明按原文发
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{src_id}/init"),
            &admin,
            "admin-dev",
            json!({ "substitute": false,
                    "items": [{ "data_id": "db.yml", "group": "G", "type": "yaml", "content": raw }] }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["status"], "ok", "{v}");
    assert_eq!(src.config("", "G", "db.yml").unwrap(), raw);

    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{src_id}/sync"),
            &admin,
            "admin-dev",
            json!({ "template_name": "含占位符的真实配置" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let tpl = v["template_id"].as_str().unwrap().to_string();

    // 模板被标记为「原文下发」
    let (_, tpls) = app.get("/nacos/templates", &admin, "admin-dev").await;
    let t = tpls.as_array().unwrap().iter().find(|t| t["id"] == tpl.as_str()).unwrap();
    assert_eq!(t["literal"], 1, "同步产生的模板必须是原文模式:{t}");

    // 回放到另一个集群:一个变量都不给,也必须全成功且内容一字不改
    let dst = mock_nacos().await;
    let dst_id = register_cluster(&app, &admin, &dst.addr).await;
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{dst_id}/init"),
            &admin,
            "admin-dev",
            json!({ "template_id": tpl }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "ok", "原文模板不应因缺变量而失败:{v}");
    assert_eq!(v["items"][0]["status"], "created");
    assert_eq!(dst.config("", "G", "db.yml").unwrap(), raw);

    // 显式要求代入时,才回到「变量未提供」的行为
    let dst2 = mock_nacos().await;
    let dst2_id = register_cluster(&app, &admin, &dst2.addr).await;
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{dst2_id}/init"),
            &admin,
            "admin-dev",
            json!({ "template_id": tpl, "substitute": true }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["status"], "fail");
    assert!(v["items"][0]["message"].as_str().unwrap().contains("mysql8.jdbc.url"), "{v}");
    assert!(dst2.config("", "G", "db.yml").is_none());
}

#[tokio::test]
async fn explicit_empty_namespace_targets_public_not_cluster_default() {
    // public 的 id 在 Nacos 里就是空串。如果把「空串」也当成「没填 → 用集群默认」,
    // 那么集群默认不是 public 时就永远指不到 public,删除/同步会静默落错空间。
    // 语义:字段缺失 = 集群默认;显式给值(含空串)= 就用这个。
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let (s, v) = app
        .post(
            "/nacos/clusters",
            &admin,
            "admin-dev",
            json!({ "name": "默认非 public", "server_addr": nacos.addr, "context_path": "/nacos",
                    "namespace": "tenantA", "username": "nacos", "password": "nacos-pass" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();

    // 不给 namespace → 落集群默认 tenantA
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "items": [{ "data_id": "a.yaml", "group": "G", "content": "in: tenantA" }] }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["namespace"], "tenantA");
    assert_eq!(nacos.config("tenantA", "G", "a.yaml").unwrap(), "in: tenantA");
    assert!(nacos.config("", "G", "a.yaml").is_none(), "不该落到 public");

    // 显式空串 → 落 public,而不是集群默认
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "namespace": "",
                    "items": [{ "data_id": "a.yaml", "group": "G", "content": "in: public" }] }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["namespace"], "");
    assert_eq!(nacos.config("", "G", "a.yaml").unwrap(), "in: public");
    assert_eq!(nacos.config("tenantA", "G", "a.yaml").unwrap(), "in: tenantA", "tenantA 不应被改动");

    // 列表同理:显式空串看 public,缺省看 tenantA
    let (s, v) = app.get(&format!("/nacos/clusters/{id}/configs?namespace="), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["namespace"], "");
    assert_eq!(v["total"], 1);
    let (s, v) = app.get(&format!("/nacos/clusters/{id}/configs"), &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["namespace"], "tenantA");

    // 删除也必须认显式空串(否则会删掉 tenantA 里的同名配置)
    let (s, _) = app
        .delete(
            &format!("/nacos/clusters/{id}/configs?data_id=a.yaml&group=G&namespace="),
            &admin,
            "admin-dev",
        )
        .await;
    assert_eq!(s, 200);
    assert!(nacos.config("", "G", "a.yaml").is_none());
    assert!(nacos.config("tenantA", "G", "a.yaml").is_some(), "只应删 public 那条");
}

#[tokio::test]
async fn template_namespace_becomes_default_replay_target() {
    // 模板归属命名空间:回放时默认发回同一个空间,不用每次手填。
    let app = spawn().await;
    let admin = app.admin().await;
    let nacos = mock_nacos().await;
    let id = register_cluster(&app, &admin, &nacos.addr).await;

    let (s, t) = app
        .post(
            "/nacos/templates",
            &admin,
            "admin-dev",
            json!({ "name": "带归属的模板", "namespace": "tenantB", "literal": true,
                    "items": [{ "data_id": "x.yaml", "group": "G", "content": "k: v" }] }),
        )
        .await;
    assert_eq!(s, 200, "{t}");
    let tpl = t["id"].as_str().unwrap().to_string();

    let (_, list) = app.get("/nacos/templates", &admin, "admin-dev").await;
    assert_eq!(list[0]["namespace"], "tenantB");
    assert_eq!(list[0]["literal"], 1);

    // 集群默认是 public,但模板归属 tenantB → 应发到 tenantB
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "template_id": tpl }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["namespace"], "tenantB");
    assert_eq!(nacos.config("tenantB", "G", "x.yaml").unwrap(), "k: v");

    // 显式指定则覆盖模板归属
    let (s, v) = app
        .post(
            &format!("/nacos/clusters/{id}/init"),
            &admin,
            "admin-dev",
            json!({ "template_id": tpl, "namespace": "tenantC" }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["namespace"], "tenantC");
    assert_eq!(nacos.config("tenantC", "G", "x.yaml").unwrap(), "k: v");
}
