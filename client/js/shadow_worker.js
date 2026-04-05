// client/js/shadow_worker.js — Shadow VM Web Worker 入口
//
// 載入 wasm_shadow_worker WASM 並將主線程 postMessage 路由至：
//   - handle_init(data)    — 首次訊息（ShadowInit bincode）
//   - handle_message(data) — 後續訊息（ShadowRequest bincode）
//
// 設計依據：docs/design/architecture/06-shadow-vm/
//   - module-structure.md §Worker 入口
//   - postmessage-communication.md §訊息協議
//
// 狀態機（鏡像 Rust 端 WorkerState）：
//   wasmReady=false → 佇列訊息
//   wasmReady=true, isFirstMessage=true → handle_init → isFirstMessage=false
//   wasmReady=true, isFirstMessage=false → handle_message

import init, {
  handle_init,
  handle_message,
  shadow_worker_setup,
} from './wasm_shadow_worker.js';

let wasmReady = false;
let isFirstMessage = true;
const pendingMessages = [];

/**
 * 將訊息路由至對應的 WASM 函數。
 * @param {Uint8Array} data
 */
function dispatch(data) {
  if (isFirstMessage) {
    isFirstMessage = false;
    handle_init(data);
  } else {
    handle_message(data);
  }
}

// 立即設定 onmessage，確保 WASM 載入期間抵達的訊息不遺失。
// WASM 就緒後再排出佇列。
self.onmessage = (event) => {
  const data =
    event.data instanceof ArrayBuffer
      ? new Uint8Array(event.data)
      : event.data;

  if (!wasmReady) {
    pendingMessages.push(data);
  } else {
    dispatch(data);
  }
};

// 非同步載入 WASM，完成後排出佇列
init('./wasm_shadow_worker_bg.wasm')
  .then(() => {
    shadow_worker_setup();
    wasmReady = true;
    // 排出在 WASM 載入期間抵達的訊息（通常只有 ShadowInit 一條）
    for (const data of pendingMessages) {
      dispatch(data);
    }
    pendingMessages.length = 0;
  })
  .catch((err) => {
    console.error('[shadow_worker] WASM 初始化失敗：', err);
  });
