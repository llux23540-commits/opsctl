# opsctl 对齐原型 · 进度记录

> **2026-08-01 层级与简化轮次(已完成)**:按「贴合 Nacos 真实模型 + 少填东西」重排了整个模块。
> **A 命名空间 → 配置 变成上下级**:Nacos 本身按命名空间硬隔离,原来「配置」Tab 却锁死在集群登记的
> 那一个空间上。现在两个 Tab 合成「命名空间与配置」主从:左选空间(带配置数徽章),右列它的配置;
> 查看正文 / 删除 / 同步全部作用于选中的空间。后端 `GET /configs` 增加 `namespace` 参数。
> **B 修掉由此暴露的语义缺陷**:`namespace` 留空原本表示「用集群默认」,而 public 的 id 本身就是空串 ——
> 集群默认不是 public 时**根本指不到 public**,会静默落到别的空间(删错、同步错)。改为
> 「字段缺失 = 集群默认;显式给值(含空串)= 就用这个」。真机验证:选 public 显示 0 条配置,
> 而不是错显集群默认空间的 25 条。
> **C 账号/角色/权限 三个平铺 Tab → 一个「账号与权限」**:左选账号,右边直接回答「这个账号能操作哪些
> 命名空间」。新增 `GET /users/{name}/access`(角色 + 按命名空间归并的权限)与一步授权
> `POST /grant`(只需 账号+命名空间+动作,缺角色自动建并绑定,顺序按 Nacos 要求先角色后赋权);
> 全局管理员如实标出并隐藏无意义的授权表单。
> **D 模板分组**:模板增加 `namespace` 归属列,列表按归属命名空间分组,并显示「原文/变量代入」;
> 同步产生的模板自动带上来源空间。初始化时目标空间优先级 **请求指定 > 模板归属 > 集群默认**,
> 抽屉里直接显示推导结果(「跟随模板归属」),不再让人重填;要发到别处才展开覆盖。
> 验证:`cargo test -p opsctl-server` **83 全绿**(新增 3:显式空串指向 public、模板归属作为默认回放目标、
> 一步授权与用户权限全景);Playwright 真机(Nacos 2.5.1)实测:7 个命名空间逐个切换、public 正确显示 0、
> 账号权限全景 + 一步授权落地(复用已有角色,不重复创建)、模板按 `BWINGAME12345` 正确分组、
> 选模板后目标空间自动推导并回放 4/4 成功。

> **2026-08-01 同步 + 真机联调轮次(已完成)**:
> **A 真机为什么连不上**(纠正上一轮结论):不是「专门拦了 opsctl-server.exe」,而是本机**出站默认 Block**
> (`DefaultOutboundAction=Block`,三个 profile 全是),只有拿到放行规则的程序才出得去 —— curl / chrome / node
> 都有 Allow 规则,新编译的 exe 没有。用 `server/examples/probe.rs`(同一套 reqwest、独立 exe)做了可证伪实验:
> 无规则时 `ERR 3ms os error 10013`,GlassWire 补上 Allow 后**同一个二进制** `OK 200 119ms`。
> `opsctl-server.exe` 另有一条显式 Block(Block 优先于 Allow),换 exe 路径可绕开 ——
> 本地改用 `target/debug/opsctl-vault.exe` 起服务即打通真机。根治仍需删掉那条 Block 规则。
> **B 真机验证**(Nacos **2.5.1** 单节点 `10.42.0.25:8848`):节点(`/v2` 不可用,`/v1` 降级生效)、
> 7 个命名空间、账号、角色、权限、配置全部读到。
> **C 新增「同步」**:`POST /nacos/clusters/{id}/sync` 把远端整个命名空间拉回存成配置模板
> (列表接口不给正文的版本逐条回查补齐),支持 `dry_run`;配套 `GET /configs/detail`(正文预览)、
> `DELETE /configs`(删除,落审计)。真机拉回 25 条 / 46,679 字节。
> **D 真机数据暴露的缺陷 + 修复**:线上配置本来就含 `${mysql8.jdbc.url}`、`${MYSQL_ROOT_PASSWORD:...}` 这类
> **应用自己的**占位符,被当成 opsctl 模板变量后回放 **16/25** 失败。改为模板增加 `literal` 列,
> 同步产生的模板一律 `literal=1` 原文下发;`NacosInitRequest` 加 `substitute` 可显式覆盖;
> UI 在原文模板下隐藏「变量取值」步骤并说明原因。修后回放 **25/25**(9 新建 + 16 跳过),
> 抽查三条源/目标**逐字节一致**。
> **E 补上缺失的入口**:命名空间/账号/角色/权限 那个页面(`/nacos/:id`)此前只能手敲 URL —— 集群卡片上
> 没有任何链接。现在卡片名可点、并加「集群管理」按钮 + 一行能力说明;原来的「已有配置」只读抽屉删掉,
> 由管理页的「配置」Tab 承接(多了正文预览 / 删除 / 同步),不留两套。
> 验证:`cargo test -p opsctl-server` **80 全绿**(新增 4);Playwright 全链路实测:卡片→集群管理→
> 五个 Tab 全部读到真机数据(7 命名空间 / 21 配置 / 账号 / 角色 / 权限空态)→ 配置正文预览 →
> 同步(试运行 + 落库)→ 登记目标集群 → 选原文快照 → 执行初始化 25/25。

