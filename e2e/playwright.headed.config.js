// 有头(headed)实时观测配置:在基础 playwright.config.js 之上,只覆盖显示相关项,
// 让浏览器窗口弹出、动作放慢,便于肉眼跟随;并开 video/trace 作为回放兜底。
// 用法:npx playwright test flow -c playwright.headed.config.js
import base from './playwright.config.js';

export default {
  ...base,
  retries: 0, // 单次干净观看,不自动重试
  reporter: [['list']],
  use: {
    ...base.use,
    headless: false,           // 弹出真实 Chrome 窗口
    launchOptions: { slowMo: 400 }, // 每个动作放慢 400ms,过程可见
    video: 'on',               // 录像兜底
    trace: 'on',               // trace 兜底(可 show-trace 逐步回放)
  },
};
