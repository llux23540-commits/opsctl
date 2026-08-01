//! Nacos 管理:集群注册表(看到所有集群 + 实时节点状态)与配置初始化。
//!
//! 与平台其它模块一致:集群口令进金库加密存储,取用时解封;所有远程写操作
//! (初始化配置)落审计 + 初始化记录,便于追溯。
//!
//! 协议用 Nacos **v1 Open API**(2.x 仍然保留,兼容面最广):
//! - 鉴权 `POST /v1/auth/login` → `accessToken`(未开鉴权插件时 404,按免鉴权处理)
//! - 集群节点 `GET /v2/core/cluster/nodes`,回退 `GET /v1/core/cluster/nodes`
//! - 存活探测 `GET /v1/console/health/readiness`
//! - 配置读/写/列表 `GET|POST /v1/cs/configs`
//!
//! 仅支持 http(未启用 TLS 特性),IPv6 字面量地址暂不支持。

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::Json;
use opsctl_core::api::{
    default_nacos_group, NacosConfigItem, NacosInitRequest, NacosInitResult, NacosItemResult,
    NacosNodeView,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::is_admin;
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::{now_secs, AppState};
use crate::store::{NacosClusterRow, NacosRunRow, NacosTemplateRow};

/// Shared HTTP client (connection pooling); Nacos calls are short and bounded.
static HTTP: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .build()
        .expect("build reqwest client")
});

/// Resolved connection for one cluster (password already decrypted).
pub struct NacosConn {
    /// Normalized base URLs, one per configured server address.
    pub bases: Vec<String>,
    pub namespace: String,
    pub username: String,
    pub password: String,
}

/// `10.0.0.1, http://n2:8848/nacos` + ctx `/nacos`
/// → `["http://10.0.0.1:8848/nacos", "http://n2:8848/nacos"]`.
pub fn base_urls(server_addr: &str, context_path: &str) -> Vec<String> {
    let ctx = {
        let c = context_path.trim().trim_end_matches('/');
        if c.is_empty() {
            String::new()
        } else if c.starts_with('/') {
            c.to_string()
        } else {
            format!("/{c}")
        }
    };
    server_addr
        .split(',')
        .filter_map(|raw| normalize_base(raw, &ctx))
        .collect()
}

fn normalize_base(raw: &str, ctx: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches('/');
    if s.is_empty() {
        return None;
    }
    let (scheme, rest) = match s.split_once("://") {
        Some((sc, r)) => (sc, r),
        None => ("http", s),
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if hostport.is_empty() {
        return None;
    }
    let hostport = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:8848")
    };
    // an address that already carries a path keeps it; otherwise use the context
    let path = if path.is_empty() { ctx } else { path };
    Some(format!("{scheme}://{hostport}{path}"))
}

/// reqwest 的 Display 只给最外层("error sending request for url …"),真正的原因
/// (连接被拒 / DNS / 代理 / TLS)藏在 source 链里。排障必须带上,否则只能猜。
fn why(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let msg = s.to_string();
        if !out.contains(&msg) {
            out.push_str(" ← ");
            out.push_str(&msg);
        }
        cur = s.source();
    }
    out
}

/// Log in when the cluster has credentials. `Ok(None)` = no token needed
/// (no username configured, or the auth plugin is not enabled).
async fn access_token(base: &str, user: &str, pass: &str) -> Result<Option<String>, String> {
    if user.trim().is_empty() {
        return Ok(None);
    }
    let url = format!("{base}/v1/auth/login");
    let resp = HTTP
        .post(&url)
        .form(&[("username", user), ("password", pass)])
        .send()
        .await
        .map_err(|e| format!("登录 Nacos 失败:{}", why(&e)))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None); // 未启用鉴权插件
    }
    if !status.is_success() {
        return Err(format!("Nacos 鉴权失败(HTTP {})", status.as_u16()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("鉴权响应无法解析:{e}"))?;
    match v.get("accessToken").and_then(|t| t.as_str()) {
        Some(t) if !t.is_empty() => Ok(Some(t.to_string())),
        _ => Err("Nacos 鉴权未返回 accessToken".into()),
    }
}

/// Query params shared by every config call (token + tenant).
fn common_params<'a>(token: &'a Option<String>, tenant: &'a str) -> Vec<(&'static str, String)> {
    let mut p = Vec::new();
    if let Some(t) = token {
        p.push(("accessToken", t.clone()));
    }
    if !tenant.is_empty() {
        p.push(("tenant", tenant.to_string()));
    }
    p
}

// ---- 集群视图:节点 + 探活 ----

pub struct Inspection {
    /// v2 | v1 | probe
    pub source: String,
    pub nodes: Vec<NacosNodeView>,
    pub message: String,
}