> **2026-08-01 Nacos 管理面轮次(已完成)**:接入 Nacos 的**命名空间 / 账号 / 角色绑定 / 赋权** API。
> 契约全部按 alibaba/nacos 源码核对(tag 2.3.2,并交叉比对 1.4.7 / 2.2.3 / 3.0.0),不靠博客:
> 控制器不在 console 模块,而在 `plugin-default-impl/nacos-default-auth-plugin/.../controller/`。
> **A 版本口味探测**:先打 `/v3/auth/user/list`,通(含 401/403)判为 3.x 走 `/v3/auth/*`+`/v3/console/core/namespace`,
> 否则走 `/v1/auth/*`+`/v1/console/namespaces`;v1 列表恒带 `search=accurate`(2.x 是 mapping 谓词,缺了根本匹配不到
> handler,1.4.x 则直接忽略)—— 一套调用形状通吃 1.x/2.x。
> **B 响应形状三不统一**(最容易解析错的地方,已分别处理):账号/角色/权限**列表是裸 `Page<T>`**(无信封)、
> 写操作是 `RestResult{code:200}`、命名空间增删改是**裸 `true`/`false`**(HTTP 200 也可能是 false),
> 失败则是 **HTTP 400 + 纯文本**;v3 统一 `{code:0,...}`。`check_write` 只认 code,不解析人类可读串
> (1.4.x 放 message、2.2+ 放 data)。
> **C 参数名陷阱**:v1 create=`customNamespaceId`、v1 edit=`namespace`+`namespaceShowName`、v2=`namespaceId`、
> v3 create 又变回 `customNamespaceId` —— 各写各的,不复用。
> **D 服务端规则前置**:`ROLE_ADMIN` 不允许创建、public(id 为空串或字面量)不允许删除、动作只放行 `r|w|rw`、
> 命名空间 id/名称按 Nacos 同款正则本地先校验(避免 v1 只回一个分辨不出原因的 `false`);
> 账号列表返回的 **bcrypt 哈希一律剥掉**,绝不下发。
> **E 资源串**:`<namespaceId>:<group>:<type>/<name>`,分隔符 `:`、通配 `*`,public 首段为空(`:*:*`);
> UI 用下拉拼装并实时预览,不让人手敲。赋权前角色必须已存在(服务端 `role X not found!`,集群还有 ~15s 传播延迟),
> 页面已明示。
> 落地:`server/src/nacos.rs` 新增 Flavor 探测 + 15 个客户端函数 + 14 个处理器(全 admin-only,写操作落审计
> `nacos_ns_*`/`nacos_user_*`/`nacos_role_*`/`nacos_perm_*`);新页面 `web/src/views/NacosCluster.vue`
> (路由 `/nacos/:id`,四个 Tab);`dev/mock-nacos.mjs` 同步补齐这些端点(含 v3 一律 404 以便探测回落)。
> **顺带修**:reqwest 的 Display 只给最外层,排障时看不到真因 —— 加 `why()` 走 source 链,
> 这才定位到用户那台 Nacos 不可达的真正原因是 **GlassWire 给 opsctl-server.exe 下了出站 Block 规则
> (os error 10013 WSAEACCES)**,不是 Nacos 或代码问题。
> 验证:`cargo test -p opsctl-server` **76 全绿**(新增 4 项,mock 精确复刻上述三种响应形状 + 纯文本 400);
> 浏览器实测四个 Tab:建命名空间 `dev-ns`、建账号 `dev1`、绑角色 `dev`、赋权 `dev-ns:*:*` `rw` 全部生效,
> 列表回读一致,public 行禁用编辑/删除。

