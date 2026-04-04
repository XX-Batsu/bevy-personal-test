//! ShadowVmClient — 主線程端 Shadow VM 客戶端
//!
//! 負責建立 Web Worker、發送 `ShadowInit` 初始化、
//! 以及序列化 `ShadowRequest` 透過 postMessage 傳送給 Worker。
//!
//! ## 設計原則
//! - Fire-and-forget：`send_request` 立即回傳，不等待 Worker 回應
//! - `onmessage` 回調由外部 Bevy system（Phase 15 整合層）驅動
//! - 僅在 `wasm32` target 下編譯（依賴 `web_sys::Worker`、`js_sys`）

use bridge_types::{ShadowInit, ShadowRequest, ShadowResponse};
use js_sys::Uint8Array;
use web_sys::Worker;

use crate::error::ShadowVmError;

/// 主線程端 Shadow VM 客戶端
///
/// 持有 Web Worker handle，負責發送 `ShadowInit` 與 `ShadowRequest`。
/// 接收 `ShadowResponse` 由外部 `worker.set_onmessage` 回調驅動（Phase 15 整合層）。
pub struct ShadowVmClient {
    worker: Worker,
}

impl ShadowVmClient {
    /// 建立 Shadow Worker 並傳送 ShadowInit（bytecode + blake3 hash）
    ///
    /// 自動計算 `blake3::hash(&bytecode)` 填入 `ShadowInit.bytecode_hash`，
    /// Worker 端以此驗證 bytecode 完整性（false-positive-protection.md §防護機制 #1）。
    ///
    /// # Errors
    /// - `ShadowVmError::EngineInitFailed` — Worker 建立失敗或初始化 postMessage 失敗
    /// - `ShadowVmError::SerializationFailed` — ShadowInit bincode 序列化失敗
    pub fn new(worker_url: &str, bytecode: Vec<u8>) -> Result<Self, ShadowVmError> {
        // 1. 建立 Worker
        let worker = Worker::new(worker_url)
            .map_err(|e| ShadowVmError::EngineInitFailed(format!("Worker 建立失敗：{e:?}")))?;

        // 2. 計算 bytecode blake3 hash 並建構 ShadowInit
        let bytecode_hash = *blake3::hash(&bytecode).as_bytes();
        let init = ShadowInit {
            bytecode,
            bytecode_hash,
        };
        let bytes = bincode::serialize(&init)
            .map_err(|e| ShadowVmError::SerializationFailed(e.to_string()))?;

        // 3. 發送 ShadowInit（postMessage）
        let arr = Uint8Array::from(bytes.as_slice());
        worker.post_message(&arr).map_err(|e| {
            ShadowVmError::EngineInitFailed(format!("ShadowInit postMessage 失敗：{e:?}"))
        })?;

        tracing::info!(
            "Shadow Worker 已啟動，發送 ShadowInit（bytecode_hash 前 8 bytes：{:02x?}）",
            &bytecode_hash[..8]
        );

        Ok(Self { worker })
    }

    /// 序列化 ShadowRequest 為 bincode → Uint8Array → postMessage
    ///
    /// 非同步：此函數立即回傳，不等待 Worker 回應（fire-and-forget）。
    ///
    /// # Errors
    /// - `ShadowVmError::SerializationFailed` — bincode 序列化失敗
    /// - `ShadowVmError::CommunicationFailed` — postMessage 失敗（Worker 未初始化、訊息無法傳遞）
    pub fn send_request(&self, request: &ShadowRequest) -> Result<(), ShadowVmError> {
        let bytes = bincode::serialize(request)
            .map_err(|e| ShadowVmError::SerializationFailed(e.to_string()))?;

        tracing::debug!(
            "發送 ShadowRequest：{} 幀，{} bytes",
            request.frames.len(),
            bytes.len()
        );

        let arr = Uint8Array::from(bytes.as_slice());
        // postMessage 失敗是通訊錯誤（Worker 未初始化、訊息無法傳遞），而非序列化錯誤
        self.worker.post_message(&arr).map_err(|e| {
            ShadowVmError::CommunicationFailed(format!("ShadowRequest postMessage 失敗：{e:?}"))
        })?;

        Ok(())
    }

    /// 處理來自 Shadow Worker 的 postMessage 回應。
    ///
    /// 呼叫方式：在 `worker.set_onmessage` callback 中取得 `MessageEvent.data()`
    /// 後傳入此方法。內部以 bincode 反序列化為 `ShadowResponse`。
    ///
    /// # 錯誤
    /// - `ShadowVmError::DeserializationFailed`：bincode 解析失敗
    pub fn on_message(
        &mut self,
        data: &wasm_bindgen::JsValue,
    ) -> Result<Option<ShadowResponse>, ShadowVmError> {
        let array = Uint8Array::new(data);
        let bytes: Vec<u8> = array.to_vec();
        let response: ShadowResponse = bincode::deserialize(&bytes)
            .map_err(|e| ShadowVmError::DeserializationFailed(e.to_string()))?;
        Ok(Some(response))
    }
}
