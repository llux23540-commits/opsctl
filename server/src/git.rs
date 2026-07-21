//! Real Git sync: commit the exported config (JSON) into a working repo using
//! the local `git` binary; push in remote mode. Detects git, and can trigger a
//! platform install when it's missing.

use std::path::Path;

use anyhow::{anyhow, Result};
use tokio::process::Command;

/// `git --version` → Some(version line) if git is on PATH, else None.
pub async fn version() -> Option<String> {
    let out = Command::new("git").arg("--version").output().await.ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// Best-effort platform install of git. Returns a summary; errors carry a manual
/// hint when no package manager is available.
pub async fn install() -> Result<String> {
    if cfg!(target_os = "windows") {
        if which("winget").await {
            let out = Command::new("winget")
                .args([
                    "install", "--id", "Git.Git", "-e", "--source", "winget",
                    "--accept-package-agreements", "--accept-source-agreements",
                ])
                .output()
                .await?;
            return Ok(fmt_out("winget install Git.Git", &out));
        }
        if which("choco").await {
            let out = Command::new("choco").args(["install", "git", "-y"]).output().await?;
            return Ok(fmt_out("choco install git", &out));
        }
        Err(anyhow!("未找到 winget/choco,请手动安装 git:https://git-scm.com/download/win"))
    } else {
        // Linux/macOS: try apt, then apk, then brew.
        if which("apt-get").await {
            let out = Command::new("sh").arg("-c").arg("apt-get update && apt-get install -y git").output().await?;
            return Ok(fmt_out("apt-get install git", &out));
        }
        if which("apk").await {
            let out = Command::new("apk").args(["add", "git"]).output().await?;
            return Ok(fmt_out("apk add git", &out));
        }
        if which("brew").await {
            let out = Command::new("brew").args(["install", "git"]).output().await?;
            return Ok(fmt_out("brew install git", &out));
        }
        Err(anyhow!("未找到包管理器,请手动安装 git"))
    }
}

async fn which(cmd: &str) -> bool {
    let probe = if cfg!(target_os = "windows") { "where" } else { "which" };
    Command::new(probe).arg(cmd).output().await.map(|o| o.status.success()).unwrap_or(false)
}

fn fmt_out(label: &str, out: &std::process::Output) -> String {
    let tail = |b: &[u8]| {
        let s = String::from_utf8_lossy(b);
        s.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ")
    };
    if out.status.success() {
        format!("{label} 成功。{}", tail(&out.stdout))
    } else {
        format!("{label} 失败:{} {}", tail(&out.stdout), tail(&out.stderr))
    }
}

/// Build an authenticated https URL by injecting `username:credential@`.
/// For ssh urls (git@… / ssh://) the credential is a deploy key, not a password,
/// so we leave the url untouched. Empty username/credential → url unchanged.
pub fn authed_url(url: &str, username: &str, credential: &str) -> String {
    if username.is_empty() && credential.is_empty() {
        return url.to_string();
    }
    if let Some(rest) = url.strip_prefix("https://") {
        // strip any existing creds in the url
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let user = if username.is_empty() { "git" } else { username };
        let enc = |s: &str| s.replace('%', "%25").replace(':', "%3A").replace('@', "%40").replace('/', "%2F");
        return format!("https://{}:{}@{}", enc(user), enc(credential), rest);
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let user = if username.is_empty() { "git" } else { username };
        return format!("http://{user}:{credential}@{rest}");
    }
    url.to_string() // ssh/other: creds handled out-of-band (deploy key)
}

/// Redact credentials from a message for safe display/logging.
fn redact(s: &str, credential: &str) -> String {
    if credential.is_empty() { s.to_string() } else { s.replace(credential, "***") }
}

/// Run a git subcommand in `work_dir` with a fixed commit identity (no global config).
async fn git(work_dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    let out = Command::new("git")
        .arg("-C").arg(work_dir)
        .args(["-c", "user.name=opsctl", "-c", "user.email=opsctl@local"])
        .args(args)
        .output()
        .await?;
    Ok(out)
}

fn ok_or_err(label: &str, out: &std::process::Output) -> Result<String> {
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(anyhow!("git {label} 失败:{}", String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// Git config resolved from the stored settings JSON.
#[derive(Clone, Default)]
pub struct GitCfg {
    pub mode: String,       // folder | local | remote
    pub url: String,
    pub branch: String,
    pub username: String,
    pub credential: String, // password/token (https) or unused (ssh)
    pub auto_push: bool,
}
impl GitCfg {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            mode: { let m = s("mode"); if m.is_empty() { "folder".into() } else { m } },
            url: s("url"),
            branch: { let b = s("branch"); if b.is_empty() { "main".into() } else { b } },
            username: s("username"),
            credential: s("credential"),
            auto_push: v.get("auto_push").and_then(|x| x.as_bool()).unwrap_or(false),
        }
    }
    fn remote(&self) -> String {
        authed_url(&self.url, &self.username, &self.credential)
    }
}

async fn ensure_repo(cfg: &GitCfg, work_dir: &Path) -> Result<()> {
    if version().await.is_none() {
        return Err(anyhow!("未检测到 git,请先安装"));
    }
    tokio::fs::create_dir_all(work_dir).await?;
    if !work_dir.join(".git").exists() {
        ok_or_err("init", &git(work_dir, &["init", "-b", &cfg.branch]).await?)?;
    }
    // (re)point origin at the (authed) configured url for remote mode
    if cfg.mode == "remote" && !cfg.url.is_empty() {
        let _ = git(work_dir, &["remote", "remove", "origin"]).await;
        let _ = git(work_dir, &["remote", "add", "origin", &cfg.remote()]).await;
    }
    Ok(())
}

pub struct SyncResult {
    pub committed: bool,
    pub commit: Option<String>,
    pub note: String,
}

/// Export files into `work_dir`, commit, and (remote + auto_push) push.
pub async fn sync(cfg: &GitCfg, work_dir: &Path, files: &[(String, String)]) -> Result<SyncResult> {
    ensure_repo(cfg, work_dir).await?;
    for (name, content) in files {
        let path = work_dir.join(name);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content).await?;
    }
    ok_or_err("add", &git(work_dir, &["add", "-A"]).await?)?;
    let status = git(work_dir, &["status", "--porcelain"]).await?;
    if String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        return Ok(SyncResult { committed: false, commit: None, note: "无变更,未提交".into() });
    }
    let msg = format!("opsctl sync ({} files)", files.len());
    ok_or_err("commit", &git(work_dir, &["commit", "-m", &msg]).await?)?;
    let hash = ok_or_err("rev-parse", &git(work_dir, &["rev-parse", "--short", "HEAD"]).await?)?;

    let mut note = format!("已提交 {hash}");
    if cfg.mode == "remote" && !cfg.url.is_empty() && cfg.auto_push {
        match push(cfg, work_dir).await {
            Ok(m) => note.push_str(&format!(" · {m}")),
            Err(e) => note.push_str(&format!(" · 推送失败:{}", redact(&e.to_string(), &cfg.credential))),
        }
    }
    Ok(SyncResult { committed: true, commit: Some(hash), note })
}