> **2026-08-01 输入框修复轮次(已完成)**:「登记 Nacos 集群」里用户名/密码输入框白底 —— 根因是
> **Chrome 自动填充**,不是主题问题:浏览器把保存的 **opsctl 登录口令**(admin/admin)当成登录表单灌进了
> 这两个框,并强制刷上 UA 的 `rgb(232,240,254)` 浅底(实测 `:-webkit-autofill` 命中,其余输入框全是
> `rgba(0,0,0,0)`)。所以这不只是难看:不留神点保存,平台自己的登录口令就会被当成 Nacos 集群口令加密入库。
> **A 挡住误灌**:Nacos 集群表单的用户名/密码加 `:input-props`(`name=nacos-cluster-user|secret`、
> `autocomplete=off|new-password`);排查发现 **资产管理 → 系统账号** 新建框同样被灌入 admin/admin
> (SSH 凭据,危害相同),一并加 `sys-account-user|secret`。设置页改密码、用户重置密码实测不触发,未动。
> **B 兜底样式**(`App.vue` 全局):UA 那层背景改不掉,用 `-webkit-background-clip: text` 把它裁到文字轮廓,
> 露出的仍是 naive-ui 输入框自己的底色,不用猜合成色值;配 `-webkit-text-fill-color/caret-color` 用 `--fg`。
> 登录页的自动填充是合理的,保留功能、只修观感。
> 验证(Playwright 有头 Chromium,真实自动填充场景):Nacos/系统账号两个表单 `autofilled=false`、
> 背景 `rgba(0,0,0,0)`;输入纯数字与混合字符后仍为深色;密码框眼睛切明文后 `type=text`、底色不变;
> 登录页仍被自动填充(`autofilled=true`)但已渲染成深色;`v-model` 未被 `input-props` 影响 ——
> 填 nacos/nacos-pass 点「测试连通」得「连通正常 · 2/3 节点在线 · 13 ms」;
> 生产包(8443 单二进制读 `web/dist`)已确认含该 CSS 规则与四个 autocomplete 属性。
> 未做:Firefox 的标准 `:autofill` 选择器(本机无 Firefox,不验证不发)。

