// client/wasm_loader — WASM 入口點（cdylib）

pub mod crash_dump;
mod handshake;
pub mod startup;

#[cfg(feature = "single-player")]
mod single_player;

use protocol::{codec, NetMessage};
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::prelude::*;

/// 防止 wasm_start_game() 重複呼叫。
static GAME_STARTED: AtomicBool = AtomicBool::new(false);

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
            Ok(result) => {
                tracing::info!("ECDH 握手成功，session key 已建立");
                // session_key 生命週期由 JS 層的 SessionState 管理：
                // JS 側透過 __game.sessionState.establish(key) 儲存金鑰，
                // 後續 netcode 訊息加解密透過 JS 層轉發。
                // WASM 側不保留 session_key（Zeroizing 在此 scope 結束時清零）。
                let _ = result.session_key; // Zeroizing<[u8; 32]> — drop 時自動清零
                js_cancel_handshake_timeout();
                js_start_game_loop();
            }
            Err(e) => {
                tracing::error!("握手失敗：{e}");
                js_show_error(&format!("{e}"));
            }
        }
    } else {
        match handshake::decrypt_incoming(data) {
            Ok(msg) => {
                tracing::debug!("收到遊戲訊息（待 Phase 15 實作完整分派）：{:?}", msg);
                // Phase 15 補齊：排入 Bevy netcode 佇列
            }
            Err(e) => {
                tracing::warn!("遊戲訊息解密失敗：{e}");
            }
        }
    }
}

/// 握手 timeout callback（由 JS setTimeout 5s 後呼叫）。
#[wasm_bindgen]
pub fn wasm_on_handshake_timeout() {
    handshake::on_timeout();
}

/// 建立並啟動 Bevy 歡迎畫面 App。
/// 只能呼叫一次（由 js_start_game_loop() callback 觸發）；重複呼叫靜默忽略。
/// WASM 環境：WinitPlugin 接管 rAF，此函式非阻塞返回。
#[wasm_bindgen]
pub fn wasm_start_game() {
    if GAME_STARTED.swap(true, Ordering::SeqCst) {
        tracing::warn!("wasm_start_game() 已呼叫過，忽略重複呼叫");
        return;
    }
    bevy_runtime::start_welcome_app("#game-canvas");
}

/// 推進一個 game frame（由 JS requestAnimationFrame 呼叫）。
#[wasm_bindgen]
pub fn wasm_tick(timestamp: f64) {
    #[cfg(feature = "single-player")]
    single_player::tick(timestamp);

    #[cfg(not(feature = "single-player"))]
    {
        // 多人模式：Bevy App 由 JS 層 requestAnimationFrame 驅動。
        // Bevy 的 WinitPlugin 在 WASM 環境下自行管理 requestAnimationFrame loop，
        // 因此 wasm_tick 在多人模式下不需要手動推進 Bevy App。
        // timestamp 僅在 single-player 模式下由此入口使用。
        let _ = timestamp;
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
        // Shadow VM 結果由 Web Worker 透過 postMessage 傳回主線程，
        // JS 層呼叫 wasm_on_shadow_result 轉入此處。
        // 解碼為 ShadowResponse（bincode），若 Mismatch 則觸發 mismatch 處理流程
        // （見 docs/design/architecture/06-shadow-vm/mismatch-handling.md）。
        // 目前由 Bevy system 消費（vm_bevy_bridge 負責 Shadow 結果的排程處理）。
        tracing::debug!("收到 Shadow VM 結果：{} bytes", data.len());
    }
}

/// 編碼 ClientHello 訊息為 codec wire frame。
/// 內部核心邏輯（不涉及 JsValue，可在 native 環境測試）。
///
/// # Errors
/// - `InvalidPublicKeyLength` — 輸入不是 32 bytes
/// - 編碼失敗
fn encode_client_hello_inner(public_key: &[u8]) -> Result<Vec<u8>, String> {
    if public_key.len() != 32 {
        return Err(format!(
            "ClientHello 公鑰長度錯誤：預期 32 bytes，實際 {} bytes",
            public_key.len()
        ));
    }
    let mut pk = [0u8; 32];
    pk.copy_from_slice(public_key);
    let msg = NetMessage::ClientHello { public_key: pk };
    codec::encode(&msg).map_err(|e| format!("ClientHello 編碼失敗：{e}"))
}

/// 將 client X25519 公鑰封裝為標準 codec wire frame（供 bootstrap.js 呼叫）。
///
/// `public_key` 必須恰好 32 bytes，否則回傳 Err(JsValue)。
/// 輸出格式：`[4B LE len] | [bincode(NetMessage::ClientHello { public_key })]`
#[wasm_bindgen]
pub fn wasm_encode_client_hello(public_key: &[u8]) -> Result<Box<[u8]>, JsValue> {
    encode_client_hello_inner(public_key)
        .map(|frame| frame.into_boxed_slice())
        .map_err(|e| JsValue::from_str(&e))
}

/// 加密並送出一則遊戲訊息（整合測試輔助用途）。
///
/// `msg_bytes` = `bincode::serialize(&NetMessage)` 的結果（已序列化 bytes）。
/// 內部：deserialize → encrypt_outgoing → js_send_websocket。
///
/// 注意：Bevy system 送訊息應直接呼叫 `handshake::encrypt_outgoing()` 再
/// `js_send_websocket()`，避免跨 WASM 邊界的額外序列化開銷。
/// 此 export 主要供整合測試使用；Phase 15 補齊完整 Bevy 分派整合。
#[wasm_bindgen]
pub fn wasm_send_message(msg_bytes: &[u8]) -> Result<(), JsValue> {
    let msg: NetMessage = bincode::deserialize(msg_bytes)
        .map_err(|e| JsValue::from_str(&format!("訊息反序列化失敗：{e}")))?;
    let frame = handshake::encrypt_outgoing(&msg)
        .map_err(|e| JsValue::from_str(&format!("訊息加密失敗：{e}")))?;
    js_send_websocket(&frame);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::handshake::HandshakeError;
    use protocol::{codec, NetMessage};

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

    #[test]
    fn encode_client_hello_codec_format() {
        let pk = [0x42u8; 32];
        let frame = super::encode_client_hello_inner(&pk).expect("encode 應成功");
        // 輸出應可被 codec::decode 解析為 ClientHello
        let msg = codec::decode(&frame).expect("codec::decode 應成功");
        assert_eq!(
            msg,
            NetMessage::ClientHello {
                public_key: [0x42u8; 32]
            },
            "decode 後應為 ClientHello"
        );
    }

    #[test]
    fn encode_client_hello_wrong_length_fails() {
        let result = super::encode_client_hello_inner(&[0u8; 16]); // 非 32 bytes
        assert!(result.is_err(), "非 32 bytes 輸入應回傳 Err");
    }
}
