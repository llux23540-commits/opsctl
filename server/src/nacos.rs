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
///
/// 端口缺省规则:裸 `host` 按 Nacos 惯例补 8848;写明 scheme 的
/// `http(s)://host` 按 URL 语义走 80/443(反代部署常见)。
pub fn base_urls(server_addr: &str, context_path: &str) -> Vec<String> {
    let ctx = norm_ctx(context_path);
    server_addr
        .split(',')
        .filter_map(|raw| normalize_base(raw, &ctx))
        .collect()
}

/// `"nacos"` / `"/nacos/"` → `"/nacos"`;空白 → `""`。
fn norm_ctx(context_path: &str) -> String {
    let c = context_path.trim().trim_end_matches('/');
    if c.is_empty() {
        String::new()
    } else if c.starts_with('/') {
        c.to_string()
    } else {
        format!("/{c}")
    }
}

/// One parsed entry of `server_addr`. `scheme_explicit` 决定缺省端口语义:
/// 裸 `host` 是「Nacos 地址」(默认 8848),`http(s)://host` 是 URL(默认 80/443)。
struct AddrParts<'a> {
    scheme: &'a str,
    scheme_explicit: bool,
    host: &'a str,
    port: Option<&'a str>,
    /// 地址自带的路径(已去尾部 `/`);为空时用集群的 context path。
    path: &'a str,
}

fn parse_addr(raw: &str) -> Option<AddrParts<'_>> {
    let s = raw.trim().trim_end_matches('/');
    if s.is_empty() {
        return None;
    }
    let (scheme, scheme_explicit, rest) = match s.split_once("://") {
        Some((sc, r)) => (sc, true, r),
        None => ("http", false, s),
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    // IPv6 字面量:标准写法带括号 `[::1]:8848`;裸写 `2001:db8::1`(多个冒号、
    // 无括号)整体视作 host、没写端口。其余按 `host[:port]` 拆。
    let (host, port) = if let Some(inner) = hostport.strip_prefix('[') {
        let (h, tail) = inner.split_once(']')?; // 括号不闭合 → 整条地址无效
        let port = match tail {
            "" => None,
            t => Some(t.strip_prefix(':')?), // `]` 后只允许 `:port`
        };
        (h, port)
    } else if hostport.matches(':').count() > 1 {
        (hostport, None)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    Some(AddrParts { scheme, scheme_explicit, host, port, path })
}

/// URL/`host:port` 场景下的主机写法:IPv6 字面量必须带括号。
fn host_disp(host: &str) -> String {
    if host.contains(':') { format!("[{host}]") } else { host.to_string() }
}

fn default_port(p: &AddrParts) -> &'static str {
    if !p.scheme_explicit {
        "8848" // Nacos 惯例:裸 host 视作 Nacos 服务地址
    } else if p.scheme.eq_ignore_ascii_case("https") {
        "443"
    } else {
        "80" // 写明了 scheme 就按 URL 语义走标准端口
    }
}

fn assemble(p: &AddrParts, port: &str, ctx: &str) -> String {
    // an address that already carries a path keeps it; otherwise use the context
    let path = if p.path.is_empty() { ctx } else { p.path };
    format!("{}://{}:{}{}", p.scheme, host_disp(p.host), port, path)
}

fn normalize_base(raw: &str, ctx: &str) -> Option<String> {
    let p = parse_addr(raw)?;
    let port = p.port.unwrap_or_else(|| default_port(&p));
    Some(assemble(&p, port, ctx))
}

/// 没写端口的地址,探测失败时值得按「另一种惯例」再试:裸 host(默认 8848)
/// 试 80;`http(s)://host`(默认 80/443)试 8848。返回 (候选 base, 表单建议写法)。
fn alternate_base(raw: &str, ctx: &str) -> Option<(String, String)> {
    let p = parse_addr(raw)?;
    if p.port.is_some() {
        return None;
    }
    let alt = if default_port(&p) == "8848" { "80" } else { "8848" };
    let scheme = if p.scheme_explicit { format!("{}://", p.scheme) } else { String::new() };
    let suggest = format!("{scheme}{}:{alt}{}", host_disp(p.host), p.path);
    Some((assemble(&p, alt, ctx), suggest))
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
        // 把 Nacos 的原话带出来:比如「User xxx not found」一眼就能看出用户名拼错。
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Nacos 鉴权失败(HTTP {code} {})", truncate_msg(body.trim())));
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

/// 401/403 几乎总是鉴权问题;把「为什么」直接写进错误,免得用户对着状态码猜。
fn auth_hint(code: u16, token: &Option<String>) -> &'static str {
    if code != 401 && code != 403 {
        return "";
    }
    if token.is_none() {
        "(Nacos 已开启鉴权,但该集群未配置账号 —— 请编辑集群,填写用户名/密码)"
    } else {
        "(token 被拒:检查该账号对此命名空间的权限,或密码是否已变更)"
    }
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
        code => Err(format!("读取配置返回 HTTP {code}{}", auth_hint(code, token))),
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
        Err(format!("发布配置被拒绝(HTTP {code} {}){}", body.trim(), auth_hint(code, token)))
    }
}

