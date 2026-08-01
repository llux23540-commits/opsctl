//! opsctl-server library: app assembly + seeding, shared by the binary and the
//! integration test suite.

pub mod api;
pub mod auth;
pub mod backup;
pub mod config;
pub mod error;
pub mod git;
pub mod jobs;
pub mod nacos;
pub mod rbac;
pub mod sql;
pub mod ssh;
pub mod state;
pub mod store;
pub mod totp;
pub mod vault;

use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::AuthUser;
use crate::state::AppState;
use crate::store::Store;

/// Connect to sqlite and create the schema.
pub async fn connect_and_init(url: &str) -> anyhow::Result<Store> {
    let store = Store::connect(url).await?;
    store.init().await?;
    Ok(store)
}

/// Assemble the full router (API under `/api`, legacy top-level endpoints, SPA
/// fallback, permissive CORS). Used by both `main` and tests.
pub fn build_router(state: AppState) -> Router {
    let cors = tower_http::cors::CorsLayer::very_permissive();

    let api = Router::new()
        .route("/login", post(auth::login))
        .route("/login/otp", post(auth::login_otp))
        .route("/register", post(auth::register))
        .route("/flags", get(api::get_flags).put(api::put_flags))
        .route("/me", get(me))
        .route("/audit", get(audit_list))
        .route("/audit/export", get(api::audit_export))
        .route("/vault/status", get(api::vault_status))
        .route("/vault/unseal", post(api::vault_unseal))
        .route("/vault/seal", post(api::vault_seal))
        .route("/assets", get(api::list_assets).post(api::create_asset))
        .route(
            "/assets/{id}",
            get(api::get_asset).put(api::update_asset).delete(api::delete_asset),
        )
        .route("/assets/{id}/file", get(api::asset_file_view))
        .route("/assets/{id}/history", get(api::node_history))
        .route("/assets/probe", post(api::probe_asset))
        .route("/accounts", get(api::list_accounts).post(api::create_account))
        .route("/accounts/{id}", put(api::update_account).delete(api::delete_account))
        .route("/tags", get(api::list_tags).post(api::create_tag))
        .route("/tags/{id}", put(api::update_tag).delete(api::delete_tag))
        .route("/rules", get(api::list_rules).post(api::create_rule))
        .route("/rules/{id}", put(api::update_rule).delete(api::delete_rule))
        .route("/users", get(api::list_users).post(api::create_user))
        .route("/users/{id}", put(api::update_user).delete(api::delete_user))
        .route("/users/{id}/reset-password", post(api::reset_password))
        .route("/jobs", get(api::list_jobs))
        .route("/jobs/ssh", post(api::submit_ssh))
        .route("/jobs/sql", post(api::submit_sql))
        .route("/jobs/{id}", get(api::get_job_detail))
        .route("/approvals", get(api::list_approvals))
        .route("/approvals/decide-batch", post(api::decide_batch))
        .route("/approvals/{id}/decide", post(api::decide_approval))
        .route("/messages", get(api::list_messages))
        .route("/messages/unread-count", get(api::unread_count))
        .route("/messages/read-all", post(api::mark_all_messages_read))
        .route("/messages/{id}/read", post(api::mark_message_read))
        .route("/messages/{id}/unread", post(api::mark_message_unread))
        .route("/messages/{id}", axum::routing::delete(api::delete_message))
        .route("/profile", get(api::get_profile).put(api::update_profile))
        .route("/profile/totp/start", post(api::totp_start))
        .route("/profile/totp/confirm", post(api::totp_confirm))
        .route("/profile/totp/disable", post(api::totp_disable))
        .route("/profile/password", put(api::change_password))
        .route("/sessions", get(api::list_sessions))
        .route("/sessions/{sid}/revoke", post(api::revoke_session))
        .route("/telegram/bind/start", post(api::telegram_bind_start))
        .route("/telegram/bind/confirm", post(api::telegram_bind_confirm))
        .route("/telegram/unbind", post(api::telegram_unbind))
        .route("/settings/git", get(api::get_git).put(api::put_git))
        .route("/settings/git/reveal", post(api::git_reveal))
        .route("/settings/git/{what}", post(api::git_action))
        .route("/templates", get(api::list_templates).post(api::save_template))
        .route("/templates/{id}", post(api::save_template).delete(api::delete_template))
        .route("/templates/{id}/file", get(api::template_file_view))
        .route("/backup/status", get(api::backup_status))
        .route("/backup/run", post(api::backup_run))
        // ---- Nacos 管理(集群总览 + 配置初始化,均 admin-only)----
        .route("/nacos/clusters", get(nacos::list_clusters).post(nacos::create_cluster))
        .route(
            "/nacos/clusters/{id}",
            put(nacos::update_cluster).delete(nacos::delete_cluster),
        )
        .route("/nacos/clusters/{id}/nodes", get(nacos::cluster_nodes))
        .route(
            "/nacos/clusters/{id}/configs",
            get(nacos::cluster_configs).delete(nacos::delete_config_api),
        )
        .route("/nacos/clusters/{id}/configs/detail", get(nacos::config_detail))
        .route("/nacos/clusters/{id}/sync", post(nacos::sync_cluster))
        .route("/nacos/clusters/{id}/init", post(nacos::init_cluster))
        .route("/nacos/probe", post(nacos::probe_cluster))
        .route("/nacos/templates", get(nacos::list_templates).post(nacos::save_template))
        .route("/nacos/templates/{id}", axum::routing::delete(nacos::delete_template))
        .route("/nacos/runs", get(nacos::list_runs))
        // ---- Nacos 管理面:命名空间 / 账号 / 角色 / 权限(直连 Nacos Open API)----
        .route(
            "/nacos/clusters/{id}/namespaces",
            get(nacos::list_namespaces_api)
                .post(nacos::create_namespace_api)
                .put(nacos::update_namespace_api),
        )
        .route(
            "/nacos/clusters/{id}/namespaces/{ns}",
            axum::routing::delete(nacos::delete_namespace_api),
        )
        .route(
            "/nacos/clusters/{id}/users",
            get(nacos::list_users_api).post(nacos::create_user_api).put(nacos::reset_user_api),
        )
        .route(
            "/nacos/clusters/{id}/users/{username}",
            axum::routing::delete(nacos::delete_user_api),
        )
        .route(
            "/nacos/clusters/{id}/roles",
            get(nacos::list_roles_api)
                .post(nacos::bind_role_api)
                .delete(nacos::unbind_role_api),
        )
        .route(
            "/nacos/clusters/{id}/permissions",
            get(nacos::list_permissions_api)
                .post(nacos::grant_permission_api)
                .delete(nacos::revoke_permission_api),
        );

    Router::new()
        .route("/health", get(health))
        // legacy desktop login/me kept (no RBAC bypass); /entries and top-level
        // /jobs/ssh removed — they bypassed the rule engine (only Role::can).
        .route("/login", post(auth::login))
        .route("/me", get(me))
        .nest("/api", api)
        .fallback(static_handler)
        .layer(cors)
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

/// Embedded web frontend (built Vue app). Debug builds read from disk.
#[derive(rust_embed::RustEmbed)]
#[folder = "../web/dist"]
struct WebAssets;

/// Serve embedded static files; SPA fallback to index.html.
async fn static_handler(uri: axum::http::Uri) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };
    if let Some(content) = WebAssets::get(path) {
        let ct = mime_guess::from_path(path).first_or_octet_stream().to_string();
        return ([(header::CONTENT_TYPE, ct)], content.data.into_owned()).into_response();
    }
    match WebAssets::get("index.html") {
        Some(c) => (
            [(header::CONTENT_TYPE, "text/html".to_string())],
            c.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "frontend not built (run: cd web && npm run build)")
            .into_response(),
    }
}

