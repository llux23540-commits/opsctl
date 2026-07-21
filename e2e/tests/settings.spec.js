const { test, expect } = require('./fixtures');

test.describe('设置', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/#/settings');
    await expect(page.getByText('个人信息')).toBeVisible();
  });

  test('TOTP 开启展示二维码 + 密钥后备', async ({ page }) => {
    await page.getByRole('button', { name: '开启' }).first().click();
    await expect(page.locator('.totp-qr canvas')).toBeVisible();
    await expect(page.locator('.totp-secret code')).toBeVisible();
  });

  test('登录有效期含 14 天选项', async ({ page }) => {
    await page.locator('.n-base-selection').first().click();
    await expect(page.getByText('14 天')).toBeVisible();
  });

  test('git 本地文件夹模式有路径输入', async ({ page }) => {
    await expect(page.getByPlaceholder(/配置导出目录/)).toBeVisible();
  });

  test('git 本地仓库模式切换后有路径输入', async ({ page }) => {
    // 设置页多区块异步加载会引起布局抖动,naive radio label 也会拦截指针;
    // 先等 radio 可见,再 force 点击绕过 actionability 抖动,消除偶发超时。
    const radio = page.locator('.n-radio-button', { hasText: '本地 Git' });
    await expect(radio).toBeVisible();
    await radio.scrollIntoViewIfNeeded();
    await radio.click({ force: true });
    await expect(page.getByPlaceholder(/本地 git 仓库路径/)).toBeVisible();
  });

  test('git 远程模式含仓库地址/凭据字段与推送/拉取按钮', async ({ page }) => {
    // 先等 loadGit 异步回填完成(默认 folder 模式输入出现),否则它会覆盖手动切换、按钮闪掉
    await expect(page.getByPlaceholder(/配置导出目录/)).toBeVisible();
    const radio = page.locator('.n-radio-button', { hasText: '远程 Git' });
    await expect(radio).toBeVisible();
    await radio.scrollIntoViewIfNeeded();
    await radio.click({ force: true });
    // 远程配置字段
    await expect(page.getByPlaceholder(/github\.com\/org\/repo|git@/)).toBeVisible(); // 仓库地址
    await expect(page.locator('.n-form-item', { hasText: '密码/令牌' }).locator('input')).toBeVisible();
    // 远程专属操作按钮:推送 / 拉取(= 下载到工作目录)
    await expect(page.getByRole('button', { name: /推送/ })).toBeVisible();
    await expect(page.getByRole('button', { name: /拉取/ })).toBeVisible();
  });
});
