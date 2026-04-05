// client/wasm_loader — WASM 入口點（cdylib）

pub mod crash_dump;
mod handshake;
pub mod startup;

#[cfg(feature = "single-player")]
mod single_player;

use wasm_bindgen::prelude::*;

// ── JS imports（WASM 呼叫 JS）──

#[wasm_bindgen]
extern "C" {
    /// 取得當前時間戳（DOMHighResTimeStamp，毫秒）。
    /// WASM 環境禁止 std::time::Instant，使用此替代。
    #[wasm_bindgen(js_namespace = ["performance"], js_name = "now")]
    fn js_get_timestamp() -> f64;

    /// 傳送 binary 資料至 server（由 transport.js sendBinary() 實作）。
    #[wasm_bindgen(js_namespace = ["window", "__game"])]
    fn js_send_websocket(data: &[u8]);

    /// 傳送任務至 Shadow VM Web Worker（zero-copy transfer）。
    #[wasm_bindgen(js_namespace = ["window", "__game"])]
    fn js_post_to_shadow_worker(data: &[u8]);

    /// 握手重試：通知 JS 重新發起 startHandshakeWithTimeout()。
    #[wasm_bindgen(js_namespace = ["window", "__game"])]
    fn js_retry_handshake();

    /// 顯示錯誤畫面（握手最終失敗時呼叫，不降級為未加密連線）。
    #[wasm_bindgen(js_namespace = ["window", "__game"])]
    fn js_show_error(message: &str);

    /// 握手成功後通知 JS 啟動 game loop。
    #[wasm_bindgen(js_namespace = ["window", "__game"])]
    fn js_start_game_loop();

    /// 取消握手 timeout（握手成功後呼叫）。
    #[wasm_bindgen(js_namespace = ["window", "__game"])]
    fn js_cancel_handshake_timeout();
}

// ── WASM exports（JS 呼叫 WASM）──

/// 初始化 WASM 模組：panic hook → tracing → ECDH keygen。
/// 回傳 client X25519 公鑰（32 bytes）。
#[wasm_bindgen]
pub fn wasm_init() -> Result<Box<[u8]>, JsValue> {
    #[cfg(feature = "debug-mode")]
    console_error_panic_hook::set_once();

    tracing::info!("WASM 初始化開始");

    let public_key =
        handshake::begin().map_err(|e| JsValue::from_str(&format!("ECDH keygen 失敗：{e}")))?;

    tracing::info!("WASM 初始化完成，ECDH 公鑰已生成");
    Ok(public_key.to_vec().into_boxed_slice())
}

/// WebSocket 訊息接收。
/// 握手中→解析 EcdhServerResponse；遊戲中→netcode 佇列。
#[wasm_bindgen]
pub fn wasm_on_websocket_message(data: &[u8]) {
    if !handshake::is_completed() {
        match handshake::complete(data) {
            Ok(_result) => {
                tracing::info!("ECDH 握手成功");
                // TODO: _result.session_key → SessionState::establish()
                js_cancel_handshake_timeout();
                js_start_game_loop();
            }
            Err(e) => {
                tracing::error!("握手失敗：{e}");
                js_show_error(&format!("{e}"));
            }
        }
    } else {
        tracing::debug!("收到遊戲訊息：{} bytes", data.len());
        // TODO: Phase 11 netcode 整合
    }
}

/// 握手 timeout callback（由 JS setTimeout 5s 後呼叫）。
#[wasm_bindgen]
pub fn wasm_on_handshake_timeout() {
    handshake::on_timeout();
}

/// 推進一個 game frame（由 JS requestAnimationFrame 呼叫）。
#[wasm_bindgen]
pub fn wasm_tick(timestamp: f64) {
    #[cfg(feature = "single-player")]
    single_player::tick(timestamp);

    #[cfg(not(feature = "single-player"))]
    {
        let _ = timestamp;
        // TODO: Phase 9 Bevy App 驅動
    }
}

/// 轉發 Shadow VM 結果至 WASM。
#[wasm_bindgen]
pub fn wasm_on_shadow_result(data: &[u8]) {
    #[cfg(feature = "no-shadow-vm")]
    {
        let _ = data;
        return;
    }
    #[allow(unreachable_code)]
    {
        tracing::debug!("收到 Shadow VM 結果：{} bytes", data.len());
        // TODO: Phase 14 Shadow VM 結果處理
    }
}

#[cfg(test)]
mod tests {
    use super::handshake::HandshakeError;

    #[test]
    fn test_handshake_error_display() {
        let err = HandshakeError::InvalidParameterLength {
            field: "server_public_key",
            expected: 32,
            got: 16,
        };
        assert!(format!("{err}").contains("server_public_key"));
        assert!(format!("{err}").contains("32"));
    }

    #[test]
    fn test_entry_points_compile() {
        // 編譯成功即驗證 wasm_bindgen 簽名正確
    }

    #[test]
    fn test_shadow_result_no_panic() {
        super::wasm_on_shadow_result(&[1, 2, 3]);
    }
}
