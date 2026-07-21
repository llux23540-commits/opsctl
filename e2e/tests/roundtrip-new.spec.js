// 本轮四个新功能的端到端回路:模板留痕 → 执行来源/审计备份 → 备份横幅 → 审核方式。
const { test, expect } = require('./fixtures');

test.describe('原型差距补齐回路', () => {
  test.setTimeout(60000); // headed slowMo 下的长链路用例

  async function authHeaders(page) {
    const token = await page.evaluate(() => localStorage.getItem('opsctl_token'));
    const device = await page.evaluate(() => localStorage.getItem('opsctl_device'));
    return { Authorization: `Bearer ${token}`, 'x-device-id': device, device };
  }

  test('模板执行留痕:载入模板执行 → 执行记录出现模板列', async ({ page }) => {
    await page.goto('/#/console');
    await expect(page.locator('.n-tree-node').first()).toBeVisible();
    await page.getByRole('button', { name: '全选可见' }).click();
    await expect(page.getByText(/数据库区 · SQL \([1-9]/)).toBeVisible();

    // SQL 区载入「统计行数」模板(变量实时代入 → SELECT count(*) FROM servers)
    const sqlCard = page.locator('.typecard', { hasText: '数据库区' });
    await sqlCard.locator('.n-base-selection', { hasText: '载入模板' }).click();
    await page.locator('.n-base-select-option', { hasText: '统计行数' }).first().click();
    await sqlCard.getByRole('button', { name: '▶ 执行' }).click();
    await expect(sqlCard.getByText(/✓/).first()).toBeVisible();

    // 执行记录表格:模板列 + 值
    await page.goto('/#/audit');
    await expect(page.getByRole('table').getByText('模板', { exact: true })).toBeVisible();
    await expect(page.getByRole('table').getByText('统计行数').first()).toBeVisible();
  });

  test('执行来源与审计备份区块:record 页动态渲染', async ({ page, request }) => {
    await page.goto('/#/console'); // localStorage 需已导航的同源文档
    const h = await authHeaders(page);
    // 经 API 提交一条带模板的 SQL(同一 device,来源可断言)
    const res = await request.post('/api/jobs/sql', {
      headers: { Authorization: h.Authorization, 'x-device-id': h.device },
      data: { targets: ['db-demo'], query: 'SELECT count(*) FROM servers', template_id: 'tpl-count' },
    });
    const { job_id } = await res.json();

    await page.goto(`/#/record/${job_id}`);
    // 执行来源:Web 控制台 · 127.0.0.1 · e2e-device
    const source = page.getByText(/Web 控制台 · 127\.0\.0\.1 · /);
    await expect(source).toBeVisible();
    await expect(source).toContainText(h.device);
    // 概要模板行
    await expect(page.getByText('统计行数').first()).toBeVisible();
    // 审计与备份区块(真实状态文案)
    const prov = page.locator('[data-test="audit-backup"]');
    await expect(prov).toBeVisible();
    await expect(prov).toContainText('每日 03:00');
    await expect(prov).toContainText('本页含固定任务 id');
  });

  test('执行记录页备份横幅展示真实备份状态', async ({ page }) => {
    await page.goto('/#/audit');
    const banner = page.locator('[data-test="backup-banner"]');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('自动备份:每日 03:00');
    await expect(banner).toContainText('保留 30 天');
    // 启动补偿备份必然已执行过
    await expect(banner).toContainText('上次成功');
  });

  test('审核方式分级:规则设 TG → 审批列表/抽屉展示', async ({ page, request }) => {
    // 把 seed 的 operator-ssh 规则改为「需审批 + TG 一键」(新建会被先命中的旧规则遮蔽)
    await page.goto('/#/access');
    const row = page.locator('tr', { hasText: 'operator 可 SSH web 标签' });
    await row.getByRole('button', { name: '编辑' }).click();
    const modal = page.locator('.n-card').filter({ has: page.locator('.n-card-header', { hasText: '编辑授权规则' }) });
    // 前序 spec 可能已开过需审批(共享同一实例)→ 仅在关闭时才切换
    const sw = modal.locator('.n-form-item', { hasText: '需审批' }).locator('.n-switch');
    if (!(await sw.evaluate((el) => el.classList.contains('n-switch--active')))) await sw.click();
    await modal.locator('.n-form-item', { hasText: '审核方式' }).locator('.n-base-selection').first().click();
    await page.locator('.n-base-select-option', { hasText: 'TG 内联一键' }).first().click();
    await modal.getByRole('button', { name: '保存' }).click();
    await expect(page.getByText('已保存')).toBeVisible();

    // operator 提交 → 挂起
    const login = await request.post('/api/login', {
      data: { username: 'operator', password: 'operator', device_id: 'e2e-op-quick' },
    });
    const op = await login.json();
    const sub = await request.post('/api/jobs/ssh', {
      headers: { Authorization: `Bearer ${op.token}`, 'x-device-id': 'e2e-op-quick' },
      data: { targets: ['web-01'], command: 'uptime' },
    });
    const subBody = await sub.json();
    expect(subBody.results[0].pending).toBe(true);

    // 审批页:任务行展开 → 子任务带「TG 一键 + 演示」tag,抽屉含审核方式
    await page.goto('/#/approvals');
    const task = page.locator('.task', { hasText: 'operator@local' }).first();
    await task.locator('.task-hd').click();
    await expect(task.getByText('TG 一键').first()).toBeVisible();
    await expect(task.getByText('演示').first()).toBeVisible();
    await task.getByRole('button', { name: '详情' }).first().click();
    const drawer = page.locator('.n-drawer-content');
    await expect(drawer.getByText('审核方式')).toBeVisible();
    await expect(drawer.getByText('TG 内联一键(演示)')).toBeVisible();
  });
});