/// Audit rows (admin only) with optional filters via query string.
async fn audit_list(
    user: AuthUser,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    axum::extract::State(st): axum::extract::State<AppState>,
) -> Result<Json<serde_json::Value>, crate::error::AppError> {
    if user.role != opsctl_core::Role::Admin {
        return Err(crate::error::AppError::Forbidden);
    }
    let g = |k: &str| q.get(k).cloned().unwrap_or_default();
    let rows = st
        .store
        .list_audit_filtered(&g("action"), &g("result"), &g("operator"), &g("q"), 200)
        .await
        .map_err(crate::error::AppError::Internal)?;
    Ok(Json(serde_json::to_value(rows).unwrap_or_default()))
}

/// Protected: exercises the AuthUser extractor (JWT + device + session checks).
async fn me(user: AuthUser) -> Json<serde_json::Value> {
    Json(json!({
        "user_id": user.user_id,
        "email": user.email,
        "role": user.role,
        "sid": user.sid,
        "device_id": user.device_id,
    }))
}

/// Deterministic fixture for tests: admin/operator/viewer + JumpServer-lite demo
/// (site/servers/tag/accounts/rules), a sqlite database asset at `demo_db`, and
/// two execution templates. Passwords equal the usernames.
pub async fn seed_fixture(store: &Store, demo_db: &str) -> anyhow::Result<()> {
    use store::{AssetRow, RuleRow, SystemUserRow, TagRow, TemplateRow};
    let now = crate::state::now_secs();

    for (id, name, role) in [
        ("u-admin", "admin", "admin"),
        ("u-admin2", "admin2", "admin"),
        ("u-op", "operator", "operator"),
        ("u-viewer", "viewer", "viewer"),
    ] {
        let hash = auth::hash_password(name)?;
        store.create_user(id, name, &format!("{name}@local"), role, &hash, 7 * 24 * 3600).await?;
    }

    store.create_asset(&AssetRow { id: "site-east".into(), name: "华东生产".into(), kind: "site".into(), parent_id: None, host: String::new(), port: 0, status: "enabled".into(), created_at: now, env: "prod".into() }).await?;
    for id in ["web-01", "web-02"] {
        store.create_asset(&AssetRow { id: id.into(), name: id.into(), kind: "server".into(), parent_id: Some("site-east".into()), host: "127.0.0.1".into(), port: 22, status: "enabled".into(), created_at: now, env: String::new() }).await?;
    }
    store.create_tag(&TagRow { id: "tag-web".into(), name: "web".into(), color: "#19b8a6".into(), usage_count: 0 }).await?;
    store.add_asset_tag("web-01", "tag-web").await?;
    store.add_asset_tag("web-02", "tag-web").await?;
    store.create_system_user(&SystemUserRow { id: "su-webssh".into(), name: "web-ssh".into(), kind: "ssh_pw".into(), username: "root".into(), secret: "pw".into() }).await?;
    store.add_asset_account("web-01", "su-webssh").await?;
    store.add_asset_account("web-02", "su-webssh").await?;

    store.create_asset(&AssetRow { id: "db-demo".into(), name: "demo-sqlite".into(), kind: "database".into(), parent_id: Some("site-east".into()), host: demo_db.into(), port: 0, status: "enabled".into(), created_at: now, env: String::new() }).await?;
    store.create_system_user(&SystemUserRow { id: "su-demodb".into(), name: "demo-db".into(), kind: "db_pw".into(), username: "demo".into(), secret: String::new() }).await?;
    store.add_asset_account("db-demo", "su-demodb").await?;
    let _ = sql::run_query(demo_db, "CREATE TABLE IF NOT EXISTS servers(id INTEGER PRIMARY KEY, name TEXT, site TEXT)").await;
    let _ = sql::run_query(demo_db, "DELETE FROM servers").await;
    let _ = sql::run_query(demo_db, "INSERT INTO servers(name,site) VALUES ('web-01','east'),('web-02','east')").await;

    store.create_rule(&RuleRow { id: "rule-op-web".into(), name: "op ssh web".into(), subject_user_id: "u-op".into(), selector_kind: "tag".into(), selector: "tag-web".into(), system_user_id: "su-webssh".into(), actions: "ssh".into(), valid_from: 0, valid_until: None, needs_approval: 0, min_approvals: 1, approver_ids: String::new(), quick: "console".into() }).await?;
    store.create_rule(&RuleRow { id: "rule-op-sql".into(), name: "op sql demo".into(), subject_user_id: "u-op".into(), selector_kind: "assets".into(), selector: "db-demo".into(), system_user_id: "su-demodb".into(), actions: "sql".into(), valid_from: 0, valid_until: None, needs_approval: 0, min_approvals: 1, approver_ids: String::new(), quick: "console".into() }).await?;

    store.upsert_template(&TemplateRow { id: "tpl-restart".into(), name: "restart".into(), kind: "ssh".into(), command: "systemctl restart {{service}}".into(), variables: r#"[{"name":"service","default":"nginx"}]"#.into(), approver_ids: String::new(), created_at: now, parent_id: None, sort: 0 }).await?;
    store.upsert_template(&TemplateRow { id: "tpl-count".into(), name: "count".into(), kind: "sql".into(), command: "SELECT count(*) FROM {{table}}".into(), variables: r#"[{"name":"table","default":"servers"}]"#.into(), approver_ids: String::new(), created_at: now, parent_id: None, sort: 0 }).await?;

    Ok(())
}
