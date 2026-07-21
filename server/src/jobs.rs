//! Job endpoints: RBAC-guarded, server-side brokered execution + audit.

use axum::extract::State;
use axum::Json;
use opsctl_core::api::{JobResult, SubmitSshJob, TargetResult};
use opsctl_core::model::{Action, Role};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::{now_secs, AppState};
use crate::ssh;
use crate::store::EntryRow;

/// `POST /jobs/ssh` — run one command on N target hosts (server-side).
pub async fn submit_ssh(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<SubmitSshJob>,
) -> Result<Json<JobResult>, AppError> {
    if !user.role.can(Action::ExecSsh) {
        return Err(AppError::Forbidden);
    }
    let job_id = Uuid::new_v4().to_string();
    let mut results = Vec::new();

    for target in &req.targets {
        let entry = st.store.get_entry(target).await.map_err(AppError::Internal)?;
        let tr = match entry {
            None => TargetResult {
                target: target.clone(),
                ok: false,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                error: Some("entry not found".into()),
                ..Default::default()
            },
            Some(e) => match ssh::run_command(
                &e.host,
                e.port as u16,
                &e.username,
                &e.secret,
                &req.command,
            )
            .await
            {
                Ok(out) => TargetResult {
                    target: target.clone(),
                    ok: out.exit_code == Some(0),
                    exit_code: out.exit_code,
                    stdout: out.stdout,
                    stderr: out.stderr,
                    error: None,
                    ..Default::default()
                },
                Err(err) => TargetResult {
                    target: target.clone(),
                    ok: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    error: Some(err.to_string()),
                    ..Default::default()
                },
            },
        };

        // Enforced per-target audit.
        let _ = st
            .store
            .insert_audit(
                &Uuid::new_v4().to_string(),
                now_secs(),
                &user.user_id,
                &user.email,
                "ssh.exec",
                target,
                &req.command,
                if tr.ok { "ok" } else { "fail" },
                &job_id,
            )
            .await;

        results.push(tr);
    }

    Ok(Json(JobResult { job_id, results }))
}

#[derive(Debug, Deserialize)]
pub struct CreateEntry {
    pub id: Option<String>,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: i64,
    pub username: String,
    /// v1: SSH password stored as-is (SOPS/vault wraps this later).
    pub secret: String,
}

fn default_port() -> i64 {
    22
}

/// `POST /entries` — register an SSH target (admin only, M1 convenience).
pub async fn create_entry(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<CreateEntry>,
) -> Result<Json<serde_json::Value>, AppError> {
    if user.role != Role::Admin {
        return Err(AppError::Forbidden);
    }
    let id = req.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    st.store
        .create_entry(&EntryRow {
            id: id.clone(),
            project: String::new(),
            name: req.name,
            kind: "db_server".into(),
            host: req.host,
            port: req.port,
            username: req.username,
            secret: req.secret,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}
