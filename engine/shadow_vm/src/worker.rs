//! Shadow VM Web Worker 端實作
//!
//! 處理來自主線程的 ShadowInit（初始化）和 ShadowRequest（驗證）訊息。
//! 此模組僅在 `wasm32` target 下編譯，依賴 wasm-bindgen / web-sys / js-sys。
//!
//! ## 狀態機
//! - `Uninitialized`：尚未收到 ShadowInit
//! - `Ready(ShadowValidator)`：已初始化，可接收 ShadowRequest
//!
//! ## 訊息協議
//! - 所有訊息使用 bincode 序列化
//! - 主線程 → Worker：`ShadowInit`（初始化）或 `ShadowRequest`（驗證）
//! - Worker → 主線程：`ShadowInitAck`（初始化成功）或 `ShadowResponse`（驗證結果）

use bridge_types::{ShadowInit, ShadowInitAck, ShadowResponse, ShadowStatus};

use crate::{executor::ShadowScript, validator::ShadowValidator};

/// Worker 端狀態機
///
/// `Ready` 以 Box 儲存 `ShadowValidator`，避免 enum variants 大小差異過大（clippy::large_enum_variant）。
enum WorkerState {
    /// 尚未收到 ShadowInit
    Uninitialized,
    /// 已初始化，可接收 ShadowRequest
    Ready(Box<ShadowValidator>),
}

// Worker 全域狀態（Web Worker 單執行緒，以 thread_local! + RefCell 取代 static mut）
//
// Web Worker 為單執行緒環境，`thread_local!` 搭配 `RefCell` 即可安全存取，
// 無需 `Mutex` 也不需 `unsafe`（WASM 亦支援 thread_local!）。
thread_local! {
    static WORKER_STATE: std::cell::RefCell<WorkerState> =
        std::cell::RefCell::new(WorkerState::Uninitialized);
}

/// 處理來自主線程的初始化訊息
///
/// 序列化格式：bincode，`ShadowInit { bytecode: Vec<u8>, bytecode_hash: [u8; 32] }`
///
/// 流程：
/// 1. bincode 反序列化 → `ShadowInit`
/// 2. blake3 驗證 bytecode 完整性（`blake3::hash(&bytecode) == bytecode_hash`）
/// 3. `ShadowValidator::new(bytecode)` 初始化 Rhai engine
/// 4. 成功 → 發送 `ShadowInitAck`（bincode 序列化）回主線程
/// 5. 失敗 → 發送 `ShadowResponse { status: Error(...), checked_ticks: [] }`
///
/// # Errors（透過 postMessage 回傳 ShadowResponse::Error）
/// - bincode 反序列化失敗：資料格式不符
/// - bytecode hash 不一致：傳輸過程中 bytecode 損毀
/// - `ShadowVmError::BytecodeLoadFailed`：bytecode 格式無效
/// - `ShadowVmError::EngineInitFailed`：Rhai Engine 初始化失敗
///
/// # 注意
/// `#[wasm_bindgen]` 匯出定義於 `client/wasm_shadow_worker`（cdylib 層），本函數為純 pub。
pub fn handle_init(data: &[u8]) {
    // 1. 反序列化 ShadowInit
    let init: ShadowInit = match bincode::deserialize(data) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Shadow Worker 反序列化 ShadowInit 失敗：{e}");
            post_error_response("反序列化 ShadowInit 失敗");
            return;
        }
    };

    // 2. 驗證 bytecode 完整性（false-positive-protection.md §防護機制 #1）
    let computed_hash = blake3::hash(&init.bytecode);
    if *computed_hash.as_bytes() != init.bytecode_hash {
        tracing::error!(
            "Shadow Worker bytecode hash 不一致：預期 {:?}，實際 {:?}",
            init.bytecode_hash,
            computed_hash.as_bytes()
        );
        post_error_response("bytecode hash 驗證失敗");
        return;
    }

    // 3. 將 bytecode（Vec<u8>）轉換為 ShadowScript
    // Shadow 架構中 bytecode 為已解密的 raw Rhai script 文字
    let script_text = match String::from_utf8(init.bytecode) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Shadow Worker bytecode UTF-8 解碼失敗：{e}");
            post_error_response("bytecode UTF-8 解碼失敗");
            return;
        }
    };
    let script = ShadowScript(script_text);

    // 4. 初始化 ShadowValidator
    match ShadowValidator::new(script) {
        Ok(validator) => {
            WORKER_STATE.with(|state| {
                *state.borrow_mut() = WorkerState::Ready(Box::new(validator));
            });
            tracing::info!("Shadow Worker 初始化完成");
            post_init_ack();
        }
        Err(e) => {
            tracing::error!("Shadow Worker 初始化失敗：{e}");
            post_error_response(&e.to_string());
        }
    }
}

