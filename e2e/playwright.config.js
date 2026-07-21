const { defineConfig } = require('@playwright/test');

// Random high port (override with OPSCTL_E2E_PORT). FE and BE are the SAME
// process/port (Rust serves the SPA + /api), so a random port is safe — only the
// test driver needs it, and we do. Playwright launches an ISOLATED opsctl-server
// on that port with a throwaway DB + unsealed vault, waits for /health, tears
// it down after (global-teardown removes the DB).
function freePort() {
  if (process.env.OPSCTL_E2E_PORT) return process.env.OPSCTL_E2E_PORT;
  return String(20000 + Math.floor(Math.random() * 20000)); // 20000–39999
}

const PORT = freePort();
const DB = `target/e2e-${PORT}.db`;
process.env.OPSCTL_E2E_PORT = PORT;
process.env.OPSCTL_E2E_DB = DB;

module.exports = defineConfig({
  testDir: './tests',
  timeout: 30000,
  expect: { timeout: 8000 },
  fullyParallel: false,
  workers: 1,
  retries: 1,
  reporter: [['list'], ['html', { open: 'never' }]],
  globalTeardown: require.resolve('./global-teardown.js'),
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    channel: 'chrome',
    headless: true,
    viewport: { width: 1500, height: 950 },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'cargo run -p opsctl-server',
    cwd: '..',
    url: `http://127.0.0.1:${PORT}/health`,
    reuseExistingServer: false,
    timeout: 180000,
    stdout: 'pipe',
    stderr: 'pipe',
    env: {
      OPSCTL_SERVER__BIND: `127.0.0.1:${PORT}`,
      OPSCTL_STORE__URL: `sqlite://${DB}?mode=rwc`,
      OPSCTL_AUTH__JWT_SECRET: 'e2e-secret',
      OPSCTL_VAULT__PASSPHRASE: 'e2e-pass',
      OPSCTL_DEV__SEED: 'true',
      OPSCTL_BOOTSTRAP__ADMIN_USER: 'admin',
      OPSCTL_BOOTSTRAP__ADMIN_PASSWORD: 'admin',
      // isolated backup snapshot dir (global-teardown ignores it; target/ is throwaway)
      OPSCTL_BACKUP__DIR: `target/e2e-backups-${PORT}`,
    },
  },
});
