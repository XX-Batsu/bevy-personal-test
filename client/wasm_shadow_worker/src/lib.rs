// client/wasm_shadow_worker — Shadow VM Web Worker cdylib
//
// 此 cdylib 是 shadow_worker.js 載入的 WASM 模組。
// 實際邏輯位於 engine/shadow_vm/src/worker.rs（pub fn，非 #[wasm_bindgen]）。
// 本 crate 在 cdylib 層定義 #[wasm_bindgen] 匯出，確保函數進入最終 WASM binary。
//
// 訊息協議（bincode）：
//   主線程 → handle_init(ShadowInit)      — 首次訊息，初始化 ShadowValidator
//   主線程 → handle_message(ShadowRequest) — 後續訊息，執行 hash 驗證
//   Worker → postMessage(ShadowInitAck / ShadowResponse) — 透過 DedicatedWorkerGlobalScope

use wasm_bindgen::prelude::*;

/// Shadow Worker 啟動初始化，由 shadow_worker.js 在 WASM init() 後呼叫。
/// debug-mode 下設定 console_error_panic_hook，確保 Rust panic 可在瀏覽器 console 顯示。
#[wasm_bindgen]
pub fn shadow_worker_setup() {
    #[cfg(feature = "debug-mode")]
    console_error_panic_hook::set_once();
    tracing::info!("Shadow Worker WASM 已初始化");
}

/// 接收主線程發送的 ShadowInit（bincode），初始化 ShadowValidator。
/// 對應 worker.rs §handle_init 完整實作。
#[wasm_bindgen]
pub fn handle_init(data: &[u8]) {
    shadow_vm::worker::handle_init(data);
}

/// 接收主線程發送的 ShadowRequest（bincode），執行 hash 驗證並回傳 ShadowResponse。
/// 對應 worker.rs §handle_message 完整實作。
#[wasm_bindgen]
pub fn handle_message(data: &[u8]) {
    shadow_vm::worker::handle_message(data);
}
