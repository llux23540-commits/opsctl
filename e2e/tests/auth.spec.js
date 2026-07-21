// Login page — no injected auth here (tests the real login UI).
const { test, expect } = require('@playwright/test');

test.describe('登录', () => {
  test('登录页有记住此设备与忘记密码', async ({ page }) => {
    await page.goto('/#/login');
    await expect(page.getByText('记住此设备')).toBeVisible();
    await expect(page.getByText('忘记密码')).toBeVisible();
  });

  test('无预填账号密码', async ({ page }) => {
    await page.goto('/#/login');
    await expect(page.getByPlaceholder('用户名')).toHaveValue('');
    await expect(page.getByPlaceholder('密码')).toHaveValue('');
  });

  test('登录成功进入控制台', async ({ page }) => {
    await page.goto('/#/login');
    await page.getByPlaceholder('用户名').fill('admin');
    await page.getByPlaceholder('密码').fill('admin');
    await page.getByRole('button', { name: '登 录' }).click();
    await expect(page).toHaveURL(/#\/console/);
  });

  test('错误密码给出中文友好提示', async ({ page }) => {
    await page.goto('/#/login');
    await page.getByPlaceholder('用户名').fill('admin');
    await page.getByPlaceholder('密码').fill('wrong-pass');
    await page.getByRole('button', { name: '登 录' }).click();
    // must be friendly Chinese, never the raw backend "unauthorized"
    await expect(page.locator('.n-message')).toContainText('账号或密码错误');
    await expect(page.locator('.n-message')).not.toContainText(/unauthorized/i);
  });
});
