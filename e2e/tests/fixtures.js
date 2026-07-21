// Shared fixture: an already-authenticated `page`. Logs in via the REST API and
// injects the token into localStorage before any app script runs, so specs skip
// the login UI.
//
// - RELATIVE '/api/login' so it follows use.baseURL (the isolated random port),
//   never a hardcoded one.
// - UNIQUE device_id per test: the server keeps only one active session per
//   (user, device) and drops older ones, so a shared device would let each
//   test's login invalidate the previous test's token (401 → auto-logout).
const base = require('@playwright/test');

const test = base.test.extend({
  page: async ({ page, request }, use, testInfo) => {
    const device = `e2e-${testInfo.testId}`;
    const res = await request.post('/api/login', {
      data: { username: 'admin', password: 'admin', device_id: device },
    });
    const body = await res.json();
    if (!body.token) throw new Error('e2e login failed: ' + JSON.stringify(body));
    await page.addInitScript((args) => {
      localStorage.setItem('opsctl_token', args.token);
      localStorage.setItem('opsctl_user', args.user);
      localStorage.setItem('opsctl_device', args.device);
    }, { token: body.token, user: JSON.stringify(body.user), device });
    await use(page);
  },
});

module.exports = { test, expect: base.expect };
