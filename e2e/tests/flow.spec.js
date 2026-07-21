// 完整审批业务链路(真人全流程,全新隔离库,全程 UI 点击,不用 API 注入):
//   admin 建标签 → 建生产站点 → 建 DB 节点(打标签+绑账号)→ 建授权规则(operator 按标签 SQL,需审批)
//   → 登出 → operator 登录 → 提交 SQL → 命中审批挂起
//   → 登出 → admin 登录 → 审批页放行(SQL 真执行)→ 执行记录核对
// 标签用于授权:UI 建的节点 id 是后端 UUID,测试拿不到,故用"按标签"授权。
// 字段标签均已用 bash grep 核对真实源码(不臆测)。
const { test, expect } = require('@playwright/test');

const TAG = 'f' + Date.now().toString().slice(-6);
const TAGN = `t-${TAG}`;
const SITE = `prod-${TAG}`;
const NODE = `db-${TAG}`;
const NODE2 = `db2-${TAG}`;
const ACCT = `dba-${TAG}`;
const RULE = `rule-${TAG}`;

function modal(page, title) {
  return page.locator('.n-card').filter({ has: page.locator('.n-card-header', { hasText: title }) });
}
async function fillField(scope, label, value) {
  await scope.locator('.n-form-item', { hasText: label }).locator('input, textarea').first().fill(value);
}
async function pickField(page, scope, label, optionText) {
  await scope.locator('.n-form-item', { hasText: label }).locator('.n-base-selection').first().click();
  await page.locator('.n-base-select-option', { hasText: optionText }).first().click();
  await page.waitForTimeout(200);
}
async function save(scope) { await scope.getByRole('button', { name: '保存' }).click(); }

