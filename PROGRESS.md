# opsctl 对齐原型 · 进度记录

> **2026-07-16 轮次(已完成)**:三项体验改进——
> **A git 文件「打开所在位置」**(文件查看接口加 abs_path/exists(模板视图仅 admin 可见);新端点 POST /settings/git/reveal:admin-only,`resolve_in_work_dir` 严格校验路径不可越出 work_dir,Windows `explorer /select,` 定位文件、mac `open -R`、Linux `xdg-open`;get_git 返回 work_dir_abs;资产/模板文件弹窗显示磁盘位置+「打开所在位置」(未同步禁用+提示),设置页 Git 卡片「打开文件夹」+绝对路径展示)、
> **B 头像菜单「我的会话与设备」锚点直达**(跳 `/settings#sessions`,hash 路由下 vue-router 正常解析 route.hash;Settings 会话卡片 scrollIntoView 平滑滚动 + 1.8s 高亮描边动画)、
> **C 全局滚动条美化**(App.vue:WebKit ::-webkit-scrollbar 9px 圆角细条 + Firefox scrollbar-width/color,深色主题配色)。
> **附带修复存量 bug**:Templates.vue 的文件查看 n-modal 原来放在 n-grid 直接子级——naive-ui grid 静默丢弃非 n-gi 子节点,该弹窗从未渲染过;移出 grid 后恢复。
> 验证:`cargo test -p opsctl-server` 61 全绿(新增 2:reveal 路径校验);e2e 全量 46 项通过;Playwright UI 实测 5/5(锚点落点、打开文件夹按钮、两个弹窗磁盘位置+按钮);reveal 真实弹出资源管理器选中文件,越界路径 400「非法路径」。dist-win 打包版已更新并在跑。

> **2026-07-10 轮次(已完成)**:补齐四个原型功能差距——
> **A 执行来源真实化**(ClientIp 提取器:XFF 首段→ConnectInfo peer;sessions.ip 真实落库;jobs 表加 source_ip/source_device;Record 页动态渲染「Web 控制台 · IP · 设备」)、
> **B 模板执行留痕**(SubmitSsh/SqlJob 加 template_id,jobs 表存 template_id/template_name,Console 提交时带当前选中模板,Audit 执行记录加「模板」列,Record 概要加模板行)、
> **C 真实备份子系统**(`server/src/backup.rs`:启动补偿 + 每日本地 03:00 `VACUUM INTO` 快照至 `[backup] dir`(默认 data/backups),retention_days 清理(默认 30 天),settings 记 backup_last_at/file;端点 GET /api/backup/status(登录可读)+ POST /api/backup/run(admin);Audit 执行记录 Tab 备份横幅 + Record「审计与备份」区块,文案如实=本地快照)、
> **D 审批审核方式分级**(rules/approvals 加 quick 列 console|tg,CreateRule 校验,rbac::Authz 传递,ApprovalView 带出;Access 表单「审核方式」select,Approvals 列表/子任务/抽屉展示「控制台 / TG 一键·演示」;TG 真实执行路径未做)。
> 验证:`cargo test -p opsctl-server` **59 全绿**(新增 6:session ip/job source/template×1/backup×2/quick×1);e2e 新增 `roundtrip-new.spec.js` 4 用例,全量 46 项通过;headed(有头窗口,slowMo+video)演示通过。e2e webServer 加 `OPSCTL_BACKUP__DIR` 隔离。测试 harness 与 main 均已接 `into_make_service_with_connect_info`。

> 保存时间:2026-07-07。本轮任务:将 opsctl 实现与 Open Design 原型
> (`C:\Users\llux2\AppData\Roaming\Open Design\namespaces\release-stable-win\data\projects\38b421fd-c18c-475c-a8c8-3428292f4b0c`)
> 全面比对后,修 bug + 补核心缺失功能 + 加 job 聚合与追溯页 + 配色对齐。**已全部完成并验证通过。**

## 状态:✅ 完成

- 后端 `cargo build` / `cargo test -p opsctl-server` 全绿(52 项测试,含 3 项新增 job 测试)。
- 前端 `npm run build` 成功。
- 浏览器端到端手工验证全部通过(见文末验证清单)。

---

## 一、已完成的改动(按阶段)