/// Push the current branch to origin.
pub async fn push(cfg: &GitCfg, work_dir: &Path) -> Result<String> {
    ensure_repo(cfg, work_dir).await?;
    if cfg.mode != "remote" || cfg.url.is_empty() {
        return Err(anyhow!("非远程模式或未配置仓库地址"));
    }
    let out = git(work_dir, &["push", "-u", "origin", &cfg.branch]).await?;
    if out.status.success() {
        Ok("已推送远程".into())
    } else {
        Err(anyhow!("{}", redact(&String::from_utf8_lossy(&out.stderr), &cfg.credential)))
    }
}

/// Pull (fetch + merge) from origin into the working repo.
pub async fn pull(cfg: &GitCfg, work_dir: &Path) -> Result<String> {
    ensure_repo(cfg, work_dir).await?;
    if cfg.mode != "remote" || cfg.url.is_empty() {
        return Err(anyhow!("非远程模式或未配置仓库地址"));
    }
    let out = git(work_dir, &["pull", "--no-rebase", "origin", &cfg.branch]).await?;
    if out.status.success() {
        Ok(redact(String::from_utf8_lossy(&out.stdout).trim(), &cfg.credential))
    } else {
        Err(anyhow!("{}", redact(&String::from_utf8_lossy(&out.stderr), &cfg.credential)))
    }
}

/// Connectivity/availability test.
pub async fn test(cfg: &GitCfg) -> Result<String> {
    let v = version().await.ok_or_else(|| anyhow!("未检测到 git"))?;
    if cfg.mode == "remote" && !cfg.url.is_empty() {
        let out = Command::new("git").args(["ls-remote", &cfg.remote()]).output().await?;
        if out.status.success() {
            return Ok(format!("{v} · 远程可达"));
        }
        return Err(anyhow!("远程不可达:{}", redact(&String::from_utf8_lossy(&out.stderr), &cfg.credential)));
    }
    Ok(format!("{v} · 本地可用"))
}

/// Short log of the working repo (for status display).
pub async fn last_commit(work_dir: &Path) -> Option<String> {
    if !work_dir.join(".git").exists() {
        return None;
    }
    let out = git(work_dir, &["log", "-1", "--format=%h %ci %s"]).await.ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    } else {
        None
    }
}
