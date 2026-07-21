// Full real-user journey against the REAL app implementation:
//   建账号 → 建空间(站点) → 在空间里建数据库节点并绑定账号 → 执行页勾选节点
//   → 输入 SQL 执行(指向真实 demo sqlite,命令真正跑通)→ 执行记录里核对
//
// Field labels verified against web/src/views/Assets.vue:
//   账号弹窗: 名称 / 类型 / 登录用户名 / 密码/密钥
//   资产弹窗: 名称 / 类型 / 所属站点 / 主机 / 端口 / 标签 / 绑定账号 / 状态
// naive-ui keeps both modals in the DOM (display-toggled) and modal content +
// footer live in a `.n-card`, so每个字段操作限定在按标题定位的当前弹窗 card。
const { test, expect } = require('./fixtures');

const TAG = 'j' + Date.now().toString().slice(-6);
const ACCT = `acct-${TAG}`;
const SITE = `space-${TAG}`;
const NODE = `dbnode-${TAG}`;

function modal(page, title) {
  // the modal card is the ONLY .n-card whose header holds the title (the outer
  // page card has no header), so this avoids the nested-card double match
  return page.locator('.n-card').filter({ has: page.locator('.n-card-header', { hasText: title }) });
}
async function fillField(scope, label, value) {
  await scope.locator('.n-form-item', { hasText: label }).locator('input, textarea').first().fill(value);
}
async function pickField(page, scope, label, optionText) {
  await scope.locator('.n-form-item', { hasText: label }).locator('.n-base-selection').first().click();
  await page.locator('.n-base-select-option', { hasText: optionText }).first().click();
  await page.waitForTimeout(200); // let the dropdown close
}
async function save(scope) {
  const btn = scope.getByRole('button', { name: '保存' });
  await expect(btn).toBeEnabled();
  await btn.click();
}

test('真人全流程:建空间 → 加节点 → 执行命令 → 查记录', async ({ page }) => {
  test.setTimeout(90000);

  await test.step('打开资产管理', async () => {
    await page.goto('/#/assets');
    await expect(page.getByText('站点与节点')).toBeVisible();
  });

  await test.step('新建数据库账号', async () => {
    await page.getByText('系统账号', { exact: true }).click();
    await page.getByRole('button', { name: '+ 新建账号' }).click();
    const m = modal(page, '新建账号');
    await expect(m).toBeVisible();
    await fillField(m, '名称', ACCT);
    await pickField(page, m, '类型', '数据库密码');
    await fillField(m, '登录用户名', 'demo');
    await fillField(m, '密码/密钥', 'demo-pass');
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: ACCT })).toBeVisible();
  });

  await test.step('新建空间(站点)', async () => {
    await page.getByText('站点与节点', { exact: true }).click();
    await page.getByRole('button', { name: '+ 新建资产' }).click();
    const m = modal(page, '新建资产');
    await expect(m).toBeVisible();
    await fillField(m, '名称', SITE);
    await pickField(page, m, '类型', '站点(分组)');
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: SITE })).toBeVisible();
  });

  await test.step('在空间里建数据库节点并绑定账号', async () => {
    await page.getByRole('button', { name: '+ 新建资产' }).click();
    const m = modal(page, '新建资产');
    await expect(m).toBeVisible();
    await fillField(m, '名称', NODE);
    await pickField(page, m, '类型', '数据库');
    await pickField(page, m, '所属站点', SITE);
    await fillField(m, '主机', 'data/demo.db');
    await pickField(page, m, '绑定账号', ACCT);
    await page.keyboard.press('Escape'); // close multi-select dropdown
    await save(m);
    await expect(page.locator('.n-data-table-td', { hasText: NODE })).toBeVisible();
  });

  await test.step('执行页勾选该节点', async () => {
    await page.goto('/#/console');
    await expect(page.getByText('资产树', { exact: false })).toBeVisible();
    const node = page.locator('.n-tree-node', { hasText: NODE });
    await expect(node).toBeVisible();
    await node.locator('.n-checkbox').first().click();
    await expect(page.locator('.typecard.db').getByText(NODE)).toBeVisible();
  });

  await test.step('输入 SQL 执行,验证真实返回', async () => {
    await page.getByPlaceholder('SQL 查询,如 SELECT 1').fill('SELECT count(*) AS n FROM servers');
    await page.locator('.typecard.db').getByRole('button', { name: '▶ 执行' }).click();
    await expect(page.getByText(/✓ 成功 1/)).toBeVisible();
    await expect(page.getByText(/汇总 1\/1 成功/)).toBeVisible();
  });

  await test.step('执行记录里确认这次执行', async () => {
    await page.goto('/#/audit');
    await expect(page.locator('.n-tabs-tab', { hasText: '执行记录' })).toBeVisible();
    await expect(page.getByText('SELECT count(*) AS n FROM servers').first()).toBeVisible();
  });
});