/// Probe every configured address, then ask the cluster for its authoritative
/// member list. Falls back to the probe result when the cluster API is not
/// reachable, so the UI always shows something truthful.
pub async fn inspect(conn: &NacosConn) -> Inspection {
    let mut probes: Vec<NacosNodeView> = Vec::new();
    for base in &conn.bases {
        probes.push(probe_base(base).await);
    }
    let reachable: Vec<&String> = conn
        .bases
        .iter()
        .zip(probes.iter())
        .filter(|(_, p)| p.ok)
        .map(|(b, _)| b)
        .collect();

    let mut last_err = String::new();
    for base in reachable {
        let token = match access_token(base, &conn.username, &conn.password).await {
            Ok(t) => t,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        for (path, source) in [
            ("/v2/core/cluster/nodes", "v2"),
            ("/v1/core/cluster/nodes", "v1"),
        ] {
            match fetch_nodes(base, path, &token).await {
                Ok(mut nodes) if !nodes.is_empty() => {
                    // carry over measured latency for addresses we probed
                    for n in nodes.iter_mut() {
                        if let Some(p) = probes.iter().find(|p| same_host(&p.address, &n.address)) {
                            n.latency_ms = p.latency_ms;
                        }
                    }
                    return Inspection {
                        source: source.into(),
                        nodes,
                        message: String::new(),
                    };
                }
                Ok(_) => {}
                Err(e) => last_err = e,
            }
        }
    }
    let msg = if probes.iter().any(|p| p.ok) {
        format!(
            "集群节点接口不可用,已降级为地址探活{}",
            if last_err.is_empty() { String::new() } else { format!(":{last_err}") }
        )
    } else {
        "所有配置地址均不可达".to_string()
    };
    Inspection { source: "probe".into(), nodes: probes, message: msg }
}

/// Two addresses point at the same member when their `host:port` match. Nacos
/// reports bare `host:port`; our probes carry the full base URL.
fn same_host(probe_addr: &str, node_addr: &str) -> bool {
    let strip = |s: &str| {
        let s = s.split("://").last().unwrap_or(s);
        s.split('/').next().unwrap_or(s).to_string()
    };
    strip(probe_addr) == strip(node_addr)
}

async fn probe_base(base: &str) -> NacosNodeView {
    let started = Instant::now();
    let url = format!("{base}/v1/console/health/readiness");
    match HTTP.get(&url).send().await {
        // any HTTP answer means the port is serving Nacos (404 = older build)
        Ok(r) => {
            let code = r.status().as_u16();
            NacosNodeView {
                address: base.to_string(),
                state: if r.status().is_success() { "UP".into() } else { "SUSPICIOUS".into() },
                version: String::new(),
                ok: r.status().is_success(),
                latency_ms: started.elapsed().as_millis() as i64,
                message: if r.status().is_success() {
                    "readiness 探活通过".into()
                } else {
                    format!("readiness 返回 HTTP {code}")
                },
            }
        }
        Err(e) => NacosNodeView {
            address: base.to_string(),
            state: "unreachable".into(),
            version: String::new(),
            ok: false,
            latency_ms: started.elapsed().as_millis() as i64,
            message: format!("不可达:{}", why(&e)),
        },
    }
}

async fn fetch_nodes(
    base: &str,
    path: &str,
    token: &Option<String>,
) -> Result<Vec<NacosNodeView>, String> {
    let url = format!("{base}{path}");
    let mut req = HTTP.get(&url);
    if let Some(t) = token {
        req = req.query(&[("accessToken", t)]);
    }
    let resp = req.send().await.map_err(|e| format!("{path} 请求失败:{}", why(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("{path} 返回 HTTP {}", resp.status().as_u16()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("{path} 响应无法解析:{e}"))?;
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| format!("{path} 响应缺少 data 数组"))?;
    Ok(arr.iter().map(node_from_json).collect())
}

fn node_from_json(item: &Value) -> NacosNodeView {
    let address = item
        .get("address")
        .and_then(|a| a.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let ip = item.get("ip").and_then(|i| i.as_str()).unwrap_or("");
            let port = item.get("port").and_then(|p| p.as_i64()).unwrap_or(8848);
            format!("{ip}:{port}")
        });
    let state = item.get("state").and_then(|s| s.as_str()).unwrap_or("UNKNOWN").to_string();
    let version = item
        .get("extendInfo")
        .and_then(|e| e.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    NacosNodeView {
        ok: state.eq_ignore_ascii_case("UP"),
        address,
        state,
        version,
        latency_ms: 0,
        message: String::new(),
    }
}

// ---- 配置读写 ----

/// Existing config content, `None` when the dataId is absent.
async fn get_config(
    base: &str,
    token: &Option<String>,
    tenant: &str,
    data_id: &str,
    group: &str,
) -> Result<Option<String>, String> {
    let url = format!("{base}/v1/cs/configs");
    let mut params = common_params(token, tenant);
    params.push(("dataId", data_id.to_string()));
    params.push(("group", group.to_string()));
    let resp = HTTP
        .get(&url)
        .query(&params)
        .send()
        .await
        .map_err(|e| format!("读取配置失败:{}", why(&e)))?;
    match resp.status().as_u16() {
        200 => Ok(Some(resp.text().await.unwrap_or_default())),
        404 => Ok(None),
        code => Err(format!("读取配置返回 HTTP {code}")),
    }
}

async fn publish_config(
    base: &str,
    token: &Option<String>,
    tenant: &str,
    item: &NacosConfigItem,
) -> Result<(), String> {
    let url = format!("{base}/v1/cs/configs");
    let mut query = Vec::new();
    if let Some(t) = token {
        query.push(("accessToken", t.clone()));
    }
    let mut form: Vec<(&str, String)> = vec![
        ("dataId", item.data_id.clone()),
        ("group", item.group.clone()),
        ("content", item.content.clone()),
        ("type", item.kind.clone()),
    ];
    if !tenant.is_empty() {
        form.push(("tenant", tenant.to_string()));
    }
    let resp = HTTP
        .post(&url)
        .query(&query)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("发布配置失败:{}", why(&e)))?;
    let code = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    if code == 200 && body.trim().eq_ignore_ascii_case("true") {
        Ok(())
    } else {
        Err(format!("发布配置被拒绝(HTTP {code} {})", body.trim()))
    }
}

/// Existing configs in the namespace (compact listing for the UI).
async fn list_configs(
    conn: &NacosConn,
    page_no: i64,
    page_size: i64,
) -> Result<(i64, Vec<Value>), String> {
    let base = conn.bases.first().ok_or("集群没有配置任何地址")?;
    let token = access_token(base, &conn.username, &conn.password).await?;
    let url = format!("{base}/v1/cs/configs");
    let mut params = common_params(&token, &conn.namespace);
    params.push(("search", "accurate".into()));
    params.push(("dataId", String::new()));
    params.push(("group", String::new()));
    params.push(("pageNo", page_no.to_string()));
    params.push(("pageSize", page_size.to_string()));
    let resp = HTTP
        .get(&url)
        .query(&params)
        .send()
        .await
        .map_err(|e| format!("查询配置列表失败:{}", why(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("查询配置列表返回 HTTP {}", resp.status().as_u16()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("配置列表无法解析:{e}"))?;
    let total = v.get("totalCount").and_then(|t| t.as_i64()).unwrap_or(0);
    let items = v
        .get("pageItems")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .map(|i| {
                    json!({
                        "data_id": i.get("dataId").and_then(|x| x.as_str()).unwrap_or(""),
                        "group": i.get("group").and_then(|x| x.as_str()).unwrap_or(""),
                        "type": i.get("type").and_then(|x| x.as_str()).unwrap_or(""),
                        "app_name": i.get("appName").and_then(|x| x.as_str()).unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((total, items))
}

// ---- 管理面 API:命名空间 / 账号 / 角色 / 权限 ----
//
// Nacos 的管理接口在 3.0 换了路径并把 v1/v2 控制台 API 默认关成 410 Gone,所以先探一次
// 版本口味(v3 优先,失败退 v1),再按口味发请求。1.4.x 上 v1 也在,只是 `search=accurate`
// 会被忽略 —— 因此始终带上它,一套调用形状通吃 1.x/2.x。
//
// 响应形状极不统一(源码核对 alibaba/nacos@2.3.2):
//   列表(user/role/permission) → 裸 Page<T>,无信封
//   写操作(user/role/permission) → RestResult { code:200, data:"xxx ok!" }
//   命名空间列表 → RestResult;命名空间增删改 → 裸 true/false(HTTP 200 也可能是 false)
//   失败 → HTTP 400 + 纯文本正文(不是 JSON)
// v3 则统一为 { code:0, message, data }。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// Nacos 1.x / 2.x:`/v1/auth/*`、`/v1/console/namespaces`
    V1,
    /// Nacos 3.x:`/v3/auth/*`、`/v3/console/core/namespace`
    V3,
}

impl Flavor {
    pub fn as_str(self) -> &'static str {
        match self {
            Flavor::V1 => "v1",
            Flavor::V3 => "v3",
        }
    }
}

/// 探测一次即可:v3 的用户列表存在就是 3.x,否则按 v1 走。
async fn detect_flavor(base: &str, token: &Option<String>) -> Flavor {
    let url = format!("{base}/v3/auth/user/list");
    let mut req = HTTP.get(&url).query(&[("pageNo", "1"), ("pageSize", "1")]);
    if let Some(t) = token {
        req = req.query(&[("accessToken", t)]);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => Flavor::V3,
        // 403/401 也说明这个端点存在(只是当前账号无权),同样判定为 3.x
        Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => Flavor::V3,
        _ => Flavor::V1,
    }
}

/// 一次管理面调用所需的上下文:基址 + token + 版本口味。
pub struct AdminCtx {
    pub base: String,
    pub token: Option<String>,
    pub flavor: Flavor,
}

pub async fn admin_ctx(conn: &NacosConn) -> Result<AdminCtx, String> {
    let base = conn.bases.first().ok_or("集群没有配置任何地址")?.clone();
    let token = access_token(&base, &conn.username, &conn.password).await?;
    let flavor = detect_flavor(&base, &token).await;
    Ok(AdminCtx { base, token, flavor })
}

impl AdminCtx {
    fn q(&self) -> Vec<(&'static str, String)> {
        match &self.token {
            Some(t) => vec![("accessToken", t.clone())],
            None => Vec::new(),
        }
    }

    /// 统一收口:非 2xx 时正文可能是纯文本(v1 抛 IllegalArgumentException 的情况),
    /// 直接把它当错误信息带回去,比 "HTTP 400" 有用得多。
    async fn finish(&self, resp: reqwest::Response, what: &str) -> Result<String, String> {
        let code = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        if (200..300).contains(&code) {
            return Ok(body);
        }
        let detail = body.trim();
        if detail.is_empty() {
            Err(format!("{what}失败(HTTP {code})"))
        } else {
            Err(format!("{what}失败(HTTP {code}):{}", truncate_msg(detail)))
        }
    }
}

fn truncate_msg(s: &str) -> String {
    const MAX: usize = 300;
    if s.len() <= MAX {
        return s.to_string();
    }
    let mut end = MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// v1 的写操作返回 RestResult{code:200,…};v3 返回 {code:0,…}。都可能是纯文本 "true"。
/// 只认 code,不解析人类可读串(1.4.x 把它放 message,2.2+ 放 data)。
fn check_write(body: &str, flavor: Flavor, what: &str) -> Result<(), String> {
    let trimmed = body.trim();
    if trimmed.eq_ignore_ascii_case("true") {
        return Ok(());
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return Err(format!("{what}失败:Nacos 返回 false(常见原因:id 已存在 / 不合法 / 超长)"));
    }
    let v: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        // 非 JSON 又非布尔:2xx 情况下当成功(部分版本返回空体)
        Err(_) => return Ok(()),
    };
    let ok_code = match flavor {
        Flavor::V1 => 200,
        Flavor::V3 => 0,
    };
    match v.get("code").and_then(|c| c.as_i64()) {
        Some(c) if c == ok_code => Ok(()),
        Some(c) => {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
                .or_else(|| v.get("data").and_then(|d| d.as_str()))
                .unwrap_or("未知错误");
            Err(format!("{what}失败(code {c}):{}", truncate_msg(msg)))
        }
        None => Ok(()),
    }
}

/// 裸 `Page<T>`(v1)或 `{code,data:Page<T>}`(v3)都取出 (total, items)。
fn read_page(body: &str, flavor: Flavor) -> Result<(i64, Vec<Value>), String> {
    let v: Value = serde_json::from_str(body.trim())
        .map_err(|e| format!("响应无法解析:{e}(原文:{})", truncate_msg(body.trim())))?;
    let page = match flavor {
        Flavor::V1 => &v,
        Flavor::V3 => v.get("data").unwrap_or(&v),
    };
    let total = page.get("totalCount").and_then(|t| t.as_i64()).unwrap_or(0);
    let items = page
        .get("pageItems")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    Ok((total, items))
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

// ---- 命名空间 ----

pub async fn list_namespaces(ctx: &AdminCtx) -> Result<Vec<Value>, String> {
    let url = match ctx.flavor {
        Flavor::V1 => format!("{}/v1/console/namespaces", ctx.base),
        Flavor::V3 => format!("{}/v3/console/core/namespace/list", ctx.base),
    };
    let resp = HTTP
        .get(&url)
        .query(&ctx.q())
        .send()
        .await
        .map_err(|e| format!("查询命名空间失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "查询命名空间").await?;
    let v: Value = serde_json::from_str(body.trim())
        .map_err(|e| format!("命名空间响应无法解析:{e}"))?;
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .iter()
        .map(|n| {
            json!({
                "namespace_id": str_at(n, "namespace"),
                "name": str_at(n, "namespaceShowName"),
                "desc": n.get("namespaceDesc").and_then(|d| d.as_str()).unwrap_or_default(),
                "quota": n.get("quota").and_then(|q| q.as_i64()).unwrap_or(0),
                "config_count": n.get("configCount").and_then(|c| c.as_i64()).unwrap_or(0),
                "type": n.get("type").and_then(|t| t.as_i64()).unwrap_or(2),
            })
        })
        .collect())
}

/// 命名空间 id / 名称的合法性,与 Nacos 服务端同一套正则(提前挡掉 v1 那个
/// 只会返回 `false`、分辨不出原因的失败)。
fn validate_namespace(id: &str, name: &str) -> Result<(), String> {
    if !id.is_empty() {
        if id.len() > 128 {
            return Err("命名空间 ID 超过 128 字符".into());
        }
        if !id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return Err("命名空间 ID 只能包含字母、数字、下划线、连字符".into());
        }
    }
    if name.trim().is_empty() {
        return Err("请填写命名空间名称".into());
    }
    if name.contains(['@', '#', '$', '%', '^', '&', '*']) {
        return Err("命名空间名称不能包含 @#$%^&* 等字符".into());
    }
    Ok(())
}

pub async fn create_namespace(
    ctx: &AdminCtx,
    id: &str,
    name: &str,
    desc: &str,
) -> Result<(), String> {
    validate_namespace(id, name)?;
    // v1 / v3 create 都叫 customNamespaceId,唯独 v2 叫 namespaceId —— 这里不用 v2
    let (url, form) = match ctx.flavor {
        Flavor::V1 => (
            format!("{}/v1/console/namespaces", ctx.base),
            vec![
                ("customNamespaceId", id.to_string()),
                ("namespaceName", name.to_string()),
                ("namespaceDesc", desc.to_string()),
            ],
        ),
        Flavor::V3 => (
            format!("{}/v3/console/core/namespace", ctx.base),
            vec![
                ("customNamespaceId", id.to_string()),
                ("namespaceName", name.to_string()),
                ("namespaceDesc", desc.to_string()),
            ],
        ),
    };
    let resp = HTTP
        .post(&url)
        .query(&ctx.q())
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("创建命名空间失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "创建命名空间").await?;
    check_write(&body, ctx.flavor, "创建命名空间")
}

pub async fn update_namespace(
    ctx: &AdminCtx,
    id: &str,
    name: &str,
    desc: &str,
) -> Result<(), String> {
    validate_namespace(id, name)?;
    // v1 edit 的参数名和 create 完全不同:namespace + namespaceShowName
    let (url, form) = match ctx.flavor {
        Flavor::V1 => (
            format!("{}/v1/console/namespaces", ctx.base),
            vec![
                ("namespace", id.to_string()),
                ("namespaceShowName", name.to_string()),
                ("namespaceDesc", desc.to_string()),
            ],
        ),
        Flavor::V3 => (
            format!("{}/v3/console/core/namespace", ctx.base),
            vec![
                ("namespaceId", id.to_string()),
                ("namespaceName", name.to_string()),
                ("namespaceDesc", desc.to_string()),
            ],
        ),
    };
    let resp = HTTP
        .put(&url)
        .query(&ctx.q())
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("修改命名空间失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "修改命名空间").await?;
    check_write(&body, ctx.flavor, "修改命名空间")
}

pub async fn delete_namespace(ctx: &AdminCtx, id: &str) -> Result<(), String> {
    // public 的 id 在列表里是空串,但有的部署会把字面量 "public" 传进来;两种都挡掉。
    // (空串走不到这里 —— `/namespaces/` 匹配不上路由,会落到 SPA fallback,所以 UI
    //  对 public 行直接禁用删除按钮。)
    if id.is_empty() || id == "public" {
        return Err("public 命名空间不可删除".into());
    }
    let url = match ctx.flavor {
        Flavor::V1 => format!("{}/v1/console/namespaces", ctx.base),
        Flavor::V3 => format!("{}/v3/console/core/namespace", ctx.base),
    };
    let mut q = ctx.q();
    q.push(("namespaceId", id.to_string()));
    let resp = HTTP
        .delete(&url)
        .query(&q)
        .send()
        .await
        .map_err(|e| format!("删除命名空间失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "删除命名空间").await?;
    check_write(&body, ctx.flavor, "删除命名空间")
}

// ---- 账号 ----

fn auth_path(ctx: &AdminCtx, what: &str) -> String {
    match ctx.flavor {
        Flavor::V1 => format!("{}/v1/auth/{what}s", ctx.base),
        Flavor::V3 => format!("{}/v3/auth/{what}", ctx.base),
    }
}

/// 列表:v1 用 `search=accurate` 这个 mapping 谓词(2.x 必带,1.4.x 忽略),
/// v3 走 `/list` 且 search 是普通可选参数。
fn list_url(ctx: &AdminCtx, what: &str) -> String {
    match ctx.flavor {
        Flavor::V1 => auth_path(ctx, what),
        Flavor::V3 => format!("{}/list", auth_path(ctx, what)),
    }
}

pub async fn list_users(
    ctx: &AdminCtx,
    page_no: i64,
    page_size: i64,
) -> Result<(i64, Vec<Value>), String> {
    let mut q = ctx.q();
    q.push(("search", "accurate".into()));
    q.push(("pageNo", page_no.to_string()));
    q.push(("pageSize", page_size.to_string()));
    let resp = HTTP
        .get(list_url(ctx, "user"))
        .query(&q)
        .send()
        .await
        .map_err(|e| format!("查询账号失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "查询账号").await?;
    let (total, items) = read_page(&body, ctx.flavor)?;
    // pageItems[].password 是 bcrypt 哈希,绝不外泄
    Ok((total, items.iter().map(|u| json!({ "username": str_at(u, "username") })).collect()))
}

pub async fn create_user(ctx: &AdminCtx, username: &str, password: &str) -> Result<(), String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("账号与密码不能为空".into());
    }
    let resp = HTTP
        .post(auth_path(ctx, "user"))
        .query(&ctx.q())
        .form(&[("username", username), ("password", password)])
        .send()
        .await
        .map_err(|e| format!("创建账号失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "创建账号").await?;
    check_write(&body, ctx.flavor, "创建账号")
}

pub async fn reset_user_password(
    ctx: &AdminCtx,
    username: &str,
    new_password: &str,
) -> Result<(), String> {
    if new_password.is_empty() {
        return Err("新密码不能为空".into());
    }
    let resp = HTTP
        .put(auth_path(ctx, "user"))
        .query(&ctx.q())
        .form(&[("username", username), ("newPassword", new_password)])
        .send()
        .await
        .map_err(|e| format!("重置密码失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "重置密码").await?;
    check_write(&body, ctx.flavor, "重置密码")
}

pub async fn delete_user(ctx: &AdminCtx, username: &str) -> Result<(), String> {
    let mut q = ctx.q();
    q.push(("username", username.to_string()));
    let resp = HTTP
        .delete(auth_path(ctx, "user"))
        .query(&q)
        .send()
        .await
        .map_err(|e| format!("删除账号失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "删除账号").await?;
    check_write(&body, ctx.flavor, "删除账号")
}

// ---- 角色绑定 ----

pub async fn list_roles(
    ctx: &AdminCtx,
    page_no: i64,
    page_size: i64,
) -> Result<(i64, Vec<Value>), String> {
    let mut q = ctx.q();
    q.push(("search", "accurate".into()));
    q.push(("pageNo", page_no.to_string()));
    q.push(("pageSize", page_size.to_string()));
    let resp = HTTP
        .get(list_url(ctx, "role"))
        .query(&q)
        .send()
        .await
        .map_err(|e| format!("查询角色失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "查询角色").await?;
    let (total, items) = read_page(&body, ctx.flavor)?;
    Ok((
        total,
        items
            .iter()
            .map(|r| json!({ "role": str_at(r, "role"), "username": str_at(r, "username") }))
            .collect(),
    ))
}

pub async fn bind_role(ctx: &AdminCtx, role: &str, username: &str) -> Result<(), String> {
    if role.trim().is_empty() || username.trim().is_empty() {
        return Err("角色名与账号不能为空".into());
    }
    if role == "ROLE_ADMIN" {
        return Err("Nacos 不允许通过接口创建 ROLE_ADMIN".into());
    }
    let resp = HTTP
        .post(auth_path(ctx, "role"))
        .query(&ctx.q())
        .form(&[("role", role), ("username", username)])
        .send()
        .await
        .map_err(|e| format!("绑定角色失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "绑定角色").await?;
    check_write(&body, ctx.flavor, "绑定角色")
}

/// `username` 留空 = 对所有用户删除该角色(Nacos 的语义,UI 需要显式提示)。
pub async fn unbind_role(ctx: &AdminCtx, role: &str, username: &str) -> Result<(), String> {
    let mut q = ctx.q();
    q.push(("role", role.to_string()));
    if !username.is_empty() {
        q.push(("username", username.to_string()));
    }
    let resp = HTTP
        .delete(auth_path(ctx, "role"))
        .query(&q)
        .send()
        .await
        .map_err(|e| format!("解绑角色失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "解绑角色").await?;
    check_write(&body, ctx.flavor, "解绑角色")
}

// ---- 权限 ----

/// 只放行 Nacos 控制台实际使用的三种动作。服务端不校验,写错了不会报错但永远匹配不上。
fn validate_action(action: &str) -> Result<(), String> {
    match action {
        "r" | "w" | "rw" => Ok(()),
        _ => Err("动作只能是 r / w / rw".into()),
    }
}

pub async fn list_permissions(
    ctx: &AdminCtx,
    page_no: i64,
    page_size: i64,
    role: &str,
) -> Result<(i64, Vec<Value>), String> {
    let mut q = ctx.q();
    q.push(("search", "accurate".into()));
    q.push(("pageNo", page_no.to_string()));
    q.push(("pageSize", page_size.to_string()));
    if !role.is_empty() {
        q.push(("role", role.to_string()));
    }
    let resp = HTTP
        .get(list_url(ctx, "permission"))
        .query(&q)
        .send()
        .await
        .map_err(|e| format!("查询权限失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "查询权限").await?;
    let (total, items) = read_page(&body, ctx.flavor)?;
    Ok((
        total,
        items
            .iter()
            .map(|p| {
                json!({
                    "role": str_at(p, "role"),
                    "resource": str_at(p, "resource"),
                    "action": str_at(p, "action"),
                })
            })
            .collect(),
    ))
}

pub async fn grant_permission(
    ctx: &AdminCtx,
    role: &str,
    resource: &str,
    action: &str,
) -> Result<(), String> {
    if role.trim().is_empty() || resource.trim().is_empty() {
        return Err("角色与资源不能为空".into());
    }
    validate_action(action)?;
    let resp = HTTP
        .post(auth_path(ctx, "permission"))
        .query(&ctx.q())
        .form(&[("role", role), ("resource", resource), ("action", action)])
        .send()
        .await
        .map_err(|e| format!("赋权失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "赋权").await?;
    check_write(&body, ctx.flavor, "赋权")
}

pub async fn revoke_permission(
    ctx: &AdminCtx,
    role: &str,
    resource: &str,
    action: &str,
) -> Result<(), String> {
    let mut q = ctx.q();
    q.push(("role", role.to_string()));
    q.push(("resource", resource.to_string()));
    q.push(("action", action.to_string()));
    let resp = HTTP
        .delete(auth_path(ctx, "permission"))
        .query(&q)
        .send()
        .await
        .map_err(|e| format!("收回权限失败:{}", why(&e)))?;
    let body = ctx.finish(resp, "收回权限").await?;
    check_write(&body, ctx.flavor, "收回权限")
}

// ---- 初始化:变量代入 + 逐条发布 ----

/// Substitute `${name}` from `vars`; returns the unresolved names, if any.
fn substitute(input: &str, vars: &BTreeMap<String, String>, missing: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = input[i + 2..].find('}') {
                let name = &input[i + 2..i + 2 + end];
                match vars.get(name) {
                    Some(v) => out.push_str(v),
                    None => {
                        missing.push(name.to_string());
                        out.push_str(&input[i..i + 3 + end]);
                    }
                }
                i += 3 + end;
                continue;
            }
        }
        let ch_len = input[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Apply the vars to one item. `Err` = the item references undefined variables.
fn resolve_item(
    item: &NacosConfigItem,
    vars: &BTreeMap<String, String>,
) -> Result<NacosConfigItem, (NacosConfigItem, Vec<String>)> {
    let mut missing = Vec::new();
    let resolved = NacosConfigItem {
        data_id: substitute(&item.data_id, vars, &mut missing),
        group: substitute(&item.group, vars, &mut missing),
        kind: item.kind.clone(),
        content: substitute(&item.content, vars, &mut missing),
    };
    if missing.is_empty() {
        Ok(resolved)
    } else {
        missing.sort();
        missing.dedup();
        Err((resolved, missing))
    }
}

/// Publish every item. `overwrite=false` leaves an existing dataId untouched;
/// `dry_run=true` only reports what would happen.
pub async fn init_configs(
    conn: &NacosConn,
    items: &[NacosConfigItem],
    vars: &BTreeMap<String, String>,
    overwrite: bool,
    dry_run: bool,
) -> Vec<NacosItemResult> {
    let mut out = Vec::with_capacity(items.len());
    let base = match conn.bases.first() {
        Some(b) => b.clone(),
        None => {
            return items
                .iter()
                .map(|i| NacosItemResult {
                    data_id: i.data_id.clone(),
                    group: i.group.clone(),
                    status: "fail".into(),
                    message: "集群没有配置任何地址".into(),
                })
                .collect()
        }
    };
    let token = match access_token(&base, &conn.username, &conn.password).await {
        Ok(t) => t,
        Err(e) => {
            return items
                .iter()
                .map(|i| NacosItemResult {
                    data_id: i.data_id.clone(),
                    group: i.group.clone(),
                    status: "fail".into(),
                    message: e.clone(),
                })
                .collect()
        }
    };
    let mark = |s: &str| if dry_run { format!("would_{s}") } else { s.to_string() };

    for raw in items {
        let item = match resolve_item(raw, vars) {
            Ok(i) => i,
            Err((i, missing)) => {
                out.push(NacosItemResult {
                    data_id: i.data_id,
                    group: i.group,
                    status: "fail".into(),
                    message: format!("变量未提供:{}", missing.join(", ")),
                });
                continue;
            }
        };
        if item.data_id.trim().is_empty() {
            out.push(NacosItemResult {
                data_id: item.data_id,
                group: item.group,
                status: "fail".into(),
                message: "dataId 为空".into(),
            });
            continue;
        }
        let existing =
            match get_config(&base, &token, &conn.namespace, &item.data_id, &item.group).await {
                Ok(e) => e,
                Err(e) => {
                    out.push(NacosItemResult {
                        data_id: item.data_id,
                        group: item.group,
                        status: "fail".into(),
                        message: e,
                    });
                    continue;
                }
            };
        match (&existing, overwrite) {
            (Some(cur), false) => {
                out.push(NacosItemResult {
                    data_id: item.data_id,
                    group: item.group,
                    status: mark("skipped"),
                    message: format!("已存在({} 字节),未覆盖", cur.len()),
                });
                continue;
            }
            (Some(cur), true) if *cur == item.content => {
                out.push(NacosItemResult {
                    data_id: item.data_id,
                    group: item.group,
                    status: mark("skipped"),
                    message: "内容一致,无需变更".into(),
                });
                continue;
            }
            _ => {}
        }
        let verb = if existing.is_some() { "updated" } else { "created" };
        if dry_run {
            out.push(NacosItemResult {
                data_id: item.data_id,
                group: item.group,
                status: mark(verb),
                message: "试运行:未写入".into(),
            });
            continue;
        }
        match publish_config(&base, &token, &conn.namespace, &item).await {
            Ok(()) => out.push(NacosItemResult {
                data_id: item.data_id,
                group: item.group,
                status: verb.into(),
                message: String::new(),
            }),
            Err(e) => out.push(NacosItemResult {
                data_id: item.data_id,
                group: item.group,
                status: "fail".into(),
                message: e,
            }),
        }
    }
    out
}

// ---- HTTP 处理器(全部 admin-only:集群地址与初始化都属基础设施操作) ----

fn admin(user: &AuthUser) -> Result<(), AppError> {
    if is_admin(user) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Decrypt the stored password and build a live connection descriptor.
fn conn_for(st: &AppState, c: &NacosClusterRow) -> Result<NacosConn, AppError> {
    let password = if c.secret.is_empty() {
        String::new()
    } else {
        if st.vault.is_sealed() {
            return Err(AppError::Sealed);
        }
        st.vault.decrypt(&c.secret).map_err(AppError::Internal)?
    };
    Ok(NacosConn {
        bases: base_urls(&c.server_addr, &c.context_path),
        namespace: c.namespace.clone(),
        username: c.username.clone(),
        password,
    })
}

async fn load_cluster(st: &AppState, id: &str) -> Result<NacosClusterRow, AppError> {
    st.store
        .get_nacos_cluster(id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("集群不存在".into()))
}

/// `GET /nacos/clusters` — every registered cluster + its last init summary.
pub async fn list_clusters(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let rows = st.store.list_nacos_clusters().await.map_err(AppError::Internal)?;
    let mut out = Vec::with_capacity(rows.len());
    for c in rows {
        let last = st.store.last_nacos_run(&c.id).await.unwrap_or_default();
        let mut v = serde_json::to_value(&c).unwrap_or_default();
        if let Some(obj) = v.as_object_mut() {
            obj.insert("has_secret".into(), json!(!c.secret.is_empty()));
            obj.insert("endpoints".into(), json!(base_urls(&c.server_addr, &c.context_path)));
            obj.insert(
                "last_init".into(),
                match last {
                    Some(r) => json!({
                        "ts": r.ts, "status": r.status, "total": r.total,
                        "ok_count": r.ok_count, "operator": r.operator_email,
                        "template_name": r.template_name,
                    }),
                    None => Value::Null,
                },
            );
        }
        out.push(v);
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
pub struct SaveCluster {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub server_addr: String,
    #[serde(default = "def_context_path")]
    pub context_path: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub username: String,
    /// Empty on update = keep the stored password.
    #[serde(default)]
    pub password: String,
    #[serde(default = "def_enabled")]
    pub status: String,
    #[serde(default)]
    pub note: String,
}
fn def_context_path() -> String {
    "/nacos".into()
}
fn def_enabled() -> String {
    "enabled".into()
}

fn row_from(req: &SaveCluster, id: String, secret: String, created_at: i64) -> NacosClusterRow {
    NacosClusterRow {
        id,
        name: req.name.trim().to_string(),
        env: req.env.trim().to_string(),
        server_addr: req.server_addr.trim().to_string(),
        context_path: if req.context_path.trim().is_empty() {
            "/nacos".into()
        } else {
            req.context_path.trim().to_string()
        },
        namespace: req.namespace.trim().to_string(),
        username: req.username.trim().to_string(),
        secret,
        status: if req.status == "disabled" { "disabled".into() } else { "enabled".into() },
        note: req.note.clone(),
        created_at,
    }
}

/// Encrypt a password through the vault (503 while sealed). Empty passes through.
fn encrypt_password(st: &AppState, password: &str) -> Result<String, AppError> {
    if password.is_empty() {
        return Ok(String::new());
    }
    if st.vault.is_sealed() {
        return Err(AppError::Sealed);
    }
    st.vault.encrypt(password).map_err(AppError::Internal)
}

pub async fn create_cluster(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<SaveCluster>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("请填写集群名称".into()));
    }
    if base_urls(&req.server_addr, &req.context_path).is_empty() {
        return Err(AppError::BadRequest("请填写至少一个服务地址(host:port)".into()));
    }
    let id = req.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
    let secret = encrypt_password(&st, &req.password)?;
    let row = row_from(&req, id.clone(), secret, now_secs());
    st.store.create_nacos_cluster(&row).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn update_cluster(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SaveCluster>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let cur = load_cluster(&st, &id).await?;
    if base_urls(&req.server_addr, &req.context_path).is_empty() {
        return Err(AppError::BadRequest("请填写至少一个服务地址(host:port)".into()));
    }
    // empty password → store layer keeps the existing ciphertext
    let secret = encrypt_password(&st, &req.password)?;
    let row = row_from(&req, id.clone(), secret, cur.created_at);
    st.store.update_nacos_cluster(&row).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_cluster(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    load_cluster(&st, &id).await?;
    st.store.delete_nacos_cluster(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

/// `GET /nacos/clusters/{id}/nodes` — live member list (or address probes).
pub async fn cluster_nodes(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let c = load_cluster(&st, &id).await?;
    let conn = conn_for(&st, &c)?;
    let started = Instant::now();
    let ins = inspect(&conn).await;
    Ok(Json(json!({
        "cluster_id": c.id,
        "cluster_name": c.name,
        "source": ins.source,
        "ok": ins.nodes.iter().any(|n| n.ok),
        "latency_ms": started.elapsed().as_millis() as i64,
        "message": ins.message,
        "nodes": ins.nodes,
    })))
}

#[derive(Deserialize)]
pub struct ProbeCluster {
    #[serde(default)]
    pub server_addr: String,
    #[serde(default = "def_context_path")]
    pub context_path: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

/// `POST /nacos/probe` — 测试连通 for the create/edit form (nothing is stored).
pub async fn probe_cluster(
    user: AuthUser,
    State(_st): State<AppState>,
    Json(req): Json<ProbeCluster>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let bases = base_urls(&req.server_addr, &req.context_path);
    if bases.is_empty() {
        return Err(AppError::BadRequest("请填写至少一个服务地址(host:port)".into()));
    }
    let conn = NacosConn {
        bases,
        namespace: req.namespace,
        username: req.username,
        password: req.password,
    };
    let started = Instant::now();
    let ins = inspect(&conn).await;
    Ok(Json(json!({
        "ok": ins.nodes.iter().any(|n| n.ok),
        "source": ins.source,
        "latency_ms": started.elapsed().as_millis() as i64,
        "message": ins.message,
        "nodes": ins.nodes,
    })))
}

#[derive(Deserialize)]
pub struct ConfigsQuery {
    #[serde(default = "def_page")]
    pub page_no: i64,
    #[serde(default = "def_page_size")]
    pub page_size: i64,
}
fn def_page() -> i64 {
    1
}
fn def_page_size() -> i64 {
    100
}

/// `GET /nacos/clusters/{id}/configs` — what the namespace already holds.
pub async fn cluster_configs(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ConfigsQuery>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let c = load_cluster(&st, &id).await?;
    let conn = conn_for(&st, &c)?;
    match list_configs(&conn, q.page_no, q.page_size.clamp(1, 500)).await {
        Ok((total, items)) => Ok(Json(json!({ "ok": true, "total": total, "items": items }))),
        Err(e) => Ok(Json(json!({ "ok": false, "total": 0, "items": [], "message": e }))),
    }
}

/// `POST /nacos/clusters/{id}/init` — 初始化配置(模板或即席条目)。
pub async fn init_cluster(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<NacosInitRequest>,
) -> Result<Json<NacosInitResult>, AppError> {
    admin(&user)?;
    let c = load_cluster(&st, &id).await?;
    if c.status == "disabled" {
        return Err(AppError::BadRequest("集群已停用,无法初始化".into()));
    }

    // items: explicit ones win, otherwise the template's
    let (items, template_id, template_name) = if !req.items.is_empty() {
        (req.items.clone(), String::new(), String::new())
    } else if let Some(tid) = req.template_id.clone().filter(|t| !t.is_empty()) {
        let t = st
            .store
            .get_nacos_template(&tid)
            .await
            .map_err(AppError::Internal)?
            .ok_or_else(|| AppError::BadRequest("配置模板不存在".into()))?;
        let parsed: Vec<NacosConfigItem> = serde_json::from_str(&t.items)
            .map_err(|e| AppError::BadRequest(format!("模板配置项无法解析:{e}")))?;
        (parsed, t.id, t.name)
    } else {
        (Vec::new(), String::new(), String::new())
    };
    if items.is_empty() {
        return Err(AppError::BadRequest("没有要初始化的配置项".into()));
    }

    let mut conn = conn_for(&st, &c)?;
    if let Some(ns) = req.namespace.clone().filter(|n| !n.trim().is_empty()) {
        conn.namespace = ns.trim().to_string();
    }

    let results = init_configs(&conn, &items, &req.vars, req.overwrite, req.dry_run).await;
    let total = results.len() as i64;
    let ok_count = results.iter().filter(|r| r.status != "fail").count() as i64;
    let status = if ok_count == total {
        "ok"
    } else if ok_count == 0 {
        "fail"
    } else {
        "partial"
    };

    let run_id = Uuid::new_v4().to_string();
    let items_json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
    let run = NacosRunRow {
        id: run_id.clone(),
        cluster_id: c.id.clone(),
        cluster_name: c.name.clone(),
        template_id,
        template_name: template_name.clone(),
        operator_id: user.user_id.clone(),
        operator_email: user.email.clone(),
        namespace: conn.namespace.clone(),
        status: status.to_string(),
        total,
        ok_count,
        dry_run: i64::from(req.dry_run),
        items: items_json,
        ts: now_secs(),
    };
    st.store.insert_nacos_run(&run).await.map_err(AppError::Internal)?;

    let _ = st
        .store
        .insert_audit(
            &Uuid::new_v4().to_string(),
            now_secs(),
            &user.user_id,
            &user.email,
            if req.dry_run { "nacos_init_dry_run" } else { "nacos_init" },
            &format!("{} [{}]", c.name, conn.namespace),
            &json!({
                "cluster_id": c.id,
                "namespace": conn.namespace,
                "template": template_name,
                "overwrite": req.overwrite,
                "run_id": run_id,
                "items": results.iter().map(|r| json!({
                    "data_id": r.data_id, "group": r.group, "status": r.status
                })).collect::<Vec<_>>(),
            })
            .to_string(),
            status,
            "",
        )
        .await;

    Ok(Json(NacosInitResult {
        run_id,
        cluster_id: c.id,
        cluster_name: c.name,
        namespace: conn.namespace,
        status: status.to_string(),
        total,
        ok_count,
        dry_run: req.dry_run,
        items: results,
    }))
}

// ---- 配置模板 ----

pub async fn list_templates(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<NacosTemplateRow>>, AppError> {
    admin(&user)?;
    Ok(Json(st.store.list_nacos_templates().await.map_err(AppError::Internal)?))
}

#[derive(Deserialize)]
pub struct SaveNacosTemplate {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub items: Vec<NacosConfigItem>,
}

pub async fn save_template(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<SaveNacosTemplate>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("请填写模板名称".into()));
    }
    let id = req.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
    let created_at = match st.store.get_nacos_template(&id).await.map_err(AppError::Internal)? {
        Some(old) => old.created_at,
        None => now_secs(),
    };
    let items: Vec<NacosConfigItem> = req
        .items
        .into_iter()
        .map(|mut i| {
            if i.group.trim().is_empty() {
                i.group = default_nacos_group();
            }
            i
        })
        .collect();
    st.store
        .save_nacos_template(&NacosTemplateRow {
            id: id.clone(),
            name: req.name.trim().to_string(),
            note: req.note,
            items: serde_json::to_string(&items).unwrap_or_else(|_| "[]".into()),
            created_at,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_template(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    st.store.delete_nacos_template(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

// ---- 初始化记录 ----

#[derive(Deserialize)]
pub struct RunsQuery {
    #[serde(default)]
    pub cluster_id: String,
    #[serde(default = "def_run_limit")]
    pub limit: i64,
}
fn def_run_limit() -> i64 {
    100
}

pub async fn list_runs(
    user: AuthUser,
    State(st): State<AppState>,
    Query(q): Query<RunsQuery>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let rows = st
        .store
        .list_nacos_runs(&q.cluster_id, q.limit.clamp(1, 500))
        .await
        .map_err(AppError::Internal)?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let items: Value = serde_json::from_str(&r.items).unwrap_or_else(|_| json!([]));
            json!({
                "id": r.id, "cluster_id": r.cluster_id, "cluster_name": r.cluster_name,
                "template_id": r.template_id, "template_name": r.template_name,
                "operator_email": r.operator_email, "namespace": r.namespace,
                "status": r.status, "total": r.total, "ok_count": r.ok_count,
                "dry_run": r.dry_run != 0, "ts": r.ts, "items": items,
            })
        })
        .collect();
    Ok(Json(json!(out)))
}

// ---- 管理面处理器:命名空间 / 账号 / 角色 / 权限(admin-only,写操作全部落审计) ----

/// 取集群 → 解封凭据 → 登录 → 判定版本口味。所有管理面处理器的统一入口。
async fn ctx_for(st: &AppState, id: &str) -> Result<(NacosClusterRow, AdminCtx), AppError> {
    let c = load_cluster(st, id).await?;
    let conn = conn_for(st, &c)?;
    let ctx = admin_ctx(&conn).await.map_err(AppError::BadRequest)?;
    Ok((c, ctx))
}

/// 远端写操作留痕:action 形如 `nacos_ns_create` / `nacos_user_delete`。
async fn audit_admin(
    st: &AppState,
    user: &AuthUser,
    c: &NacosClusterRow,
    action: &str,
    payload: Value,
    result: &str,
) {
    let _ = st
        .store
        .insert_audit(
            &Uuid::new_v4().to_string(),
            now_secs(),
            &user.user_id,
            &user.email,
            action,
            &c.name,
            &payload.to_string(),
            result,
            "",
        )
        .await;
}

/// 把 `Result<(), String>` 收敛成 `{ok:true}` / 400,并写审计。
async fn done(
    st: &AppState,
    user: &AuthUser,
    c: &NacosClusterRow,
    action: &str,
    payload: Value,
    outcome: Result<(), String>,
) -> Result<Json<Value>, AppError> {
    match outcome {
        Ok(()) => {
            audit_admin(st, user, c, action, payload, "ok").await;
            Ok(Json(json!({ "ok": true })))
        }
        Err(e) => {
            let mut p = payload;
            if let Some(o) = p.as_object_mut() {
                o.insert("error".into(), json!(e));
            }
            audit_admin(st, user, c, action, p, "fail").await;
            Err(AppError::BadRequest(e))
        }
    }
}

#[derive(Deserialize)]
pub struct PageQuery {
    #[serde(default = "def_page")]
    pub page_no: i64,
    #[serde(default = "def_page_size")]
    pub page_size: i64,
    #[serde(default)]
    pub role: String,
}

// -- 命名空间 --

pub async fn list_namespaces_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (_, ctx) = ctx_for(&st, &id).await?;
    match list_namespaces(&ctx).await {
        Ok(items) => Ok(Json(json!({
            "ok": true, "flavor": ctx.flavor.as_str(), "items": items, "message": ""
        }))),
        Err(e) => Ok(Json(json!({
            "ok": false, "flavor": ctx.flavor.as_str(), "items": [], "message": e
        }))),
    }
}

#[derive(Deserialize)]
pub struct NamespaceReq {
    #[serde(default)]
    pub namespace_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub desc: String,
}

pub async fn create_namespace_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<NamespaceReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let payload = json!({ "namespace_id": req.namespace_id, "name": req.name });
    let out = create_namespace(&ctx, &req.namespace_id, &req.name, &req.desc).await;
    let ok = out.is_ok();
    let resp = done(&st, &user, &c, "nacos_ns_create", payload, out).await?;
    Ok(if ok {
        Json(json!({ "ok": true, "namespace_id": req.namespace_id }))
    } else {
        resp
    })
}

pub async fn update_namespace_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<NamespaceReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let payload = json!({ "namespace_id": req.namespace_id, "name": req.name });
    let out = update_namespace(&ctx, &req.namespace_id, &req.name, &req.desc).await;
    done(&st, &user, &c, "nacos_ns_update", payload, out).await
}

pub async fn delete_namespace_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path((id, ns)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = delete_namespace(&ctx, &ns).await;
    done(&st, &user, &c, "nacos_ns_delete", json!({ "namespace_id": ns }), out).await
}

// -- 账号 --

pub async fn list_users_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (_, ctx) = ctx_for(&st, &id).await?;
    match list_users(&ctx, q.page_no, q.page_size.clamp(1, 500)).await {
        Ok((total, items)) => Ok(Json(json!({ "ok": true, "total": total, "items": items }))),
        Err(e) => Ok(Json(json!({ "ok": false, "total": 0, "items": [], "message": e }))),
    }
}

#[derive(Deserialize)]
pub struct CreateUserReq {
    pub username: String,
    #[serde(default)]
    pub password: String,
}

pub async fn create_user_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateUserReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = create_user(&ctx, &req.username, &req.password).await;
    done(&st, &user, &c, "nacos_user_create", json!({ "username": req.username }), out).await
}

#[derive(Deserialize)]
pub struct ResetUserReq {
    pub username: String,
    #[serde(default)]
    pub new_password: String,
}

pub async fn reset_user_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ResetUserReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = reset_user_password(&ctx, &req.username, &req.new_password).await;
    done(&st, &user, &c, "nacos_user_reset", json!({ "username": req.username }), out).await
}

pub async fn delete_user_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path((id, username)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = delete_user(&ctx, &username).await;
    done(&st, &user, &c, "nacos_user_delete", json!({ "username": username }), out).await
}

// -- 角色 --

pub async fn list_roles_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (_, ctx) = ctx_for(&st, &id).await?;
    match list_roles(&ctx, q.page_no, q.page_size.clamp(1, 500)).await {
        Ok((total, items)) => Ok(Json(json!({ "ok": true, "total": total, "items": items }))),
        Err(e) => Ok(Json(json!({ "ok": false, "total": 0, "items": [], "message": e }))),
    }
}

#[derive(Deserialize)]
pub struct RoleReq {
    pub role: String,
    #[serde(default)]
    pub username: String,
}

pub async fn bind_role_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RoleReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = bind_role(&ctx, &req.role, &req.username).await;
    let payload = json!({ "role": req.role, "username": req.username });
    done(&st, &user, &c, "nacos_role_bind", payload, out).await
}

pub async fn unbind_role_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RoleReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = unbind_role(&ctx, &q.role, &q.username).await;
    let payload = json!({ "role": q.role, "username": q.username });
    done(&st, &user, &c, "nacos_role_unbind", payload, out).await
}

// -- 权限 --

pub async fn list_permissions_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (_, ctx) = ctx_for(&st, &id).await?;
    match list_permissions(&ctx, q.page_no, q.page_size.clamp(1, 500), &q.role).await {
        Ok((total, items)) => Ok(Json(json!({ "ok": true, "total": total, "items": items }))),
        Err(e) => Ok(Json(json!({ "ok": false, "total": 0, "items": [], "message": e }))),
    }
}

#[derive(Deserialize)]
pub struct PermissionReq {
    pub role: String,
    pub resource: String,
    pub action: String,
}

pub async fn grant_permission_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PermissionReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = grant_permission(&ctx, &req.role, &req.resource, &req.action).await;
    let payload = json!({ "role": req.role, "resource": req.resource, "action": req.action });
    done(&st, &user, &c, "nacos_perm_grant", payload, out).await
}

pub async fn revoke_permission_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PermissionReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let out = revoke_permission(&ctx, &q.role, &q.resource, &q.action).await;
    let payload = json!({ "role": q.role, "resource": q.resource, "action": q.action });
    done(&st, &user, &c, "nacos_perm_revoke", payload, out).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls_normalize_scheme_port_and_context() {
        assert_eq!(
            base_urls("10.0.0.1, http://n2:8849/nacos ,", "/nacos"),
            vec!["http://10.0.0.1:8848/nacos", "http://n2:8849/nacos"]
        );
        // empty context path (standalone deployments serving at the root)
        assert_eq!(base_urls("n1:8848", "/"), vec!["http://n1:8848"]);
        assert!(base_urls("  ,  ", "/nacos").is_empty());
    }

    #[test]
    fn substitute_reports_missing_vars() {
        let mut vars = BTreeMap::new();
        vars.insert("env".to_string(), "prod".to_string());
        let mut missing = Vec::new();
        let out = substitute("url=${env}-db:${port}", &vars, &mut missing);
        assert_eq!(out, "url=prod-db:${port}");
        assert_eq!(missing, vec!["port".to_string()]);
    }

    #[test]
    fn same_host_matches_base_url_against_bare_address() {
        assert!(same_host("http://10.0.0.1:8848/nacos", "10.0.0.1:8848"));
        assert!(!same_host("http://10.0.0.1:8848/nacos", "10.0.0.2:8848"));
    }
}
