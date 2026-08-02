//! WebSocket 实时通道:在线登记(区分 web/桌面客户端 + 登录设备)、会话撤销即踢、
//! 管理员集群广播。
//!
//! 多实例部署(k8s 副本)下连接会「漂移」到任意节点,所以在线表和消息都落 SQLite,
//! 各节点用消息游标轮询投递、按节拍心跳自己的在线行 —— DB 即总线,不引入
//! Redis/NATS(与本项目 SQLite 单文件的部署形态一致)。单实例时同一条路径退化为
//! 节拍内(≤2s)的本地投递。节点崩溃不清表:超过窗口没心跳的在线行按下线处理。
//!
//! 可见性:管理员看全量在线(跨节点);普通用户只能看到自己账号的连接。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{decode, DecodingKey, Validation};
use opsctl_core::api::Claims;
use opsctl_core::model::Role;
use serde_json::{json, Value};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::auth::{AuthUser, ClientIp};
use crate::error::AppError;
use crate::state::{now_secs, AppState};
use crate::store::WsPresenceRow;

/// 分发节拍(消息投递 / 心跳 / 撤销检查)。
const TICK_SECS: u64 = 2;
/// 超过这个窗口没心跳的在线行视为死连接(节点崩溃、断网)。
const STALE_AFTER_SECS: i64 = 30;

/// 本节点的连接注册表。跨节点的真相在 `ws_presence` 表里,这里只管
/// 「帧要写给哪些本地 socket」。
pub struct WsHub {
    pub node_id: String,
    next: AtomicU64,
    conns: RwLock<HashMap<String, Conn>>,
}

/// 每连接发送队列上限。WS 是尽力而为的实时层(可靠层是站内信),客户端卡死
/// 时队列塞满即踢,内存永远有界。
const SEND_QUEUE: usize = 64;

#[derive(Clone)]
struct Conn {
    user_id: String,
    sid: String,
    tx: mpsc::Sender<String>,
}

impl Default for WsHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WsHub {
    pub fn new() -> Self {
        Self {
            // 节点身份跟进程走:重启换新 id,旧在线行由心跳窗口自然过期。
            node_id: Uuid::new_v4().simple().to_string(),
            next: AtomicU64::new(1),
            conns: RwLock::new(HashMap::new()),
        }
    }

    fn conn_id(&self) -> String {
        format!("{}:{}", self.node_id, self.next.fetch_add(1, Ordering::Relaxed))
    }

    async fn register(&self, id: String, conn: Conn) {
        self.conns.write().await.insert(id, conn);
    }

    async fn unregister(&self, id: &str) {
        self.conns.write().await.remove(id);
    }

    /// `target_user` 为空 = 全员;否则只投给该用户的连接(可能多设备)。
    /// 返回队列已满的「卡死」连接,由调用方(分发器)统一踢线。
    async fn deliver(&self, target_user: &str, frame: &str) -> Vec<String> {
        let mut stuck = Vec::new();
        for (id, c) in self.conns.read().await.iter() {
            if target_user.is_empty() || c.user_id == target_user {
                if let Err(mpsc::error::TrySendError::Full(_)) = c.tx.try_send(frame.to_string()) {
                    stuck.push(id.clone());
                }
            }
        }
        stuck
    }

    async fn snapshot(&self) -> Vec<(String, Conn)> {
        self.conns.read().await.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::Admin => "admin",
        Role::Operator => "operator",
        Role::Viewer => "viewer",
    }
}

#[derive(serde::Deserialize)]
pub struct WsQuery {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub did: String,
    /// 客户端类型标注:web | desktop(其余归为 other)。
    #[serde(default)]
    pub client: String,
}

