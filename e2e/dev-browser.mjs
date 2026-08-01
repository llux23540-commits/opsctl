// 本地调整用的独立 Playwright 浏览器:用 Playwright 自带的 Chromium + 临时 profile,
// 不碰系统安装的 Chrome / 不读用户数据。开着 CDP 端口,便于外部工具挂上来一起驱动。
//
//   node e2e/dev-browser.mjs                       # 打开 http://127.0.0.1:5173/
//   TARGET=http://127.0.0.1:8443/ node e2e/dev-browser.mjs
//   CDP_PORT=9222 HEADLESS=1 node e2e/dev-browser.mjs
import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import { chromium } from 'playwright';

const TARGET = process.env.TARGET || 'http://127.0.0.1:5173/';
const CDP_PORT = Number(process.env.CDP_PORT || 9222);
const HEADLESS = process.env.HEADLESS === '1';

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'opsctl-dev-profile-'));

const ctx = await chromium.launchPersistentContext(profile, {
  headless: HEADLESS,
  viewport: null,
  args: [`--remote-debugging-port=${CDP_PORT}`, '--window-size=1600,1000', '--window-position=40,40'],
});

const page = ctx.pages()[0] || (await ctx.newPage());
page.on('console', (m) => {
  if (m.type() === 'error') console.log(`[console.error] ${m.text().slice(0, 300)}`);
});
page.on('pageerror', (e) => console.log(`[pageerror] ${String(e).slice(0, 300)}`));
page.on('response', (r) => {
  if (r.status() >= 400) console.log(`[http ${r.status()}] ${r.request().method()} ${r.url()}`);
});

await page.goto(TARGET, { waitUntil: 'domcontentloaded' });
console.log(`playwright chromium ready → ${TARGET}`);
console.log(`cdp: http://127.0.0.1:${CDP_PORT}   profile: ${profile}`);

ctx.on('close', () => process.exit(0));
process.on('SIGTERM', async () => {
  await ctx.close().catch(() => {});
  process.exit(0);
});
// 保持进程存活,直到窗口被关掉或进程被杀
await new Promise(() => {});
