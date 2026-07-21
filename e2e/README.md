# opsctl e2e (Playwright)

独立的端到端测试工程,与 `web/` 完全隔离(自己的 `package.json` / `node_modules`)。

## 端口 / 隔离策略
- **前端和后端是同一个进程、同一个端口**:Rust 服务端(`opsctl-server`)既提供 `/api/*`,
  又托管 `web/dist` 的 SPA,前端用相对路径 `/api`。所以前后端天然同源,**端口随机不影响通信**,
  只有测试驱动器需要知道端口(它知道)。
- 每次运行,`playwright.config.js` 选一个**随机高端口**(可用 `OPSCTL_E2E_PORT` 覆盖),
  用 Playwright `webServer` 启动一个**隔离的 opsctl-server**:
  - `OPSCTL_SERVER__BIND` = 该随机端口
  - `OPSCTL_STORE__URL` = 全新临时库 `target/e2e-<port>.db`
  - `OPSCTL_VAULT__PASSPHRASE=e2e-pass`(全新库首次解封即设定,金库解封→可建账号/密钥)
  - `OPSCTL_DEV__SEED=true`,admin/admin
- 跑完 `global-teardown.js` 清扫所有 `target/e2e-*.db`。不依赖、不污染任何手动起的 dev 服务。

## 前置(一次)
```powershell
cd e2e
$env:PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1"   # 复用系统 Chrome(channel),不下载 chromium
npm install
```

## 运行
```powershell
cd e2e
[Console]::OutputEncoding=[Text.Encoding]::UTF8; chcp 65001   # 中文 UTF-8 输出
$env:PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1"
npm test            # 自动起隔离服务、随机端口、跑完销毁
npm run report      # 打开 HTML 报告
```

## 关键约定
- 认证:`tests/fixtures.js` 通过 REST API 登录并注入 localStorage token。**每个用例唯一 device_id**
  (`e2e-<testId>`)—— 服务端「每个 (user,device) 只保留一个 session」,共享 device 会互相踢下线(401)。
- 所有 API 调用用**相对路径**(跟随 `baseURL` 的随机端口),不硬编码端口。
- 需要数据的用例**自造数据**:如 `approvals.spec.js` 通过「admin 把规则设为需审批 → operator 提交命令」
  产生真实待审批,不依赖预存数据。

## 覆盖(10 spec / 30 用例)
- auth:登录 UI(记住设备/忘记密码/无预填/**中文错误提示**)
- nav:右上角用户下拉、进入设置、侧栏菜单齐全
- console:工具条/已选计数/全选/破坏性二次确认/IP 搜索
- approvals:今日统计、审批详情抽屉(自造待审批数据)
- messages:同步筛选档、双栏详情+来源、标记未读回切
- settings:TOTP 二维码+密钥后备、14 天有效期、git 本地路径输入
- assets:行内启停/编辑/删除、启停切换、账号/标签 tab
- templates:审批人选择器
- jobs:执行记录双 Tab、时间/状态筛选、record 追溯页、抽屉单独打开
- **journey(真人全流程)**:建空间 → 在空间里建数据库节点并绑账号 → 执行页勾选节点 →
  输入 SQL 执行(指向真实 demo sqlite,命令真正跑通返回结果)→ 执行记录里核对
