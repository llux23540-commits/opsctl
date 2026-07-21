const { test, expect } = require('./fixtures');

test.describe('消息中心', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/#/messages');
    await expect(page.getByText('消息中心')).toBeVisible();
  });

  test('筛选含同步档', async ({ page }) => {
    await expect(page.getByText(/^同步 \d+$/)).toBeVisible();
  });

  test('点消息展示右侧详情含来源', async ({ page }) => {
    await page.locator('.n-list-item').first().click();
    const detail = page.locator('.msg-detail');
    await expect(detail.getByText('来源')).toBeVisible();
    await expect(detail.getByText('类型')).toBeVisible();
    await expect(detail.getByRole('button', { name: '标记为未读' })).toBeVisible();
  });

  test('标记为未读回切生效', async ({ page }) => {
    await page.locator('.n-list-item').first().click();
    const detail = page.locator('.msg-detail');
    await expect(detail).toBeVisible();
    // opening marks it read → status shows 已读 (unique in the detail pane)
    await expect(detail.getByText('已读')).toBeVisible();
    await detail.getByRole('button', { name: '标记为未读' }).click();
    // 已读 disappears (status became 未读); avoid matching the 标记为未读 button
    await expect(detail.getByText('已读')).toHaveCount(0);
  });
});