### 阶段 1:后端数据模型
- `core/src/api.rs`:`TargetResult` 加 `duration_ms: i64`(`#[serde(default)]`)。
- `server/src/store.rs`:
  - 新表 `jobs`(id/kind/command/operator_id/operator_email/created_at/finished_at/status/total/ok_count)。
  - 新表 `job_targets`(id/job_id/asset_id/asset_name/status/exit_code/stdout/stderr/error/duration_ms/approval_id/ts)。
  - `audit` 加列 `job_id`(幂等 ALTER,忽略 duplicate column);`AuditRow` 加 `#[sqlx(default)] job_id`。
  - 新结构体 `JobRow`/`JobTargetRow`/`VoteRow`;新方法 `create_job`/`insert_job_target`/
    `update_job_target_result`/`finalize_job_if_done`(返回 `Option<JobRow>`,兼容历史 approval)/
    `list_jobs_filtered`/`get_job`/`list_job_targets`/`list_approvals_for_job`/`list_votes`。
  - `truncate_output`(64KB 截断)保护 DB。

### 阶段 2:后端 API(`server/src/api.rs`、`lib.rs`、`auth.rs`、`jobs.rs`)
- **bug**:`create_asset` 尊重传入 `status`(不再硬编码 enabled),`CreateAsset` 加 `status` 字段。
- `submit_ssh`/`submit_sql`:执行前 `create_job`,`run_ssh_on`/`run_sql_on` 用 `Instant` 计时填 `duration_ms`,
  每 target `insert_job_target`(未授权→fail/命中审批→pending+approval_id/执行→ok|fail),循环后 `finalize_job`。
- `insert_audit` 签名加 `job_id: &str`,所有调用点(jobs.rs/auth.rs/git_action)已同步。
- `decide_one`:approve 达票执行后 / reject 后都 `update_job_target_result` + `finalize_job`,通知 link 改 `/record/{job_id}`。
- `decide_batch`:返回分类统计 `{approved, pending, rejected, failed}`(保留 `ok` 兼容)。
- **消息生产者**:执行完成推 `exec` 消息;git sync 成功推 `sync` 消息(login 消息本就存在)。
- 新端点:`GET /jobs`(非 admin 强制只看自己)、`GET /jobs/{id}`(admin 或属主)。已在 `lib.rs` 注册路由。
- 新集成测试:`server/tests/jobs.rs` 加 `job_history_aggregates_and_scopes_to_owner`、`rejected_approval_finalizes_job`。

### 阶段 3:前端 API 层与路由
- `web/src/api/index.js`:加 `jobs(params)`、`jobDetail(id)`。
- `web/src/router/index.js`:`/audit` 标题改「执行记录」;新增 `/record/:id` → `Record.vue`。

### 阶段 4:视图
- **Console.vue(重写)**:破坏性正则加 `update`;删站点 disabled 逻辑→整站级联勾选;工具条(全选可见/展开/折叠/清空);
  已选 N 计数;搜索扩展到 name+host+站点名;`render-label` 富节点行(图标色/host·类型 meta/标签色点/停用徽标);
  模板变量编辑 UI(实时代入);结果每行耗时 ms + 底部汇总行。
- **Login.vue**:清空预填账号密码、删测试账号提示。
- **Templates.vue**:加「审批人」`n-select multiple`(载入 `api.users()`)。
- **Approvals.vue**:`batchApprove` 按新返回值分类展示(已放行/已投票待会签/驳回/失败)。
- **Messages.vue**:filters 加 `sync` 档位。
- **Audit.vue(重写)**:双 Tab —「执行记录」(job 聚合,人人可见,时间/状态/类型/操作人/关键字筛选,partial 状态,
  抽屉+「单独打开」→ record)+「审计流水」(admin-only,resultText 修正 pending/rejected 为中性/驳回)。
- **Record.vue(新建)**:判决横幅 verdict / 概要 kv / 审批追溯行 / 命令块 / 逐目标结果 / `@media print` 打印存档。
- **MainLayout.vue**:「审计」菜单改名「执行记录」,保持全员可见。

### 阶段 5:配色 token
- `App.vue`:themeOverrides 换青色(primaryColor `#19b8a6` 等)+ 定义全局 CSS 变量(--bg/--surface/--accent 等,镜像原型 app.css)。
- 全局替换 10 个文件里的 GitHub 蓝硬编码色为 CSS 变量;Console 服务器青/数据库 `--accent-2` 双色;seed 默认 tag 色改青。

### 补充轮次(逐页对齐原型 + 用户新需求)
- **MainLayout**:右上角用户区改 n-dropdown(头像+名字·角色+菜单:个人设置/我的会话/退出)。
- **Login**:6 格 OTP(自动跳格/粘贴分发/退格回跳)+ 记住此设备 + 忘记密码链接。
- **Settings**:登录有效期加 14 天;git 本地文件夹/本地仓库模式渲染路径输入(work_dir,put_git 原样存);
  **TOTP 开启加二维码**(naive-ui n-qr-code,零依赖 canvas 渲染,白底黑码可扫 + 密钥文本后备)。
