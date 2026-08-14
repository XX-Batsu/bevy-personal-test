// client/js/bootstrap.js — 應用啟動協調

import * as transport from './transport.js';
import { OFFLINE_DEMO } from './config.js';
import init, {
  wasm_init,
  wasm_encode_client_hello,
  wasm_on_websocket_message,
  wasm_on_shadow_result,
  wasm_on_handshake_timeout,
  wasm_tick,
  wasm_start_game,
} from './wasm_loader.js';

let shadowWorker = null;
let handshakeTimeoutId = null;

/**
 * 是否為離線 demo 模式（config.js 旗標或網址 `?offline`）。
 * @returns {boolean}
 */
function isOfflineDemo() {
  return OFFLINE_DEMO || new URLSearchParams(window.location.search).has('offline');
}

/**
 * 離線 demo：無 gateway server，跳過 WebSocket 與 ECDH 握手直接啟動 Bevy App。
 * ECDH 金鑰仍在本地產生（已於 start() 完成），只是不送出 ClientHello。
 * Shadow VM Worker 需要 session 才有作用，離線模式不啟動。
 */
function startOfflineDemo() {
  console.info('[bootstrap] 離線 demo 模式：跳過 WebSocket 連線與 ECDH 握手');

  const badge = document.getElementById('demo-badge');
  if (badge) {
    badge.textContent = 'Offline demo — no server, no netcode session';
    badge.hidden = false;
  }

  updateProgress(45, '離線 Demo 模式（無 server）');
  updateProgress(60, '引擎初始化');
  startGameLoop();
}

/**
 * 啟動應用：WASM init -> WebSocket open -> ECDH 握手 -> 等待 WASM callback
 * game loop 由 WASM 握手成功後透過 js_start_game_loop() callback 觸發。
 *
 * 離線 demo 模式下於步驟 2 之後分岔，不進行任何網路連線。
 * @returns {Promise<void>}
 */
export async function start() {
  // 1. 載入 WASM 模組（wasm-bindgen default export）
  await init({ module_or_path: './wasm_loader_bg.wasm' });

  // 2. Rust 初始化：panic hook → tracing → ECDH keygen
  //    回傳 client X25519 公鑰（32 bytes）
  const publicKeyBytes = wasm_init();

  // 2.5 離線 demo 分岔（靜態託管，無 server）
  if (isOfflineDemo()) {
    startOfflineDemo();
    return;
  }

  // 3. WebSocket open
  await transport.connect();

  // 4. 訊息轉發（JS 不區分握手與遊戲訊息，統一轉發至 WASM）
  transport.onMessage((data) => wasm_on_websocket_message(data));

  // 5. 設定握手 timeout（5s）
  handshakeTimeoutId = setTimeout(() => wasm_on_handshake_timeout(), 5000);

  // 6. 送 codec-encoded ClientHello 至 server（觸發 ECDH 握手）
  const clientHelloFrame = wasm_encode_client_hello(publicKeyBytes);
  transport.sendBinary(clientHelloFrame.buffer);

  // 7. 啟動 Shadow VM Worker
  startShadowWorker();

  // game loop 由 WASM 握手成功 callback js_start_game_loop() 觸發
}

/**
 * 取消握手 timeout（由 WASM js_cancel_handshake_timeout() callback 呼叫）。
 */
export function cancelHandshakeTimeout() {
  if (handshakeTimeoutId !== null) {
    clearTimeout(handshakeTimeoutId);
    handshakeTimeoutId = null;
  }
}

/**
 * 啟動 Shadow VM Web Worker。
 * Worker 通訊使用 postMessage(ArrayBuffer, [transfer])（zero-copy transfer）。
 */
export function startShadowWorker() {
  if (typeof Worker === 'undefined') return; // 不支援 Worker 時靜默停用
  shadowWorker = new Worker('./shadow_worker.js', { type: 'module' });
  shadowWorker.onmessage = (event) => {
    if (event.data instanceof ArrayBuffer) {
      wasm_on_shadow_result(new Uint8Array(event.data));
    }
  };
}

/**
 * 發送資料至 Shadow VM Worker（zero-copy transfer list）。
 * @param {ArrayBuffer} data
 */
export function postToShadowWorker(data) {
  if (shadowWorker) shadowWorker.postMessage(data, [data]);
}

/**
 * 由 WASM js_start_game_loop() callback 觸發。
 * 切換 loading screen -> game canvas，由 wasm_start_game() 啟動 Bevy App。
 * Bevy WinitPlugin 在 WASM 環境下自行接管 requestAnimationFrame，
 * 此處不再手動建立 rAF loop。
 */
export function startGameLoop() {
  const loading = document.getElementById('loading-screen');
  const canvas = document.getElementById('game-canvas');
  if (loading) loading.style.display = 'none';

  // winit 建立 window 時把 canvas 設回 display:none（視窗建立時預設不可見，
  // 由 winit 於首幀後才顯示；WASM 路徑下不會被翻回來，畫面因此永遠是黑的）。
  // 改寫發生在 wasm_start_game() 的同步執行期間，故 observer 必須先掛上。
  if (canvas) {
    canvas.style.display = 'block';
    keepCanvasVisible(canvas);
  }

  wasm_start_game();
}

/**
 * 於 durationMs 內持續確保 canvas 維持 display:block。
 * 回呼中的寫入會再次觸發 observer，但條件判斷使其於第二次即停止，不會遞迴。
 * @param {HTMLElement} canvas
 * @param {number} durationMs
 */
function keepCanvasVisible(canvas, durationMs = 3000) {
  if (typeof MutationObserver === 'undefined') return;
  const observer = new MutationObserver(() => {
    if (canvas.style.display !== 'block') canvas.style.display = 'block';
  });
  observer.observe(canvas, { attributes: true, attributeFilter: ['style'] });
  setTimeout(() => observer.disconnect(), durationMs);
}

/**
 * 更新載入進度條（由 WASM startup.rs js_update_progress() callback 觸發）。
 * @param {number} percent 進度百分比（0-100）
 * @param {string} stageName 目前階段名稱（zh-TW）
 */
export function updateProgress(percent, stageName) {
  const bar = document.getElementById('progress-bar');
  const text = document.getElementById('loading-text');
  if (bar) bar.style.width = percent + '%';
  if (text) text.textContent = stageName;
}

// 掛載至 window.__game 供 WASM callback 呼叫
window.__game = window.__game || {};
window.__game.js_start_game_loop = startGameLoop;
window.__game.js_cancel_handshake_timeout = cancelHandshakeTimeout;
window.__game.js_update_progress = updateProgress;
window.__game.js_send_websocket = (data) => transport.sendBinary(data);
window.__game.js_retry_handshake = () => {
  console.warn('[bootstrap] ECDH 握手重試');
  // wasm_init() 內部 set_once() 靜默忽略重複呼叫，屬預期行為
  const pk = wasm_init();
  const clientHelloFrame = wasm_encode_client_hello(pk);
  transport.sendBinary(clientHelloFrame.buffer);
};
window.__game.js_post_to_shadow_worker = (data) => postToShadowWorker(data);
window.__game.js_show_error = (message) => {
  console.error('[遊戲錯誤]', message);
  const loading = document.getElementById('loading-screen');
  if (loading) loading.style.display = 'block';
  const text = document.getElementById('loading-text');
  if (text) text.textContent = `錯誤：${message}`;
};
