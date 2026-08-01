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

// ---- Nacos 管理 ----

/// Default Nacos config group when the caller doesn't set one.
pub fn default_nacos_group() -> String {
    "DEFAULT_GROUP".into()
}
fn default_nacos_type() -> String {
    "properties".into()
}

/// One config item to publish into a Nacos cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NacosConfigItem {
    pub data_id: String,
    #[serde(default = "default_nacos_group")]
    pub group: String,
    /// properties | yaml | json | text | xml | html (Nacos `type`).
    #[serde(default = "default_nacos_type", rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub content: String,
}

/// `POST /nacos/clusters/{id}/init` — 初始化配置.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NacosInitRequest {
    /// Config template to apply; its items are used when `items` is empty.
    #[serde(default)]
    pub template_id: Option<String>,
    /// Ad-hoc items (override the template's when present).
    #[serde(default)]
    pub items: Vec<NacosConfigItem>,
    /// `${name}` substitutions applied to every item's dataId/group/content.
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    /// Namespace (tenant) override; `None` = the cluster's configured one.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Overwrite an already-present dataId (default: leave it untouched).
    #[serde(default)]
    pub overwrite: bool,
    /// 强制覆盖模板的「原文下发」标记:`Some(false)` = 按原文,`Some(true)` = 做变量代入。
    /// 留空则跟随模板自身设置。
    #[serde(default)]
    pub substitute: Option<bool>,
    /// Resolve + diff only, publish nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// Per-item outcome of one init run. `status` is one of
/// `created` | `updated` | `skipped` | `fail`, prefixed with `would_` in a dry run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NacosItemResult {
    pub data_id: String,
    pub group: String,
    pub status: String,
    #[serde(default)]
    pub message: String,
}

/// `POST /nacos/clusters/{id}/init` response (also the stored run record).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NacosInitResult {
    pub run_id: String,
    pub cluster_id: String,
    pub cluster_name: String,
    pub namespace: String,
    /// ok | partial | fail
    pub status: String,
    pub total: i64,
    pub ok_count: i64,
    pub dry_run: bool,
    pub items: Vec<NacosItemResult>,
}

/// One member of a Nacos cluster, as reported by the cluster itself or probed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NacosNodeView {
    pub address: String,
    /// UP / DOWN / SUSPICIOUS (Nacos states) or `unreachable` when probed.
    pub state: String,
    #[serde(default)]
    pub version: String,
    pub ok: bool,
    pub latency_ms: i64,
    #[serde(default)]
    pub message: String,
}
