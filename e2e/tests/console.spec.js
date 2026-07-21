const { test, expect } = require('./fixtures');

test.describe('节点执行', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/#/console');
    await expect(page.getByText('资产树', { exact: false })).toBeVisible();
    // wait for the async-loaded tree to actually render nodes before interacting
    await expect(page.locator('.n-tree-node').first()).toBeVisible();
  });

  test('工具条四按钮 + 已选计数', async ({ page }) => {
    await expect(page.getByRole('button', { name: '全选可见' })).toBeVisible();
    await expect(page.getByRole('button', { name: '展开' })).toBeVisible();
    await expect(page.getByRole('button', { name: '折叠' })).toBeVisible();
    await expect(page.getByRole('button', { name: '清空' })).toBeVisible();
    await expect(page.getByText(/已选 \d+ 个节点/)).toBeVisible();
  });

  test('全选可见后计数增加', async ({ page }) => {
    await page.getByRole('button', { name: '全选可见' }).click();
    await expect(page.getByText(/已选 [1-9]\d* 个节点/)).toBeVisible();
  });

  test('破坏性 SSH 触发二次确认', async ({ page }) => {
    await page.getByRole('button', { name: '全选可见' }).click();
    // wait until servers are actually selected (title shows non-zero count)
    await expect(page.getByText(/服务器区 · SSH \([1-9]/)).toBeVisible();
    const sshBox = page.getByPlaceholder('SSH 命令,如 uname -a');
    await sshBox.fill('rm -rf /tmp/x');
    await page.getByRole('button', { name: '▶ 执行' }).first().click();
    await expect(page.getByText('破坏性命令二次确认')).toBeVisible();
    await page.getByRole('button', { name: '取消' }).click();
  });

  test('IP 搜索命中节点', async ({ page }) => {
    await page.getByPlaceholder(/搜索节点名/).fill('127.0');
    await expect(page.getByText('web-01')).toBeVisible();
  });

  test('typecard 模板入口跳转执行模板页', async ({ page }) => {
    // prototype console.js: each type card header links to the templates page
    await page.getByRole('button', { name: '模板', exact: true }).first().click();
    await expect(page).toHaveURL(/#\/templates/);
  });

  test('节点记录抽屉展示该节点执行历史', async ({ page, request }) => {
    // seed one execution on db-demo via API so the node has history
    const token = await page.evaluate(() => localStorage.getItem('opsctl_token'));
    const device = await page.evaluate(() => localStorage.getItem('opsctl_device'));
    await request.post('/api/jobs/sql', {
      headers: { Authorization: `Bearer ${token}`, 'x-device-id': device },
      data: { targets: ['db-demo'], query: 'SELECT 42 AS answer' },
    });
    await page.reload();
    await expect(page.locator('.n-tree-node').first()).toBeVisible();
    // open the demo-sqlite node's history (记录 link, revealed on hover)
    const node = page.locator('.n-tree-node', { hasText: 'demo-sqlite' });
    await node.getByText('记录', { exact: true }).click();
    const drawer = page.locator('.n-drawer-content');
    await expect(drawer).toBeVisible();
    await expect(drawer.getByText(/节点记录/)).toBeVisible();
    await expect(drawer.getByText('SELECT 42 AS answer')).toBeVisible();
    await expect(drawer.getByText('查看完整 →').first()).toBeVisible();
  });
});