/// `GET /api/ws?token=&did=&client=web` — WebSocket 升级。
///
/// 浏览器的 WebSocket API 不能带自定义请求头,鉴权参数改走 query;
/// 校验逻辑与 `AuthUser` 完全一致:JWT + did 设备绑定 + 会话未撤销。
pub async fn ws_upgrade(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, AppError> {
    let data = decode::<Claims>(
        &q.token,
        &DecodingKey::from_secret(st.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::Unauthorized)?;
    let claims = data.claims;
    if q.did.is_empty() || claims.did != q.did {
        return Err(AppError::Unauthorized);
    }
    let sess = st
        .store
        .get_session(&claims.sid)
        .await
        .map_err(AppError::Internal)?
        .ok_or(AppError::Unauthorized)?;
    if sess.revoked != 0 || sess.user_id != claims.sub || sess.device_id != claims.did {
        return Err(AppError::Unauthorized);
    }
    let email = st
        .store
        .get_user_by_id(&claims.sub)
        .await
        .ok()
        .flatten()
        .map(|u| u.email)
        .unwrap_or_default();

    let client = match q.client.as_str() {
        "" | "web" => "web".to_string(),
        "desktop" => "desktop".to_string(),
        _ => "other".to_string(),
    };
    let row = WsPresenceRow {
        conn_id: String::new(), // serve_socket 里分配
        node_id: st.ws.node_id.clone(),
        user_id: claims.sub,
        email,
        role: role_str(claims.role).to_string(),
        device_id: claims.did,
        client,
        ip,
        connected_at: 0,
        last_seen: 0,
    };
    let sid = claims.sid;
    Ok(ws.on_upgrade(move |socket| serve_socket(st, socket, row, sid)))
}

async fn serve_socket(st: AppState, socket: WebSocket, mut row: WsPresenceRow, sid: String) {
    let conn_id = st.ws.conn_id();
    let now = now_secs();
    row.conn_id = conn_id.clone();
    row.connected_at = now;
    row.last_seen = now;

    let (tx, mut rx) = mpsc::channel::<String>(SEND_QUEUE);
    st.ws
        .register(
            conn_id.clone(),
            Conn { user_id: row.user_id.clone(), sid, tx: tx.clone() },
        )
        .await;
    let _ = st.store.upsert_ws_presence(&row).await;

    // hello 只带连接自己的账号信息 —— 普通用户在这条通道上看不到别人。
    let _ = tx.try_send(
        json!({
            "type": "hello",
            "conn_id": conn_id,
            "node": st.ws.node_id,
            "user": {
                "user_id": row.user_id, "email": row.email, "role": row.role,
                "device_id": row.device_id, "client": row.client,
            },
        })
        .to_string(),
    );
    // 此后 hub 里的 clone 是唯一 sender:被踢(unregister)即 rx 收到 None,
    // 循环退出、socket 关闭 —— 否则本地 tx 会让被撤销的连接变僵尸。
    drop(tx);

    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                // 队列被分发器关闭(会话撤销被踢)或 hub 移除 → 结束
                None => break,
                Some(text) => {
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
            },
            inbound = stream.next() => match inbound {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Ping(p))) => { let _ = sink.send(Message::Pong(p)).await; }
                // 单向推送通道:客户端上行帧暂无语义,忽略。
                Some(Ok(_)) => {}
            },
        }
    }

    st.ws.unregister(&conn_id).await;
    let _ = st.store.delete_ws_presence(&conn_id).await;
}

