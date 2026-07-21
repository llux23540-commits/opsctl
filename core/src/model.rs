//! Core domain model shared across the platform.

use serde::{Deserialize, Serialize};

/// Built-in RBAC roles (v1). Pluggable Authorizer may extend later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    /// Whether this role may perform the given action (coarse v1 policy).
    pub fn can(self, action: Action) -> bool {
        use Action::*;
        use Role::*;
        match self {
            Admin => true,
            Operator => matches!(
                action,
                ViewConfig | ExecSql | ExecSsh | InstallK3s | SubmitJob
            ),
            Viewer => matches!(action, ViewConfig),
        }
    }
}

/// Auditable / authorizable actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ViewConfig,
    SubmitJob,
    ExecSql,
    ExecSsh,
    InstallK3s,
    AddNode,
    AddAccount,
    ManageRecipients,
}

/// Kinds of connection entry a project can hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Database,
    Api,
    DbServer,
}

/// A user as visible to clients (never carries the password hash).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserView {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: Role,
    pub telegram_bound: bool,
    /// Login token lifetime in seconds (capped at 30 days server-side).
    pub login_ttl_secs: i64,
}

/// Max login lifetime: 30 days.
pub const MAX_LOGIN_TTL_SECS: i64 = 30 * 24 * 3600;
