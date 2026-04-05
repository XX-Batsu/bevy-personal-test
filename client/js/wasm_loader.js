// client/js/wasm_loader.js — WASM 模組載入與初始化

import init, {
  wasm_init,
  wasm_start_handshake,
  wasm_tick,
  wasm_on_websocket_message,
  wasm_on_shadow_result,
  wasm_on_handshake_timeout,
} from '../pkg/wasm_loader.js'; // wasm-bindgen 產出（crate name = wasm_loader）

/**
 * 初始化 WASM 模組（wasm-bindgen init）。
 * @param {string} wasmPath - .wasm 檔案 URL
 * @returns {Promise<void>}
 */
export async function initWasm(wasmPath) {
  await init(wasmPath);
  wasm_init();
}

/**
 * 呼叫 WASM wasm_start_handshake()，回傳 wire-format bytes。
 * @returns {Uint8Array} wire-format encoded client public key
 */
export function startHandshake() {
  return wasm_start_handshake();
}

// --- 握手 timeout 管理（5s + wasm_on_handshake_timeout callback）---
let handshakeTimer = null;

/**
 * startHandshake() + 5s setTimeout 包裝。
 * timeout 時呼叫 WASM wasm_on_handshake_timeout()。
 * @returns {Uint8Array} wire-format encoded client public key
 */
export function startHandshakeWithTimeout() {
  const wireBytes = startHandshake();
  handshakeTimer = setTimeout(() => wasm_on_handshake_timeout(), 5000);
  return wireBytes;
}

/**
 * 取消握手 timeout（握手成功後由 WASM callback 呼叫）。
 */
export function cancelHandshakeTimeout() {
  if (handshakeTimer !== null) {
    clearTimeout(handshakeTimer);
    handshakeTimer = null;
  }
}

// --- 訊息轉發（JS 不區分握手/遊戲訊息，統一轉發至 WASM）---

/**
 * 轉發 server WebSocket 訊息至 WASM。
 * @param {ArrayBuffer} data
 */
export function onWebSocketMessage(data) {
  wasm_on_websocket_message(new Uint8Array(data));
}

/**
 * 轉發 Shadow VM 結果至 WASM。
 * @param {ArrayBuffer} data
 */
export function onShadowResult(data) {
  wasm_on_shadow_result(new Uint8Array(data));
}

export { wasm_tick };