/// 處理來自主線程的驗證請求（穩態訊息）
///
/// 序列化格式：bincode
/// - 輸入：`ShadowRequest`（bincode）
/// - 輸出：`ShadowResponse`（bincode），透過 postMessage 回傳
///
/// 若 Worker 尚未初始化（Uninitialized 狀態），回傳 Error("Worker 尚未初始化")
///
/// # 注意
/// `#[wasm_bindgen]` 匯出定義於 `client/wasm_shadow_worker`（cdylib 層），本函數為純 pub。
pub fn handle_message(data: &[u8]) {
    // 2. 反序列化 ShadowRequest（在進入 thread_local 借用前完成，避免巢狀借用）
    let request: bridge_types::ShadowRequest = match bincode::deserialize(data) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Shadow Worker 反序列化請求失敗：{e}");
            post_error_response("反序列化 ShadowRequest 失敗");
            return;
        }
    };

    tracing::debug!("收到驗證請求：{} 幀", request.frames.len());

    WORKER_STATE.with(|state| {
        let mut state = state.borrow_mut();

        // 1. 檢查 Worker 狀態
        let validator = match &mut *state {
            WorkerState::Ready(v) => v.as_mut(),
            WorkerState::Uninitialized => {
                tracing::warn!("Shadow Worker 收到請求但尚未初始化");
                post_error_response("Worker 尚未初始化");
                return;
            }
        };

        // 3. 執行驗證（validate_sync 目前不回傳 Err，錯誤已包裝為 ShadowStatus::Error）
        let response = match validator.validate_sync(&request) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Shadow Worker 驗證失敗：{e}");
                ShadowResponse {
                    status: ShadowStatus::Error(e.to_string()),
                    checked_ticks: vec![],
                }
            }
        };

        // 4. 記錄 Mismatch 警告（Mismatch 為預期偵測結果，非系統錯誤）
        if let ShadowStatus::Mismatch {
            tick,
            expected_hash,
            actual_hash,
        } = &response.status
        {
            tracing::warn!(
                "Shadow VM 驗證不匹配：tick={}, 預期={:?}, 實際={:?}",
                tick,
                expected_hash,
                actual_hash
            );
        }

        tracing::debug!("重播完成：{:?}", response.status);

        // 5. 序列化回應並 postMessage（降級策略：序列化失敗時改用 Error response）
        let bytes = match bincode::serialize(&response) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("Shadow Worker 序列化回應失敗：{e}");
                let err_resp = ShadowResponse {
                    status: ShadowStatus::Error("序列化回應失敗".to_string()),
                    checked_ticks: vec![],
                };
                // unwrap_or_default()：OOM 時回傳空 bytes，主線程反序列化失敗觸發錯誤處理
                bincode::serialize(&err_resp).unwrap_or_default()
            }
        };

        post_message_bytes(&bytes);
    });
}

/// 發送 ShadowInitAck 至主線程
fn post_init_ack() {
    let ack = ShadowInitAck;
    // Safety: ShadowInitAck 為零欄位 unit struct，bincode 序列化不會失敗。
    let bytes = bincode::serialize(&ack).expect("序列化 ShadowInitAck 失敗");
    post_message_bytes(&bytes);
}

/// 發送錯誤回應至主線程
fn post_error_response(msg: &str) {
    let resp = ShadowResponse {
        status: ShadowStatus::Error(msg.to_string()),
        checked_ticks: vec![],
    };
    // Safety: ShadowResponse { Error(String), vec![] } 為已知固定結構，序列化不會失敗。
    let bytes = bincode::serialize(&resp).expect("序列化錯誤回應失敗");
    post_message_bytes(&bytes);
}

/// 透過 `DedicatedWorkerGlobalScope.postMessage` 傳送 bytes 至主線程
///
/// # Panics
/// - 此函數僅在 Web Worker 環境中呼叫，`js_sys::global()` 保證回傳 `DedicatedWorkerGlobalScope`。
/// - 若在非 Worker 環境誤呼叫（如主線程），panic 為正確行為。
/// - `post_message()` 失敗表示 Worker 已斷開，此為不可恢復的環境錯誤。
fn post_message_bytes(bytes: &[u8]) {
    use js_sys::Uint8Array;
    use wasm_bindgen::JsCast;
    use web_sys::DedicatedWorkerGlobalScope;

    let global = js_sys::global()
        .dyn_into::<DedicatedWorkerGlobalScope>()
        .expect("post_message_bytes 必須在 Web Worker 環境中呼叫");
    let arr = Uint8Array::from(bytes);
    global.post_message(&arr).expect("postMessage 失敗");
}
