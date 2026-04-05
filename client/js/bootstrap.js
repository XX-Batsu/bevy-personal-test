// client/js/bootstrap.js — 應用啟動協調

import * as transport from './transport.js';
import * as wasmLoader from './wasm_loader.js';

let shadowWorker = null;

/**
 * 啟動應用：WASM init -> WebSocket open -> ECDH 握手 -> 等待 WASM callback
 * game loop 由 WASM 握手成功後透過 js_start_game_loop() callback 觸發。
 * @returns {Promise<void>}
 */
export async function start() {
  // 1. WASM init
  await wasmLoader.initWasm('./wasm_loader_bg.wasm');

  // 2. WebSocket open
  await transport.connect();

  // 3. 訊息轉發（JS 不區分握手與遊戲訊息，統一轉發至 WASM）
  transport.onMessage((data) => wasmLoader.onWebSocketMessage(data));

  // 4. ECDH 握手 + 5s timeout
  const wireBytes = wasmLoader.startHandshakeWithTimeout();

  // 5. 送 client 公鑰至 server
  transport.sendBinary(wireBytes.buffer);

  // 6. 啟動 Shadow VM Worker
  startShadowWorker();

  // game loop 由 WASM 握手成功 callback js_start_game_loop() 觸發
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
      wasmLoader.onShadowResult(event.data);
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
 * 切換 loading screen -> game canvas + 啟動 requestAnimationFrame loop。
 */
export function startGameLoop() {
  const loading = document.getElementById('loading-screen');
  const canvas = document.getElementById('game-canvas');
  if (loading) loading.style.display = 'none';
  if (canvas) canvas.style.display = 'block';

  function tick(ts) {
    wasmLoader.wasm_tick(ts);
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
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
window.__game.js_cancel_handshake_timeout = wasmLoader.cancelHandshakeTimeout;
window.__game.js_update_progress = updateProgress;
