mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn status_and_sync_commits_config() {
    let app = spawn().await;
    let admin = app.admin().await;

    // status: local git detected
    let (s, cfg) = app.get("/settings/git", &admin, "admin-dev").await;
    assert_eq!(s, 200);
    assert_eq!(cfg["git_installed"], true);
    assert!(cfg["git_version"].as_str().unwrap().contains("git version"));

    // point the working repo at a unique temp dir, folder mode
    let wd = format!("target/test-dbs/gitrepo-{}", uuid::Uuid::new_v4().simple());
    let (s, _) = app.put("/settings/git", &admin, "admin-dev",
        json!({"mode":"folder","branch":"main","work_dir": wd})).await;
    assert_eq!(s, 200);

    // sync → real commit
    let (s, r) = app.post("/settings/git/sync", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(r["committed"], true);
    assert!(r["commit"].as_str().unwrap().len() >= 4);

    // exported files exist on disk
    let dir = std::path::Path::new(&wd);
    assert!(dir.join(".git").exists());
    assert!(dir.join("accounts.json").exists());
    assert!(dir.join("rules.json").exists());
    assert!(dir.join("users.json").exists());
    // accounts export carries the encrypted secret, never plaintext
    let accounts = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
    assert!(accounts.contains("secret_enc"));

    // part 1 — SSH config: one ENCRYPTED file per server node
    let ssh1 = std::fs::read_to_string(dir.join("ssh").join("web-01.md")).unwrap();
    assert!(ssh1.contains("opsctl-node: web-01"), "node identity in frontmatter");
    assert!(ssh1.contains("v1:") && !ssh1.contains("root") && !ssh1.contains("127.0.0.1"),
        "ssh node config must be encrypted, got: {ssh1}");
    assert!(dir.join("ssh").join("web-02.md").exists());

    // part 2 — templates: RAW (unencrypted) .md; kind is the frontmatter marker
    assert!(!dir.join("templates.json").exists());
    let md = std::fs::read_to_string(dir.join("templates").join("restart.md")).unwrap();
    assert!(md.contains("kind: ssh"), "frontmatter carries the type marker");
    assert!(md.contains("systemctl restart") && !md.contains("v1:"),
        "templates are raw (not encrypted), got: {md}");
    assert!(dir.join("templates").join("count.md").exists());

    // second sync with no data change → nothing to commit
    let (_s, r2) = app.post("/settings/git/sync", &admin, "admin-dev", json!({})).await;
    assert_eq!(r2["committed"], false);

    // audit recorded the sync
    let (_s, audit) = app.get("/audit?action=git.sync", &admin, "admin-dev").await;
    assert!(!audit.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn git_actions_are_admin_only() {
    let app = spawn().await;
    let op = app.operator().await;
    // is_admin is checked before any git/install command runs (install never
    // reaches winget for a non-admin)
    for what in ["test", "sync", "push", "pull", "install"] {
        let (s, _) = app.post(&format!("/settings/git/{what}"), &op, "op-dev", json!({})).await;
        assert_eq!(s, 403, "operator must be forbidden on {what}");
    }
}

#[tokio::test]
async fn test_local_mode_ok() {
    let app = spawn().await;
    let admin = app.admin().await;
    app.put("/settings/git", &admin, "admin-dev", json!({"mode":"folder"})).await;
    let (s, r) = app.post("/settings/git/test", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200);
    assert!(r["note"].as_str().unwrap().contains("本地可用"));

    // push/pull are rejected in non-remote mode
    let (s, _) = app.post("/settings/git/push", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 400);
}

/// Remote mode round-trip against a local `file://` bare repo (no network):
/// sync must push the exported config to the remote, and pull must download the
/// remote content into a fresh working directory. This is the "远程 + 下载到工作
/// 目录" flow the UI exposes (拉取 pull) but which nothing exercised end-to-end.
#[tokio::test]
async fn remote_push_pull_round_trip_via_file_bare_repo() {
    // same assumption as the other git tests: git is on PATH
    let git_ok = std::process::Command::new("git").arg("--version").output()
        .map(|o| o.status.success()).unwrap_or(false);
    if !git_ok {
        eprintln!("git not installed; skipping remote round-trip test");
        return;
    }

    let app = spawn().await;
    let admin = app.admin().await;

    let uid = uuid::Uuid::new_v4().simple().to_string();
    let cwd = std::env::current_dir().unwrap();
    std::fs::create_dir_all("target/test-dbs").ok();
    let remote_abs = cwd.join(format!("target/test-dbs/remote-{uid}.git"));
    let work_a = cwd.join(format!("target/test-dbs/wa-{uid}"));
    let work_b = cwd.join(format!("target/test-dbs/wb-{uid}"));

    // a bare repo standing in for the remote
    let out = std::process::Command::new("git")
        .args(["init", "--bare", "-b", "main"]).arg(&remote_abs).output().unwrap();
    assert!(out.status.success(), "git init --bare failed: {}", String::from_utf8_lossy(&out.stderr));

    // cross-platform file:// url (windows: file:///D:/...; unix: file:///home/...)
    let to_url = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
    };
    let remote_url = to_url(&remote_abs);
    let wa = work_a.to_string_lossy().replace('\\', "/");
    let wb = work_b.to_string_lossy().replace('\\', "/");

    // remote mode, work_dir A, auto-push on
    let (s, _) = app.put("/settings/git", &admin, "admin-dev", json!({
        "mode": "remote", "url": remote_url, "branch": "main",
        "username": "", "credential": "", "auto_push": true, "work_dir": wa,
    })).await;
    assert_eq!(s, 200);

    // test → remote reachable (ls-remote on the bare repo)
    let (s, r) = app.post("/settings/git/test", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200, "test resp: {r}");
    assert!(r["note"].as_str().unwrap().contains("远程可达"), "expected 远程可达, got {r}");

    // sync → local commit + auto push to the bare remote
    let (s, r) = app.post("/settings/git/sync", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200, "sync resp: {r}");
    assert_eq!(r["committed"], true, "sync resp: {r}");

    // the bare remote actually received the commit on main
    let log = std::process::Command::new("git")
        .arg(format!("--git-dir={}", remote_abs.to_string_lossy()))
        .args(["log", "-1", "--format=%s", "main"]).output().unwrap();
    assert!(log.status.success(), "remote has no main ref: {}", String::from_utf8_lossy(&log.stderr));
    assert!(String::from_utf8_lossy(&log.stdout).contains("opsctl sync"),
        "remote commit subject: {}", String::from_utf8_lossy(&log.stdout));

    // point at a FRESH empty work_dir B, then pull → downloads remote into B
    let (s, _) = app.put("/settings/git", &admin, "admin-dev", json!({
        "mode": "remote", "url": remote_url, "branch": "main", "auto_push": true, "work_dir": wb,
    })).await;
    assert_eq!(s, 200);
    let (s, r) = app.post("/settings/git/pull", &admin, "admin-dev", json!({})).await;
    assert_eq!(s, 200, "pull resp: {r}");
    assert_eq!(r["ok"], true, "pull resp: {r}");

    // downloaded to work dir: exported config files now exist in B
    assert!(work_b.join("accounts.json").exists(), "pull did not download accounts.json into work_dir B");
    assert!(work_b.join("rules.json").exists(), "pull did not download rules.json into work_dir B");
    assert!(work_b.join("users.json").exists(), "pull did not download users.json into work_dir B");
}
