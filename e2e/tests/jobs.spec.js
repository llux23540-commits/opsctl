const { test, expect } = require('./fixtures');

test.describe('执行记录与追溯', () => {
  test('执行记录页双 Tab', async ({ page }) => {
    await page.goto('/#/audit');
    await expect(page.locator('.n-tabs-tab', { hasText: '执行记录' })).toBeVisible();
    await expect(page.locator('.n-tabs-tab', { hasText: '审计流水' })).toBeVisible();
  });

  test('执行记录有时间范围与状态筛选', async ({ page }) => {
    await page.goto('/#/audit');
    await expect(page.getByText('全部时间')).toBeVisible();
    await expect(page.getByText('全部状态')).toBeVisible();
  });

  test('新执行落库并可打开 record 追溯页', async ({ page, request }) => {
    // land on an app page first so localStorage is readable, then reuse the token
    await page.goto('/#/audit');
    const token = await page.evaluate(() => localStorage.getItem('opsctl_token'));
    const device = await page.evaluate(() => localStorage.getItem('opsctl_device'));
    const res = await request.post('/api/jobs/sql', {
      headers: { Authorization: `Bearer ${token}`, 'x-device-id': device },
      data: { targets: ['db-demo'], query: 'SELECT count(*) FROM servers' },
    });
    const body = await res.json();
    expect(body.job_id).toBeTruthy();
    await page.goto(`/#/record/${body.job_id}`);
    await expect(page.getByText(/JOB /)).toBeVisible();
    await expect(page.getByText('逐目标结果', { exact: false })).toBeVisible();
    await expect(page.getByText('审批追溯')).toBeVisible();
  });

  test('执行记录抽屉可单独打开', async ({ page }) => {
    await page.goto('/#/audit');
    const detailBtn = page.getByRole('button', { name: '详情' }).first();
    await detailBtn.click();
    await expect(page.locator('.n-drawer-content')).toBeVisible();
    await expect(page.getByRole('button', { name: /单独打开/ })).toBeVisible();
  });
});
