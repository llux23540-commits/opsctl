const { test, expect } = require('./fixtures');

test.describe('执行模板', () => {
  test('模板编辑器含审批人选择器', async ({ page }) => {
    await page.goto('/#/templates');
    await expect(page.getByText('模板列表')).toBeVisible();
    await page.locator('.n-list-item').first().click();
    const item = page.locator('.n-form-item', { hasText: '审批人' });
    await expect(item).toBeVisible();
    await expect(item.locator('.n-base-selection')).toBeVisible();
  });

  test('任务编排:建 pipeline → 加子任务 → 上移下移 → 移除', async ({ page }) => {
    const PIPE = 'pipe-' + Date.now().toString().slice(-6);
    await page.goto('/#/templates');
    await expect(page.getByText('模板列表')).toBeVisible();

    // 1) 新建编排模板
    await page.getByRole('button', { name: '+ 新建', exact: true }).click();
    await page.locator('.n-form-item', { hasText: '名称' }).locator('input').first().fill(PIPE);
    await page.locator('.n-radio-button', { hasText: '编排 Pipeline' }).click();
    await page.getByRole('button', { name: '保存' }).click();
    await expect(page.locator('.n-list-item', { hasText: PIPE })).toBeVisible();
    // 列表显示 "0 步"
    await expect(page.locator('.n-list-item', { hasText: PIPE }).getByText('0 步')).toBeVisible();

    // 2) 编辑该编排,新增两个子任务
    await page.locator('.n-list-item', { hasText: PIPE }).first().click();
    await expect(page.getByText('子任务', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: '+ 新增子任务' }).click();
    await expect(page.locator('.steprow')).toHaveCount(1);
    await page.getByRole('button', { name: '+ 新增子任务' }).click();
    await expect(page.locator('.steprow')).toHaveCount(2);

    // 3) 第一个子任务重命名后,记录顺序
    const firstName = (await page.locator('.steprow').first().locator('.step-name').innerText()).trim();
    // 下移第一个 → 顺序翻转
    await page.locator('.steprow').first().getByRole('button', { name: '↓' }).click();
    await expect(page.locator('.steprow').nth(1).locator('.step-name')).toHaveText(firstName);

    // 4) 移除一个子任务 → 剩 1
    await page.locator('.steprow').first().getByRole('button', { name: '×' }).click();
    await page.getByRole('button', { name: '确定' }).click();
    await expect(page.locator('.steprow')).toHaveCount(1);
  });
});
