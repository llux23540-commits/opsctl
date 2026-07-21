//! API DTOs exchanged between client and server (REST JSON).

use serde::{Deserialize, Serialize};

use crate::model::{Role, UserView};

/// `POST /login` request. `device_id` is the client machine-code hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub device_id: String,
}

/// `POST /login` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: UserView,
    /// Absolute expiry (unix seconds).
    pub expires_at: i64,
}

/// JWT claims (device-bound session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// user id
    pub sub: String,
    /// device id (machine-code hash)
    pub did: String,
    /// session id
    pub sid: String,
    pub role: Role,
    pub iat: i64,
    pub exp: i64,
}

/// `POST /jobs/ssh` request — v1 supports a single SSH command action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitSshJob {
    /// Target host entry ids (batch fan-out done server-side).
    pub targets: Vec<String>,
    pub command: String,
    /// Template the command was loaded from (provenance only; optional).
    #[serde(default)]
    pub template_id: Option<String>,
}

/// `POST /jobs/sql` request — run a query against database assets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitSqlJob {
    pub targets: Vec<String>,
    pub query: String,
    /// Template the query was loaded from (provenance only; optional).
    #[serde(default)]
    pub template_id: Option<String>,
}

/// Per-target execution result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetResult {
    pub target: String,
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    /// Held awaiting admin approval (needs_approval rule matched); not executed.
    #[serde(default)]
    pub pending: bool,
    /// Approval request id when `pending` is true.
    #[serde(default)]
    pub approval_id: Option<String>,
    /// Wall-clock execution time in milliseconds (0 when not executed).
    #[serde(default)]
    pub duration_ms: i64,
}

/// `POST /jobs` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub job_id: String,
    pub results: Vec<TargetResult>,
}

/// A pending/decided approval request (admin审批确认视图).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalView {
    pub id: String,
    pub job_id: String,
    pub requester_email: String,
    pub target_id: String,
    pub target_name: String,
    pub account_name: String,
    pub action: String,
    pub command: String,
    /// pending | approved | rejected
    pub state: String,
    pub reason: Option<String>,
    pub decided_by: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    /// 会签:需要的批准人数与当前已批准票数
    #[serde(default)]
    pub min_approvals: i64,
    #[serde(default)]
    pub approve_votes: i64,
    /// designated approver names (empty = any admin)
    #[serde(default)]
    pub approvers: Vec<String>,
    /// target node environment (prod|staging|dev), inherited from parent site
    #[serde(default)]
    pub env: String,
    /// review channel from the matched rule: "console" | "tg"
    #[serde(default)]
    pub quick: String,
}

/// `POST /approvals/:id/decide` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecideRequest {
    /// approve | reject
    pub verdict: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Simple error envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}
