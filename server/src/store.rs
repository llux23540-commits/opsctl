//! SQLite-backed state store (users / sessions / audit / entries).
//!
//! Uses the sqlx runtime query API (no compile-time DB checks) so the build
//! needs no `DATABASE_URL`. PostgreSQL support is a later swap behind the same
//! interface.

use anyhow::Result;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{FromRow, SqlitePool};

#[derive(Clone)]
pub struct Store {
    pub pool: SqlitePool,
}

#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: String,
    pub pass_hash: String,
    pub telegram_chat_id: Option<String>,
    pub login_ttl_secs: i64,
    #[sqlx(default)]
    pub login_alert: i64,
    /// Confirmed TOTP secret (vault-encrypted base32); empty = 2FA off.
    #[sqlx(default)]
    pub totp_secret: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct SessionRow {
    pub sid: String,
    pub user_id: String,
    pub device_id: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub ip: String,
    pub revoked: i64,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct AuditRow {
    pub id: String,
    pub ts: i64,
    pub operator_id: String,
    pub operator_email: String,
    pub action: String,
    pub targets: String,
    pub payload: String,
    pub result: String,
    /// Owning job for exec rows; empty for non-job events (login, git.sync…).
    #[sqlx(default)]
    pub job_id: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct EntryRow {
    pub id: String,
    pub project: String,
    pub name: String,
    pub kind: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    /// v1: SSH password/secret in plaintext column; SOPS/vault wraps this later.
    pub secret: String,
}

// ---- JumpServer-lite model rows ----

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct AssetRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub parent_id: Option<String>,
    pub host: String,
    pub port: i64,
    pub status: String,
    pub created_at: i64,
    /// environment marker: prod | staging | dev | "" (sites carry it; nodes inherit)
    #[sqlx(default)]
    pub env: String,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct SystemUserRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub secret: String,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct TagRow {
    pub id: String,
    pub name: String,
    pub color: String,
    /// number of assets referencing this tag (computed in list_tags)
    #[sqlx(default)]
    pub usage_count: i64,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct ApprovalRow {
    pub id: String,
    pub job_id: String,
    pub requester_id: String,
    pub requester_email: String,
    pub asset_id: String,
    pub account_id: String,
    pub action: String,
    pub command: String,
    pub state: String, // pending | approved | rejected
    pub reason: Option<String>,
    pub decided_by: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    #[sqlx(default)]
    pub min_approvals: i64,
    #[sqlx(default)]
    pub approver_ids: String,
    /// Review channel: "console" (strong auth) | "tg" (inline one-tap, demo). Empty = console.
    #[sqlx(default)]
    pub quick: String,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct RuleRow {
    pub id: String,
    pub name: String,
    pub subject_user_id: String,
    pub selector_kind: String,
    pub selector: String,
    pub system_user_id: String,
    pub actions: String,
    pub valid_from: i64,
    pub valid_until: Option<i64>,
    pub needs_approval: i64,
    #[sqlx(default)]
    pub min_approvals: i64,
    /// CSV of designated approver user ids; empty = any admin.
    #[sqlx(default)]
    pub approver_ids: String,
    /// Review channel: "console" (strong auth) | "tg" (inline one-tap, demo). Empty = console.
    #[sqlx(default)]
    pub quick: String,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct TemplateRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub command: String,
    pub variables: String,
    pub approver_ids: String,
    pub created_at: i64,
    #[sqlx(default)]
    pub parent_id: Option<String>,
    #[sqlx(default)]
    pub sort: i64,
}

/// One submitted execution (a batch of targets sharing a command).
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct JobRow {
    pub id: String,
    pub kind: String, // ssh | sql
    pub command: String,
    pub operator_id: String,
    pub operator_email: String,
    pub created_at: i64,
    pub finished_at: Option<i64>, // NULL while targets are pending approval
    pub status: String,           // pending | ok | partial | fail
    pub total: i64,
    pub ok_count: i64,
    #[sqlx(default)]
    pub source_ip: String,
    #[sqlx(default)]
    pub source_device: String,
    #[sqlx(default)]
    pub template_id: String,
    #[sqlx(default)]
    pub template_name: String,
}

/// Per-target outcome of a job (holds output; audit rows stay lean).
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct JobTargetRow {
    pub id: String,
    pub job_id: String,
    pub asset_id: String,
    pub asset_name: String,
    pub status: String, // pending | ok | fail | rejected
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub duration_ms: i64,
    pub approval_id: Option<String>,
    pub ts: i64,
}

/// One row of a single node's execution history (job_target joined with its job).
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NodeHistoryRow {
    pub ts: i64,
    pub status: String,
    pub exit_code: Option<i64>,
    pub duration_ms: i64,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub command: String,
    pub operator_email: String,
    pub job_id: String,
    pub kind: String,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct VoteRow {
    pub approval_id: String,
    pub approver_id: String,
    pub approver_email: String,
    pub verdict: String,
    pub reason: Option<String>,
    pub ts: i64,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NotificationRow {
    pub id: String,
    pub user_id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub link: String,
    pub ts: i64,
    pub read: i64,
}

/// One registered Nacos cluster (connection + vault credential handle).
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NacosClusterRow {
    pub id: String,
    pub name: String,
    /// dev | test | prod | ''
    pub env: String,
    /// One or more `host:port` (or `http://host:port`) entries, comma separated.
    pub server_addr: String,
    /// Nacos context path, normally `/nacos` (2.x standalone can be `/`).
    pub context_path: String,
    /// Tenant id; empty = the `public` namespace.
    pub namespace: String,
    pub username: String,
    /// Vault-encrypted password — never serialized to clients.
    #[serde(skip_serializing)]
    pub secret: String,
    pub status: String,
    pub note: String,
    pub created_at: i64,
}

/// A named set of config items used to bootstrap a cluster.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NacosTemplateRow {
    pub id: String,
    pub name: String,
    pub note: String,
    /// json: `[{data_id,group,type,content}]`
    pub items: String,
    pub created_at: i64,
    /// 1 = 按原文下发,不做 `${}` 变量代入。
    /// 从远端同步回来的配置里那些 `${...}` 是应用自己的占位符(Spring 等),
    /// 当成 opsctl 模板变量去要求填值只会让回放失败。
    pub literal: i64,
}

/// One recorded "初始化配置" run against a cluster.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NacosRunRow {
    pub id: String,
    pub cluster_id: String,
    pub cluster_name: String,
    pub template_id: String,
    pub template_name: String,
    pub operator_id: String,
    pub operator_email: String,
    pub namespace: String,
    /// ok | partial | fail
    pub status: String,
    pub total: i64,
    pub ok_count: i64,
    pub dry_run: i64,
    /// json: `[{data_id,group,status,message}]`
    pub items: String,
    pub ts: i64,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Idempotent schema creation (M1 stand-in for real migrations).
    pub async fn init(&self) -> Result<()> {
        let ddl = r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            email TEXT NOT NULL DEFAULT '',
            role TEXT NOT NULL DEFAULT 'viewer',
            pass_hash TEXT NOT NULL,
            telegram_chat_id TEXT,
            login_ttl_secs INTEGER NOT NULL DEFAULT 604800
        );
        CREATE TABLE IF NOT EXISTS sessions (
            sid TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            device_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen INTEGER NOT NULL,
            ip TEXT NOT NULL DEFAULT '',
            revoked INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS audit (
            id TEXT PRIMARY KEY,
            ts INTEGER NOT NULL,
            operator_id TEXT NOT NULL,
            operator_email TEXT NOT NULL,
            action TEXT NOT NULL,
            targets TEXT NOT NULL DEFAULT '',
            payload TEXT NOT NULL DEFAULT '',
            result TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS entries (
            id TEXT PRIMARY KEY,
            project TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            host TEXT NOT NULL DEFAULT '',
            port INTEGER NOT NULL DEFAULT 22,
            username TEXT NOT NULL DEFAULT '',
            secret TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS assets (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,                       -- site | server | database
            parent_id TEXT,                           -- tree parent site or NULL for root
            host TEXT NOT NULL DEFAULT '',
            port INTEGER NOT NULL DEFAULT 22,
            status TEXT NOT NULL DEFAULT 'enabled',    -- enabled | disabled
            created_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS system_users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'ssh_pw',       -- ssh_pw | ssh_key | db_pw
            username TEXT NOT NULL DEFAULT '',
            secret TEXT NOT NULL DEFAULT ''            -- v1 placeholder, SOPS ref later
        );
        CREATE TABLE IF NOT EXISTS asset_accounts (
            asset_id TEXT NOT NULL,
            system_user_id TEXT NOT NULL,
            PRIMARY KEY (asset_id, system_user_id)
        );
        CREATE TABLE IF NOT EXISTS tags (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            color TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS asset_tags (
            asset_id TEXT NOT NULL,
            tag_id TEXT NOT NULL,
            PRIMARY KEY (asset_id, tag_id)
        );
        CREATE TABLE IF NOT EXISTS authorization_rules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            subject_user_id TEXT NOT NULL,             -- 精简:主体=用户(组留后续)
            selector_kind TEXT NOT NULL,               -- subtree | tag | assets
            selector TEXT NOT NULL,                    -- subtree:node id / tag:tag id / assets:csv ids
            system_user_id TEXT NOT NULL DEFAULT '',   -- 连接用账号
            actions TEXT NOT NULL DEFAULT '',          -- csv: ssh,sql,upload
            valid_from INTEGER NOT NULL DEFAULT 0,
            valid_until INTEGER,                       -- NULL = 长期
            needs_approval INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS approvals (
            id TEXT PRIMARY KEY,
            job_id TEXT NOT NULL,
            requester_id TEXT NOT NULL,
            requester_email TEXT NOT NULL DEFAULT '',
            asset_id TEXT NOT NULL,
            account_id TEXT NOT NULL,
            action TEXT NOT NULL DEFAULT 'ssh',
            command TEXT NOT NULL DEFAULT '',
            state TEXT NOT NULL DEFAULT 'pending',      -- pending | approved | rejected
            reason TEXT,                                -- 驳回理由
            decided_by TEXT,
            created_at INTEGER NOT NULL,
            decided_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS settings (
            k TEXT PRIMARY KEY,
            v TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS approval_votes (
            approval_id TEXT NOT NULL,
            approver_id TEXT NOT NULL,
            approver_email TEXT NOT NULL DEFAULT '',
            verdict TEXT NOT NULL,               -- approve | reject
            reason TEXT,
            ts INTEGER NOT NULL,
            PRIMARY KEY (approval_id, approver_id)
        );
        CREATE TABLE IF NOT EXISTS templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'ssh',          -- ssh | sql
            command TEXT NOT NULL DEFAULT '',
            variables TEXT NOT NULL DEFAULT '[]',       -- json: [{name,default}]
            approver_ids TEXT NOT NULL DEFAULT '',      -- csv user ids
            created_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,                         -- ssh | sql
            command TEXT NOT NULL DEFAULT '',
            operator_id TEXT NOT NULL,
            operator_email TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL,
            finished_at INTEGER,                        -- NULL = pending targets remain
            status TEXT NOT NULL DEFAULT 'pending',      -- pending | ok | partial | fail
            total INTEGER NOT NULL DEFAULT 0,
            ok_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS job_targets (
            id TEXT PRIMARY KEY,
            job_id TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            asset_name TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',      -- pending | ok | fail | rejected
            exit_code INTEGER,
            stdout TEXT NOT NULL DEFAULT '',
            stderr TEXT NOT NULL DEFAULT '',
            error TEXT,
            duration_ms INTEGER NOT NULL DEFAULT 0,
            approval_id TEXT,
            ts INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS notifications (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'exec',          -- login | approval | sync | exec
            title TEXT NOT NULL DEFAULT '',
            body TEXT NOT NULL DEFAULT '',
            link TEXT NOT NULL DEFAULT '',
            ts INTEGER NOT NULL,
            read INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS nacos_clusters (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            env TEXT NOT NULL DEFAULT '',                  -- dev | test | prod
            server_addr TEXT NOT NULL DEFAULT '',          -- host:port 列表,逗号分隔
            context_path TEXT NOT NULL DEFAULT '/nacos',
            namespace TEXT NOT NULL DEFAULT '',            -- tenant id,空=public
            username TEXT NOT NULL DEFAULT '',
            secret TEXT NOT NULL DEFAULT '',               -- 金库加密的密码
            status TEXT NOT NULL DEFAULT 'enabled',        -- enabled | disabled
            note TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS nacos_config_templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            note TEXT NOT NULL DEFAULT '',
            items TEXT NOT NULL DEFAULT '[]',              -- json: [{data_id,group,type,content}]
            created_at INTEGER NOT NULL DEFAULT 0,
            literal INTEGER NOT NULL DEFAULT 0            -- 1 = 原文下发,不做变量代入
        );
        CREATE TABLE IF NOT EXISTS nacos_init_runs (
            id TEXT PRIMARY KEY,
            cluster_id TEXT NOT NULL,
            cluster_name TEXT NOT NULL DEFAULT '',
            template_id TEXT NOT NULL DEFAULT '',
            template_name TEXT NOT NULL DEFAULT '',
            operator_id TEXT NOT NULL DEFAULT '',
            operator_email TEXT NOT NULL DEFAULT '',
            namespace TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'ok',             -- ok | partial | fail
            total INTEGER NOT NULL DEFAULT 0,
            ok_count INTEGER NOT NULL DEFAULT 0,
            dry_run INTEGER NOT NULL DEFAULT 0,
            items TEXT NOT NULL DEFAULT '[]',              -- json: 每条配置的结果
            ts INTEGER NOT NULL
        );
        "#;
        // execute() runs a single statement; split on ';' for the batch.
        for stmt in ddl.split(';') {
            let s = stmt.trim();
            if !s.is_empty() {
                sqlx::query(s).execute(&self.pool).await?;
            }
        }
        // Idempotent migration: add columns to pre-existing tables (sqlite has no
        // ADD COLUMN IF NOT EXISTS, so ignore the "duplicate column" error).
        let _ = sqlx::query("ALTER TABLE users ADD COLUMN login_alert INTEGER NOT NULL DEFAULT 1")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE authorization_rules ADD COLUMN min_approvals INTEGER NOT NULL DEFAULT 1")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE approvals ADD COLUMN min_approvals INTEGER NOT NULL DEFAULT 1")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE users ADD COLUMN totp_secret TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE authorization_rules ADD COLUMN approver_ids TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE approvals ADD COLUMN approver_ids TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE audit ADD COLUMN job_id TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE assets ADD COLUMN env TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE templates ADD COLUMN parent_id TEXT")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE templates ADD COLUMN sort INTEGER NOT NULL DEFAULT 0")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE jobs ADD COLUMN source_ip TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE jobs ADD COLUMN source_device TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE jobs ADD COLUMN template_id TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE jobs ADD COLUMN template_name TEXT NOT NULL DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE authorization_rules ADD COLUMN quick TEXT NOT NULL DEFAULT 'console'")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE approvals ADD COLUMN quick TEXT NOT NULL DEFAULT 'console'")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query(
            "ALTER TABLE nacos_config_templates ADD COLUMN literal INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&self.pool)
        .await;
        Ok(())
    }

    // ---- users ----

    pub async fn count_users(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    pub async fn create_user(
        &self,
        id: &str,
        name: &str,
        email: &str,
        role: &str,
        pass_hash: &str,
        login_ttl_secs: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO users (id,name,email,role,pass_hash,login_ttl_secs) VALUES (?,?,?,?,?,?)",
        )
        .bind(id)
        .bind(name)
        .bind(email)
        .bind(role)
        .bind(pass_hash)
        .bind(login_ttl_secs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_user_by_name(&self, name: &str) -> Result<Option<UserRow>> {
        let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn admin_ids(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM users WHERE role = 'admin'")
            .fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|(s,)| s).collect())
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<UserRow>> {
        let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn list_users(&self) -> Result<Vec<UserRow>> {
        Ok(sqlx::query_as::<_, UserRow>("SELECT * FROM users ORDER BY name")
            .fetch_all(&self.pool).await?)
    }
    pub async fn update_user_fields(&self, id: &str, name: &str, email: &str, role: &str) -> Result<()> {
        sqlx::query("UPDATE users SET name=?,email=?,role=? WHERE id=?")
            .bind(name).bind(email).bind(role).bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn set_password(&self, id: &str, pass_hash: &str) -> Result<()> {
        sqlx::query("UPDATE users SET pass_hash=? WHERE id=?")
            .bind(pass_hash).bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_user(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM notifications WHERE user_id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM users WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn count_admins(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'admin'")
            .fetch_one(&self.pool).await?;
        Ok(n)
    }
    pub async fn count_rules_for_subject(&self, user_id: &str) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM authorization_rules WHERE subject_user_id = ?")
            .bind(user_id).fetch_one(&self.pool).await?;
        Ok(n)
    }

    // ---- sessions ----

    pub async fn upsert_session(&self, s: &SessionRow) -> Result<()> {
        // One active session per (user, device): drop old ones for this device.
        sqlx::query("DELETE FROM sessions WHERE user_id = ? AND device_id = ?")
            .bind(&s.user_id)
            .bind(&s.device_id)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO sessions (sid,user_id,device_id,created_at,last_seen,ip,revoked) VALUES (?,?,?,?,?,?,0)",
        )
        .bind(&s.sid)
        .bind(&s.user_id)
        .bind(&s.device_id)
        .bind(s.created_at)
        .bind(s.last_seen)
        .bind(&s.ip)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_session(&self, sid: &str) -> Result<Option<SessionRow>> {
        let row = sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions WHERE sid = ?")
            .bind(sid)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }
    /// Refresh a session's last-seen timestamp (activity tracking).
    pub async fn touch_session(&self, sid: &str, ts: i64) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_seen=? WHERE sid=?")
            .bind(ts).bind(sid).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn list_sessions(&self, user_id: &str) -> Result<Vec<SessionRow>> {
        Ok(sqlx::query_as::<_, SessionRow>(
            "SELECT * FROM sessions WHERE user_id = ? AND revoked = 0 ORDER BY last_seen DESC")
            .bind(user_id).fetch_all(&self.pool).await?)
    }
    /// Revoke a session (only if it belongs to this user). Returns rows affected.
    pub async fn revoke_session(&self, sid: &str, user_id: &str) -> Result<u64> {
        let r = sqlx::query("UPDATE sessions SET revoked = 1 WHERE sid = ? AND user_id = ?")
            .bind(sid).bind(user_id).execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    // ---- profile / telegram ----
    pub async fn update_profile(
        &self, id: &str, name: &str, email: &str, login_ttl_secs: i64, login_alert: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE users SET name=?,email=?,login_ttl_secs=?,login_alert=? WHERE id=?")
            .bind(name).bind(email).bind(login_ttl_secs).bind(login_alert).bind(id)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn set_telegram(&self, id: &str, chat: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE users SET telegram_chat_id=? WHERE id=?")
            .bind(chat).bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn set_totp_secret(&self, id: &str, secret: &str) -> Result<()> {
        sqlx::query("UPDATE users SET totp_secret=? WHERE id=?")
            .bind(secret).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    // ---- settings (key/value: git config, telegram pending codes) ----
    pub async fn get_setting(&self, k: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT v FROM settings WHERE k = ?")
            .bind(k).fetch_optional(&self.pool).await?;
        Ok(row.map(|(v,)| v))
    }
    pub async fn set_setting(&self, k: &str, v: &str) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO settings (k,v) VALUES (?,?)")
            .bind(k).bind(v).execute(&self.pool).await?;
        Ok(())
    }

    // ---- audit ----

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_audit(
        &self,
        id: &str,
        ts: i64,
        operator_id: &str,
        operator_email: &str,
        action: &str,
        targets: &str,
        payload: &str,
        result: &str,
        job_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit (id,ts,operator_id,operator_email,action,targets,payload,result,job_id) VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(id)
        .bind(ts)
        .bind(operator_id)
        .bind(operator_email)
        .bind(action)
        .bind(targets)
        .bind(payload)
        .bind(result)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_audit(&self, limit: i64) -> Result<Vec<AuditRow>> {
        let rows = sqlx::query_as::<_, AuditRow>(
            "SELECT id,ts,operator_id,operator_email,action,targets,payload,result,job_id FROM audit ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Filtered audit query (empty string = no filter for that field).
    pub async fn list_audit_filtered(
        &self, action: &str, result: &str, operator: &str, keyword: &str, limit: i64,
    ) -> Result<Vec<AuditRow>> {
        let sql = "SELECT id,ts,operator_id,operator_email,action,targets,payload,result,job_id FROM audit \
             WHERE (?1 = '' OR action = ?1) \
               AND (?2 = '' OR result = ?2) \
               AND (?3 = '' OR operator_email LIKE '%'||?3||'%') \
               AND (?4 = '' OR targets LIKE '%'||?4||'%' OR payload LIKE '%'||?4||'%') \
             ORDER BY ts DESC LIMIT ?5";
        Ok(sqlx::query_as::<_, AuditRow>(sql)
            .bind(action).bind(result).bind(operator).bind(keyword).bind(limit)
            .fetch_all(&self.pool).await?)
    }

    // ---- entries (SSH targets for M1) ----

    pub async fn create_entry(&self, e: &EntryRow) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO entries (id,project,name,kind,host,port,username,secret) VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(&e.id)
        .bind(&e.project)
        .bind(&e.name)
        .bind(&e.kind)
        .bind(&e.host)
        .bind(e.port)
        .bind(&e.username)
        .bind(&e.secret)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_entry(&self, id: &str) -> Result<Option<EntryRow>> {
        let row = sqlx::query_as::<_, EntryRow>("SELECT * FROM entries WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }
}

/// JumpServer-lite model: assets / system-users / tags / authorization rules.
impl Store {
    // ---- assets ----
    pub async fn list_assets(&self) -> Result<Vec<AssetRow>> {
        Ok(sqlx::query_as::<_, AssetRow>("SELECT * FROM assets ORDER BY kind,name")
            .fetch_all(&self.pool)
            .await?)
    }
    pub async fn get_asset(&self, id: &str) -> Result<Option<AssetRow>> {
        Ok(sqlx::query_as::<_, AssetRow>("SELECT * FROM assets WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }
    pub async fn create_asset(&self, a: &AssetRow) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO assets (id,name,kind,parent_id,host,port,status,created_at,env) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(&a.id).bind(&a.name).bind(&a.kind).bind(&a.parent_id)
            .bind(&a.host).bind(a.port).bind(&a.status).bind(a.created_at).bind(&a.env)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn update_asset(&self, a: &AssetRow) -> Result<()> {
        sqlx::query("UPDATE assets SET name=?,kind=?,parent_id=?,host=?,port=?,status=?,env=? WHERE id=?")
            .bind(&a.name).bind(&a.kind).bind(&a.parent_id)
            .bind(&a.host).bind(a.port).bind(&a.status).bind(&a.env).bind(&a.id)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn count_asset_children(&self, id: &str) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM assets WHERE parent_id = ?")
            .bind(id).fetch_one(&self.pool).await?;
        Ok(n)
    }
    pub async fn delete_asset(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM asset_tags WHERE asset_id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM asset_accounts WHERE asset_id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM assets WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }
    /// Replace the tag set of an asset.
    pub async fn set_asset_tags(&self, asset_id: &str, tag_ids: &[String]) -> Result<()> {
        sqlx::query("DELETE FROM asset_tags WHERE asset_id = ?").bind(asset_id).execute(&self.pool).await?;
        for t in tag_ids {
            self.add_asset_tag(asset_id, t).await?;
        }
        Ok(())
    }
    /// Replace the bound accounts of an asset.
    pub async fn set_asset_accounts(&self, asset_id: &str, su_ids: &[String]) -> Result<()> {
        sqlx::query("DELETE FROM asset_accounts WHERE asset_id = ?").bind(asset_id).execute(&self.pool).await?;
        for s in su_ids {
            self.add_asset_account(asset_id, s).await?;
        }
        Ok(())
    }

    // ---- system users (accounts) ----
    pub async fn list_system_users(&self) -> Result<Vec<SystemUserRow>> {
        Ok(sqlx::query_as::<_, SystemUserRow>("SELECT * FROM system_users ORDER BY name")
            .fetch_all(&self.pool).await?)
    }
    pub async fn get_system_user(&self, id: &str) -> Result<Option<SystemUserRow>> {
        Ok(sqlx::query_as::<_, SystemUserRow>("SELECT * FROM system_users WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }
    pub async fn create_system_user(&self, s: &SystemUserRow) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO system_users (id,name,kind,username,secret) VALUES (?,?,?,?,?)")
            .bind(&s.id).bind(&s.name).bind(&s.kind).bind(&s.username).bind(&s.secret)
            .execute(&self.pool).await?;
        Ok(())
    }
    /// (id, secret) of accounts whose secret is non-empty and not yet encrypted.
    pub async fn list_plaintext_secrets(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let like = format!("{prefix}%");
        Ok(sqlx::query_as(
            "SELECT id, secret FROM system_users WHERE secret != '' AND secret NOT LIKE ?")
            .bind(like).fetch_all(&self.pool).await?)
    }
    pub async fn update_system_user_secret(&self, id: &str, secret: &str) -> Result<()> {
        sqlx::query("UPDATE system_users SET secret=? WHERE id=?")
            .bind(secret).bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn add_asset_account(&self, asset_id: &str, su_id: &str) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO asset_accounts (asset_id,system_user_id) VALUES (?,?)")
            .bind(asset_id).bind(su_id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn accounts_of_asset(&self, asset_id: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT system_user_id FROM asset_accounts WHERE asset_id = ?")
            .bind(asset_id).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|(s,)| s).collect())
    }
    /// Update; empty `secret` keeps the stored one.
    pub async fn update_system_user(&self, s: &SystemUserRow) -> Result<()> {
        if s.secret.is_empty() {
            sqlx::query("UPDATE system_users SET name=?,kind=?,username=? WHERE id=?")
                .bind(&s.name).bind(&s.kind).bind(&s.username).bind(&s.id)
                .execute(&self.pool).await?;
        } else {
            sqlx::query("UPDATE system_users SET name=?,kind=?,username=?,secret=? WHERE id=?")
                .bind(&s.name).bind(&s.kind).bind(&s.username).bind(&s.secret).bind(&s.id)
                .execute(&self.pool).await?;
        }
        Ok(())
    }
    pub async fn delete_system_user(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM asset_accounts WHERE system_user_id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM system_users WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn count_rules_using_account(&self, su_id: &str) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM authorization_rules WHERE system_user_id = ?")
            .bind(su_id).fetch_one(&self.pool).await?;
        Ok(n)
    }

    // ---- tags ----
    pub async fn list_tags(&self) -> Result<Vec<TagRow>> {
        Ok(sqlx::query_as::<_, TagRow>(
            "SELECT t.id, t.name, t.color, \
             (SELECT COUNT(*) FROM asset_tags at WHERE at.tag_id = t.id) AS usage_count \
             FROM tags t ORDER BY t.name",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn create_tag(&self, t: &TagRow) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO tags (id,name,color) VALUES (?,?,?)")
            .bind(&t.id).bind(&t.name).bind(&t.color).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn add_asset_tag(&self, asset_id: &str, tag_id: &str) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO asset_tags (asset_id,tag_id) VALUES (?,?)")
            .bind(asset_id).bind(tag_id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn asset_tag_ids(&self, asset_id: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT tag_id FROM asset_tags WHERE asset_id = ?")
            .bind(asset_id).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }
    pub async fn update_tag(&self, t: &TagRow) -> Result<()> {
        sqlx::query("UPDATE tags SET name=?,color=? WHERE id=?")
            .bind(&t.name).bind(&t.color).bind(&t.id)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_tag(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM asset_tags WHERE tag_id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM tags WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn count_rules_using_tag(&self, tag_id: &str) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM authorization_rules WHERE selector_kind = 'tag' AND selector = ?")
            .bind(tag_id).fetch_one(&self.pool).await?;
        Ok(n)
    }

    // ---- authorization rules ----
    pub async fn list_rules(&self) -> Result<Vec<RuleRow>> {
        Ok(sqlx::query_as::<_, RuleRow>("SELECT * FROM authorization_rules ORDER BY name")
            .fetch_all(&self.pool).await?)
    }
    pub async fn list_rules_for_user(&self, user_id: &str) -> Result<Vec<RuleRow>> {
        Ok(sqlx::query_as::<_, RuleRow>("SELECT * FROM authorization_rules WHERE subject_user_id = ?")
            .bind(user_id).fetch_all(&self.pool).await?)
    }
    pub async fn create_rule(&self, r: &RuleRow) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO authorization_rules (id,name,subject_user_id,selector_kind,selector,system_user_id,actions,valid_from,valid_until,needs_approval,min_approvals,approver_ids,quick) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&r.id).bind(&r.name).bind(&r.subject_user_id).bind(&r.selector_kind)
            .bind(&r.selector).bind(&r.system_user_id).bind(&r.actions)
            .bind(r.valid_from).bind(&r.valid_until).bind(r.needs_approval)
            .bind(r.min_approvals.max(1)).bind(&r.approver_ids)
            .bind(if r.quick.is_empty() { "console" } else { r.quick.as_str() })
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn get_rule(&self, id: &str) -> Result<Option<RuleRow>> {
        Ok(sqlx::query_as::<_, RuleRow>("SELECT * FROM authorization_rules WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }

    // ---- templates ----
    pub async fn list_templates(&self) -> Result<Vec<TemplateRow>> {
        Ok(sqlx::query_as::<_, TemplateRow>("SELECT * FROM templates ORDER BY kind,name")
            .fetch_all(&self.pool).await?)
    }
    pub async fn get_template(&self, id: &str) -> Result<Option<TemplateRow>> {
        Ok(sqlx::query_as::<_, TemplateRow>("SELECT * FROM templates WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }
    pub async fn upsert_template(&self, t: &TemplateRow) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO templates (id,name,kind,command,variables,approver_ids,created_at,parent_id,sort) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(&t.id).bind(&t.name).bind(&t.kind).bind(&t.command)
            .bind(&t.variables).bind(&t.approver_ids).bind(t.created_at).bind(&t.parent_id).bind(t.sort)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_template(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM templates WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    // ---- notifications ----

    /// Keep at most this many notifications per user; the UI only ever lists the
    /// newest 100, so older rows are dead weight (a long-lived dev box piles up
    /// hundreds of 新设备登录 alerts).
    const NOTIFICATION_KEEP: i64 = 200;

    #[allow(clippy::too_many_arguments)]
    pub async fn push_notification(
        &self, user_id: &str, kind: &str, title: &str, body: &str, link: &str, ts: i64,
    ) -> Result<()> {
        sqlx::query("INSERT INTO notifications (id,user_id,kind,title,body,link,ts,read) VALUES (?,?,?,?,?,?,?,0)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(user_id).bind(kind).bind(title).bind(body).bind(link).bind(ts)
            .execute(&self.pool).await?;
        sqlx::query(
            "DELETE FROM notifications WHERE user_id = ?1 AND id NOT IN \
             (SELECT id FROM notifications WHERE user_id = ?1 ORDER BY ts DESC, rowid DESC LIMIT ?2)")
            .bind(user_id).bind(Self::NOTIFICATION_KEEP)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn list_notifications(&self, user_id: &str, limit: i64) -> Result<Vec<NotificationRow>> {
        Ok(sqlx::query_as::<_, NotificationRow>(
            "SELECT * FROM notifications WHERE user_id = ? ORDER BY ts DESC LIMIT ?")
            .bind(user_id).bind(limit).fetch_all(&self.pool).await?)
    }
    pub async fn count_unread(&self, user_id: &str) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM notifications WHERE user_id = ? AND read = 0")
            .bind(user_id).fetch_one(&self.pool).await?;
        Ok(n)
    }
    pub async fn mark_read(&self, id: &str, user_id: &str) -> Result<()> {
        sqlx::query("UPDATE notifications SET read = 1 WHERE id = ? AND user_id = ?")
            .bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn mark_unread(&self, id: &str, user_id: &str) -> Result<()> {
        sqlx::query("UPDATE notifications SET read = 0 WHERE id = ? AND user_id = ?")
            .bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn mark_all_read(&self, user_id: &str) -> Result<()> {
        sqlx::query("UPDATE notifications SET read = 1 WHERE user_id = ?")
            .bind(user_id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_notification(&self, id: &str, user_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM notifications WHERE id = ? AND user_id = ?")
            .bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_rule(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM authorization_rules WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    // ---- approvals ----
    pub async fn create_approval(&self, a: &ApprovalRow) -> Result<()> {
        sqlx::query("INSERT INTO approvals (id,job_id,requester_id,requester_email,asset_id,account_id,action,command,state,reason,decided_by,created_at,decided_at,min_approvals,approver_ids,quick) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&a.id).bind(&a.job_id).bind(&a.requester_id).bind(&a.requester_email)
            .bind(&a.asset_id).bind(&a.account_id).bind(&a.action).bind(&a.command)
            .bind(&a.state).bind(&a.reason).bind(&a.decided_by).bind(a.created_at).bind(&a.decided_at)
            .bind(a.min_approvals.max(1)).bind(&a.approver_ids)
            .bind(if a.quick.is_empty() { "console" } else { a.quick.as_str() })
            .execute(&self.pool).await?;
        Ok(())
    }
    /// Record one approver's vote (idempotent per approver). Returns false if
    /// this approver already voted on this approval.
    pub async fn add_vote(&self, approval_id: &str, approver_id: &str, approver_email: &str, verdict: &str, reason: Option<&str>, ts: i64) -> Result<bool> {
        let r = sqlx::query("INSERT OR IGNORE INTO approval_votes (approval_id,approver_id,approver_email,verdict,reason,ts) VALUES (?,?,?,?,?,?)")
            .bind(approval_id).bind(approver_id).bind(approver_email).bind(verdict).bind(reason).bind(ts)
            .execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }
    pub async fn count_votes(&self, approval_id: &str, verdict: &str) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM approval_votes WHERE approval_id = ? AND verdict = ?")
            .bind(approval_id).bind(verdict).fetch_one(&self.pool).await?;
        Ok(n)
    }
    pub async fn get_approval(&self, id: &str) -> Result<Option<ApprovalRow>> {
        Ok(sqlx::query_as::<_, ApprovalRow>("SELECT * FROM approvals WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }
    /// Pending first, then most-recent decided.
    pub async fn list_approvals(&self, limit: i64) -> Result<Vec<ApprovalRow>> {
        Ok(sqlx::query_as::<_, ApprovalRow>(
            "SELECT * FROM approvals ORDER BY (state = 'pending') DESC, created_at DESC LIMIT ?")
            .bind(limit).fetch_all(&self.pool).await?)
    }
    pub async fn count_pending_approvals(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM approvals WHERE state = 'pending'")
            .fetch_one(&self.pool).await?;
        Ok(n)
    }
    pub async fn decide_approval(
        &self, id: &str, state: &str, decided_by: &str, reason: Option<&str>, ts: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE approvals SET state=?,decided_by=?,reason=?,decided_at=? WHERE id=?")
            .bind(state).bind(decided_by).bind(reason).bind(ts).bind(id)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn list_approvals_for_job(&self, job_id: &str) -> Result<Vec<ApprovalRow>> {
        Ok(sqlx::query_as::<_, ApprovalRow>(
            "SELECT * FROM approvals WHERE job_id = ? ORDER BY created_at")
            .bind(job_id).fetch_all(&self.pool).await?)
    }
    pub async fn list_votes(&self, approval_id: &str) -> Result<Vec<VoteRow>> {
        Ok(sqlx::query_as::<_, VoteRow>(
            "SELECT * FROM approval_votes WHERE approval_id = ? ORDER BY ts")
            .bind(approval_id).fetch_all(&self.pool).await?)
    }
}

/// Job aggregation: one row per submitted execution + per-target outcomes.
impl Store {
    pub async fn create_job(&self, j: &JobRow) -> Result<()> {
        sqlx::query("INSERT INTO jobs (id,kind,command,operator_id,operator_email,created_at,finished_at,status,total,ok_count,source_ip,source_device,template_id,template_name) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&j.id).bind(&j.kind).bind(&j.command)
            .bind(&j.operator_id).bind(&j.operator_email)
            .bind(j.created_at).bind(j.finished_at).bind(&j.status)
            .bind(j.total).bind(j.ok_count)
            .bind(&j.source_ip).bind(&j.source_device)
            .bind(&j.template_id).bind(&j.template_name)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn insert_job_target(&self, t: &JobTargetRow) -> Result<()> {
        sqlx::query("INSERT INTO job_targets (id,job_id,asset_id,asset_name,status,exit_code,stdout,stderr,error,duration_ms,approval_id,ts) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&t.id).bind(&t.job_id).bind(&t.asset_id).bind(&t.asset_name)
            .bind(&t.status).bind(t.exit_code)
            .bind(truncate_output(&t.stdout)).bind(truncate_output(&t.stderr))
            .bind(&t.error).bind(t.duration_ms).bind(&t.approval_id).bind(t.ts)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Fill in the outcome of a previously-pending target (approval decided).
    #[allow(clippy::too_many_arguments)]
    pub async fn update_job_target_result(
        &self, approval_id: &str, status: &str, exit_code: Option<i64>,
        stdout: &str, stderr: &str, error: Option<&str>, duration_ms: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE job_targets SET status=?,exit_code=?,stdout=?,stderr=?,error=?,duration_ms=? WHERE approval_id=?")
            .bind(status).bind(exit_code)
            .bind(truncate_output(stdout)).bind(truncate_output(stderr))
            .bind(error).bind(duration_ms).bind(approval_id)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Recompute job status/ok_count; set finished_at when no target is pending.
    /// Returns None for unknown job ids (e.g. approvals created before the jobs
    /// table existed).
    pub async fn finalize_job_if_done(&self, job_id: &str, now: i64) -> Result<Option<JobRow>> {
        let (total, ok, pending): (i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(status='ok'),0), COALESCE(SUM(status='pending'),0) FROM job_targets WHERE job_id = ?")
            .bind(job_id).fetch_one(&self.pool).await?;
        if pending > 0 {
            sqlx::query("UPDATE jobs SET ok_count=?, total=?, status='pending' WHERE id=?")
                .bind(ok).bind(total).bind(job_id).execute(&self.pool).await?;
        } else {
            let status = if ok == total && total > 0 { "ok" } else if ok == 0 { "fail" } else { "partial" };
            sqlx::query("UPDATE jobs SET ok_count=?, total=?, status=?, finished_at=? WHERE id=?")
                .bind(ok).bind(total).bind(status).bind(now).bind(job_id)
                .execute(&self.pool).await?;
        }
        Ok(self.get_job(job_id).await?)
    }

    /// Filtered job list (empty string / 0 = no filter for that field).
    pub async fn list_jobs_filtered(
        &self, operator: &str, status: &str, kind: &str, keyword: &str, from_ts: i64, limit: i64,
    ) -> Result<Vec<JobRow>> {
        let sql = "SELECT * FROM jobs \
             WHERE (?1 = '' OR operator_id = ?1) \
               AND (?2 = '' OR status = ?2) \
               AND (?3 = '' OR kind = ?3) \
               AND (?4 = '' OR command LIKE '%'||?4||'%' OR operator_email LIKE '%'||?4||'%') \
               AND (?5 <= 0 OR created_at >= ?5) \
             ORDER BY created_at DESC LIMIT ?6";
        Ok(sqlx::query_as::<_, JobRow>(sql)
            .bind(operator).bind(status).bind(kind).bind(keyword).bind(from_ts).bind(limit)
            .fetch_all(&self.pool).await?)
    }

    pub async fn get_job(&self, id: &str) -> Result<Option<JobRow>> {
        Ok(sqlx::query_as::<_, JobRow>("SELECT * FROM jobs WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn list_job_targets(&self, job_id: &str) -> Result<Vec<JobTargetRow>> {
        Ok(sqlx::query_as::<_, JobTargetRow>(
            "SELECT * FROM job_targets WHERE job_id = ? ORDER BY ts, asset_name")
            .bind(job_id).fetch_all(&self.pool).await?)
    }

    /// Per-node execution history: every job_target on one asset, joined with its
    /// job for the command/operator. `operator=""` = all (admin); else own only.
    pub async fn node_history(&self, asset_id: &str, operator: &str, limit: i64) -> Result<Vec<NodeHistoryRow>> {
        let sql = "SELECT jt.ts AS ts, jt.status AS status, jt.exit_code AS exit_code, \
                jt.duration_ms AS duration_ms, jt.stdout AS stdout, jt.stderr AS stderr, jt.error AS error, \
                j.command AS command, j.operator_email AS operator_email, j.id AS job_id, j.kind AS kind \
             FROM job_targets jt JOIN jobs j ON jt.job_id = j.id \
             WHERE jt.asset_id = ?1 AND (?2 = '' OR j.operator_id = ?2) \
             ORDER BY jt.ts DESC LIMIT ?3";
        Ok(sqlx::query_as::<_, NodeHistoryRow>(sql)
            .bind(asset_id).bind(operator).bind(limit).fetch_all(&self.pool).await?)
    }
}

/// Nacos 管理:集群注册表 / 配置模板 / 初始化记录.
impl Store {
    pub async fn list_nacos_clusters(&self) -> Result<Vec<NacosClusterRow>> {
        Ok(sqlx::query_as::<_, NacosClusterRow>(
            "SELECT * FROM nacos_clusters ORDER BY env, name")
            .fetch_all(&self.pool).await?)
    }

    pub async fn get_nacos_cluster(&self, id: &str) -> Result<Option<NacosClusterRow>> {
        Ok(sqlx::query_as::<_, NacosClusterRow>("SELECT * FROM nacos_clusters WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn create_nacos_cluster(&self, c: &NacosClusterRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO nacos_clusters (id,name,env,server_addr,context_path,namespace,username,secret,status,note,created_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&c.id).bind(&c.name).bind(&c.env).bind(&c.server_addr).bind(&c.context_path)
            .bind(&c.namespace).bind(&c.username).bind(&c.secret).bind(&c.status).bind(&c.note)
            .bind(c.created_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Update; empty `secret` keeps the stored one.
    pub async fn update_nacos_cluster(&self, c: &NacosClusterRow) -> Result<()> {
        if c.secret.is_empty() {
            sqlx::query(
                "UPDATE nacos_clusters SET name=?,env=?,server_addr=?,context_path=?,namespace=?,\
                 username=?,status=?,note=? WHERE id=?")
                .bind(&c.name).bind(&c.env).bind(&c.server_addr).bind(&c.context_path)
                .bind(&c.namespace).bind(&c.username).bind(&c.status).bind(&c.note).bind(&c.id)
                .execute(&self.pool).await?;
        } else {
            sqlx::query(
                "UPDATE nacos_clusters SET name=?,env=?,server_addr=?,context_path=?,namespace=?,\
                 username=?,secret=?,status=?,note=? WHERE id=?")
                .bind(&c.name).bind(&c.env).bind(&c.server_addr).bind(&c.context_path)
                .bind(&c.namespace).bind(&c.username).bind(&c.secret).bind(&c.status).bind(&c.note)
                .bind(&c.id)
                .execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn delete_nacos_cluster(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM nacos_clusters WHERE id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM nacos_init_runs WHERE cluster_id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn list_nacos_templates(&self) -> Result<Vec<NacosTemplateRow>> {
        Ok(sqlx::query_as::<_, NacosTemplateRow>(
            "SELECT * FROM nacos_config_templates ORDER BY created_at DESC")
            .fetch_all(&self.pool).await?)
    }

    pub async fn get_nacos_template(&self, id: &str) -> Result<Option<NacosTemplateRow>> {
        Ok(sqlx::query_as::<_, NacosTemplateRow>(
            "SELECT * FROM nacos_config_templates WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn save_nacos_template(&self, t: &NacosTemplateRow) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO nacos_config_templates (id,name,note,items,created_at,literal) \
             VALUES (?,?,?,?,?,?)")
            .bind(&t.id).bind(&t.name).bind(&t.note).bind(&t.items).bind(t.created_at)
            .bind(t.literal)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn delete_nacos_template(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM nacos_config_templates WHERE id = ?")
            .bind(id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn insert_nacos_run(&self, r: &NacosRunRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO nacos_init_runs (id,cluster_id,cluster_name,template_id,template_name,\
             operator_id,operator_email,namespace,status,total,ok_count,dry_run,items,ts) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&r.id).bind(&r.cluster_id).bind(&r.cluster_name).bind(&r.template_id)
            .bind(&r.template_name).bind(&r.operator_id).bind(&r.operator_email).bind(&r.namespace)
            .bind(&r.status).bind(r.total).bind(r.ok_count).bind(r.dry_run).bind(&r.items).bind(r.ts)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Init history; `cluster_id=""` = every cluster.
    pub async fn list_nacos_runs(&self, cluster_id: &str, limit: i64) -> Result<Vec<NacosRunRow>> {
        Ok(sqlx::query_as::<_, NacosRunRow>(
            "SELECT * FROM nacos_init_runs WHERE (?1 = '' OR cluster_id = ?1) \
             ORDER BY ts DESC, rowid DESC LIMIT ?2")
            .bind(cluster_id).bind(limit).fetch_all(&self.pool).await?)
    }

    /// Most recent *applied* run (dry runs don't count as an initialization).
    pub async fn last_nacos_run(&self, cluster_id: &str) -> Result<Option<NacosRunRow>> {
        Ok(sqlx::query_as::<_, NacosRunRow>(
            "SELECT * FROM nacos_init_runs WHERE cluster_id = ? AND dry_run = 0 \
             ORDER BY ts DESC, rowid DESC LIMIT 1")
            .bind(cluster_id).fetch_optional(&self.pool).await?)
    }
}

/// Cap stored command output so one noisy target can't bloat the DB.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

fn truncate_output(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s.to_string();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated {} bytes]", &s[..end], s.len() - end)
}
