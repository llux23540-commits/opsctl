//! opsctl-vault server entrypoint.

use opsctl_server::config::Config;
use opsctl_server::state::{now_secs, AppState};
use opsctl_server::store::{
    self, AssetRow, RuleRow, Store, SystemUserRow, TagRow, TemplateRow,
};
use opsctl_server::vault::Vault;
use opsctl_server::{auth, build_router, connect_and_init, sql};

use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = Config::load()?;
    if cfg.auth.jwt_secret == "dev-insecure-change-me" {
        tracing::warn!("using INSECURE default jwt_secret — set OPSCTL_AUTH__JWT_SECRET in prod");
    }

    std::fs::create_dir_all("data").ok();

    let store = connect_and_init(&cfg.store.url).await?;
    bootstrap_admin(&store, &cfg).await?;
    seed_dev(&store, &cfg).await?;

    // Vault: auto-unseal from the configured passphrase (and encrypt any
    // plaintext secrets); otherwise the server starts sealed.
    let vault = Arc::new(Vault::new());
    match &cfg.vault.passphrase {
        Some(p) if !p.is_empty() => match vault.unseal(p, &store).await {
            Ok(()) => {
                let n = vault.migrate_plaintext(&store).await?;
                tracing::info!(migrated = n, "vault unsealed at startup");
            }
            // A wrong passphrase must not take the platform down: sealed is a
            // supported state (login / audit / history / 只读视图 all work) and an
            // admin can unseal from 设置 → 凭据金库.
            Err(e) => tracing::error!(
                error = %e,
                "vault unseal FAILED — starting SEALED; fix OPSCTL_VAULT__PASSPHRASE or unseal via 设置 → 凭据金库"
            ),
        },
        _ => tracing::warn!("vault SEALED — set OPSCTL_VAULT__PASSPHRASE or POST /api/vault/unseal"),
    }

    let state = AppState {
        store,
        jwt_secret: Arc::new(cfg.auth.jwt_secret.clone()),
        default_ttl_secs: cfg.auth.default_ttl_secs,
        vault,
        backup: Arc::new(cfg.backup.clone()),
    };

    if cfg.backup.enabled {
        tokio::spawn(opsctl_server::backup::scheduler(state.clone()));
    }

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(&cfg.server.bind).await?;
    tracing::info!(bind = %cfg.server.bind, "opsctl-server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Seed the bootstrap admin if there are no users yet.
async fn bootstrap_admin(store: &Store, cfg: &Config) -> anyhow::Result<()> {
    if store.count_users().await? > 0 {
        return Ok(());
    }
    let hash = auth::hash_password(&cfg.bootstrap.admin_password)?;
    store
        .create_user(
            &uuid::Uuid::new_v4().to_string(),
            &cfg.bootstrap.admin_user,
            "admin@local",
            "admin",
            &hash,
            cfg.auth.default_ttl_secs,
        )
        .await?;
    tracing::warn!(user = %cfg.bootstrap.admin_user, "seeded bootstrap admin (change the password!)");
    Ok(())
}

/// Seed dev accounts + JumpServer-lite demo data for the running binary.
async fn seed_dev(store: &Store, cfg: &Config) -> anyhow::Result<()> {
    if !cfg.dev.seed {
        return Ok(());
    }
    for (name, pass, role) in [("operator", "operator", "operator"), ("viewer", "viewer", "viewer")] {
        if store.get_user_by_name(name).await?.is_none() {
            let hash = auth::hash_password(pass)?;
            store
                .create_user(&uuid::Uuid::new_v4().to_string(), name, &format!("{name}@local"), role, &hash, cfg.auth.default_ttl_secs)
                .await?;
            tracing::info!(user = %name, role = %role, "seeded test account");
        }
    }

    store
        .create_entry(&store::EntryRow {
            id: "node1".into(),
            project: "demo".into(),
            name: "sample-node".into(),
            kind: "db_server".into(),
            host: cfg.dev.sample_host.clone(),
            port: cfg.dev.sample_port,
            username: cfg.dev.sample_user.clone(),
            secret: cfg.dev.sample_password.clone(),
        })
        .await?;

    let now = now_secs();
    store.create_asset(&AssetRow { id: "site-east".into(), name: "华东生产".into(), kind: "site".into(), parent_id: None, host: String::new(), port: 0, status: "enabled".into(), created_at: now, env: "prod".into() }).await?;
    for (id, name) in [("web-01", "web-01"), ("web-02", "web-02")] {
        store.create_asset(&AssetRow { id: id.into(), name: name.into(), kind: "server".into(), parent_id: Some("site-east".into()), host: cfg.dev.sample_host.clone(), port: cfg.dev.sample_port, status: "enabled".into(), created_at: now, env: String::new() }).await?;
    }
    store.create_tag(&TagRow { id: "tag-web".into(), name: "web".into(), color: "#388bfd".into(), usage_count: 0 }).await?;
    store.add_asset_tag("web-01", "tag-web").await?;
    store.add_asset_tag("web-02", "tag-web").await?;
    store.create_system_user(&SystemUserRow { id: "su-webssh".into(), name: "web-ssh".into(), kind: "ssh_pw".into(), username: cfg.dev.sample_user.clone(), secret: cfg.dev.sample_password.clone() }).await?;
    store.add_asset_account("web-01", "su-webssh").await?;
    store.add_asset_account("web-02", "su-webssh").await?;

    let db_path = "data/demo.db";
    store.create_asset(&AssetRow { id: "db-demo".into(), name: "demo-sqlite".into(), kind: "database".into(), parent_id: Some("site-east".into()), host: db_path.into(), port: 0, status: "enabled".into(), created_at: now, env: String::new() }).await?;
    store.create_system_user(&SystemUserRow { id: "su-demodb".into(), name: "demo-db".into(), kind: "db_pw".into(), username: "demo".into(), secret: String::new() }).await?;
    store.add_asset_account("db-demo", "su-demodb").await?;
    store.add_asset_tag("db-demo", "tag-web").await?;
    let _ = sql::run_query(db_path, "CREATE TABLE IF NOT EXISTS servers(id INTEGER PRIMARY KEY, name TEXT, site TEXT)").await;
    if let Ok(o) = sql::run_query(db_path, "SELECT count(*) FROM servers").await {
        if o.output.contains("0") {
            let _ = sql::run_query(db_path, "INSERT INTO servers(name,site) VALUES ('web-01','华东生产'),('web-02','华东生产')").await;
        }
    }
    if let Some(op) = store.get_user_by_name("operator").await? {
        store.create_rule(&RuleRow { id: "rule-op-web".into(), name: "operator 可 SSH web 标签".into(), subject_user_id: op.id.clone(), selector_kind: "tag".into(), selector: "tag-web".into(), system_user_id: "su-webssh".into(), actions: "ssh".into(), valid_from: 0, valid_until: None, needs_approval: 0, min_approvals: 1, approver_ids: String::new(), quick: "console".into() }).await?;
        store.create_rule(&RuleRow { id: "rule-op-sql".into(), name: "operator 可 SQL demo 库".into(), subject_user_id: op.id, selector_kind: "assets".into(), selector: "db-demo".into(), system_user_id: "su-demodb".into(), actions: "sql".into(), valid_from: 0, valid_until: None, needs_approval: 0, min_approvals: 1, approver_ids: String::new(), quick: "console".into() }).await?;
    }
    store.upsert_template(&TemplateRow { id: "tpl-restart".into(), name: "重启服务".into(), kind: "ssh".into(), command: "systemctl restart {{service}}".into(), variables: r#"[{"name":"service","default":"nginx"}]"#.into(), approver_ids: String::new(), created_at: now, parent_id: None, sort: 0 }).await?;
    store.upsert_template(&TemplateRow { id: "tpl-count".into(), name: "统计行数".into(), kind: "sql".into(), command: "SELECT count(*) FROM {{table}}".into(), variables: r#"[{"name":"table","default":"servers"}]"#.into(), approver_ids: String::new(), created_at: now, parent_id: None, sort: 0 }).await?;

    tracing::info!("seeded JumpServer-lite demo");
    Ok(())
}
