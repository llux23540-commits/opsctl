//! Local sqlite snapshot backups: a daily 03:00 (local time) `VACUUM INTO`
//! snapshot plus a startup catch-up run, with age-based pruning. Status is
//! tracked in the settings table (`backup_last_at` / `backup_last_file`).

use std::path::Path;

use time::macros::format_description;
use time::{Duration, OffsetDateTime, Time, UtcOffset};

use crate::config::BackupCfg;
use crate::state::{now_secs, AppState};
use crate::store::Store;

/// Local wall-clock now; falls back to UTC when the offset is indeterminate
/// (possible on some unix setups once threads are running).
fn local_now() -> OffsetDateTime {
    let off = UtcOffset::current_local_offset().unwrap_or_else(|_| {
        tracing::warn!("local UTC offset indeterminate — backup schedule uses UTC");
        UtcOffset::UTC
    });
    OffsetDateTime::now_utc().to_offset(off)
}

/// Unix ts of the next local 03:00 (status endpoint helper).
pub fn next_run_ts() -> i64 {
    next_run_at(local_now())
}

/// Unix ts of the next local 03:00 strictly after `now`.
pub fn next_run_at(now: OffsetDateTime) -> i64 {
    let three = Time::from_hms(3, 0, 0).expect("03:00 is valid");
    let today3 = now.replace_time(three);
    let next = if now < today3 { today3 } else { today3 + Duration::days(1) };
    next.unix_timestamp()
}

/// Take one snapshot. Returns (file path, unix ts).
pub async fn run_backup(store: &Store, dir: &str) -> anyhow::Result<(String, i64)> {
    tokio::fs::create_dir_all(dir).await?;
    let stamp = local_now()
        .format(format_description!("[year][month][day]-[hour][minute][second]"))
        .unwrap_or_else(|_| "unknown".into());
    let mut path = format!("{}/opsctl-{stamp}.db", dir.trim_end_matches(['/', '\\']));
    // VACUUM INTO refuses to overwrite; disambiguate same-second reruns.
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        path = format!(
            "{}/opsctl-{stamp}-{}.db",
            dir.trim_end_matches(['/', '\\']),
            &uuid::Uuid::new_v4().to_string()[..6]
        );
    }
    sqlx::query("VACUUM INTO ?")
        .bind(&path)
        .execute(&store.pool)
        .await?;
    let at = now_secs();
    store.set_setting("backup_last_at", &at.to_string()).await?;
    store.set_setting("backup_last_file", &path).await?;
    tracing::info!(file = %path, "backup snapshot written");
    Ok((path, at))
}

/// Delete snapshots older than `retention_days` (by file modified time).
pub async fn cleanup(dir: &str, retention_days: i64) {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs((retention_days.max(1) as u64) * 86400);
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("opsctl-") && name.ends_with(".db")) {
            continue;
        }
        let Ok(meta) = entry.metadata().await else { continue };
        if meta.modified().map(|m| m < cutoff).unwrap_or(false) {
            let _ = tokio::fs::remove_file(entry.path()).await;
            tracing::info!(file = %name, "pruned expired backup");
        }
    }
}

/// Number of `opsctl-*.db` snapshots currently in `dir`.
pub async fn snapshot_count(dir: &str) -> i64 {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return 0 };
    let mut n = 0;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("opsctl-") && name.ends_with(".db") {
            n += 1;
        }
    }
    n
}

pub async fn last_backup_at(store: &Store) -> Option<i64> {
    store
        .get_setting("backup_last_at")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|v| *v > 0)
}

/// Long-running scheduler: startup catch-up (never backed up, or >24h ago),
/// then one snapshot at every local 03:00. Spawned from `main` when enabled.
pub async fn scheduler(st: AppState) {
    let cfg: &BackupCfg = &st.backup;
    let stale = match last_backup_at(&st.store).await {
        None => true,
        Some(at) => now_secs() - at > 86400,
    };
    if stale {
        if let Err(e) = run_backup(&st.store, &cfg.dir).await {
            tracing::warn!(error = %e, "startup catch-up backup failed");
        }
        cleanup(&cfg.dir, cfg.retention_days).await;
    }
    loop {
        let wait = (next_run_at(local_now()) - now_secs()).max(60);
        tokio::time::sleep(std::time::Duration::from_secs(wait as u64)).await;
        if let Err(e) = run_backup(&st.store, &cfg.dir).await {
            tracing::warn!(error = %e, "scheduled backup failed");
        }
        cleanup(&cfg.dir, cfg.retention_days).await;
    }
}

/// True when the backup dir exists or can be described (used by status).
pub fn dir_display(dir: &str) -> String {
    Path::new(dir).to_string_lossy().replace('\\', "/")
}
