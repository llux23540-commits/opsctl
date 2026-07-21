mod common;
use common::spawn;
use serde_json::json;

#[tokio::test]
async fn sql_returns_seeded_rows_and_audits() {
    let app = spawn().await;
    let admin = app.admin().await;
    let (s, v) = app.post("/jobs/sql", &admin, "admin-dev", json!({"targets":["db-demo"],"query":"SELECT * FROM servers"})).await;
    assert_eq!(s, 200);
    let out = v["results"][0]["stdout"].as_str().unwrap();
    assert!(out.contains("web-01") && out.contains("web-02"), "sql output: {out}");

    // audit recorded a sql.exec
    let (_s, audit) = app.get("/audit?action=sql.exec", &admin, "admin-dev").await;
    assert!(audit.as_array().unwrap().iter().all(|r| r["action"] == "sql.exec"));
    assert!(!audit.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn approval_gate_holds_and_notifies() {
    let app = spawn().await;
    let admin = app.admin().await;

    // turn on approval for the operator ssh rule
    let (_s, rules) = app.get("/rules", &admin, "admin-dev").await;
    let rule = rules.as_array().unwrap().iter().find(|r| r["id"] == "rule-op-web").unwrap().clone();
    let (s, _) = app.put("/rules/rule-op-web", &admin, "admin-dev", json!({
        "name":"op web","subject_user_id": rule["subject_user_id"],
        "selector_kind":"tag","selector":"tag-web","system_user_id":"su-webssh",
        "actions":["ssh"],"needs_approval":true
    })).await;
    assert_eq!(s, 200);

    let admin_unread_before = app.get("/messages/unread-count", &admin, "admin-dev").await.1["count"].as_i64().unwrap();

    // operator submits → held pending, not executed
    let op = app.operator().await;
    let (_s, v) = app.post("/jobs/ssh", &op, "op-dev", json!({"targets":["web-01"],"command":"uptime"})).await;
    assert_eq!(v["results"][0]["pending"], true);
    assert!(v["results"][0]["approval_id"].is_string());

    // an ssh.request/pending audit row exists
    let (_s, audit) = app.get("/audit?action=ssh.request", &admin, "admin-dev").await;
    assert!(!audit.as_array().unwrap().is_empty());

    // admin got an approval notification
    let admin_unread_after = app.get("/messages/unread-count", &admin, "admin-dev").await.1["count"].as_i64().unwrap();
    assert!(admin_unread_after > admin_unread_before, "admin should be notified of pending approval");
}

#[tokio::test]
async fn job_history_aggregates_and_scopes_to_owner() {
    let app = spawn().await;
    let op = app.operator().await;

    // operator runs a sql job → aggregated job row appears
    let (s, v) = app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT * FROM servers"})).await;
    assert_eq!(s, 200);
    let job_id = v["job_id"].as_str().unwrap().to_string();

    let (s, jobs) = app.get("/jobs", &op, "op-dev").await;
    assert_eq!(s, 200);
    let job = jobs.as_array().unwrap().iter().find(|j| j["id"] == job_id.as_str()).expect("own job listed");
    assert_eq!(job["status"], "ok");
    assert_eq!(job["total"], 1);
    assert_eq!(job["ok_count"], 1);
    assert_eq!(job["kind"], "sql");

    // detail: per-target outcome carries the output
    let (s, detail) = app.get(&format!("/jobs/{job_id}"), &op, "op-dev").await;
    assert_eq!(s, 200);
    assert_eq!(detail["job"]["id"], job_id.as_str());
    let t = &detail["targets"][0];
    assert_eq!(t["status"], "ok");
    assert!(t["stdout"].as_str().unwrap().contains("web-01"));

    // an admin's job is invisible to the operator (list + detail)
    let admin = app.admin().await;
    let (_s, av) = app.post("/jobs/sql", &admin, "admin-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    let admin_job = av["job_id"].as_str().unwrap();
    let (_s, jobs) = app.get("/jobs", &op, "op-dev").await;
    assert!(jobs.as_array().unwrap().iter().all(|j| j["id"] != admin_job), "operator must not see admin jobs");
    let (s, _) = app.get(&format!("/jobs/{admin_job}"), &op, "op-dev").await;
    assert_eq!(s, 403);
    // admin sees both
    let (_s, jobs) = app.get("/jobs", &admin, "admin-dev").await;
    assert!(jobs.as_array().unwrap().iter().any(|j| j["id"] == job_id.as_str()));
}

#[tokio::test]
async fn rejected_approval_finalizes_job() {
    let app = spawn().await;
    let admin = app.admin().await;

    // gate the operator's ssh rule behind approval
    let (_s, rules) = app.get("/rules", &admin, "admin-dev").await;
    let rule = rules.as_array().unwrap().iter().find(|r| r["id"] == "rule-op-web").unwrap().clone();
    app.put("/rules/rule-op-web", &admin, "admin-dev", json!({
        "name":"op web","subject_user_id": rule["subject_user_id"],
        "selector_kind":"tag","selector":"tag-web","system_user_id":"su-webssh",
        "actions":["ssh"],"needs_approval":true
    })).await;

    let op = app.operator().await;
    let (_s, v) = app.post("/jobs/ssh", &op, "op-dev", json!({"targets":["web-01"],"command":"uptime"})).await;
    let job_id = v["job_id"].as_str().unwrap().to_string();
    let approval_id = v["results"][0]["approval_id"].as_str().unwrap().to_string();

    // held job is pending
    let (_s, detail) = app.get(&format!("/jobs/{job_id}"), &op, "op-dev").await;
    assert_eq!(detail["job"]["status"], "pending");
    assert_eq!(detail["targets"][0]["status"], "pending");

    // admin rejects → target rejected, job finalized as fail, trail exposed
    let (s, _) = app.post(&format!("/approvals/{approval_id}/decide"), &admin, "admin-dev",
        json!({"verdict":"reject","reason":"高危命令"})).await;
    assert_eq!(s, 200);
    let (_s, detail) = app.get(&format!("/jobs/{job_id}"), &op, "op-dev").await;
    assert_eq!(detail["job"]["status"], "fail");
    assert_eq!(detail["targets"][0]["status"], "rejected");
    assert_eq!(detail["approvals"][0]["state"], "rejected");
    assert_eq!(detail["approvals"][0]["votes"][0]["verdict"], "reject");
}

#[tokio::test]
async fn job_records_source_ip_and_device() {
    let app = spawn().await;
    let op = app.operator().await;
    let (s, v) = app.post("/jobs/sql", &op, "op-dev", json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    assert_eq!(s, 200);
    let job_id = v["job_id"].as_str().unwrap().to_string();
    let (s, d) = app.get(&format!("/jobs/{job_id}"), &op, "op-dev").await;
    assert_eq!(s, 200);
    assert_eq!(d["job"]["source_ip"], "127.0.0.1");
    assert_eq!(d["job"]["source_device"], "op-dev");
}

#[tokio::test]
async fn job_records_template_provenance() {
    let app = spawn().await;
    let admin = app.admin().await;

    // with a known template id → name recorded
    let (s, v) = app.post("/jobs/sql", &admin, "admin-dev",
        json!({"targets":["db-demo"],"query":"SELECT count(*) FROM servers","template_id":"tpl-count"})).await;
    assert_eq!(s, 200);
    let job_id = v["job_id"].as_str().unwrap().to_string();
    let (_s, d) = app.get(&format!("/jobs/{job_id}"), &admin, "admin-dev").await;
    assert_eq!(d["job"]["template_id"], "tpl-count");
    assert_eq!(d["job"]["template_name"], "count");

    // unknown template id → ignored, still 200
    let (s, v) = app.post("/jobs/sql", &admin, "admin-dev",
        json!({"targets":["db-demo"],"query":"SELECT 1","template_id":"nope"})).await;
    assert_eq!(s, 200);
    let job_id = v["job_id"].as_str().unwrap().to_string();
    let (_s, d) = app.get(&format!("/jobs/{job_id}"), &admin, "admin-dev").await;
    assert_eq!(d["job"]["template_name"], "");

    // omitted field → backward compatible
    let (s, _v) = app.post("/jobs/sql", &admin, "admin-dev",
        json!({"targets":["db-demo"],"query":"SELECT 1"})).await;
    assert_eq!(s, 200);

    // list rows carry template_name for the table column
    let (_s, list) = app.get("/jobs", &admin, "admin-dev").await;
    assert!(list.as_array().unwrap().iter().any(|j| j["template_name"] == "count"));
}
