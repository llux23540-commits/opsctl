const { test, expect } = require('./fixtures');

test.describe('导航与用户区', () => {
  test('右上角用户区可展开下拉菜单', async ({ page }) => {
    await page.goto('/#/console');
    await page.locator('.who').click();
    await expect(page.getByText('个人信息与设置')).toBeVisible();
    await expect(page.getByText('我的会话与设备')).toBeVisible();
    await expect(page.getByText('退出登录')).toBeVisible();
  });

  test('用户菜单进入设置', async ({ page }) => {
    await page.goto('/#/console');
    await page.locator('.who').click();
    await page.getByText('个人信息与设置').click();
    await expect(page).toHaveURL(/#\/settings/);
  });

  test('侧栏菜单项齐全(admin)', async ({ page }) => {
    await page.goto('/#/console');
    for (const label of ['节点执行', '用户与权限', '资产管理', '授权规则', '执行模板', '消息', '执行记录', '设置']) {
      await expect(page.locator('.n-menu').getByText(label, { exact: true })).toBeVisible();
    }
  });

  test('铃铛展开通知面板(内联,非直接跳转)', async ({ page }) => {
    await page.goto('/#/console');
    await page.getByRole('button', { name: '🔔' }).click();
    await expect(page.locator('.notif-panel')).toBeVisible();
    await expect(page.locator('.notif-panel').getByText('通知')).toBeVisible();
    await expect(page.getByText('查看全部消息 →')).toBeVisible();
    // still on console — the bell opened a panel, did not navigate away
    await expect(page).toHaveURL(/#\/console/);
  });
});
