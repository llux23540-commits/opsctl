//! Server configuration (figment: defaults < config file < env `OPSCTL_`).

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerCfg,
    pub store: StoreCfg,
    pub auth: AuthCfg,
    /// Bootstrap admin, seeded on first start if the user table is empty.
    pub bootstrap: BootstrapCfg,
    /// Dev seed data (test accounts + a sample SSH target) for easy testing.
    pub dev: DevCfg,
    /// Vault: optional unseal passphrase for auto-unseal at boot.
    pub vault: VaultCfg,
    /// Local sqlite snapshot backups (daily 03:00 + startup catch-up).
    pub backup: BackupCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupCfg {
    pub enabled: bool,
    /// Snapshots older than this many days are pruned.
    pub retention_days: i64,
    /// Directory the `opsctl-*.db` snapshots are written to.
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultCfg {
    /// If set (e.g. `OPSCTL_VAULT__PASSPHRASE`), the server unseals at startup.
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevCfg {
    /// Seed operator/viewer test accounts + a sample entry on startup.
    pub seed: bool,
    /// Sample SSH target (point at a real host via `OPSCTL_DEV__SAMPLE_*`).
    pub sample_host: String,
    pub sample_port: i64,
    pub sample_user: String,
    pub sample_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCfg {
    pub bind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreCfg {
    /// e.g. `sqlite://data/opsctl.db?mode=rwc` or `postgres://...`
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCfg {
    /// HMAC secret for signing JWTs. MUST be overridden in production.
    pub jwt_secret: String,
    /// Default login lifetime (seconds) when the user has no personal setting.
    pub default_ttl_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapCfg {
    pub admin_user: String,
    pub admin_password: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerCfg {
                bind: "127.0.0.1:8443".into(),
            },
            store: StoreCfg {
                url: "sqlite://data/opsctl.db?mode=rwc".into(),
            },
            auth: AuthCfg {
                // Dev default — logged as a warning at startup. Override via config/env.
                jwt_secret: "dev-insecure-change-me".into(),
                default_ttl_secs: 7 * 24 * 3600,
            },
            bootstrap: BootstrapCfg {
                admin_user: "admin".into(),
                admin_password: "admin".into(),
            },
            dev: DevCfg {
                seed: true,
                sample_host: "127.0.0.1".into(),
                sample_port: 22,
                sample_user: "root".into(),
                sample_password: String::new(),
            },
            vault: VaultCfg { passphrase: None },
            backup: BackupCfg {
                enabled: true,
                retention_days: 30,
                dir: "data/backups".into(),
            },
        }
    }
}

impl Config {
    /// Load defaults, overlay `config.toml` (if present) and `OPSCTL_*` env vars.
    /// Env uses `__` as the nesting separator, e.g. `OPSCTL_AUTH__JWT_SECRET`.
    pub fn load() -> anyhow::Result<Self> {
        let cfg: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("config.toml"))
            .merge(Env::prefixed("OPSCTL_").split("__"))
            .extract()?;
        Ok(cfg)
    }
}
