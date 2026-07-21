# opsctl — 运维金库(vault)平台

一个「运维金库」:对服务器/数据库的远程操作都经受控服务端(vault)代理执行,统一管人、管权、管密钥、管审计。参照 JumpServer 的「资产授权规则」模型。

Rust workspace:`core`(共享模型/DTO)、`server`(opsctl-vault,axum + SQLite,内嵌 Web 前端)、`client`(egui 桌面端,暂停维护)。前端 `web/`(Vue3 + NaiveUI)。

## 架构
- **单二进制**:release 编译期用 rust-embed 把 `web/dist` 内嵌进服务端;运行时 `/` 出 SPA、`/api` 出接口,同源。
- **鉴权**:账号 argon2;登录 = 设备绑定 JWT(`sub/did/sid/role/exp`)+ DB 会话注册表;每请求校验 `did`+`sid`,撤销即时生效;可选 OTP 两步、自助注册开关。
- **授权**:资产授权规则引擎(主体 × 资产[子树/标签/集] × 系统账号 × 动作[ssh/sql] × 有效期)。
- **凭据金库**:系统账号密码/密钥用口令派生密钥(argon2 + ChaCha20-Poly1305)**加密静态存储**;启动口令解封,封存态拒绝取用凭据。
- **执行**:服务端代理 SSH(russh)/ SQL(sqlite);命中「需审批」规则则挂起,管理员放行/驳回(支持批量),全程逐目标审计。
- **其它屏**:执行模板(变量代入)、消息中心(站内通知)、审计(筛选+详情+CSV/JSON 导出)、设置(个人/会话撤销/金库/Telegram·Git 配置)。

> 演示态(未接外部系统,UI 标注):Telegram bot、真实 git push、mysql/postgres。

## 本地运行
最简单(自动解封 + 内嵌前端,http://127.0.0.1:8443/,登录 admin/admin):
```bash
./run.sh              # 或 Windows: ./run.ps1
./run.sh --release    # release 单二进制(SPA 内嵌)
```
前后端分离热开发(前端 :5173 代理到后端 :8443):
```bash
OPSCTL_VAULT__PASSPHRASE=dev cargo run -p opsctl-server      # 后端
cd web && npm run dev                                        # 前端
```
> 不设 `OPSCTL_VAULT__PASSPHRASE` 则金库封存:带密码的账号建不了、远程执行取不到凭据,需登录后在「设置 → 凭据金库」手动解封。

## 测试
```bash
cargo test -p opsctl-server      # 32 个 API 集成测试(临时 sqlite,隔离并行)
```

## 部署
见 [`deploy/README.md`](deploy/README.md):docker compose / Kubernetes / 裸机单二进制,含密钥(JWT、金库口令、admin 密码)与 TLS/持久化说明。

## 构建备忘(Windows 原生依赖)
- `jsonwebtoken` 10 → 启用 `rust_crypto` provider。
- `russh` → `default-features=false, features=["ring","flate2","rsa"]`(避开 aws-lc-rs/NASM)。
- sqlx 仅编译 `sqlite`(mysql/pg 待接);SQLite 单写入,横向扩容需换 Postgres。
