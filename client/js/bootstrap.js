// client/js/bootstrap.js — 應用啟動協調

import * as transport from './transport.js';
import init, {
  wasm_init,
  wasm_on_websocket_message,
  wasm_on_shadow_result,
  wasm_on_handshake_timeout,
  wasm_tick,
  wasm_start_game,
} from './wasm_loader.js';

let shadowWorker = null;
let handshakeTimeoutId = null;

/**
 * 啟動應用：WASM init -> WebSocket open -> ECDH 握手 -> 等待 WASM callback
 * game loop 由 WASM 握手成功後透過 js_start_game_loop() callback 觸發。
 * @returns {Promise<void>}
 */
export async function start() {
  // 1. 載入 WASM 模組（wasm-bindgen default export）
  await init({ module_or_path: './wasm_loader_bg.wasm' });

  // 2. Rust 初始化：panic hook → tracing → ECDH keygen
  //    回傳 client X25519 公鑰（32 bytes）
  const publicKeyBytes = wasm_init();

  // 3. WebSocket open
  await transport.connect();

  // 4. 訊息轉發（JS 不區分握手與遊戲訊息，統一轉發至 WASM）
  transport.onMessage((data) => wasm_on_websocket_message(data));

  // 5. 設定握手 timeout（5s）
  handshakeTimeoutId = setTimeout(() => wasm_on_handshake_timeout(), 5000);

  // 6. 送 client 公鑰至 server（觸發 ECDH 握手）
  transport.sendBinary(publicKeyBytes.buffer);

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
  if (canvas) canvas.style.display = 'block';

  wasm_start_game();
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
  transport.sendBinary(wasm_init().buffer);
};
window.__game.js_post_to_shadow_worker = (data) => postToShadowWorker(data);
window.__game.js_show_error = (message) => {
  console.error('[遊戲錯誤]', message);
  const loading = document.getElementById('loading-screen');
  if (loading) loading.style.display = 'block';
  const text = document.getElementById('loading-text');
  if (text) text.textContent = `錯誤：${message}`;
};