/// 每个节点跑一个:按节拍投递新集群消息、心跳本节点在线行、
/// 踢掉会话已撤销的连接、清理其它节点崩溃留下的陈旧行。
///
/// 每节拍的 DB 开销与连接数解耦:1 次消息游标读 + 1 次节点心跳写 +
/// ceil(N/400) 次撤销批查 + 1 次陈旧清理 —— 连接数只花内存,不放大 DB 压力。
pub async fn run_dispatcher(st: AppState) {
    // 游标从当前最大 seq 开始:节点重启不重放历史消息。
    let mut cursor = st.store.ws_message_max_seq().await.unwrap_or(0);
    loop {
        tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;
        let now = now_secs();

        // 1) 新集群消息 → 投给本节点匹配的连接;队列塞满的(卡死客户端)记下来踢
        let mut kick: Vec<String> = Vec::new();
        if let Ok(msgs) = st.store.ws_messages_after(cursor, 200).await {
            for m in msgs {
                cursor = m.seq;
                let frame = json!({
                    "type": m.kind, "title": m.title, "body": m.body,
                    "from": m.sender_email, "ts": m.ts,
                })
                .to_string();
                kick.extend(st.ws.deliver(&m.target_user_id, &frame).await);
            }
        }

        // 2) 节点心跳:一条 UPDATE 覆盖本节点全部连接
        let _ = st.store.touch_ws_presence_node(&st.ws.node_id, now).await;

        // 3) 会话撤销即踢(「设置 → 会话」撤销即时生效,WS 不能例外):批量查
        let conns = st.ws.snapshot().await;
        if !conns.is_empty() {
            let sids: Vec<String> = conns.iter().map(|(_, c)| c.sid.clone()).collect();
            if let Ok(revoked) = st.store.revoked_among(&sids).await {
                if !revoked.is_empty() {
                    let revoked: std::collections::HashSet<_> = revoked.into_iter().collect();
                    for (conn_id, conn) in &conns {
                        if revoked.contains(&conn.sid) {
                            let _ = conn.tx.try_send(
                                json!({ "type": "bye", "reason": "session revoked" }).to_string(),
                            );
                            kick.push(conn_id.clone());
                        }
                    }
                }
            }
        }

        // 4) 统一踢线:从 hub 移除即丢弃 sender,socket 循环退出并自清理;
        //    在线行这里同步删掉,管理页不用等 socket 收尾。
        for conn_id in kick {
            st.ws.unregister(&conn_id).await;
            let _ = st.store.delete_ws_presence(&conn_id).await;
        }

        // 5) 陈旧在线行(别的节点崩溃/断电,没机会删自己的行)
        let _ = st.store.purge_stale_ws_presence(now - STALE_AFTER_SECS).await;
    }
}

/// `GET /api/ws/online` — 在线连接一览(跨节点,来自 ws_presence 表)。
/// 管理员看全部;普通用户只能看到自己账号的连接。
pub async fn online(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let fresh_after = now_secs() - STALE_AFTER_SECS;
    let rows = if user.role == Role::Admin {
        st.store.list_ws_presence(fresh_after).await
    } else {
        st.store.list_ws_presence_for(&user.user_id, fresh_after).await
    }
    .map_err(AppError::Internal)?;
    Ok(Json(json!({ "total": rows.len(), "items": rows, "node": st.ws.node_id })))
}

#[derive(serde::Deserialize)]
pub struct BroadcastReq {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// 留空 = 全员广播;指定则只发给该用户(其所有设备)。
    #[serde(default)]
    pub user_id: String,
}

/// `POST /api/ws/broadcast` — 管理员集群消息。
///
/// 消息落 `ws_messages`,由**每个节点**的分发器投给自己的在线连接 ——
/// 漂移到哪个节点都能收到;同时写站内信,离线用户上线后在消息中心可见。
pub async fn broadcast(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<BroadcastReq>,
) -> Result<Json<Value>, AppError> {
    if user.role != Role::Admin {
        return Err(AppError::Forbidden);
    }
    let (title, body) = (req.title.trim(), req.body.trim());
    if title.is_empty() && body.is_empty() {
        return Err(AppError::BadRequest("消息不能为空".into()));
    }
    let ts = now_secs();
    let seq = st
        .store
        .insert_ws_message(&req.user_id, "broadcast", title, body, &user.email, ts)
        .await
        .map_err(AppError::Internal)?;

    // 站内信兜底:不在线也能看到。全员用单条 INSERT…SELECT,
    // 万人也不会把接口拖成逐条 await 的秒级慢请求。
    let notified = if req.user_id.is_empty() {
        st.store
            .push_notification_all("broadcast", title, body, "/messages", ts)
            .await
            .map_err(AppError::Internal)?
    } else {
        st.store
            .push_notification(&req.user_id, "broadcast", title, body, "/messages", ts)
            .await
            .map_err(AppError::Internal)?;
        1
    };

    let _ = st
        .store
        .insert_audit(
            &Uuid::new_v4().to_string(),
            ts,
            &user.user_id,
            &user.email,
            "ws_broadcast",
            if req.user_id.is_empty() { "all" } else { &req.user_id },
            &json!({ "title": title, "seq": seq, "targets": notified }).to_string(),
            "ok",
            "",
        )
        .await;

    Ok(Json(json!({ "ok": true, "seq": seq, "notified": notified })))
}
