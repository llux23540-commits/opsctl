//! REST API for the web frontend (JumpServer-lite). Mounted under `/api`.

use axum::extract::{Path, State};
use axum::Json;
use opsctl_core::api::{
    ApprovalView, DecideRequest, JobResult, SubmitSqlJob, SubmitSshJob, TargetResult,
};
use opsctl_core::model::Role;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::{now_secs, AppState};
use crate::store::{AssetRow, RuleRow, SystemUserRow, TagRow};
use crate::{rbac, sql, ssh};

pub(crate) fn is_admin(u: &AuthUser) -> bool {
    u.role == Role::Admin
}

// ---- assets ----

pub async fn list_assets(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let vis = rbac::visible_asset_ids(&st.store, &user.user_id, is_admin(&user)).await;
    let all = st.store.list_assets().await.map_err(AppError::Internal)?;
    let mut out = Vec::new();
    for a in all.into_iter().filter(|a| vis.contains(&a.id)) {
        // attach tag ids so the console can offer tag quick-filters
        let tag_ids = st.store.asset_tag_ids(&a.id).await.unwrap_or_default();
        let mut v = serde_json::to_value(&a).unwrap_or_default();
        if let Some(obj) = v.as_object_mut() {
            obj.insert("tag_ids".into(), json!(tag_ids));
        }
        out.push(v);
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
pub struct CreateAsset {
    pub id: Option<String>,
    pub name: String,
    pub kind: String, // site | server | database
    pub parent_id: Option<String>,
    #[serde(default)]
    pub host: String,
    #[serde(default = "def_port")]
    pub port: i64,
    #[serde(default)]
    pub account_id: Option<String>, // bind an account
    #[serde(default)]
    pub tag_ids: Vec<String>,
    #[serde(default = "def_status")]
    pub status: String,
    #[serde(default)]
    pub env: String,
}
fn def_port() -> i64 {
    22
}

pub async fn create_asset(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<CreateAsset>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let id = req.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    st.store
        .create_asset(&AssetRow {
            id: id.clone(),
            name: req.name,
            kind: req.kind,
            parent_id: req.parent_id,
            host: req.host,
            port: req.port,
            status: if req.status == "disabled" { "disabled".into() } else { "enabled".into() },
            created_at: now_secs(),
            env: req.env,
        })
        .await
        .map_err(AppError::Internal)?;
    if let Some(acc) = req.account_id {
        let _ = st.store.add_asset_account(&id, &acc).await;
    }
    for t in req.tag_ids {
        let _ = st.store.add_asset_tag(&id, &t).await;
    }
    Ok(Json(json!({ "id": id })))
}

/// Asset detail (admin): row + bound tag/account ids, for the edit form.
pub async fn get_asset(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let asset = st.store.get_asset(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("资产不存在".into()))?;
    let tag_ids = st.store.asset_tag_ids(&id).await.map_err(AppError::Internal)?;
    let account_ids = st.store.accounts_of_asset(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "asset": asset, "tag_ids": tag_ids, "account_ids": account_ids })))
}

#[derive(Deserialize)]
pub struct UpdateAsset {
    pub name: String,
    pub kind: String,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub host: String,
    #[serde(default = "def_port")]
    pub port: i64,
    #[serde(default = "def_status")]
    pub status: String,
    #[serde(default)]
    pub tag_ids: Option<Vec<String>>, // None = leave unchanged
    #[serde(default)]
    pub account_ids: Option<Vec<String>>, // None = leave unchanged
    #[serde(default)]
    pub env: String,
}
fn def_status() -> String {
    "enabled".into()
}