async function uiLogin(page, user, pass) {
  await page.goto('/#/login');
  await page.getByPlaceholder('用户名').fill(user);
  await page.getByPlaceholder('密码').fill(pass);
  await page.getByRole('button', { name: '登 录' }).click();
  await expect(page).toHaveURL(/#\/console/);
}
async function logout(page) {
  await page.locator('.who').click();
  await page.getByText('退出登录').click();
  await expect(page).toHaveURL(/#\/login/);
}

test('完整审批链路(真人全流程,全新库)', async ({ page }) => {
  test.setTimeout(150000);

  await test.step('admin 登录', async () => {
    await uiLogin(page, 'admin', 'admin');
  });

  await test.step('建标签', async () => {
    await page.goto('/#/assets');
    await page.locator('.n-tabs-tab', { hasText: '标签' }).click();
    await page.getByRole('button', { name: '+ 新建标签' }).click();
    const m = modal(page, '新建标签');
    await fillField(m, '名称', TAGN);
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: TAGN })).toBeVisible();
  });

  await test.step('建生产站点(env=prod)', async () => {
    await page.locator('.n-tabs-tab', { hasText: '站点与节点' }).click();
    await page.getByRole('button', { name: '+ 新建资产' }).click();
    const m = modal(page, '新建资产');
    await fillField(m, '名称', SITE);
    await pickField(page, m, '类型', '站点(分组)');
    await pickField(page, m, '环境', '生产 prod');
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: SITE })).toBeVisible();
  });

  await test.step('建数据库账号', async () => {
    await page.locator('.n-tabs-tab', { hasText: '系统账号' }).click();
    await page.getByRole('button', { name: '+ 新建账号' }).click();
    const m = modal(page, '新建账号');
    await fillField(m, '名称', ACCT);
    await pickField(page, m, '类型', '数据库密码');
    await fillField(m, '登录用户名', 'demo');
    await fillField(m, '密码/密钥', 'demo');
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: ACCT })).toBeVisible();
  });

  await test.step('在生产站点建 DB 节点(打标签+绑账号)', async () => {
    await page.locator('.n-tabs-tab', { hasText: '站点与节点' }).click();
    await page.getByRole('button', { name: '+ 新建资产' }).click();
    const m = modal(page, '新建资产');
    await fillField(m, '名称', NODE);
    await pickField(page, m, '类型', '数据库');
    await pickField(page, m, '所属站点', SITE);
    await fillField(m, '主机', 'data/demo.db');
    await pickField(page, m, '标签', TAGN);
    await page.keyboard.press('Escape');
    await pickField(page, m, '绑定账号', ACCT);
    await page.keyboard.press('Escape');
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: NODE })).toBeVisible();
  });

  await test.step('再建第二个 DB 节点(同标签,用于多子任务)', async () => {
    await page.getByRole('button', { name: '+ 新建资产' }).click();
    const m = modal(page, '新建资产');
    await fillField(m, '名称', NODE2);
    await pickField(page, m, '类型', '数据库');
    await pickField(page, m, '所属站点', SITE);
    await fillField(m, '主机', 'data/demo.db');
    await pickField(page, m, '标签', TAGN);
    await page.keyboard.press('Escape');
    await pickField(page, m, '绑定账号', ACCT);
    await page.keyboard.press('Escape');
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: NODE2 })).toBeVisible();
  });

  await test.step('建授权规则:operator 按标签 SQL,需审批', async () => {
    await page.goto('/#/access');
    await page.getByRole('button', { name: '+ 新建规则' }).click();
    const m = modal(page, '新建授权规则');
    await fillField(m, '名称', RULE);
    await pickField(page, m, '主体', 'operator');
    await pickField(page, m, '资产维度', '按标签');
    await pickField(page, m, '资产选择', TAGN);
    await pickField(page, m, '账号', ACCT);
    await m.locator('.n-form-item', { hasText: '动作' }).getByText('SQL', { exact: true }).click();
    await m.locator('.n-form-item', { hasText: '需审批' }).locator('.n-switch').click();
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: RULE })).toBeVisible();
  });

  await test.step('切换到 operator', async () => {
    await logout(page);
    await uiLogin(page, 'operator', 'operator');
  });

  await test.step('operator 勾选两个节点提交 SQL,命中审批规则挂起(→ 一个 job 两个子任务)', async () => {
    await page.goto('/#/console');
    await expect(page.locator('.n-tree-node').first()).toBeVisible();
    const node1 = page.locator('.n-tree-node', { hasText: NODE });
    const node2 = page.locator('.n-tree-node', { hasText: NODE2 });
    await expect(node1).toBeVisible();
    await expect(node2).toBeVisible();
    await node1.locator('.n-checkbox').first().click();
    await node2.locator('.n-checkbox').first().click();
    await page.getByPlaceholder('SQL 查询,如 SELECT 1').fill('SELECT count(*) FROM servers');
    await page.locator('.typecard.db').getByRole('button', { name: '▶ 执行' }).click();
    await expect(page.getByText(/待审批|等待管理员审批/).first()).toBeVisible();
  });

  await test.step('切回 admin,审批任务显示 2 子任务、带 prod 环境标签', async () => {
    await logout(page);
    await uiLogin(page, 'admin', 'admin');
    await page.goto('/#/approvals');
    const task = page.locator('.task').filter({ hasText: 'SQL' }).first();
    await expect(task).toBeVisible();
    await expect(task.getByText('prod').first()).toBeVisible();
    await expect(task.locator('.prog')).toContainText('2 子任务');
    await expect(task.locator('.prog')).toContainText('2 待批');
  });

  await test.step('展开任务,逐子任务放行第一个(进度变 1 待批)', async () => {
    const task = page.locator('.task').filter({ hasText: 'SQL' }).first();
    await task.locator('.task-hd').click();
    await expect(page.locator('.subrow')).toHaveCount(2);
    await page.locator('.subrow').first().getByRole('button', { name: '放行' }).click();
    await expect(page.locator('.n-message').filter({ hasText: '放行' }).first()).toBeVisible();
    // 该任务仍在,进度变为 1 待批
    await expect(page.locator('.task').filter({ hasText: 'SQL' }).first().locator('.prog')).toContainText('1 待批');
  });

  await test.step('按任务全部放行剩余子任务', async () => {
    await page.locator('.task').filter({ hasText: 'SQL' }).first().getByRole('button', { name: '全部放行' }).click();
    await expect(page.locator('.n-message').filter({ hasText: '放行' }).first()).toBeVisible();
  });

  await test.step('执行记录里看到该执行', async () => {
    await page.goto('/#/audit');
    await expect(page.getByText('SELECT count(*) FROM servers').first()).toBeVisible();
  });
});
