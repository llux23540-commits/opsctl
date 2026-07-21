const { test, expect } = require('./fixtures');

// Seed a real pending approval with MULTIPLE subtasks (one job, two targets):
// admin flips operator's web rule to needs_approval, then operator submits an SSH
// command on web-01 + web-02 → one job with two pending approvals (subtasks).
async function seedPendingApproval(request) {
  const admin = await (await request.post('/api/login', {
    data: { username: 'admin', password: 'admin', device_id: 'seed-admin' },
  })).json();
  const AH = { Authorization: `Bearer ${admin.token}`, 'x-device-id': 'seed-admin' };
  const rules = await (await request.get('/api/rules', { headers: AH })).json();
  const r = rules.find((x) => x.id === 'rule-op-web');
  await request.put('/api/rules/rule-op-web', {
    headers: AH,
    data: {
      name: r.name, subject_user_id: r.subject_user_id,
      selector_kind: r.selector_kind, selector: r.selector,
      system_user_id: r.system_user_id,
      actions: String(r.actions).split(',').filter(Boolean),
      needs_approval: true, min_approvals: 1, approver_ids: [],
    },
  });
  const op = await (await request.post('/api/login', {
    data: { username: 'operator', password: 'operator', device_id: 'seed-op' },
  })).json();
  const OH = { Authorization: `Bearer ${op.token}`, 'x-device-id': 'seed-op' };
  const jr = await request.post('/api/jobs/ssh', {
    headers: OH, data: { targets: ['web-01', 'web-02'], command: 'uptime' },
  });
  const jb = await jr.json();
  const pend = (jb.results || []).filter((x) => x.pending).length;
  if (pend < 2) throw new Error('seed did not create 2 pending subtasks: ' + JSON.stringify(jb));
}

test.describe('审批确认', () => {
  test.beforeEach(async ({ page, request }) => {
    await seedPendingApproval(request);
    await page.goto('/#/approvals');
    await expect(page.getByRole('heading', { name: '待审批' }).first()).toBeVisible();
  });

  test('今日统计 pills 存在', async ({ page }) => {
    await expect(page.getByText(/今日已放行/)).toBeVisible();
    await expect(page.getByText(/今日已驳回/)).toBeVisible();
  });

  test('审批任务显示子任务进度并可展开为逐子任务', async ({ page }) => {
    const task = page.locator('.task').first();
    await expect(task).toBeVisible();
    // 进度 "2 子任务 · 2 待批"
    await expect(task.locator('.prog')).toContainText('子任务');
    await expect(task.locator('.prog')).toContainText('待批');
    // 展开 → 两个子任务行
    await task.locator('.task-hd').click();
    await expect(page.locator('.subrow')).toHaveCount(2);
    // 子任务行含目标名与命令
    await expect(page.locator('.subrow').first().locator('.scmd')).toContainText('uptime');
  });

  test('审批任务头显示环境标签(继承 site-east=prod)', async ({ page }) => {
    await expect(page.locator('.task').first().getByText('prod').first()).toBeVisible();
  });

  test('展开后子任务含放行/驳回,详情抽屉字段完整', async ({ page }) => {
    await page.locator('.task-hd').first().click();
    const sub = page.locator('.subrow').first();
    await expect(sub.getByRole('button', { name: '放行' })).toBeVisible();
    await expect(sub.getByRole('button', { name: '驳回' })).toBeVisible();
    await sub.getByRole('button', { name: '详情' }).click();
    const drawer = page.locator('.n-drawer-content');
    await expect(drawer).toBeVisible();
    await expect(drawer.getByText('完整命令')).toBeVisible();
    await expect(drawer.getByText('连接账号')).toBeVisible();
  });

  test('按任务全部放行清空该任务', async ({ page }) => {
    await expect(page.locator(".task").first()).toBeVisible();
    const before = await page.locator('.task').count();
    expect(before).toBeGreaterThan(0);
    await page.locator('.task').first().getByRole('button', { name: '全部放行' }).click();
    // 两个子任务都放行后,该任务不再出现在待审批
    // (全部放行会真执行两次 SSH,放宽超时以覆盖执行耗时)
    await expect(page.locator('.task')).toHaveCount(before - 1, { timeout: 20000 });
  });

  test('按任务全部驳回需填理由', async ({ page }) => {
    // DB 在同一 run 内跨测试共享,可能存在其他测试遗留的待审批任务,故断言"减 1"
    await expect(page.locator(".task").first()).toBeVisible();
    const before = await page.locator('.task').count();
    expect(before).toBeGreaterThan(0);
    await page.locator('.task').first().getByRole('button', { name: '全部驳回' }).click();
    const modal = page.locator('.n-card').filter({ has: page.locator('.n-card-header', { hasText: '按任务驳回' }) });
    await expect(modal).toBeVisible();
    // 不填理由直接确认 → 报错提示
    await modal.getByRole('button', { name: '确认驳回' }).click();
    await expect(page.getByText('请填写驳回理由')).toBeVisible();
    // 填理由 → 驳回成功,该任务从待审批移除
    await modal.locator('textarea').fill('生产高峰期禁止重启');
    await modal.getByRole('button', { name: '确认驳回' }).click();
    await expect(page.locator('.task')).toHaveCount(before - 1);
  });
});
