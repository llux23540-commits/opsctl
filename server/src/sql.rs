//! Server-side SQL executor. v1: SQLite targets only (sqlx `sqlite` feature).
//! `asset.host` holds the sqlite file path. mysql/postgres need extra cargo
//! features + TLS and are a later phase.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use sqlx::sqlite::{SqlitePoolOptions, SqliteRow};
use sqlx::{Column, Row};

/// sqlx's SQLite driver runs statements on a worker thread, so the SQL string
/// must be `'static`. We intern query strings in a process-global set so each
/// *distinct* query is leaked at most once (bounded), instead of leaking on
/// every call.
static QUERY_INTERN: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn intern(q: &str) -> &'static str {
    let mut set = QUERY_INTERN.lock().unwrap();
    if let Some(&existing) = set.get(q) {
        return existing;
    }
    let leaked: &'static str = Box::leak(q.to_owned().into_boxed_str());
    set.insert(leaked);
    leaked
}

pub struct SqlOutcome {
    pub ok: bool,
    /// Human-readable result: a small text table for queries, or an affected-row note.
    pub output: String,
    /// Rows affected for non-query statements (None for SELECT).
    pub affected: Option<i64>,
}

/// Best-effort stringify of a dynamically-typed sqlite cell.
fn cell_to_string(row: &SqliteRow, i: usize) -> String {
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return v.map(|x| x.to_string()).unwrap_or_else(|| "NULL".into());
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
        return v.map(|x| x.to_string()).unwrap_or_else(|| "NULL".into());
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(i) {
        return v.unwrap_or_else(|| "NULL".into());
    }
    "?".into()
}

/// Connect to a sqlite file and run one statement. SELECT/PRAGMA/WITH return a
/// text table (first 100 rows); other statements return affected-row count.
pub async fn run_query(path: &str, query: &str) -> anyhow::Result<SqlOutcome> {
    // rwc: open read-write, create the file if missing (demo-friendly).
    let url = format!("sqlite://{path}?mode=rwc");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;

    let mut conn = pool.acquire().await?;
    let head = query.trim().to_lowercase();
    let is_query = head.starts_with("select") || head.starts_with("pragma") || head.starts_with("with");
    let q: &'static str = intern(query.trim());

    if is_query {
        let rows = sqlx::query(q).fetch_all(&mut *conn).await?;
        let mut out = String::new();
        if let Some(first) = rows.first() {
            let cols: Vec<String> = first.columns().iter().map(|c| c.name().to_string()).collect();
            out.push_str(&cols.join(" | "));
            out.push('\n');
        }
        for r in rows.iter().take(100) {
            let cells: Vec<String> = (0..r.len()).map(|i| cell_to_string(r, i)).collect();
            out.push_str(&cells.join(" | "));
            out.push('\n');
        }
        out.push_str(&format!("({} 行)", rows.len()));
        drop(rows);
        drop(conn);
        pool.close().await;
        Ok(SqlOutcome { ok: true, output: out, affected: None })
    } else {
        let res = sqlx::query(q).execute(&mut *conn).await?;
        let aff = res.rows_affected() as i64;
        drop(conn);
        pool.close().await;
        Ok(SqlOutcome { ok: true, output: format!("影响 {aff} 行"), affected: Some(aff) })
    }
}