- **Assets**:资产行内启停按钮(调 updateAsset,省略 tag_ids/account_ids 保持不变)。
- **Approvals**:今日已放行/已驳回统计 pills;**审批详情抽屉**(待审批+近期决策都有「详情」入口,
  展示完整字段:状态/提交人/目标/账号/动作/时间/会签进度/审批人/决策人/驳回理由 + 完整命令代码块 +
  放行/驳回/查看执行记录操作)——解决列表横排长命令看不全的问题。
- **Messages**:双栏主从(左列表+右详情)+ 来源字段(会话服务/审批引擎/Git同步/作业队列)+
  标记为未读(后端新增 mark_unread + POST /messages/{id}/unread)+ sync 筛选档 + ?id= 深链定位。
- **验证方式**:因 MCP 浏览器 profile 被占用,改用独立 Playwright 脚本
  (`scratchpad/verify.mjs`,playwright-core + 系统 Chrome + 临时 user-data-dir + headless,不碰用户浏览器)。
  确认:TOTP 二维码 canvas 渲染、审批详情抽屉完整字段、消息标记未读、资产行内启停均通过。
- **未做(需数据模型/git 链路深改,暂缓)**:prod 站点标记、节点 git 追溯列、测试连通。

### 早期补充:右上角登录人员功能(MainLayout.vue)
- 原型顶栏「who」是头像+用户名·角色,点击进入个人设置;原实现只有不可点的静态 n-tag,功能缺失。
- 改成 `n-dropdown` 用户区:青色头像(name 首字母缩写)+「name · 角色」+ 下拉箭头;
  菜单项:个人信息与设置(→/settings)、我的会话与设备(→/settings)、退出登录。
- 已验证:头像/名字渲染正常,点击展开菜单,选「个人信息与设置」正确跳 /settings。

---

## 二、三个关键决策(已定并落地)
- **A. 审计菜单权限**:不改 admin-only,而是 `GET /jobs` 全员可见(非 admin 只看自己);Audit 双 Tab,审计流水保持 admin-only。
- **B. 逐目标结果**:新建 `job_targets` 表存 stdout/stderr,不污染 audit;audit 仅加 job_id 用于跳转。
- **C. 原审计流水**:保留为 admin Tab(治理证据 + 导出功能)。

---

## 三、本地启动与验证

**启动**(现有 `data/opsctl.db` 的金库盐口令未知 —— `dev` 和 `dev-unseal-pass` 都报 passphrase mismatch。
本地验证 UI/执行时直接**封存启动**即可,不设 PASSPHRASE;金库封存不影响登录、SQL 执行、UI 功能,
只影响需解封的 SSH 凭据解密):
```
Remove-Item Env:OPSCTL_VAULT__PASSPHRASE   # 确保不设
$env:OPSCTL_AUTH__JWT_SECRET = "dev-jwt-secret-change-me"
cargo run -p opsctl-server        # http://127.0.0.1:8443/
```
若要全新解封环境,删掉 data/opsctl.db 让它用新口令重建盐。seed 账号 admin/operator/viewer(密码同名)、admin2。
`seed_fixture` 每次启动 DELETE+重插 demo servers(site='east'),故 SQL 测试污染自动重置。

**已通过的手工验证**:
- 登录页无预填/无测试账号提示;青色主题全站生效,无残留 GitHub 蓝。
- Console:整站级联勾选、工具条、节点行 IP·类型 meta、模板变量实时代入、`UPDATE` 触发二次确认、结果带耗时+汇总。
- 执行 SQL → 执行记录页 job 聚合(状态/成功数/耗时)→ 抽屉「单独打开」→ /record/:id 判决横幅+审批追溯+逐目标。
- 审批页:遗留 pending 会签(1/2)正常渲染无 panic;批量放行分类提示。
- 模板页:审批人多选控件出现。
- 控制台仅无害的表单可访问性提示(naive-ui 内部)。

---

## 四、若要继续/收尾的可选项(本轮范围外)
- 视觉「完整对齐」(顶部横向导航、登录双栏品牌面、6 格 OTP)—— 本轮只做了配色 token。
- 更大范围原型对齐(任务编排树、测试连通、生产站点标记、消息双栏、单节点历史抽屉)—— 用户选择「暂缓」。
- 计划文件:`C:\Users\llux2\.claude\plans\magical-sprouting-panda.md`(完整方案)。
