// WebSocket 实时通道客户端:登录后常连 `/api/ws`,接收集群广播与踢线通知。
// 浏览器 WS 不能带自定义头,鉴权参数(token + 设备号)走 query,与后端约定一致。
// 断线指数退避自动重连;登出或被踢(bye 帧,会话已撤销)后停止重连。
import { deviceId } from './api';

let sock = null;
let timer = null;
let backoff = 1000;
let stopped = true;
const listeners = {}; // type -> Set<fn>

/** 订阅某类帧(hello | broadcast | bye);返回退订函数。 */
export function wsOn(type, fn) {
  (listeners[type] ||= new Set()).add(fn);
  return () => listeners[type]?.delete(fn);
}

function emit(type, payload) {
  for (const fn of listeners[type] || []) {
    try { fn(payload); } catch (e) { /* 单个订阅者出错不拖垮通道 */ }
  }
}

export function wsConnect() {
  stopped = false;
  clearTimeout(timer);
  const token = localStorage.getItem('opsctl_token');
  if (!token) return;
  if (sock && (sock.readyState === WebSocket.OPEN || sock.readyState === WebSocket.CONNECTING)) return;
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  sock = new WebSocket(
    `${proto}://${location.host}/api/ws?token=${encodeURIComponent(token)}&did=${encodeURIComponent(deviceId())}&client=web`
  );
  sock.onopen = () => { backoff = 1000; };
  sock.onmessage = (e) => {
    let m;
    try { m = JSON.parse(e.data); } catch { return; }
    if (m.type === 'bye') stopped = true; // 会话被撤销:别再重连,交给页面登出
    emit(m.type, m);
  };
  sock.onclose = () => {
    sock = null;
    if (stopped || !localStorage.getItem('opsctl_token')) return;
    // 抖动:服务端重启时几千个客户端不要在同一秒齐刷刷重连(惊群)
    timer = setTimeout(wsConnect, backoff + Math.random() * 1000);
    backoff = Math.min(backoff * 2, 30000);
  };
}

export function wsClose() {
  stopped = true;
  clearTimeout(timer);
  if (sock) {
    try { sock.close(); } catch (e) { /* already closing */ }
    sock = null;
  }
}