pub async fn update_asset(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAsset>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let old = st.store.get_asset(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("资产不存在".into()))?;
    if req.parent_id.as_deref() == Some(id.as_str()) {
        return Err(AppError::BadRequest("父级不能是自己".into()));
    }
    st.store
        .update_asset(&AssetRow {
            id: id.clone(),
            name: req.name,
            kind: req.kind,
            parent_id: req.parent_id,
            host: req.host,
            port: req.port,
            status: req.status,
            created_at: old.created_at,
            env: req.env,
        })
        .await
        .map_err(AppError::Internal)?;
    if let Some(tags) = req.tag_ids {
        st.store.set_asset_tags(&id, &tags).await.map_err(AppError::Internal)?;
    }
    if let Some(accs) = req.account_ids {
        st.store.set_asset_accounts(&id, &accs).await.map_err(AppError::Internal)?;
    }
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_asset(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let children = st.store.count_asset_children(&id).await.map_err(AppError::Internal)?;
    if children > 0 {
        return Err(AppError::BadRequest(format!("该站点下还有 {children} 个节点,请先删除或移走")));
    }
    st.store.delete_asset(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

// ---- accounts (system users) ----

pub async fn list_accounts(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<SystemUserRow>>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    Ok(Json(st.store.list_system_users().await.map_err(AppError::Internal)?))
}

#[derive(Deserialize)]
pub struct CreateAccount {
    pub name: String,
    #[serde(default = "def_kind")]
    pub kind: String,
    pub username: String,
    #[serde(default)]
    pub secret: String,
}
fn def_kind() -> String {
    "ssh_pw".into()
}

pub async fn create_account(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<CreateAccount>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let id = Uuid::new_v4().to_string();
    let secret = encrypt_secret(&st, &req.secret)?;
    st.store
        .create_system_user(&SystemUserRow {
            id: id.clone(),
            name: req.name,
            kind: req.kind,
            username: req.username,
            secret,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

/// Encrypt a non-empty secret through the vault; empty passes through. Errors
/// (503) if the vault is sealed and there is a secret to protect.
fn encrypt_secret(st: &AppState, secret: &str) -> Result<String, AppError> {
    if secret.is_empty() {
        return Ok(String::new());
    }
    if st.vault.is_sealed() {
        return Err(AppError::Sealed);
    }
    st.vault.encrypt(secret).map_err(AppError::Internal)
}

/// Update; empty secret keeps the stored one.
pub async fn update_account(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateAccount>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    st.store.get_system_user(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("账号不存在".into()))?;
    // empty secret → keep old (store handles); non-empty → encrypt
    let secret = encrypt_secret(&st, &req.secret)?;
    st.store
        .update_system_user(&SystemUserRow {
            id: id.clone(),
            name: req.name,
            kind: req.kind,
            username: req.username,
            secret,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_account(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let used = st.store.count_rules_using_account(&id).await.map_err(AppError::Internal)?;
    if used > 0 {
        return Err(AppError::BadRequest(format!("有 {used} 条授权规则在用该账号,请先调整规则")));
    }
    st.store.delete_system_user(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

// ---- tags ----

pub async fn list_tags(
    _user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<TagRow>>, AppError> {
    Ok(Json(st.store.list_tags().await.map_err(AppError::Internal)?))
}

#[derive(Deserialize)]
pub struct CreateTag {
    pub name: String,
    #[serde(default)]
    pub color: String,
}

pub async fn create_tag(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<CreateTag>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let id = Uuid::new_v4().to_string();
    st.store
        .create_tag(&TagRow {
            id: id.clone(),
            name: req.name,
            color: req.color,
            usage_count: 0,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn update_tag(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateTag>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    st.store
        .update_tag(&TagRow { id: id.clone(), name: req.name, color: req.color, usage_count: 0 })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_tag(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let used = st.store.count_rules_using_tag(&id).await.map_err(AppError::Internal)?;
    if used > 0 {
        return Err(AppError::BadRequest(format!("有 {used} 条授权规则按该标签授权,请先调整规则")));
    }
    st.store.delete_tag(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

// ---- authorization rules ----

pub async fn list_rules(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<RuleRow>>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    Ok(Json(st.store.list_rules().await.map_err(AppError::Internal)?))
}

#[derive(Deserialize)]
pub struct CreateRule {
    #[serde(default)]
    pub name: String,
    pub subject_user_id: String,
    pub selector_kind: String, // subtree | tag | assets
    pub selector: String,
    #[serde(default)]
    pub system_user_id: String,
    pub actions: Vec<String>, // ["ssh","sql","upload"]
    #[serde(default)]
    pub valid_until: Option<i64>,
    #[serde(default)]
    pub needs_approval: bool,
    /// how many distinct approvers must approve (会签); default 1
    #[serde(default = "def_min_approvals")]
    pub min_approvals: i64,
    /// designated approver user ids (empty = any admin)
    #[serde(default)]
    pub approver_ids: Vec<String>,
    /// review channel: "console" (strong auth) | "tg" (inline one-tap, demo)
    #[serde(default = "def_quick")]
    pub quick: String,
}
fn def_min_approvals() -> i64 {
    1
}
fn def_quick() -> String {
    "console".into()
}
fn validate_quick(q: &str) -> Result<(), AppError> {
    match q {
        "console" | "tg" => Ok(()),
        _ => Err(AppError::BadRequest("审核方式仅支持 console 或 tg".into())),
    }
}

pub async fn create_rule(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<CreateRule>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    validate_quick(&req.quick)?;
    let id = Uuid::new_v4().to_string();
    st.store
        .create_rule(&RuleRow {
            id: id.clone(),
            name: req.name,
            subject_user_id: req.subject_user_id,
            selector_kind: req.selector_kind,
            selector: req.selector,
            system_user_id: req.system_user_id,
            actions: req.actions.join(","),
            valid_from: now_secs(),
            valid_until: req.valid_until,
            needs_approval: req.needs_approval as i64,
            min_approvals: req.min_approvals.max(1),
            approver_ids: req.approver_ids.join(","),
            quick: req.quick,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

/// Full-row update (the form always sends every field).
pub async fn update_rule(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateRule>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let old = st.store.get_rule(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("规则不存在".into()))?;
    validate_quick(&req.quick)?;
    st.store
        .create_rule(&RuleRow {
            id: id.clone(),
            name: req.name,
            subject_user_id: req.subject_user_id,
            selector_kind: req.selector_kind,
            selector: req.selector,
            system_user_id: req.system_user_id,
            actions: req.actions.join(","),
            valid_from: old.valid_from,
            valid_until: req.valid_until,
            needs_approval: req.needs_approval as i64,
            min_approvals: req.min_approvals.max(1),
            approver_ids: req.approver_ids.join(","),
            quick: req.quick,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_rule(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    st.store.delete_rule(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

// ---- users (for rule subject picker) ----

pub async fn list_users(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let rows = st.store.list_users().await.map_err(AppError::Internal)?;
    let out: Vec<_> = rows.into_iter()
        .map(|u| json!({
            "id": u.id, "name": u.name, "email": u.email, "role": u.role,
            "totp_enabled": !u.totp_secret.is_empty(),
        }))
        .collect();
    Ok(Json(json!(out)))
}

fn valid_role(r: &str) -> bool {
    matches!(r, "admin" | "operator" | "viewer")
}

#[derive(Deserialize)]
pub struct CreateUser {
    pub name: String,
    #[serde(default)]
    pub email: String,
    pub role: String,
    pub password: String,
}

pub async fn create_user(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<CreateUser>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    if req.name.trim().is_empty() || req.password.len() < 6 {
        return Err(AppError::BadRequest("用户名必填,密码至少 6 位".into()));
    }
    if !valid_role(&req.role) {
        return Err(AppError::BadRequest("角色非法".into()));
    }
    if st.store.get_user_by_name(req.name.trim()).await.map_err(AppError::Internal)?.is_some() {
        return Err(AppError::BadRequest("用户名已存在".into()));
    }
    let hash = crate::auth::hash_password(&req.password).map_err(AppError::Internal)?;
    let id = Uuid::new_v4().to_string();
    st.store.create_user(&id, req.name.trim(), req.email.trim(), &req.role, &hash, st.default_ttl_secs)
        .await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct UpdateUser {
    pub name: String,
    #[serde(default)]
    pub email: String,
    pub role: String,
}

pub async fn update_user(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateUser>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    if !valid_role(&req.role) {
        return Err(AppError::BadRequest("角色非法".into()));
    }
    let target = st.store.get_user_by_id(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("用户不存在".into()))?;
    // don't let the last admin demote themselves out of admin
    if target.role == "admin" && req.role != "admin"
        && st.store.count_admins().await.map_err(AppError::Internal)? <= 1 {
        return Err(AppError::BadRequest("不能降级最后一个管理员".into()));
    }
    st.store.update_user_fields(&id, req.name.trim(), req.email.trim(), &req.role)
        .await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct ResetPassword { pub password: String }

pub async fn reset_password(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ResetPassword>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    if req.password.len() < 6 {
        return Err(AppError::BadRequest("密码至少 6 位".into()));
    }
    st.store.get_user_by_id(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("用户不存在".into()))?;
    let hash = crate::auth::hash_password(&req.password).map_err(AppError::Internal)?;
    st.store.set_password(&id, &hash).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_user(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    if id == user.user_id {
        return Err(AppError::BadRequest("不能删除自己".into()));
    }
    let target = st.store.get_user_by_id(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("用户不存在".into()))?;
    if target.role == "admin" && st.store.count_admins().await.map_err(AppError::Internal)? <= 1 {
        return Err(AppError::BadRequest("不能删除最后一个管理员".into()));
    }
    let rules = st.store.count_rules_for_subject(&id).await.map_err(AppError::Internal)?;
    if rules > 0 {
        return Err(AppError::BadRequest(format!("该用户有 {rules} 条授权规则,请先删除")));
    }
    st.store.delete_user(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ChangePassword {
    pub old_password: String,
    pub new_password: String,
}

/// Self password change (any authenticated user).
pub async fn change_password(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<ChangePassword>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.new_password.len() < 6 {
        return Err(AppError::BadRequest("新密码至少 6 位".into()));
    }
    let u = st.store.get_user_by_id(&user.user_id).await.map_err(AppError::Internal)?
        .ok_or(AppError::Unauthorized)?;
    if !crate::auth::verify_password(&u.pass_hash, &req.old_password) {
        return Err(AppError::BadRequest("原密码不正确".into()));
    }
    let hash = crate::auth::hash_password(&req.new_password).map_err(AppError::Internal)?;
    st.store.set_password(&user.user_id, &hash).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- SSH job (authorized via rules) ----

/// Resolve asset + account and run the command; returns a TargetResult (never
/// pending). Shared by direct execution and approval release.
async fn run_ssh_on(
    st: &AppState,
    target: &str,
    account_id: &str,
    command: &str,
) -> TargetResult {
    let started = std::time::Instant::now();
    let mut tr = run_ssh_on_inner(st, target, account_id, command).await;
    tr.duration_ms = started.elapsed().as_millis() as i64;
    tr
}

async fn run_ssh_on_inner(
    st: &AppState,
    target: &str,
    account_id: &str,
    command: &str,
) -> TargetResult {
    let asset = st.store.get_asset(target).await.ok().flatten();
    let acc = st.store.get_system_user(account_id).await.ok().flatten();
    match (asset, acc) {
        (Some(a), Some(su)) => {
            let secret = match st.vault.decrypt(&su.secret) {
                Ok(s) => s,
                Err(_) => return TargetResult {
                    target: target.to_string(), ok: false,
                    error: Some("金库已封存,请先解封".into()), ..Default::default()
                },
            };
            // Dispatch by account kind: ssh_key → public-key auth, else password.
            let run = if su.kind == "ssh_key" {
                ssh::run_command_key(&a.host, a.port as u16, &su.username, &secret, None, command).await
            } else {
                ssh::run_command(&a.host, a.port as u16, &su.username, &secret, command).await
            };
            match run {
                Ok(out) => TargetResult {
                    target: target.to_string(),
                    ok: out.exit_code == Some(0),
                    exit_code: out.exit_code,
                    stdout: out.stdout,
                    stderr: out.stderr,
                    error: None,
                    ..Default::default()
                },
                Err(e) => TargetResult {
                    target: target.to_string(),
                    ok: false,
                    error: Some(e.to_string()),
                    ..Default::default()
                },
            }
        }
        _ => TargetResult {
            target: target.to_string(),
            ok: false,
            error: Some("资产或账号缺失".into()),
            ..Default::default()
        },
    }
}

/// Look up an asset's display name (falls back to the id when unknown).
async fn asset_display_name(st: &AppState, id: &str) -> String {
    st.store.get_asset(id).await.ok().flatten()
        .map(|a| a.name)
        .unwrap_or_else(|| id.to_string())
}

/// Persist one target outcome under its job.
async fn record_job_target(
    st: &AppState, job_id: &str, target: &str, name: String, tr: &TargetResult,
) {
    let status = if tr.pending {
        "pending"
    } else if tr.ok {
        "ok"
    } else {
        "fail"
    };
    let _ = st.store.insert_job_target(&crate::store::JobTargetRow {
        id: Uuid::new_v4().to_string(),
        job_id: job_id.to_string(),
        asset_id: target.to_string(),
        asset_name: name,
        status: status.into(),
        exit_code: tr.exit_code.map(|c| c as i64),
        stdout: tr.stdout.clone(),
        stderr: tr.stderr.clone(),
        error: tr.error.clone(),
        duration_ms: tr.duration_ms,
        approval_id: tr.approval_id.clone(),
        ts: now_secs(),
    }).await;
}

/// Finalize the job and, when fully decided, notify the operator (exec message).
async fn finalize_job(st: &AppState, job_id: &str, operator_id: &str, command: &str) {
    let Ok(Some(job)) = st.store.finalize_job_if_done(job_id, now_secs()).await else { return };
    if job.status != "pending" {
        let mut cmd = command.replace('\n', " ");
        if cmd.chars().count() > 60 {
            cmd = format!("{}…", cmd.chars().take(60).collect::<String>());
        }
        let _ = st.store.push_notification(operator_id, "exec", "执行完成",
            &format!("{}/{} 成功 · {}", job.ok_count, job.total, cmd),
            &format!("/record/{job_id}"), now_secs()).await;
    }
}

/// Resolve template provenance for a job: (template_id, template_name).
/// Unknown/absent ids degrade to empty strings (provenance is best-effort).
async fn resolve_template(st: &AppState, template_id: &Option<String>) -> (String, String) {
    match template_id.as_deref().filter(|s| !s.is_empty()) {
        Some(tid) => match st.store.get_template(tid).await.ok().flatten() {
            Some(t) => (tid.to_string(), t.name),
            None => (String::new(), String::new()),
        },
        None => (String::new(), String::new()),
    }
}

pub async fn submit_ssh(
    user: AuthUser,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    State(st): State<AppState>,
    Json(req): Json<SubmitSshJob>,
) -> Result<Json<JobResult>, AppError> {
    let job_id = Uuid::new_v4().to_string();
    let mut results = Vec::new();
    let (template_id, template_name) = resolve_template(&st, &req.template_id).await;
    let _ = st.store.create_job(&crate::store::JobRow {
        id: job_id.clone(),
        kind: "ssh".into(),
        command: req.command.clone(),
        operator_id: user.user_id.clone(),
        operator_email: user.email.clone(),
        created_at: now_secs(),
        finished_at: None,
        status: "pending".into(),
        total: req.targets.len() as i64,
        ok_count: 0,
        source_ip: ip,
        source_device: user.device_id.clone(),
        template_id,
        template_name,
    }).await;

    for target in &req.targets {
        // 1) authorize + resolve which account to connect with
        let authz = rbac::authorize(&st.store, &user.user_id, is_admin(&user), target, "ssh").await;
        let (tr, audit_action, audit_result) = match authz {
            None => (
                TargetResult {
                    target: target.clone(),
                    ok: false,
                    error: Some("未授权(无匹配规则或无账号)".into()),
                    ..Default::default()
                },
                "ssh.exec",
                "fail",
            ),
            // 2) gated by approval → hold, do not execute
            Some(a) if a.needs_approval => {
                let approval_id = Uuid::new_v4().to_string();
                let _ = st
                    .store
                    .create_approval(&crate::store::ApprovalRow {
                        id: approval_id.clone(),
                        job_id: job_id.clone(),
                        requester_id: user.user_id.clone(),
                        requester_email: user.email.clone(),
                        asset_id: target.clone(),
                        account_id: a.account_id,
                        action: "ssh".into(),
                        command: req.command.clone(),
                        state: "pending".into(),
                        reason: None,
                        decided_by: None,
                        created_at: now_secs(),
                        decided_at: None,
                        min_approvals: a.min_approvals,
                        approver_ids: a.approver_ids,
                        quick: a.quick,
                    })
                    .await;
                // notify admins there's a new pending approval
                if let Ok(admins) = st.store.admin_ids().await {
                    for aid in admins {
                        let _ = st.store.push_notification(&aid, "approval", "有新的待审批",
                            &format!("{} 请求 SSH:{}", user.email, req.command), "/approvals", now_secs()).await;
                    }
                }
                (
                    TargetResult {
                        target: target.clone(),
                        ok: false,
                        pending: true,
                        approval_id: Some(approval_id),
                        ..Default::default()
                    },
                    "ssh.request",
                    "pending",
                )
            }
            // 3) allowed, no approval → execute now
            Some(a) => {
                let tr = run_ssh_on(&st, target, &a.account_id, &req.command).await;
                let res = if tr.ok { "ok" } else { "fail" };
                (tr, "ssh.exec", res)
            }
        };

        let _ = st
            .store
            .insert_audit(
                &Uuid::new_v4().to_string(),
                now_secs(),
                &user.user_id,
                &user.email,
                audit_action,
                target,
                &req.command,
                audit_result,
                &job_id,
            )
            .await;
        let name = asset_display_name(&st, target).await;
        record_job_target(&st, &job_id, target, name, &tr).await;
        results.push(tr);
    }
    finalize_job(&st, &job_id, &user.user_id, &req.command).await;

    Ok(Json(JobResult { job_id, results }))
}

// ---- SQL job (authorized via rules; sqlite targets) ----

/// Resolve asset and run the query (never pending). Shared by direct execution
/// and approval release. `asset.host` is the sqlite file path.
async fn run_sql_on(st: &AppState, target: &str, account_id: &str, query: &str) -> TargetResult {
    let started = std::time::Instant::now();
    let mut tr = run_sql_on_inner(st, target, account_id, query).await;
    tr.duration_ms = started.elapsed().as_millis() as i64;
    tr
}

async fn run_sql_on_inner(st: &AppState, target: &str, _account_id: &str, query: &str) -> TargetResult {
    match st.store.get_asset(target).await.ok().flatten() {
        Some(a) => match sql::run_query(&a.host, query).await {
            Ok(o) => TargetResult {
                target: target.to_string(),
                ok: o.ok,
                exit_code: o.affected.map(|x| x as i32),
                stdout: o.output,
                error: None,
                ..Default::default()
            },
            Err(e) => TargetResult {
                target: target.to_string(),
                ok: false,
                error: Some(e.to_string()),
                ..Default::default()
            },
        },
        None => TargetResult {
            target: target.to_string(),
            ok: false,
            error: Some("资产缺失".into()),
            ..Default::default()
        },
    }
}

pub async fn submit_sql(
    user: AuthUser,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    State(st): State<AppState>,
    Json(req): Json<SubmitSqlJob>,
) -> Result<Json<JobResult>, AppError> {
    let job_id = Uuid::new_v4().to_string();
    let mut results = Vec::new();
    let (template_id, template_name) = resolve_template(&st, &req.template_id).await;
    let _ = st.store.create_job(&crate::store::JobRow {
        id: job_id.clone(),
        kind: "sql".into(),
        command: req.query.clone(),
        operator_id: user.user_id.clone(),
        operator_email: user.email.clone(),
        created_at: now_secs(),
        finished_at: None,
        status: "pending".into(),
        total: req.targets.len() as i64,
        ok_count: 0,
        source_ip: ip,
        source_device: user.device_id.clone(),
        template_id,
        template_name,
    }).await;

    for target in &req.targets {
        let authz = rbac::authorize(&st.store, &user.user_id, is_admin(&user), target, "sql").await;
        let (tr, audit_action, audit_result) = match authz {
            None => (
                TargetResult {
                    target: target.clone(),
                    ok: false,
                    error: Some("未授权(无匹配规则或无账号)".into()),
                    ..Default::default()
                },
                "sql.exec",
                "fail",
            ),
            Some(a) if a.needs_approval => {
                let approval_id = Uuid::new_v4().to_string();
                let _ = st
                    .store
                    .create_approval(&crate::store::ApprovalRow {
                        id: approval_id.clone(),
                        job_id: job_id.clone(),
                        requester_id: user.user_id.clone(),
                        requester_email: user.email.clone(),
                        asset_id: target.clone(),
                        account_id: a.account_id,
                        action: "sql".into(),
                        command: req.query.clone(),
                        state: "pending".into(),
                        reason: None,
                        decided_by: None,
                        created_at: now_secs(),
                        decided_at: None,
                        min_approvals: a.min_approvals,
                        approver_ids: a.approver_ids,
                        quick: a.quick,
                    })
                    .await;
                if let Ok(admins) = st.store.admin_ids().await {
                    for aid in admins {
                        let _ = st.store.push_notification(&aid, "approval", "有新的待审批",
                            &format!("{} 请求 SQL:{}", user.email, req.query), "/approvals", now_secs()).await;
                    }
                }
                (
                    TargetResult {
                        target: target.clone(),
                        ok: false,
                        pending: true,
                        approval_id: Some(approval_id),
                        ..Default::default()
                    },
                    "sql.request",
                    "pending",
                )
            }
            Some(a) => {
                let tr = run_sql_on(&st, target, &a.account_id, &req.query).await;
                let res = if tr.ok { "ok" } else { "fail" };
                (tr, "sql.exec", res)
            }
        };

        let _ = st
            .store
            .insert_audit(
                &Uuid::new_v4().to_string(),
                now_secs(),
                &user.user_id,
                &user.email,
                audit_action,
                target,
                &req.query,
                audit_result,
                &job_id,
            )
            .await;
        let name = asset_display_name(&st, target).await;
        record_job_target(&st, &job_id, target, name, &tr).await;
        results.push(tr);
    }
    finalize_job(&st, &job_id, &user.user_id, &req.query).await;

    Ok(Json(JobResult { job_id, results }))
}

// ---- approvals (admin审批确认) ----

/// Build the join-enriched view for one approval row.
async fn approval_view(st: &AppState, a: crate::store::ApprovalRow) -> ApprovalView {
    let target = st.store.get_asset(&a.asset_id).await.ok().flatten();
    let target_name = target.as_ref().map(|x| x.name.clone()).unwrap_or_else(|| a.asset_id.clone());
    let mut env = target.as_ref().map(|x| x.env.clone()).unwrap_or_default();
    if env.is_empty() {
        if let Some(pid) = target.as_ref().and_then(|x| x.parent_id.clone()) {
            env = st.store.get_asset(&pid).await.ok().flatten().map(|s| s.env).unwrap_or_default();
        }
    }
    let account_name = st.store.get_system_user(&a.account_id).await.ok().flatten()
        .map(|x| x.name).unwrap_or_else(|| a.account_id.clone());
    let approve_votes = st.store.count_votes(&a.id, "approve").await.unwrap_or(0);
    let mut approvers = Vec::new();
    for uid in a.approver_ids.split(',').filter(|s| !s.is_empty()) {
        let name = st.store.get_user_by_id(uid).await.ok().flatten().map(|u| u.name).unwrap_or_else(|| uid.to_string());
        approvers.push(name);
    }
    ApprovalView {
        id: a.id,
        job_id: a.job_id,
        requester_email: a.requester_email,
        target_id: a.asset_id,
        target_name,
        account_name,
        action: a.action,
        command: a.command,
        state: a.state,
        reason: a.reason,
        decided_by: a.decided_by,
        created_at: a.created_at,
        decided_at: a.decided_at,
        min_approvals: a.min_approvals.max(1),
        approve_votes,
        approvers,
        env,
        quick: if a.quick.is_empty() { "console".into() } else { a.quick },
    }
}

pub async fn list_approvals(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<ApprovalView>>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let rows = st.store.list_approvals(100).await.map_err(AppError::Internal)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(approval_view(&st, r).await);
    }
    Ok(Json(out))
}

/// Core decide logic shared by single + batch endpoints.
async fn decide_one(
    st: &AppState,
    user: &AuthUser,
    id: &str,
    verdict: &str,
    reason: Option<&str>,
) -> Result<serde_json::Value, AppError> {
    let ap = st.store.get_approval(id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("审批不存在".into()))?;
    if ap.state != "pending" {
        return Err(AppError::BadRequest("该审批已决策".into()));
    }
    // Designated-approver rules: only listed users may decide (empty = any admin).
    let designated: Vec<&str> = ap.approver_ids.split(',').filter(|s| !s.is_empty()).collect();
    if !designated.is_empty() && !designated.contains(&user.user_id.as_str()) {
        return Err(AppError::Forbidden);
    }
    let need = ap.min_approvals.max(1);
    match verdict {
        "approve" => {
            // Record this approver's vote (idempotent per approver).
            st.store.add_vote(id, &user.user_id, &user.email, "approve", None, now_secs())
                .await.map_err(AppError::Internal)?;
            let votes = st.store.count_votes(id, "approve").await.map_err(AppError::Internal)?;

            // Quorum not yet reached → stay pending.
            if votes < need {
                return Ok(json!({ "state": "pending", "votes": votes, "need": need }));
            }

            // Threshold reached → execute the held command, dispatched by action.
            let tr = if ap.action == "sql" {
                run_sql_on(st, &ap.asset_id, &ap.account_id, &ap.command).await
            } else {
                run_ssh_on(st, &ap.asset_id, &ap.account_id, &ap.command).await
            };
            let exec_action = if ap.action == "sql" { "sql.exec" } else { "ssh.exec" };
            st.store
                .decide_approval(id, "approved", &user.email, None, now_secs())
                .await
                .map_err(AppError::Internal)?;
            let _ = st.store.insert_audit(
                &Uuid::new_v4().to_string(), now_secs(), &user.user_id, &user.email,
                exec_action, &ap.asset_id, &ap.command, if tr.ok { "ok" } else { "fail" },
                &ap.job_id,
            ).await;
            // released target: fill in its held job_target row, then re-aggregate
            let _ = st.store.update_job_target_result(
                id, if tr.ok { "ok" } else { "fail" }, tr.exit_code.map(|c| c as i64),
                &tr.stdout, &tr.stderr, tr.error.as_deref(), tr.duration_ms,
            ).await;
            finalize_job(st, &ap.job_id, &ap.requester_id, &ap.command).await;
            // notify the requester that their request was approved
            let _ = st.store.push_notification(&ap.requester_id, "approval",
                "审批已放行", &format!("{}: {}", ap.action, ap.command),
                &format!("/record/{}", ap.job_id), now_secs()).await;
            Ok(json!({ "state": "approved", "votes": votes, "need": need, "result": {
                "target": tr.target, "ok": tr.ok, "exit_code": tr.exit_code,
                "stdout": tr.stdout, "stderr": tr.stderr, "error": tr.error,
            } }))
        }
        "reject" => {
            let reason = reason.unwrap_or_default().trim().to_string();
            if reason.is_empty() {
                return Err(AppError::BadRequest("驳回须填理由".into()));
            }
            // A single reject vetoes the request (record the vote for the trail).
            let _ = st.store.add_vote(id, &user.user_id, &user.email, "reject", Some(&reason), now_secs()).await;
            st.store
                .decide_approval(id, "rejected", &user.email, Some(&reason), now_secs())
                .await
                .map_err(AppError::Internal)?;
            let reject_action = if ap.action == "sql" { "sql.reject" } else { "ssh.reject" };
            let _ = st.store.insert_audit(
                &Uuid::new_v4().to_string(), now_secs(), &user.user_id, &user.email,
                reject_action, &ap.asset_id, &reason, "rejected",
                &ap.job_id,
            ).await;
            let _ = st.store.update_job_target_result(
                id, "rejected", None, "", "", Some(&reason), 0,
            ).await;
            finalize_job(st, &ap.job_id, &ap.requester_id, &ap.command).await;
            let _ = st.store.push_notification(&ap.requester_id, "approval",
                "审批被驳回", &format!("理由:{reason}"),
                &format!("/record/{}", ap.job_id), now_secs()).await;
            Ok(json!({ "state": "rejected" }))
        }
        _ => Err(AppError::BadRequest("verdict 必须是 approve 或 reject".into())),
    }
}

pub async fn decide_approval(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<DecideRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let v = decide_one(&st, &user, &id, &req.verdict, req.reason.as_deref()).await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct DecideBatch {
    pub ids: Vec<String>,
    pub verdict: String,
    #[serde(default)]
    pub reason: Option<String>,
}

pub async fn decide_batch(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<DecideBatch>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    if req.verdict == "reject" && req.reason.as_deref().unwrap_or_default().trim().is_empty() {
        return Err(AppError::BadRequest("批量驳回须填理由".into()));
    }
    // Classify outcomes so the UI can distinguish "released" from "voted, quorum
    // not yet reached" (both are successful calls).
    let mut approved = 0u32;
    let mut pending = 0u32;
    let mut rejected = 0u32;
    let mut failed = 0u32;
    for id in &req.ids {
        match decide_one(&st, &user, id, &req.verdict, req.reason.as_deref()).await {
            Ok(v) => match v.get("state").and_then(|s| s.as_str()) {
                Some("approved") => approved += 1,
                Some("pending") => pending += 1,
                Some("rejected") => rejected += 1,
                _ => failed += 1,
            },
            Err(_) => failed += 1,
        }
    }
    Ok(Json(json!({
        "ok": approved + pending + rejected,
        "approved": approved, "pending": pending, "rejected": rejected, "failed": failed,
    })))
}

// ---- job history (执行记录:人人可见,非 admin 只见自己) ----

/// `GET /jobs` — aggregated execution history.
pub async fn list_jobs(
    user: AuthUser,
    State(st): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let g = |k: &str| q.get(k).cloned().unwrap_or_default();
    // Non-admins only ever see their own jobs, whatever they ask for.
    let operator = if is_admin(&user) { g("operator") } else { user.user_id.clone() };
    let from_ts: i64 = g("from_ts").parse().unwrap_or(0);
    let rows = st.store
        .list_jobs_filtered(&operator, &g("status"), &g("kind"), &g("q"), from_ts, 200)
        .await
        .map_err(AppError::Internal)?;
    let out: Vec<serde_json::Value> = rows.into_iter().map(|j| {
        let duration_ms = j.finished_at.map(|f| (f - j.created_at) * 1000);
        let mut v = serde_json::to_value(&j).unwrap_or_default();
        if let Some(obj) = v.as_object_mut() {
            obj.insert("duration_ms".into(), json!(duration_ms));
        }
        v
    }).collect();
    Ok(Json(json!(out)))
}

/// `GET /jobs/{id}` — one job with per-target outcomes and approval trail.
pub async fn get_job_detail(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let job = st.store.get_job(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("记录不存在".into()))?;
    if !is_admin(&user) && job.operator_id != user.user_id {
        return Err(AppError::Forbidden);
    }
    let targets = st.store.list_job_targets(&id).await.map_err(AppError::Internal)?;
    let approvals = st.store.list_approvals_for_job(&id).await.map_err(AppError::Internal)?;
    let mut ap_out = Vec::with_capacity(approvals.len());
    for a in approvals {
        let votes = st.store.list_votes(&a.id).await.unwrap_or_default();
        let target_name = asset_display_name(&st, &a.asset_id).await;
        let mut v = serde_json::to_value(&a).unwrap_or_default();
        if let Some(obj) = v.as_object_mut() {
            obj.insert("votes".into(), serde_json::to_value(&votes).unwrap_or_default());
            obj.insert("target_name".into(), json!(target_name));
        }
        ap_out.push(v);
    }
    Ok(Json(json!({ "job": job, "targets": targets, "approvals": ap_out })))
}

/// `GET /assets/{id}/history` — one node's execution history (non-admin: own only).
pub async fn node_history(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let operator = if is_admin(&user) { String::new() } else { user.user_id.clone() };
    let rows = st.store.node_history(&id, &operator, 50).await.map_err(AppError::Internal)?;
    Ok(Json(json!(rows)))
}

/// 测试连通:server 节点做 TCP 连接探测(带延迟),database 节点检查文件可访问。
/// 不涉及账密,只验证网络/路径可达性。
#[derive(Deserialize)]
pub struct ProbeReq {
    pub kind: String, // server | database
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: i64,
}

pub async fn probe_asset(
    user: AuthUser,
    State(_st): State<AppState>,
    Json(req): Json<ProbeReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(AppError::BadRequest("请先填写主机 / 地址".into()));
    }
    let started = std::time::Instant::now();
    if req.kind == "database" {
        // sqlite 节点:host 是文件路径,检查是否可访问
        match tokio::fs::metadata(&host).await {
            Ok(m) if m.is_file() => Ok(Json(json!({
                "ok": true, "latency_ms": started.elapsed().as_millis() as i64,
                "message": format!("可连通 · DB 文件可访问({} 字节)", m.len())
            }))),
            _ => Ok(Json(json!({
                "ok": false, "latency_ms": started.elapsed().as_millis() as i64,
                "message": "无法访问 DB 文件路径"
            }))),
        }
    } else {
        // server 节点:TCP 连接探测(默认 22)
        let port = if req.port > 0 { req.port as u16 } else { 22 };
        let addr = format!("{}:{}", host, port);
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(_)) => Ok(Json(json!({
                "ok": true, "latency_ms": started.elapsed().as_millis() as i64,
                "message": format!("可连通 · TCP {} 握手成功", addr)
            }))),
            Ok(Err(e)) => Ok(Json(json!({
                "ok": false, "latency_ms": started.elapsed().as_millis() as i64,
                "message": format!("连接失败:{}", e)
            }))),
            Err(_) => Ok(Json(json!({
                "ok": false, "latency_ms": started.elapsed().as_millis() as i64,
                "message": "连接超时(3s)"
            }))),
        }
    }
}

// ---- vault (凭据金库:加密/解封) ----

pub async fn vault_status(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    Ok(Json(json!({ "sealed": st.vault.is_sealed() })))
}

#[derive(Deserialize)]
pub struct UnsealReq { pub passphrase: String }

pub async fn vault_unseal(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<UnsealReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    st.vault.unseal(&req.passphrase, &st.store).await
        .map_err(|e| AppError::BadRequest(format!("解封失败:{e}")))?;
    let migrated = st.vault.migrate_plaintext(&st.store).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "sealed": false, "migrated": migrated })))
}

pub async fn vault_seal(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    st.vault.seal();
    Ok(Json(json!({ "sealed": true })))
}

// ---- audit export ----

pub async fn audit_export(
    user: AuthUser,
    State(st): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let g = |k: &str| q.get(k).cloned().unwrap_or_default();
    let rows = st.store
        .list_audit_filtered(&g("action"), &g("result"), &g("operator"), &g("q"), 5000)
        .await
        .map_err(AppError::Internal)?;

    let format = q.get("format").map(|s| s.as_str()).unwrap_or("csv");
    if format == "json" {
        let body = serde_json::to_string(&rows).unwrap_or_default();
        return Ok((
            [("content-type", "application/json"),
             ("content-disposition", "attachment; filename=\"audit.json\"")],
            body,
        ).into_response());
    }
    // CSV
    let mut out = String::from("ts,operator_email,action,targets,payload,result\n");
    let esc = |s: &str| {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    for r in &rows {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            r.ts, esc(&r.operator_email), esc(&r.action), esc(&r.targets), esc(&r.payload), esc(&r.result)
        ));
    }
    Ok((
        [("content-type", "text/csv; charset=utf-8"),
         ("content-disposition", "attachment; filename=\"audit.csv\"")],
        out,
    ).into_response())
}

// ---- login/register flags (P6) ----

/// Public: the login page needs to know if registration is open.
pub async fn get_flags(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let reg = st.store.get_setting("register_open").await.ok().flatten().as_deref() == Some("1");
    let otp = st.store.get_setting("otp_enabled").await.ok().flatten().as_deref() == Some("1");
    Ok(Json(json!({ "register_open": reg, "otp_enabled": otp })))
}

#[derive(Deserialize)]
pub struct Flags { pub register_open: bool, pub otp_enabled: bool }

pub async fn put_flags(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<Flags>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    st.store.set_setting("register_open", if req.register_open { "1" } else { "0" }).await.map_err(AppError::Internal)?;
    st.store.set_setting("otp_enabled", if req.otp_enabled { "1" } else { "0" }).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- messages / notifications (P5 消息中心) ----

pub async fn list_messages(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<crate::store::NotificationRow>>, AppError> {
    Ok(Json(st.store.list_notifications(&user.user_id, 100).await.map_err(AppError::Internal)?))
}

pub async fn unread_count(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let n = st.store.count_unread(&user.user_id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "count": n })))
}

pub async fn mark_message_read(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    st.store.mark_read(&id, &user.user_id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn mark_message_unread(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    st.store.mark_unread(&id, &user.user_id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn mark_all_messages_read(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    st.store.mark_all_read(&user.user_id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_message(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    st.store.delete_notification(&id, &user.user_id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- templates (P3 执行模板) ----

/// Render a template as a unified Markdown file `templates/<name>.md`. Metadata
/// (incl. `kind`, the execution-type marker) lives in YAML frontmatter; `body`
/// is the command — passed encrypted for git storage, or plaintext for viewing.
pub fn template_md(t: &crate::store::TemplateRow, body: &str) -> (String, String) {
    let slug = t.name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    let path = format!("templates/{slug}.md");
    let vars = serde_json::from_str::<serde_json::Value>(&t.variables).ok()
        .and_then(|v| v.as_array().map(|a| a.iter().map(|x| format!(
            "{}={}", x.get("name").and_then(|n| n.as_str()).unwrap_or(""),
            x.get("default").and_then(|d| d.as_str()).unwrap_or(""))).collect::<Vec<_>>().join("; ")))
        .unwrap_or_default();
    let content = format!(
        "---\nopsctl-id: {}\nname: {}\nkind: {}\nvars: {}\napprovers: {}\nencrypted: {}\n---\n{}\n",
        t.id, t.name, t.kind, vars, t.approver_ids, body.starts_with("v1:"), body);
    (path, content)
}

/// GET /templates/{id}/file — readable rendered .md for viewing (plaintext body);
/// git stores it with the body encrypted.
pub async fn template_file_view(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let t = st.store.get_template(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("模板不存在".into()))?;
    let (path, content) = template_md(&t, &t.command);
    let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
    let mut out = json!({ "filename": filename, "path": path, "content": content, "encrypted_in_git": false });
    // On-disk location is only shown to admins (filesystem layout is sensitive).
    if is_admin(&user) {
        let (abs_path, exists) = git_file_location(&st, &path).await;
        out["abs_path"] = json!(abs_path);
        out["exists"] = json!(exists);
    }
    Ok(Json(out))
}

/// GET /assets/{id}/file — readable view of an SSH node's git file (secrets
/// masked); git stores the body encrypted. Admin only (SSH config is sensitive).
pub async fn asset_file_view(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let a = st.store.get_asset(&id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("节点不存在".into()))?;
    let site = match a.parent_id.as_deref() {
        Some(p) => st.store.get_asset(p).await.ok().flatten().map(|x| x.name).unwrap_or_default(),
        None => String::new(),
    };
    let mut accounts = Vec::new();
    if let Ok(ids) = st.store.accounts_of_asset(&id).await {
        for aid in ids {
            if let Ok(Some(su)) = st.store.get_system_user(&aid).await {
                accounts.push(json!({ "name": su.name, "username": su.username, "kind": su.kind, "secret": "***" }));
            }
        }
    }
    let payload = serde_json::to_string_pretty(&json!({ "host": a.host, "port": a.port, "accounts": accounts })).unwrap_or_default();
    let slug = a.name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    let content = format!("---\nopsctl-node: {}\nname: {}\nsite: {}\nencrypted: true\n---\n{}\n", a.id, a.name, site, payload);
    let path = format!("ssh/{slug}.md");
    let (abs_path, exists) = git_file_location(&st, &path).await;
    Ok(Json(json!({ "filename": format!("{slug}.md"), "path": path, "content": content,
        "encrypted_in_git": true, "abs_path": abs_path, "exists": exists })))
}

pub async fn list_templates(
    _user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<crate::store::TemplateRow>>, AppError> {
    Ok(Json(st.store.list_templates().await.map_err(AppError::Internal)?))
}

#[derive(Deserialize)]
pub struct SaveTemplate {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default = "def_tpl_kind")]
    pub kind: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub variables: serde_json::Value, // [{name,default}]
    #[serde(default)]
    pub approver_ids: Vec<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub sort: i64,
}
fn def_tpl_kind() -> String { "ssh".into() }

pub async fn save_template(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<SaveTemplate>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("模板名不能为空".into()));
    }
    let id = req.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let created_at = st.store.get_template(&id).await.ok().flatten().map(|t| t.created_at).unwrap_or_else(now_secs);
    let variables = if req.variables.is_array() { req.variables.to_string() } else { "[]".into() };
    st.store
        .upsert_template(&crate::store::TemplateRow {
            id: id.clone(),
            name: req.name,
            kind: req.kind,
            command: req.command,
            variables,
            approver_ids: req.approver_ids.join(","),
            created_at,
            parent_id: req.parent_id,
            sort: req.sort,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_template(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    st.store.delete_template(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

// ---- profile / sessions / settings (P3 设置) ----

pub async fn get_profile(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let u = st.store.get_user_by_id(&user.user_id).await.map_err(AppError::Internal)?
        .ok_or(AppError::Unauthorized)?;
    Ok(Json(json!({
        "name": u.name, "email": u.email, "role": u.role,
        "login_ttl_secs": u.login_ttl_secs, "login_alert": u.login_alert != 0,
        "telegram_bound": u.telegram_chat_id.is_some(),
        "totp_enabled": !u.totp_secret.is_empty(),
    })))
}

// ---- per-user TOTP 2FA enrollment ----

pub async fn totp_start(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let secret = crate::totp::gen_secret();
    // stash the unconfirmed secret (plaintext, short-lived) keyed by user
    st.store.set_setting(&format!("totp_enroll:{}", user.user_id), &secret)
        .await.map_err(AppError::Internal)?;
    let uri = crate::totp::provisioning_uri(&secret, &user.email, "opsctl");
    Ok(Json(json!({ "secret": secret, "otpauth_uri": uri })))
}

#[derive(Deserialize)]
pub struct TotpConfirm { pub code: String }

pub async fn totp_confirm(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<TotpConfirm>,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = format!("totp_enroll:{}", user.user_id);
    let secret = st.store.get_setting(&key).await.map_err(AppError::Internal)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("请先开始绑定".into()))?;
    if !crate::totp::verify(&secret, &req.code, now_secs()) {
        return Err(AppError::BadRequest("验证码错误".into()));
    }
    if st.vault.is_sealed() {
        return Err(AppError::Sealed);
    }
    let enc = st.vault.encrypt(&secret).map_err(AppError::Internal)?;
    st.store.set_totp_secret(&user.user_id, &enc).await.map_err(AppError::Internal)?;
    let _ = st.store.set_setting(&key, "").await;
    Ok(Json(json!({ "ok": true, "totp_enabled": true })))
}

pub async fn totp_disable(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    st.store.set_totp_secret(&user.user_id, "").await.map_err(AppError::Internal)?;
    let _ = st.store.set_setting(&format!("totp_enroll:{}", user.user_id), "").await;
    Ok(Json(json!({ "ok": true, "totp_enabled": false })))
}

#[derive(Deserialize)]
pub struct UpdateProfile {
    pub name: String,
    #[serde(default)]
    pub email: String,
    pub login_ttl_secs: i64,
    #[serde(default)]
    pub login_alert: bool,
}

pub async fn update_profile(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<UpdateProfile>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("显示名不能为空".into()));
    }
    let ttl = req.login_ttl_secs.clamp(60, opsctl_core::model::MAX_LOGIN_TTL_SECS);
    st.store
        .update_profile(&user.user_id, req.name.trim(), req.email.trim(), ttl, req.login_alert as i64)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true, "login_ttl_secs": ttl })))
}

pub async fn list_sessions(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows = st.store.list_sessions(&user.user_id).await.map_err(AppError::Internal)?;
    let out: Vec<_> = rows.into_iter().map(|s| json!({
        "sid": s.sid, "device_id": s.device_id, "ip": s.ip,
        "created_at": s.created_at, "last_seen": s.last_seen,
        "current": s.sid == user.sid,
    })).collect();
    Ok(Json(json!(out)))
}

pub async fn revoke_session(
    user: AuthUser,
    State(st): State<AppState>,
    Path(sid): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let n = st.store.revoke_session(&sid, &user.user_id).await.map_err(AppError::Internal)?;
    if n == 0 {
        return Err(AppError::BadRequest("会话不存在或不属于你".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---- Telegram binding (演示态:未接真实 bot) ----

pub async fn telegram_bind_start(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 6-char code derived from a uuid; store pending code -> user id.
    let code = Uuid::new_v4().simple().to_string()[..6].to_uppercase();
    let _ = st.store.set_setting(&format!("tg_pending:{code}"), &user.user_id).await;
    Ok(Json(json!({ "code": code, "bot": "@opsctl_bot",
        "note": "演示:未接真实 bot,点「我已发送」即完成绑定" })))
}

#[derive(Deserialize)]
pub struct TgConfirm { pub code: String }

pub async fn telegram_bind_confirm(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<TgConfirm>,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = format!("tg_pending:{}", req.code.trim().to_uppercase());
    match st.store.get_setting(&key).await.map_err(AppError::Internal)? {
        Some(uid) if uid == user.user_id => {
            st.store.set_telegram(&user.user_id, Some(&format!("tg:{}", req.code.trim())))
                .await.map_err(AppError::Internal)?;
            let _ = st.store.set_setting(&key, "").await; // clear pending
            Ok(Json(json!({ "ok": true })))
        }
        _ => Err(AppError::BadRequest("绑定码无效".into())),
    }
}

pub async fn telegram_unbind(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    st.store.set_telegram(&user.user_id, None).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- Git sync config (admin; 演示态:未接真实 git) ----

async fn git_config_json(st: &AppState) -> serde_json::Value {
    st.store.get_setting("git_config").await.ok().flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| json!({ "mode": "folder", "url": "", "branch": "main",
            "username": "", "credential": "", "auto_push": false, "work_dir": "" }))
}

fn git_work_dir(cfg: &serde_json::Value) -> std::path::PathBuf {
    let wd = cfg.get("work_dir").and_then(|x| x.as_str()).unwrap_or("");
    if wd.is_empty() { std::path::PathBuf::from("data/opsctl-config") } else { std::path::PathBuf::from(wd) }
}

/// Resolve a repo-relative path to its on-disk location inside `work_dir`.
/// Rejects absolute paths and any non-plain component (`..`, drive prefixes)
/// so a request can never escape the work_dir.
fn resolve_in_work_dir(
    work_dir: &std::path::Path,
    rel: &str,
) -> Result<std::path::PathBuf, AppError> {
    use std::path::Component;
    let rel_path = std::path::Path::new(rel);
    // `\` 与 `:` 在 Unix 上是合法文件名字符,但这里的路径要跨平台安全:
    // Windows 下它们是分隔符/盘符,统一拒绝,两个平台行为才一致。
    if rel_path.as_os_str().is_empty()
        || rel_path.is_absolute()
        || rel.contains(['\\', ':'])
        || rel_path.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(AppError::BadRequest("非法路径".into()));
    }
    let joined = work_dir.join(rel_path);
    Ok(std::path::absolute(&joined).unwrap_or(joined))
}

/// (absolute display path, exists on disk) for a repo-relative git file.
async fn git_file_location(st: &AppState, rel: &str) -> (Option<String>, bool) {
    let cfg = git_config_json(st).await;
    let work_dir = git_work_dir(&cfg);
    match resolve_in_work_dir(&work_dir, rel) {
        Ok(p) => {
            let exists = tokio::fs::try_exists(&p).await.unwrap_or(false);
            (Some(p.display().to_string()), exists)
        }
        Err(_) => (None, false),
    }
}

/// GET /settings/git → config (credential redacted) + git install/status.
pub async fn get_git(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let mut cfg = git_config_json(&st).await;
    let has_cred = cfg.get("credential").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("credential".into(), json!("")); // never echo the secret back
        obj.insert("credential_set".into(), json!(has_cred));
    }
    let version = crate::git::version().await;
    let work_dir = git_work_dir(&cfg);
    let last_commit = crate::git::last_commit(&work_dir).await;
    let work_dir_abs = std::path::absolute(&work_dir).unwrap_or(work_dir);
    Ok(Json(json!({
        "config": cfg,
        "git_installed": version.is_some(),
        "git_version": version,
        "last_commit": last_commit,
        "work_dir_abs": work_dir_abs.display().to_string(),
    })))
}

pub async fn put_git(
    user: AuthUser,
    State(st): State<AppState>,
    Json(mut cfg): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    // empty credential in the request → keep the stored one (write-only field)
    let incoming_cred = cfg.get("credential").and_then(|x| x.as_str()).unwrap_or("");
    if incoming_cred.is_empty() {
        let old = git_config_json(&st).await;
        let kept = old.get("credential").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("credential".into(), json!(kept));
        }
    }
    st.store.set_setting("git_config", &cfg.to_string()).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct GitRevealReq {
    /// Repo-relative file to select; empty → open the work_dir itself.
    #[serde(default)]
    pub path: String,
}

/// POST /settings/git/reveal — open the git work_dir (or select a file inside
/// it) in the OS file manager **on the machine running the server**. Intended
/// for local single-binary deployments; admin only.
pub async fn git_reveal(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<GitRevealReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let cfg = git_config_json(&st).await;
    let work_dir = git_work_dir(&cfg);
    let (target, is_dir) = if req.path.is_empty() {
        (std::path::absolute(&work_dir).unwrap_or(work_dir), true)
    } else {
        (resolve_in_work_dir(&work_dir, &req.path)?, false)
    };
    if !tokio::fs::try_exists(&target).await.unwrap_or(false) {
        return Err(AppError::BadRequest(
            "文件尚未生成:请先在「设置 → Git 同步」执行一次同步".into(),
        ));
    }
    open_in_file_manager(&target, is_dir)
        .map_err(|e| AppError::BadRequest(format!("打开文件管理器失败:{e}")))?;
    Ok(Json(json!({ "ok": true, "path": target.display().to_string() })))
}

/// Open the platform file manager at `target` (selecting it when it's a file).
/// Fire-and-forget: explorer/open/xdg-open exit codes are unreliable.
fn open_in_file_manager(target: &std::path::Path, is_dir: bool) -> std::io::Result<()> {
    let path = target.display().to_string();
    if cfg!(target_os = "windows") {
        let arg = if is_dir { path } else { format!("/select,{path}") };
        std::process::Command::new("explorer").arg(arg).spawn()?;
    } else if cfg!(target_os = "macos") {
        let mut cmd = std::process::Command::new("open");
        if is_dir { cmd.arg(&path) } else { cmd.arg("-R").arg(&path) };
        cmd.spawn()?;
    } else {
        let dir = if is_dir {
            target.to_path_buf()
        } else {
            target.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| target.to_path_buf())
        };
        std::process::Command::new("xdg-open").arg(dir).spawn()?;
    }
    Ok(())
}

/// Serialize the exportable config to (filename, json) pairs for git.
async fn export_config(st: &AppState) -> Result<Vec<(String, String)>, AppError> {
    let store = &st.store;
    let users = store.list_users().await.map_err(AppError::Internal)?;
    let users_json: Vec<_> = users.iter()
        .map(|u| json!({ "id": u.id, "name": u.name, "email": u.email, "role": u.role,
            "totp_enabled": !u.totp_secret.is_empty() }))
        .collect();
    let pretty = |v: serde_json::Value| serde_json::to_string_pretty(&v).unwrap_or_default();
    let mut files: Vec<(String, String)> = vec![
        ("users.json".into(), pretty(json!(users_json))),
        ("rules.json".into(), pretty(serde_json::to_value(store.list_rules().await.map_err(AppError::Internal)?).unwrap_or_default())),
        ("assets.json".into(), pretty(serde_json::to_value(store.list_assets().await.map_err(AppError::Internal)?).unwrap_or_default())),
        ("tags.json".into(), pretty(serde_json::to_value(store.list_tags().await.map_err(AppError::Internal)?).unwrap_or_default())),
        // accounts keep the v1: encrypted secret (safe in git); SystemUserRow skips it on serialize,
        // so include id/name/kind/username explicitly + the ciphertext.
        ("accounts.json".into(), pretty(json!(store.list_system_users().await.map_err(AppError::Internal)?
            .iter().map(|a| json!({ "id": a.id, "name": a.name, "kind": a.kind, "username": a.username, "secret_enc": a.secret })).collect::<Vec<_>>()))),
    ];
    // part 1 — SSH config: one ENCRYPTED file per server node (ssh/<name>.md)
    let assets = store.list_assets().await.map_err(AppError::Internal)?;
    let name_by_id: std::collections::HashMap<String, String> =
        assets.iter().map(|a| (a.id.clone(), a.name.clone())).collect();
    for a in assets.iter().filter(|a| a.kind == "server") {
        let site = a.parent_id.as_deref().and_then(|p| name_by_id.get(p)).cloned().unwrap_or_default();
        let (path, content) = ssh_node_file(st, a, &site).await;
        files.push((path, content));
    }

    // part 2 — templates: raw (unencrypted) files templates/<name>.md
    for t in store.list_templates().await.map_err(AppError::Internal)? {
        let (path, content) = template_md(&t, &t.command);
        files.push((path, content));
    }
    Ok(files)
}

/// One SSH node → `ssh/<name>.md`. Readable frontmatter (id/name/site) for
/// navigation; the body is the node's connection config + credentials, vault-
/// encrypted (deterministically, so re-sync doesn't churn). Sealed vault → the
/// payload falls back to plaintext with empty secrets.
pub async fn ssh_node_file(st: &AppState, a: &crate::store::AssetRow, site: &str) -> (String, String) {
    let mut accounts = Vec::new();
    if let Ok(ids) = st.store.accounts_of_asset(&a.id).await {
        for id in ids {
            if let Ok(Some(su)) = st.store.get_system_user(&id).await {
                let secret = st.vault.decrypt(&su.secret).unwrap_or_default();
                accounts.push(json!({ "name": su.name, "username": su.username, "kind": su.kind, "secret": secret }));
            }
        }
    }
    let payload = json!({ "host": a.host, "port": a.port, "accounts": accounts }).to_string();
    let body = if st.vault.is_sealed() {
        payload.clone()
    } else {
        st.vault.encrypt_stable(&payload).unwrap_or(payload)
    };
    let slug = a.name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    let content = format!(
        "---\nopsctl-node: {}\nname: {}\nsite: {}\nencrypted: {}\n---\n{}\n",
        a.id, a.name, site, body.starts_with("v1:"), body);
    (format!("ssh/{slug}.md"), content)
}

/// POST /settings/git/{what} — install | test | sync | push | pull (admin).
pub async fn git_action(
    user: AuthUser,
    State(st): State<AppState>,
    Path(what): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    if what == "install" {
        let msg = crate::git::install().await.map_err(|e| AppError::BadRequest(e.to_string()))?;
        let version = crate::git::version().await;
        return Ok(Json(json!({ "ok": version.is_some(), "note": msg, "git_version": version })));
    }

    let cfg_json = git_config_json(&st).await;
    let cfg = crate::git::GitCfg::from_json(&cfg_json);
    let work_dir = git_work_dir(&cfg_json);

    let result = match what.as_str() {
        "test" => crate::git::test(&cfg).await.map(|m| json!({ "ok": true, "note": m })),
        "sync" => {
            let files = export_config(&st).await?;
            crate::git::sync(&cfg, &work_dir, &files).await.map(|r| {
                json!({ "ok": true, "committed": r.committed, "commit": r.commit, "note": r.note })
            })
        }
        "push" => crate::git::push(&cfg, &work_dir).await.map(|m| json!({ "ok": true, "note": m })),
        "pull" => crate::git::pull(&cfg, &work_dir).await.map(|m| json!({ "ok": true, "note": m })),
        _ => return Err(AppError::BadRequest("未知操作".into())),
    };
    match result {
        Ok(v) => {
            if what == "sync" {
                let commit = v.get("commit").and_then(|c| c.as_str()).unwrap_or("");
                let _ = st.store.insert_audit(&Uuid::new_v4().to_string(), now_secs(),
                    &user.user_id, &user.email, "git.sync", "config", commit, "ok", "").await;
                let committed = v.get("committed").and_then(|c| c.as_bool()).unwrap_or(false);
                let body = if committed && !commit.is_empty() {
                    format!("提交 {} · by {}", &commit[..commit.len().min(8)], user.email)
                } else {
                    "配置无变更,已是最新".to_string()
                };
                let _ = st.store.push_notification(&user.user_id, "sync",
                    "配置已同步", &body, "/settings", now_secs()).await;
            }
            Ok(Json(v))
        }
        Err(e) => Err(AppError::BadRequest(e.to_string())),
    }
}

// ---- backup (local sqlite snapshots) ----

/// `GET /api/backup/status` — any authenticated user (the banner is shown on
/// the execution-history page which everyone can see).
pub async fn backup_status(
    _user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let last_at = crate::backup::last_backup_at(&st.store).await;
    let last_file = st.store.get_setting("backup_last_file").await.ok().flatten()
        .filter(|s| !s.is_empty());
    let count = crate::backup::snapshot_count(&st.backup.dir).await;
    Ok(Json(json!({
        "enabled": st.backup.enabled,
        "last_at": last_at,
        "last_file": last_file,
        "next_at": crate::backup::next_run_ts(),
        "retention_days": st.backup.retention_days,
        "count": count,
        "dir": crate::backup::dir_display(&st.backup.dir),
    })))
}

/// `POST /api/backup/run` — admin-only manual snapshot.
pub async fn backup_run(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !is_admin(&user) {
        return Err(AppError::Forbidden);
    }
    let (file, at) = crate::backup::run_backup(&st.store, &st.backup.dir)
        .await
        .map_err(AppError::Internal)?;
    crate::backup::cleanup(&st.backup.dir, st.backup.retention_days).await;
    let _ = st.store.insert_audit(
        &Uuid::new_v4().to_string(), now_secs(), &user.user_id, &user.email,
        "backup.run", &file, "", "ok", "",
    ).await;
    Ok(Json(json!({ "ok": true, "file": file, "at": at })))
}

#[cfg(test)]
mod tests {
    use super::resolve_in_work_dir;
    use std::path::Path;

    #[test]
    fn resolve_accepts_plain_relative_paths() {
        let wd = Path::new("data/opsctl-config");
        let p = resolve_in_work_dir(wd, "ssh/web-01.md").unwrap();
        assert!(p.ends_with(Path::new("data/opsctl-config/ssh/web-01.md")));
    }

    #[test]
    fn resolve_rejects_escape_attempts() {
        let wd = Path::new("data/opsctl-config");
        for bad in ["", "../secrets.txt", "ssh/../../x", "/etc/passwd", r"C:\Windows\win.ini", "C:evil"] {
            assert!(resolve_in_work_dir(wd, bad).is_err(), "should reject {bad:?}");
        }
    }
}