/// 删除一条配置(补齐配置面的增删改查)。
async fn delete_config(
    base: &str,
    token: &Option<String>,
    tenant: &str,
    data_id: &str,
    group: &str,
) -> Result<(), String> {
    let url = format!("{base}/v1/cs/configs");
    let mut params = common_params(token, tenant);
    params.push(("dataId", data_id.to_string()));
    params.push(("group", group.to_string()));
    let resp = HTTP
        .delete(&url)
        .query(&params)
        .send()
        .await
        .map_err(|e| format!("删除配置失败:{}", why(&e)))?;
    let code = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    if code == 200 && body.trim().eq_ignore_ascii_case("true") {
        Ok(())
    } else {
        Err(format!(
            "删除配置被拒绝(HTTP {code} {}){}",
            truncate_msg(body.trim()),
            auth_hint(code, token)
        ))
    }
}

/// 把整个命名空间的配置拉回本地(同步)。
///
/// 列表接口在不同版本上对 `content` 的态度不一致:有的把内容一起返回,有的只给元数据。
/// 所以先用列表拿全量清单,再对缺内容的逐条 GET 补齐 —— 保证同步下来的模板是完整可回放的,
/// 而不是一堆空壳。
pub async fn pull_configs(conn: &NacosConn) -> Result<Vec<NacosConfigItem>, String> {
    let base = conn.bases.first().ok_or("集群没有配置任何地址")?.clone();
    let token = access_token(&base, &conn.username, &conn.password).await?;
    let url = format!("{base}/v1/cs/configs");

    let mut out: Vec<NacosConfigItem> = Vec::new();
    let page_size = 200;
    for page_no in 1..=50 {
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
            .map_err(|e| format!("拉取配置失败:{}", why(&e)))?;
        let code = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(format!("拉取配置返回 HTTP {code}{}", auth_hint(code, &token)));
        }
        let v: Value = resp.json().await.map_err(|e| format!("配置列表无法解析:{e}"))?;
        let items = v.get("pageItems").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        let got = items.len();
        for i in items {
            out.push(NacosConfigItem {
                data_id: str_at(&i, "dataId"),
                group: {
                    let g = str_at(&i, "group");
                    if g.is_empty() { default_nacos_group() } else { g }
                },
                kind: {
                    let t = str_at(&i, "type");
                    if t.is_empty() { "text".into() } else { t }
                },
                content: str_at(&i, "content"),
            });
        }
        let pages = v.get("pagesAvailable").and_then(|p| p.as_i64()).unwrap_or(1);
        if got < page_size as usize || page_no >= pages {
            break;
        }
    }

    // 列表没带内容的,逐条补齐
    for item in out.iter_mut() {
        if item.content.is_empty() {
            if let Ok(Some(c)) =
                get_config(&base, &token, &conn.namespace, &item.data_id, &item.group).await
            {
                item.content = c;
            }
        }
    }
    Ok(out)
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
    let code = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(format!("查询配置列表返回 HTTP {code}{}", auth_hint(code, &token)));
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
/// `literal=true` 跳过 `${}` 代入,按原文下发(同步回来的真实配置走这条路)。
pub async fn init_configs(
    conn: &NacosConn,
    items: &[NacosConfigItem],
    vars: &BTreeMap<String, String>,
    literal: bool,
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
        let item = if literal {
            raw.clone()
        } else {
            match resolve_item(raw, vars) {
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
    let ok = ins.nodes.iter().any(|n| n.ok);
    // 全部不可达且有地址没写端口 → 按另一种端口惯例(8848↔80/443)补一轮探活,
    // 命中就把正确写法直接放进提示,免得用户对着 Connection refused 猜端口。
    let mut message = ins.message;
    if !ok {
        let ctx = norm_ctx(&req.context_path);
        for raw in req.server_addr.split(',') {
            if let Some((alt_base, suggest)) = alternate_base(raw, &ctx) {
                if probe_base(&alt_base).await.ok {
                    message.push_str(&format!(";检测到 {alt_base} 可提供服务,请把该地址写成 {suggest}"));
                }
            }
        }
    }
    Ok(Json(json!({
        "ok": ok,
        "source": ins.source,
        "latency_ms": started.elapsed().as_millis() as i64,
        "message": message,
        "nodes": ins.nodes,
    })))
}

#[derive(Deserialize)]
pub struct ConfigsQuery {
    #[serde(default = "def_page")]
    pub page_no: i64,
    #[serde(default = "def_page_size")]
    pub page_size: i64,
    /// 命名空间是 Nacos 的硬隔离边界:配置永远从属于某个命名空间。
    /// 留空 = 用集群登记时的默认命名空间。
    #[serde(default)]
    pub namespace: Option<String>,
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
    let conn = with_namespace(conn_for(&st, &c)?, &q.namespace);
    let ns = conn.namespace.clone();
    match list_configs(&conn, q.page_no, q.page_size.clamp(1, 500)).await {
        Ok((total, items)) => {
            Ok(Json(json!({ "ok": true, "namespace": ns, "total": total, "items": items })))
        }
        Err(e) => Ok(Json(
            json!({ "ok": false, "namespace": ns, "total": 0, "items": [], "message": e }),
        )),
    }
}

#[derive(Deserialize)]
pub struct ConfigRef {
    pub data_id: String,
    #[serde(default = "default_nacos_group")]
    pub group: String,
    /// 覆盖集群默认命名空间
    #[serde(default)]
    pub namespace: Option<String>,
}

/// 覆盖命名空间。
///
/// 语义必须是「字段缺失 = 用集群默认;显式给值(**含空串**)= 就用这个」——
/// 因为 public 在 Nacos 里的 id 本身就是空串。如果把空串也当成「没填」,
/// 那么当集群登记的默认空间不是 public 时,调用方就永远无法指向 public,
/// 只会静默落到集群默认空间上(删错、同步错空间)。
fn with_namespace(mut conn: NacosConn, ns: &Option<String>) -> NacosConn {
    if let Some(n) = ns {
        conn.namespace = n.trim().to_string();
    }
    conn
}

/// `GET /nacos/clusters/{id}/configs/detail` — 取一条配置的正文(供预览/对比)。
pub async fn config_detail(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ConfigRef>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let c = load_cluster(&st, &id).await?;
    let conn = with_namespace(conn_for(&st, &c)?, &q.namespace);
    let base = conn.bases.first().ok_or_else(|| AppError::BadRequest("集群没有配置任何地址".into()))?;
    let token = access_token(base, &conn.username, &conn.password)
        .await
        .map_err(AppError::BadRequest)?;
    match get_config(base, &token, &conn.namespace, &q.data_id, &q.group).await {
        Ok(Some(content)) => Ok(Json(json!({
            "ok": true, "data_id": q.data_id, "group": q.group,
            "namespace": conn.namespace, "bytes": content.len(), "content": content
        }))),
        Ok(None) => Ok(Json(json!({ "ok": false, "message": "配置不存在" }))),
        Err(e) => Ok(Json(json!({ "ok": false, "message": e }))),
    }
}

/// `DELETE /nacos/clusters/{id}/configs` — 删除一条配置(admin,落审计)。
pub async fn delete_config_api(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ConfigRef>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let c = load_cluster(&st, &id).await?;
    let conn = with_namespace(conn_for(&st, &c)?, &q.namespace);
    let base = conn
        .bases
        .first()
        .ok_or_else(|| AppError::BadRequest("集群没有配置任何地址".into()))?
        .clone();
    let token = access_token(&base, &conn.username, &conn.password)
        .await
        .map_err(AppError::BadRequest)?;
    let out = delete_config(&base, &token, &conn.namespace, &q.data_id, &q.group).await;
    let payload = json!({ "data_id": q.data_id, "group": q.group, "namespace": conn.namespace });
    done(&st, &user, &c, "nacos_config_delete", payload, out).await
}

#[derive(Deserialize)]
pub struct SyncReq {
    /// 落地成模板的名字;留空则自动生成
    #[serde(default)]
    pub template_name: String,
    /// 覆盖集群默认命名空间
    #[serde(default)]
    pub namespace: Option<String>,
    /// 只看不存
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST /nacos/clusters/{id}/sync` — 把远端命名空间的配置整体同步回来,
/// 存成一个可直接用于「初始化」的配置模板(于是 dev→test 的克隆变成两步)。
pub async fn sync_cluster(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SyncReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let c = load_cluster(&st, &id).await?;
    let conn = with_namespace(conn_for(&st, &c)?, &req.namespace);
    let ns = conn.namespace.clone();

    let items = pull_configs(&conn).await.map_err(AppError::BadRequest)?;
    let summary: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "data_id": i.data_id, "group": i.group, "type": i.kind,
                "bytes": i.content.len(), "empty": i.content.is_empty(),
            })
        })
        .collect();
    let total = items.len() as i64;

    if req.dry_run {
        return Ok(Json(json!({
            "ok": true, "dry_run": true, "total": total, "namespace": ns,
            "template_id": Value::Null, "template_name": "", "items": summary
        })));
    }
    if items.is_empty() {
        return Err(AppError::BadRequest("该命名空间没有配置可同步".into()));
    }

    let name = if req.template_name.trim().is_empty() {
        format!("{} · {} 同步", c.name, if ns.is_empty() { "public" } else { &ns })
    } else {
        req.template_name.trim().to_string()
    };
    let tpl_id = Uuid::new_v4().to_string();
    st.store
        .save_nacos_template(&NacosTemplateRow {
            id: tpl_id.clone(),
            name: name.clone(),
            note: format!(
                "从「{}」命名空间 {} 同步,共 {} 条",
                c.name,
                if ns.is_empty() { "public".into() } else { ns.clone() },
                total
            ),
            items: serde_json::to_string(&items).unwrap_or_else(|_| "[]".into()),
            created_at: now_secs(),
            // 同步下来的是真实配置,里面的 ${} 属于应用,回放时必须原样发出去
            literal: 1,
            namespace: ns.clone(),
        })
        .await
        .map_err(AppError::Internal)?;

    audit_admin(
        &st,
        &user,
        &c,
        "nacos_sync",
        json!({ "namespace": ns, "total": total, "template_id": tpl_id, "template_name": name }),
        "ok",
    )
    .await;

    Ok(Json(json!({
        "ok": true, "dry_run": false, "total": total, "namespace": ns,
        "template_id": tpl_id, "template_name": name, "items": summary
    })))
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

    // items: explicit ones win, otherwise the template's.
    // `literal` 模板(同步回来的真实配置)按原文下发 —— 里面的 `${...}` 是应用自己的
    // 占位符,不是 opsctl 的模板变量,拿去代入只会让整批回放失败。
    let (items, template_id, template_name, mut literal, tpl_ns) = if !req.items.is_empty() {
        (req.items.clone(), String::new(), String::new(), false, String::new())
    } else if let Some(tid) = req.template_id.clone().filter(|t| !t.is_empty()) {
        let t = st
            .store
            .get_nacos_template(&tid)
            .await
            .map_err(AppError::Internal)?
            .ok_or_else(|| AppError::BadRequest("配置模板不存在".into()))?;
        let parsed: Vec<NacosConfigItem> = serde_json::from_str(&t.items)
            .map_err(|e| AppError::BadRequest(format!("模板配置项无法解析:{e}")))?;
        (parsed, t.id, t.name, t.literal != 0, t.namespace)
    } else {
        (Vec::new(), String::new(), String::new(), false, String::new())
    };
    if items.is_empty() {
        return Err(AppError::BadRequest("没有要初始化的配置项".into()));
    }
    // 调用方可以显式覆盖(substitute=false 强制原文,true 强制代入)
    if let Some(sub) = req.substitute {
        literal = !sub;
    }

    // 目标命名空间优先级:请求显式指定(含空串 = public)> 模板归属 > 集群默认。
    // 配置在 Nacos 里是按命名空间硬隔离的,发错空间等于发到别的环境。
    let mut conn = conn_for(&st, &c)?;
    if let Some(ns) = req.namespace.as_ref() {
        conn.namespace = ns.trim().to_string();
    } else if !tpl_ns.is_empty() {
        conn.namespace = tpl_ns;
    }

    // 命名空间必须真实存在:Nacos 配置接口对任意 tenant 都照单全收,写进未注册的
    // tenant 就成了控制台里看不见的「孤儿配置」。检查是尽力而为:命名空间接口
    // 查不了(老版本没有 console API / 账号权限不足)就跳过并告警,不挡初始化;
    // 确认不存在时:真跑 → 自动注册(id = name = 输入值),试运行 → 只预告。
    let mut ns_note: Option<NacosItemResult> = None;
    if !conn.namespace.is_empty() {
        let checked = match admin_ctx(&conn).await {
            Ok(ctx) => match list_namespaces(&ctx).await {
                Ok(list) => Some((ctx, list)),
                Err(e) => {
                    tracing::warn!(ns = %conn.namespace, err = %e, "查询命名空间失败,跳过存在性检查");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(ns = %conn.namespace, err = %e, "控制台接口不可用,跳过命名空间检查");
                None
            }
        };
        if let Some((ctx, list)) = checked {
            let exists = list
                .iter()
                .any(|n| n.get("namespace_id").and_then(|v| v.as_str()) == Some(conn.namespace.as_str()));
            if !exists {
                let ns = conn.namespace.clone();
                if req.dry_run {
                    ns_note = Some(NacosItemResult {
                        data_id: format!("命名空间 {ns}"),
                        group: String::new(),
                        status: "would_create".into(),
                        message: "未在 Nacos 注册,执行时将自动创建".into(),
                    });
                } else {
                    create_namespace(&ctx, &ns, &ns, "opsctl 初始化时自动创建")
                        .await
                        .map_err(|e| AppError::BadRequest(format!("命名空间 {ns} 不存在,自动创建失败:{e}")))?;
                    ns_note = Some(NacosItemResult {
                        data_id: format!("命名空间 {ns}"),
                        group: String::new(),
                        status: "created".into(),
                        message: "未在 Nacos 注册,已自动创建".into(),
                    });
                }
            }
        }
    }

    let empty_vars = BTreeMap::new();
    let vars = if literal { &empty_vars } else { &req.vars };
    let mut results = init_configs(&conn, &items, vars, literal, req.overwrite, req.dry_run).await;
    if let Some(n) = ns_note {
        results.insert(0, n);
    }
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
    /// true = 按原文下发,不做 `${}` 变量代入
    #[serde(default)]
    pub literal: bool,
    /// 模板归属/默认目标命名空间
    #[serde(default)]
    pub namespace: String,
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
            literal: i64::from(req.literal),
            namespace: req.namespace.trim().to_string(),
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

// ---- 以「用户」为中心的授权视图 ----
//
// Nacos 的模型是 用户 →(绑定)→ 角色 →(赋权)→ 资源。三张表分开看,谁能动哪个命名空间
// 根本看不出来。这里把它折叠成一个问题:**这个账号能操作哪些命名空间**,
// 并提供一步到位的授权(需要角色就自动建一个),把中间那层角色藏在后面但仍如实展示。

/// 资源串 `<namespaceId>:<group>:<type>/<name>` 的第一段就是命名空间(public 为空)。
fn resource_namespace(resource: &str) -> String {
    resource.split(':').next().unwrap_or_default().to_string()
}

/// 已存在的 (role, resource, action) 三元组。
///
/// Nacos 的 permissions 表对这三列有唯一约束,重复插入会报错;而「批量 / 模板」的前提
/// 就是可以反复执行。所以下发前先拉一次现状,已经有的直接跳过,别把重跑变成一堆失败。
async fn existing_grants(ctx: &AdminCtx) -> Vec<(String, String, String)> {
    match list_permissions(ctx, 1, 500, "").await {
        Ok((_, rows)) => rows
            .iter()
            .map(|p| (str_at(p, "role"), str_at(p, "resource"), str_at(p, "action")))
            .collect(),
        // 拉不到就当没有:后续真重复了由服务端报错,不比现在更差
        Err(_) => Vec::new(),
    }
}

/// 幂等赋权:已存在返回 `Ok(false)`(未写入),新增返回 `Ok(true)`。
async fn grant_if_absent(
    ctx: &AdminCtx,
    known: &mut Vec<(String, String, String)>,
    role: &str,
    resource: &str,
    action: &str,
) -> Result<bool, String> {
    let key = (role.to_string(), resource.to_string(), action.to_string());
    if known.contains(&key) {
        return Ok(false);
    }
    grant_permission(ctx, role, resource, action).await?;
    known.push(key);
    Ok(true)
}

/// 该用户绑定的角色(排除 ROLE_ADMIN —— 那是全局管理员,不由这里管理)。
async fn roles_of(ctx: &AdminCtx, username: &str) -> Result<(Vec<String>, bool), String> {
    let (_, rows) = list_roles(ctx, 1, 500).await?;
    let mut roles = Vec::new();
    let mut is_admin = false;
    for r in rows {
        if str_at(&r, "username") != username {
            continue;
        }
        let role = str_at(&r, "role");
        if role == "ROLE_ADMIN" {
            is_admin = true;
        } else if !roles.contains(&role) {
            roles.push(role);
        }
    }
    Ok((roles, is_admin))
}

/// `GET /nacos/clusters/{id}/users/{username}/access`
/// —— 一个账号的角色 + 这些角色带来的权限(按命名空间归并)。
pub async fn user_access(
    user: AuthUser,
    State(st): State<AppState>,
    Path((id, username)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (_, ctx) = ctx_for(&st, &id).await?;
    let (roles, is_admin) = roles_of(&ctx, &username).await.map_err(AppError::BadRequest)?;
    let (_, all) = list_permissions(&ctx, 1, 500, "").await.map_err(AppError::BadRequest)?;
    let grants: Vec<Value> = all
        .iter()
        .filter(|p| roles.contains(&str_at(p, "role")))
        .map(|p| {
            let resource = str_at(p, "resource");
            json!({
                "role": str_at(p, "role"),
                "resource": resource.clone(),
                "namespace_id": resource_namespace(&resource),
                "action": str_at(p, "action"),
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true, "username": username, "roles": roles,
        "global_admin": is_admin, "grants": grants
    })))
}

#[derive(Deserialize, Clone)]
pub struct GrantReq {
    pub username: String,
    /// 命名空间 id;空串 = public
    #[serde(default)]
    pub namespace_id: String,
    /// r | w | rw
    pub action: String,
    /// 配置分组 / 服务分组,留空或 `*` = 全部
    #[serde(default)]
    pub group: String,
    /// 资源类型:`config`(配置)| `naming`(服务)| `*`(两者都要)
    #[serde(default)]
    pub kind: String,
    /// dataId / serviceName,留空或 `*` = 全部
    #[serde(default)]
    pub name: String,
}

/// 按 Nacos 的资源模型拼资源串:`<namespaceId>:<group>:<type>/<name>`
/// (源码 `NacosRoleServiceImpl#joinResource`,分隔符 `:`、通配 `*`)。
///
/// 三个要点,少一个权限就形同虚设:
/// - public 的 id 是空串 → 首段留空,控制台写出来就是 `:*:*`
/// - 不区分配置/服务时,第三段**整体**塌缩成 `*`,而不是 `*/\*` —— 授权判定是把存储的
///   资源串按 `*`→`.*` 变成正则去整串匹配,写错第三段就永远匹配不上
/// - group / name 留空一律按 `*`
fn build_resource(namespace_id: &str, group: &str, kind: &str, name: &str) -> String {
    let g = {
        let g = group.trim();
        if g.is_empty() { "*" } else { g }
    };
    let k = kind.trim();
    if k.is_empty() || k == "*" {
        return format!("{namespace_id}:{g}:*");
    }
    let n = {
        let n = name.trim();
        if n.is_empty() { "*" } else { n }
    };
    format!("{namespace_id}:{g}:{k}/{n}")
}

fn validate_kind(kind: &str) -> Result<(), String> {
    match kind.trim() {
        "" | "*" | "config" | "naming" => Ok(()),
        _ => Err("资源类型只能是 config(配置)、naming(服务)或 *(全部)".into()),
    }
}

/// 这个用户在这个集群上的「工作角色」:已有非 ROLE_ADMIN 角色就复用第一个,
/// 否则用 `<username>-role`(Nacos 里角色就是个字符串,没有独立实体)。
fn work_role(username: &str, roles: &[String]) -> String {
    roles.first().cloned().unwrap_or_else(|| format!("{username}-role"))
}

/// `POST /nacos/clusters/{id}/grant` — 一步授权:选账号 + 命名空间 + 读写(+ 可选的
/// group / 类型 / 名称),缺角色就自动创建并绑定,再按 Nacos 的资源模型赋权。
pub async fn grant_user(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<GrantReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    validate_action(&req.action).map_err(AppError::BadRequest)?;
    validate_kind(&req.kind).map_err(AppError::BadRequest)?;
    if req.username.trim().is_empty() {
        return Err(AppError::BadRequest("请选择账号".into()));
    }

    let (roles, is_admin) = roles_of(&ctx, &req.username).await.map_err(AppError::BadRequest)?;
    if is_admin && roles.is_empty() {
        return Err(AppError::BadRequest(
            "该账号是全局管理员(ROLE_ADMIN),已拥有全部权限,无需单独授权".into(),
        ));
    }
    let role = work_role(&req.username, &roles);
    let created_role = roles.is_empty();
    let resource = build_resource(&req.namespace_id, &req.group, &req.kind, &req.name);

    let payload = json!({
        "username": req.username, "namespace_id": req.namespace_id, "resource": resource,
        "action": req.action, "role": role, "created_role": created_role
    });
    // 顺序不能反:Nacos 赋权时要求角色已存在,否则 400 "role X not found!"
    let mut known = existing_grants(&ctx).await;
    let outcome = async {
        if created_role {
            bind_role(&ctx, &role, &req.username).await?;
        }
        grant_if_absent(&ctx, &mut known, &role, &resource, &req.action).await.map(|_| ())
    }
    .await;

    match outcome {
        Ok(()) => {
            audit_admin(&st, &user, &c, "nacos_grant", payload, "ok").await;
            Ok(Json(json!({
                "ok": true, "role": role, "created_role": created_role, "resource": resource
            })))
        }
        Err(e) => {
            let mut p = payload;
            if let Some(o) = p.as_object_mut() {
                o.insert("error".into(), json!(e));
            }
            audit_admin(&st, &user, &c, "nacos_grant", p, "fail").await;
            Err(AppError::BadRequest(e))
        }
    }
}

/// `DELETE /nacos/clusters/{id}/grant?username=&namespace_id=&action=[&group=&kind=&name=]`
/// —— 收回这条授权。资源串必须和写入时**逐字一致**,否则 Nacos 找不到那一行。
/// 所以调用方应把列表里回读到的 `resource` 原样传回来(`resource` 参数优先)。
pub async fn revoke_user(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RevokeReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    let (roles, _) = roles_of(&ctx, &q.username).await.map_err(AppError::BadRequest)?;
    if roles.is_empty() {
        return Err(AppError::BadRequest("该账号没有可收回的角色".into()));
    }
    let resource = match q.resource.as_ref().filter(|r| !r.trim().is_empty()) {
        Some(r) => r.trim().to_string(),
        None => build_resource(&q.namespace_id, &q.group, &q.kind, &q.name),
    };
    let mut last_err = None;
    let mut done_any = false;
    for role in &roles {
        match revoke_permission(&ctx, role, &resource, &q.action).await {
            Ok(()) => done_any = true,
            Err(e) => last_err = Some(e),
        }
    }
    let payload = json!({ "username": q.username, "resource": resource, "action": q.action });
    let out = if done_any { Ok(()) } else { Err(last_err.unwrap_or_else(|| "收回失败".into())) };
    done(&st, &user, &c, "nacos_revoke", payload, out).await
}

#[derive(Deserialize)]
pub struct RevokeReq {
    pub username: String,
    #[serde(default)]
    pub namespace_id: String,
    pub action: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    /// 列表里回读到的资源串,原样传回最稳妥
    #[serde(default)]
    pub resource: Option<String>,
}

#[derive(Deserialize)]
pub struct BatchGrantReq {
    pub username: String,
    /// 一次给多个命名空间授同一份权限(空串 = public)
    pub namespaces: Vec<String>,
    pub action: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

/// `POST /nacos/clusters/{id}/grant/batch` — 同一个账号 × 多个命名空间,一次授完。
/// 角色只建一次;逐个命名空间报结果,某个失败不影响其它。
pub async fn grant_user_batch(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<BatchGrantReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;
    validate_action(&req.action).map_err(AppError::BadRequest)?;
    validate_kind(&req.kind).map_err(AppError::BadRequest)?;
    if req.username.trim().is_empty() {
        return Err(AppError::BadRequest("请选择账号".into()));
    }
    if req.namespaces.is_empty() {
        return Err(AppError::BadRequest("请至少选择一个命名空间".into()));
    }

    let (roles, is_admin) = roles_of(&ctx, &req.username).await.map_err(AppError::BadRequest)?;
    if is_admin && roles.is_empty() {
        return Err(AppError::BadRequest(
            "该账号是全局管理员(ROLE_ADMIN),已拥有全部权限,无需单独授权".into(),
        ));
    }
    let role = work_role(&req.username, &roles);
    let mut created_role = false;
    if roles.is_empty() {
        bind_role(&ctx, &role, &req.username).await.map_err(AppError::BadRequest)?;
        created_role = true;
    }

    let mut known = existing_grants(&ctx).await;
    let mut items = Vec::with_capacity(req.namespaces.len());
    let mut ok_count = 0;
    for ns in &req.namespaces {
        let resource = build_resource(ns, &req.group, &req.kind, &req.name);
        match grant_if_absent(&ctx, &mut known, &role, &resource, &req.action).await {
            Ok(added) => {
                ok_count += 1;
                items.push(json!({
                    "namespace_id": ns, "resource": resource,
                    "status": if added { "ok" } else { "exists" },
                    "message": if added { "" } else { "已有相同授权,未重复写入" }
                }));
            }
            Err(e) => items.push(json!({
                "namespace_id": ns, "resource": resource, "status": "fail", "message": e
            })),
        }
    }
    let total = items.len() as i64;
    let status = if ok_count == total {
        "ok"
    } else if ok_count == 0 {
        "fail"
    } else {
        "partial"
    };
    audit_admin(
        &st,
        &user,
        &c,
        "nacos_grant_batch",
        json!({
            "username": req.username, "role": role, "action": req.action,
            "total": total, "ok_count": ok_count, "items": items
        }),
        status,
    )
    .await;
    Ok(Json(json!({
        "ok": ok_count > 0, "status": status, "role": role, "created_role": created_role,
        "total": total, "ok_count": ok_count, "items": items
    })))
}

// ---- 账号模板:照着单子把人开出来 ----

/// 模板里的一条:账号 + 默认口令 + 要授到哪些命名空间。
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct AccountItem {
    pub username: String,
    /// 默认口令。真要上生产应逐个改掉,这里只是让「批量开号」可执行。
    #[serde(default)]
    pub password: String,
    /// r | w | rw;留空 = 只建账号不授权
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

pub async fn list_account_templates(
    user: AuthUser,
    State(st): State<AppState>,
) -> Result<Json<Vec<crate::store::NacosAccountTemplateRow>>, AppError> {
    admin(&user)?;
    Ok(Json(st.store.list_nacos_account_templates().await.map_err(AppError::Internal)?))
}

#[derive(Deserialize)]
pub struct SaveAccountTemplate {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub items: Vec<AccountItem>,
}

pub async fn save_account_template(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<SaveAccountTemplate>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("请填写模板名称".into()));
    }
    for it in &req.items {
        if it.username.trim().is_empty() {
            return Err(AppError::BadRequest("账号名不能为空".into()));
        }
        if !it.action.trim().is_empty() {
            validate_action(it.action.trim()).map_err(AppError::BadRequest)?;
        }
        validate_kind(&it.kind).map_err(AppError::BadRequest)?;
    }
    let id = req.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
    let created_at = match st
        .store
        .get_nacos_account_template(&id)
        .await
        .map_err(AppError::Internal)?
    {
        Some(old) => old.created_at,
        None => now_secs(),
    };
    st.store
        .save_nacos_account_template(&crate::store::NacosAccountTemplateRow {
            id: id.clone(),
            name: req.name.trim().to_string(),
            note: req.note,
            items: serde_json::to_string(&req.items).unwrap_or_else(|_| "[]".into()),
            created_at,
        })
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_account_template(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    st.store.delete_nacos_account_template(&id).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct ApplyAccountsReq {
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub items: Vec<AccountItem>,
    /// 只报告要做什么,不写远端
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST /nacos/clusters/{id}/accounts/apply` — 把账号模板落到集群上:
/// 建账号(已存在则跳过,**不会重置别人的口令**)→ 建角色 → 逐个命名空间赋权。
pub async fn apply_accounts(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ApplyAccountsReq>,
) -> Result<Json<Value>, AppError> {
    admin(&user)?;
    let (c, ctx) = ctx_for(&st, &id).await?;

    let (items, tpl_name) = if !req.items.is_empty() {
        (req.items.clone(), String::new())
    } else if let Some(tid) = req.template_id.clone().filter(|t| !t.is_empty()) {
        let t = st
            .store
            .get_nacos_account_template(&tid)
            .await
            .map_err(AppError::Internal)?
            .ok_or_else(|| AppError::BadRequest("账号模板不存在".into()))?;
        let parsed: Vec<AccountItem> = serde_json::from_str(&t.items)
            .map_err(|e| AppError::BadRequest(format!("模板条目无法解析:{e}")))?;
        (parsed, t.name)
    } else {
        (Vec::new(), String::new())
    };
    if items.is_empty() {
        return Err(AppError::BadRequest("没有要创建的账号".into()));
    }

    // 已有账号/角色先拉一次,避免逐条去问
    let (_, existing) = list_users(&ctx, 1, 500).await.map_err(AppError::BadRequest)?;
    let existing: Vec<String> = existing.iter().map(|u| str_at(u, "username")).collect();
    // 现有授权先拉一次:重跑模板时已有的直接跳过,不去撞 Nacos 的唯一约束
    let mut known_grants = existing_grants(&ctx).await;

    let mut results = Vec::with_capacity(items.len());
    let mut ok_count = 0i64;
    for it in &items {
        let username = it.username.trim().to_string();
        let mut grants = Vec::new();
        let mut failed = false;

        let already = existing.contains(&username);
        let user_status = if already {
            "exists"
        } else if req.dry_run {
            "would_create"
        } else {
            match create_user(&ctx, &username, &it.password).await {
                Ok(()) => "created",
                Err(e) => {
                    failed = true;
                    results.push(json!({
                        "username": username, "status": "fail", "message": e, "grants": []
                    }));
                    continue;
                }
            }
        };

        // 授权(action 留空 = 只建号)
        if !it.action.trim().is_empty() && !it.namespaces.is_empty() {
            let (roles, is_admin) =
                roles_of(&ctx, &username).await.unwrap_or_else(|_| (Vec::new(), false));
            let role = work_role(&username, &roles);
            if is_admin && roles.is_empty() {
                grants.push(json!({
                    "namespace_id": "*", "status": "skipped",
                    "message": "全局管理员,无需逐个命名空间授权"
                }));
            } else {
                if roles.is_empty() && !req.dry_run {
                    if let Err(e) = bind_role(&ctx, &role, &username).await {
                        failed = true;
                        grants.push(json!({
                            "namespace_id": "*", "status": "fail", "message": e
                        }));
                    }
                }
                for ns in &it.namespaces {
                    let resource = build_resource(ns, &it.group, &it.kind, &it.name);
                    if req.dry_run {
                        grants.push(json!({
                            "namespace_id": ns, "resource": resource,
                            "status": "would_grant", "message": ""
                        }));
                        continue;
                    }
                    match grant_if_absent(&ctx, &mut known_grants, &role, &resource, it.action.trim())
                        .await
                    {
                        Ok(added) => grants.push(json!({
                            "namespace_id": ns, "resource": resource,
                            "status": if added { "ok" } else { "exists" },
                            "message": if added { "" } else { "已有相同授权" }
                        })),
                        Err(e) => {
                            failed = true;
                            grants.push(json!({
                                "namespace_id": ns, "resource": resource,
                                "status": "fail", "message": e
                            }));
                        }
                    }
                }
            }
        }

        if !failed {
            ok_count += 1;
        }
        results.push(json!({
            "username": username,
            "status": if failed { "fail" } else { user_status },
            "message": "",
            "grants": grants,
        }));
    }

    let total = results.len() as i64;
    let status = if ok_count == total {
        "ok"
    } else if ok_count == 0 {
        "fail"
    } else {
        "partial"
    };
    if !req.dry_run {
        audit_admin(
            &st,
            &user,
            &c,
            "nacos_accounts_apply",
            json!({
                "template": tpl_name, "total": total, "ok_count": ok_count, "items": results
            }),
            status,
        )
        .await;
    }
    Ok(Json(json!({
        "ok": ok_count > 0, "status": status, "dry_run": req.dry_run,
        "template_name": tpl_name, "total": total, "ok_count": ok_count, "items": results
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_follows_nacos_join_rules() {
        // 整个命名空间(控制台唯一会写的形状)
        assert_eq!(build_resource("dev-ns", "", "", ""), "dev-ns:*:*");
        assert_eq!(build_resource("dev-ns", "*", "*", "*"), "dev-ns:*:*");
        // public 的 id 是空串 → 首段留空
        assert_eq!(build_resource("", "", "", ""), ":*:*");
        // 限定分组:第三段仍整体塌缩成 *
        assert_eq!(build_resource("dev-ns", "DEFAULT_GROUP", "*", "app.yaml"), "dev-ns:DEFAULT_GROUP:*");
        // 限定类型后才带 <type>/<name>
        assert_eq!(
            build_resource("dev-ns", "DEFAULT_GROUP", "config", "app.yaml"),
            "dev-ns:DEFAULT_GROUP:config/app.yaml"
        );
        assert_eq!(build_resource("dev-ns", "", "config", ""), "dev-ns:*:config/*");
        assert_eq!(build_resource("dev-ns", "G", "naming", "order-svc"), "dev-ns:G:naming/order-svc");
        // 两侧空白不该混进资源串
        assert_eq!(build_resource("dev-ns", " G ", " config ", " a.yaml "), "dev-ns:G:config/a.yaml");
    }

    #[test]
    fn kind_whitelist_rejects_typos() {
        for ok in ["", "*", "config", "naming"] {
            assert!(validate_kind(ok).is_ok(), "{ok} 应被接受");
        }
        // 写错类型服务端不会报错,但授权判定永远匹配不上 —— 必须本地挡住
        assert!(validate_kind("configs").is_err());
        assert!(validate_kind("cs").is_err());
    }

    #[test]
    fn base_urls_normalize_scheme_port_and_context() {
        assert_eq!(
            base_urls("10.0.0.1, http://n2:8849/nacos ,", "/nacos"),
            vec!["http://10.0.0.1:8848/nacos", "http://n2:8849/nacos"]
        );
        // 写明 scheme 按 URL 语义补标准端口(反代常见);裸 host 仍按 Nacos 惯例补 8848
        assert_eq!(
            base_urls("http://nacos.n11, https://nacos.n11, nacos.n11", "/nacos"),
            vec![
                "http://nacos.n11:80/nacos",
                "https://nacos.n11:443/nacos",
                "http://nacos.n11:8848/nacos"
            ]
        );
        // empty context path (standalone deployments serving at the root)
        assert_eq!(base_urls("n1:8848", "/"), vec!["http://n1:8848"]);
        assert!(base_urls("  ,  ", "/nacos").is_empty());
    }

    #[test]
    fn alternate_base_flips_the_port_convention() {
        // 裸 host(默认 8848)→ 试 80;建议写法可直接粘回表单
        assert_eq!(
            alternate_base("nacos.n11", "/nacos"),
            Some(("http://nacos.n11:80/nacos".into(), "nacos.n11:80".into()))
        );
        // 写明 scheme(默认 80)→ 试 8848
        assert_eq!(
            alternate_base("http://n1", "/nacos"),
            Some(("http://n1:8848/nacos".into(), "http://n1:8848".into()))
        );
        // 显式端口没有第二种猜法
        assert_eq!(alternate_base("n1:8848", "/nacos"), None);
    }

    #[test]
    fn ipv6_literals_bracketed_and_bare() {
        // 标准括号写法:带端口 / 不带端口(默认 8848)
        assert_eq!(
            base_urls("[2001:db8::1]:8080, [::1]", "/nacos"),
            vec!["http://[2001:db8::1]:8080/nacos", "http://[::1]:8848/nacos"]
        );
        // 裸 IPv6 字面量(无括号、多冒号)整体视作 host
        assert_eq!(base_urls("2001:db8::1", "/nacos"), vec!["http://[2001:db8::1]:8848/nacos"]);
        // 带 scheme 的 URL 写法按 URL 语义走 80
        assert_eq!(base_urls("http://[::1]/custom", "/nacos"), vec!["http://[::1]:80/custom"]);
        // 括号不闭合 / `]` 后带非法尾巴 → 拒绝
        assert!(base_urls("[::1", "/nacos").is_empty());
        assert!(base_urls("[::1]junk", "/nacos").is_empty());
        // 端口回退建议同样保持括号写法
        assert_eq!(
            alternate_base("[::1]", "/nacos"),
            Some(("http://[::1]:80/nacos".into(), "[::1]:80".into()))
        );
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
