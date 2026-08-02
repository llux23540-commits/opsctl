//! WebSocket 管理面(REST 侧契约):在线可见性按角色收窄、广播的权限/校验/
//! 站内信落地、陈旧在线行过滤。
//!
//! socket 升级、hello 帧、投递与撤销踢线走真实浏览器/Node 客户端冒烟验证
//! (浏览器 WS 不能带自定义头,reqwest 也没有 ws 客户端,这里不重复)。

mod common;

use common::spawn;
use opsctl_server::store::{Store, WsPresenceRow};
use serde_json::json;

/// 直接往该实例的 DB 里塞一条在线行(等价于别的节点注册的连接 —— 在线表
/// 本来就是跨节点共享的)。
async fn seed_presence(db_url: &str, conn_id: &str, user_id: &str, email: &str, last_seen: i64) {
    let store = Store::connect(db_url).await.unwrap();
    store
        .upsert_ws_presence(&WsPresenceRow {
            conn_id: conn_id.into(),
            node_id: "test-node".into(),
            user_id: user_id.into(),
            email: email.into(),
            role: "viewer".into(),
            device_id: "dev-x".into(),
            client: "web".into(),
            ip: "127.0.0.1".into(),
            connected_at: last_seen,
            last_seen,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn online_list_scoped_by_role_and_freshness() {
    let app = spawn().await;
    let admin = app.admin().await;
    let viewer = app.login("viewer", "viewer", "viewer-dev").await;
    let now = opsctl_server::state::now_secs();

    // 两条新鲜在线行(admin + viewer)+ 一条早已断心跳的陈旧行
    seed_presence(&app.db_url, "n1:1", "u-admin", "admin@local", now).await;
    seed_presence(&app.db_url, "n1:2", "u-viewer", "viewer@local", now).await;
    seed_presence(&app.db_url, "n1:3", "u-viewer", "viewer@local", now - 3600).await;

    // 管理员:看到全部新鲜连接(跨节点),陈旧行按下线过滤
    let (s, v) = app.get("/ws/online", &admin, "admin-dev").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["total"], 2, "{v}");

    // 普通用户:只能看到自己账号的连接
    let (s, v) = app.get("/ws/online", &viewer, "viewer-dev").await;
    assert_eq!(s, 200);
    assert_eq!(v["total"], 1, "{v}");
    assert_eq!(v["items"][0]["user_id"], "u-viewer");
    assert_eq!(v["items"][0]["device_id"], "dev-x", "登录设备信息必须带出");
}

#[tokio::test]
async fn broadcast_admin_only_validated_and_lands_in_messages() {
    let app = spawn().await;
    let admin = app.admin().await;
    let operator = app.operator().await;

    // 非管理员不能发集群消息
    let (s, _) = app
        .post("/ws/broadcast", &operator, "op-dev", json!({ "title": "x", "body": "y" }))
        .await;
    assert_eq!(s, 403);

    // 空消息被挡
    let (s, _) = app
        .post("/ws/broadcast", &admin, "admin-dev", json!({ "title": "  ", "body": "" }))
        .await;
    assert_eq!(s, 400);

    // 全员广播:落 ws_messages(seq 单调)+ 每人一条站内信
    let (s, v) = app
        .post(
            "/ws/broadcast",
            &admin,
            "admin-dev",
            json!({ "title": "停机通知", "body": "今晚 23:00 升级" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["notified"], 4, "fixture 共 4 个用户,离线也要能在消息中心看到:{v}");
    let seq1 = v["seq"].as_i64().unwrap();

    let (_, msgs) = app.get("/messages", &operator, "op-dev").await;
    assert!(msgs.to_string().contains("停机通知"), "{msgs}");

    // 定向消息:只进目标用户的消息中心
    let (s, v) = app
        .post(
            "/ws/broadcast",
            &admin,
            "admin-dev",
            json!({ "title": "单发", "body": "只给 viewer", "user_id": "u-viewer" }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["notified"], 1);
    assert!(v["seq"].as_i64().unwrap() > seq1, "消息游标必须单调递增");

    let viewer = app.login("viewer", "viewer", "viewer-dev").await;
    let (_, vm) = app.get("/messages", &viewer, "viewer-dev").await;
    assert!(vm.to_string().contains("单发"), "{vm}");
    let (_, om) = app.get("/messages", &operator, "op-dev").await;
    assert!(!om.to_string().contains("单发"), "定向消息不该进别人的消息中心:{om}");
}