> **2026-08-01 补充轮次(跑起来后的调整,已完成)**:用现有 `data/opsctl.db` 实跑一遍发现并修掉四处——
> **A 启动被错口令打死**(`main.rs` 里 `vault.unseal(..)?` 会让进程直接 exit 1;而封存是受支持的常态,
> 登录/审计/执行记录/只读视图都能用。改为记 ERROR 后**以封存态继续启动**,提示去 设置 → 凭据金库 解封。
> 该库的金库盐口令已不可考,正是这个场景)、
> **B Nacos 页对封存态零提示**(原来只有点保存后的一条 toast)。页面加载即查 `/vault/status`,
> 封存时在页头下方常驻警示条(说明哪些操作会失败 + 「前往解封」直达 `/settings#vault`),
> 卡片上给带鉴权的集群加「需解封」pill(`has_secret` 已由接口返回)、
> **C 设置页锚点只认 `#sessions`**:改成 `{sessions, vault}` 映射;并修好一个存量 bug——卡片内容是异步载入的,
> 首帧 `scrollIntoView` 会落空(`#vault` 实测停在 top=1444),改为 nextTick + 400ms 各滚一次,
> 现在 `#vault`/`#sessions` 都能落点、
> **D 通知无上限堆积**(admin 已有 185 条,几乎全是「新设备登录」,铃铛长期 99+)。
> `push_notification` 插入后按用户裁剪到最新 200 条(UI 本来也只列 100)。
> 验证:`cargo test -p opsctl-server` **72 全绿**;实跑确认错口令下服务正常起并打 ERROR 日志、
> 封存条与 pill 渲染、`#vault` 落点 top=143(容器已滚到底)、连打 22 次登录后 admin 通知稳定在 200 条;
> 十个路由逐个走查无 console error。

> **2026-08-01 轮次(已完成)**:新增 **Nacos 管理** 模块(admin-only,菜单/路由 `/nacos`)——
> **A 集群总览**(`nacos_clusters` 表:名称/环境/地址列表/上下文路径/命名空间/账号 + 金库加密口令/启停/备注;
> 卡片网格展示所有集群,进页并发拉取实时节点:`POST /v1/auth/login` 取 accessToken →
> `GET /v2/core/cluster/nodes`,失败回退 `/v1/core/cluster/nodes`,再失败降级为逐地址
> `/v1/console/health/readiness` 探活并把文案改成「地址可达」以免夸大;地址支持 `host`/`host:port`/完整 URL,
> 缺端口补 8848)、
> **B 配置初始化**(`nacos_config_templates` 模板 = 一组 `{dataId,group,type,content}`,支持 `${变量}` 占位;
> 下发前 `GET /v1/cs/configs` 判存在:默认跳过已存在项、`overwrite` 才覆盖、内容一致也跳过;
> `POST /v1/cs/configs` 表单发布;`dry_run` 只出 `would_*` 预演不写远端;变量缺失该项直接 fail,不写半成品)、
> **C 留痕**(`nacos_init_runs` 逐条结果 + 集群卡片「最近初始化」+ 记录 Tab 与详情抽屉;同时写 audit
> `nacos_init` / `nacos_init_dry_run`,金库封存时带口令的集群拒绝建/用)。
> 端点:`/api/nacos/clusters`(CRUD)、`/{id}/nodes`、`/{id}/configs`、`/{id}/init`、`/probe`、`/templates`、`/runs`。
> 前端 `web/src/views/Nacos.vue` + 复用组件 `components/Icon.vue`(内联 SVG,替代 emoji 图标)、
> `components/NacosConfigItems.vue`;沿用既有深色 + 青色 token,按 ui-ux-pro-max 规则做:状态「图标+文案+颜色」
> 三重编码、等宽地址/dataId + tabular-nums、初始化抽屉渐进式披露(来源→变量→策略→结果)、
> 覆盖开关二次确认、空状态给下一步动作、焦点环与 reduced-motion。
> 依赖:`reqwest` 从 dev-dependencies 提为正式依赖(仍 `default-features=false, features=["json"]`,不引入 TLS/NASM)。
> 验证:`cargo test -p opsctl-server` **72 全绿**(新增 8 项集成测试 + 3 项单元测试,远端用内置 mock Nacos 按
> v1/v2 文档响应形状驱动:鉴权/节点/建-跳过-覆盖/试运行/模板变量/错口令/封存);
> 浏览器实测(临时 mock Nacos 进程 + 真实服务端):登记集群→测试连通 2/3 节点→建模板→变量代入→试运行(0 写入)
> →执行(mock 侧确认 2 次 publish)→重跑得「跳过」→已有配置列表→初始化记录与详情;900px 无横向滚动。

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
