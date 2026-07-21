const { test, expect } = require('./fixtures');

test.describe('资产管理', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/#/assets');
    await expect(page.getByText('站点与节点')).toBeVisible();
  });

  test('资产行有启停/编辑/删除按钮', async ({ page }) => {
    const row = page.locator('.n-data-table-tr', { hasText: 'web-01' });
    await expect(row.getByRole('button', { name: /停用|启用/ })).toBeVisible();
    await expect(row.getByRole('button', { name: '编辑' })).toBeVisible();
    await expect(row.getByRole('button', { name: '删除' })).toBeVisible();
  });

  test('行内启停切换状态', async ({ page }) => {
    const row = () => page.locator('.n-data-table-tr', { hasText: 'web-01' });
    const toggle = () => row().getByRole('button', { name: /^(停用|启用)$/ });
    // state-agnostic + idempotent: read current label, toggle, assert flip, restore
    const cur = (await toggle().innerText()).trim();
    const other = cur === '停用' ? '启用' : '停用';
    await toggle().click();
    await expect(row().getByRole('button', { name: other, exact: true })).toBeVisible();
    await row().getByRole('button', { name: other, exact: true }).click();
    await expect(row().getByRole('button', { name: cur, exact: true })).toBeVisible();
  });

  test('标签页有系统账号/标签 tab', async ({ page }) => {
    await expect(page.getByText('系统账号')).toBeVisible();
    await expect(page.getByText('标签', { exact: true })).toBeVisible();
  });

  test('标签表格显示使用计数(web 标签已绑定节点)', async ({ page }) => {
    await page.locator('.n-tabs-tab', { hasText: '标签' }).click();
    // seed 的 web 标签绑定了 web-01/web-02 等 → 使用列显示「N 个节点」
    const webRow = page.locator('.n-data-table-tr', { hasText: 'web' }).first();
    await expect(webRow.getByText(/个节点/)).toBeVisible();
  });

  test('节点表单测试连通:database 文件可访问返回可连通', async ({ page }) => {
    await page.getByRole('button', { name: '+ 新建资产' }).click();
    const modal = page.locator('.n-card').filter({ has: page.locator('.n-card-header', { hasText: '新建资产' }) });
    await expect(modal).toBeVisible();
    await modal.locator('.n-form-item', { hasText: '类型' }).locator('.n-base-selection').first().click();
    await page.locator('.n-base-select-option', { hasText: '数据库' }).first().click();
    await modal.locator('.n-form-item', { hasText: '主机' }).locator('input').first().fill('data/demo.db');
    await modal.getByRole('button', { name: '测试连通' }).click();
    await expect(page.getByText(/可连通/)).toBeVisible();
  });

  test('节点表单测试连通:空主机提示先填写', async ({ page }) => {
    await page.getByRole('button', { name: '+ 新建资产' }).click();
    const modal = page.locator('.n-card').filter({ has: page.locator('.n-card-header', { hasText: '新建资产' }) });
    await expect(modal).toBeVisible();
    // 默认类型 server(非站点),连通性区可见;不填主机直接测 → 客户端提示
    await modal.getByRole('button', { name: '测试连通' }).click();
    await expect(page.getByText('请先填写主机 / 地址')).toBeVisible();
  });
});
