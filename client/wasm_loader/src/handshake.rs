// client/wasm_loader/src/handshake.rs — ECDH 握手狀態機
// 對齊 ecdh-handshake.md §ClientHandshake

use crypto::{build_hkdf_salt, derive_session_key, EcdhKeyPair, HKDF_INFO};
use protocol::NetMessage;
use std::cell::RefCell;
use zeroize::Zeroizing;

/// Wire format 最大 payload 長度（4 MiB，對齊 wire-format.md）
const MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

/// 握手錯誤型別
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // 部分 variant 待 complete() 路徑觸發
pub enum HandshakeError {
    #[error("ECDH 金鑰計算失敗：{0}")]
    EcdhFailed(String),
    #[error("AES-GCM 解密失敗（Server 回應被竄改）")]
    DecryptionFailed,
    #[error("反序列化失敗")]
    DeserializationFailed,
    #[error("握手已完成，不可重複執行")]
    AlreadyCompleted,
    #[error("{field} 長度不合規：預期 {expected} bytes，實際 {got} bytes")]
    InvalidParameterLength {
        field: &'static str,
        expected: usize,
        got: usize,
    },
    #[error("wire format payload 長度不合法：宣告 {declared} bytes，實際可用 {available} bytes")]
    InvalidPayloadLength { declared: usize, available: usize },
    #[error("握手尚未完成，無法加解密")]
    NotCompleted,
}

/// 握手成功結果
#[allow(dead_code)] // session_key 在後續 Phase 整合時使用
pub struct HandshakeResult {
    /// AES-256 session key（32 bytes），Zeroizing 確保 Drop 時清零
    pub session_key: Zeroizing<[u8; 32]>,
}

/// 內部狀態（thread_local + RefCell：WASM 單執行緒）
struct InnerState {
    key_pair: Option<EcdhKeyPair>,
    client_public_key: Option<[u8; 32]>,
    completed: bool,
    retry_count: u8,
    session_key: Option<Zeroizing<[u8; 32]>>,
}

thread_local! {
    static STATE: RefCell<InnerState> = const { RefCell::new(InnerState {
        key_pair: None,
        client_public_key: None,
        completed: false,
        retry_count: 0,
        session_key: None,
    }) };
}

/// 生成 ECDH key pair，回傳 client 公鑰（32 bytes）。
/// retry 時重新生成 ephemeral secret（安全性要求：不可重用舊 secret）。
pub fn begin() -> Result<[u8; 32], HandshakeError> {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.completed = false; // retry 時重置
        state.session_key = None; // 清零並釋放舊 session_key（Zeroizing Drop）
        let pair = EcdhKeyPair::generate();
        let public_key = pair.public_key();
        state.client_public_key = Some(public_key);
        state.key_pair = Some(pair);
        tracing::info!("ECDH：client 公鑰已生成");
        Ok(public_key)
    })
}

/// 收到 server 回應後完成握手。
/// 輸入為 wire format：length(u32 LE) || bincode(NetMessage::ServerHello)。
pub fn complete(data: &[u8]) -> Result<HandshakeResult, HandshakeError> {
    // 1. Wire format 解析
    if data.len() < 4 {
        return Err(HandshakeError::DeserializationFailed);
    }
    let payload_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
    if payload_len > MAX_MESSAGE_SIZE {
        return Err(HandshakeError::InvalidPayloadLength {
            declared: payload_len,
            available: data.len() - 4,
        });
    }
    let payload = data
        .get(4..4 + payload_len)
        .ok_or(HandshakeError::InvalidPayloadLength {
            declared: payload_len,
            available: data.len() - 4,
        })?;

    // 2. 反序列化 NetMessage::ServerHello
    let net_msg: NetMessage =
        bincode::deserialize(payload).map_err(|_| HandshakeError::DeserializationFailed)?;
    let (server_pub, nonce, encrypted_payload) = match net_msg {
        NetMessage::ServerHello {
            public_key,
            nonce,
            encrypted_payload,
        } => (public_key, nonce, encrypted_payload),
        _ => return Err(HandshakeError::DeserializationFailed),
    };

    STATE.with(|s| {
        let mut state = s.borrow_mut();
        if state.completed {
            return Err(HandshakeError::AlreadyCompleted);
        }

        // 3. Consume EcdhKeyPair → ECDH shared secret
        let pair = state
            .key_pair
            .take()
            .ok_or_else(|| HandshakeError::EcdhFailed("EcdhKeyPair 已被消耗".into()))?;
        let client_pub = state
            .client_public_key
            .ok_or_else(|| HandshakeError::EcdhFailed("client 公鑰遺失".into()))?;

        let shared = pair
            .derive_shared_secret(&server_pub)
            .map_err(|e| HandshakeError::EcdhFailed(format!("{e}")))?;

        // 4. HKDF-SHA256：衍生 session key
        let salt = build_hkdf_salt(&client_pub, &server_pub);
        let session_key_bytes = derive_session_key(&shared, &salt, HKDF_INFO)
            .map_err(|e| HandshakeError::EcdhFailed(format!("HKDF：{e}")))?;

        // 5. AES-256-GCM 解密 encrypted_payload
        // encrypted_payload 格式：ciphertext || tag(16)
        // 使用 aes_decrypt 便利 API 需要 nonce || ciphertext_with_tag 格式
        let mut combined = Vec::with_capacity(12 + encrypted_payload.len());
        combined.extend_from_slice(&nonce);
        combined.extend_from_slice(&encrypted_payload);
        let _plaintext = crypto::aes_decrypt(&session_key_bytes, &combined)
            .map_err(|_| HandshakeError::DecryptionFailed)?;

        state.session_key = Some(Zeroizing::new(session_key_bytes));
        state.completed = true;
        tracing::info!("ECDH 握手成功，session key 已建立");

        Ok(HandshakeResult {
            session_key: Zeroizing::new(**state.session_key.as_ref().unwrap()),
        })
    })
}

/// 查詢握手是否已完成。
pub fn is_completed() -> bool {
    STATE.with(|s| s.borrow().completed)
}

/// 握手 timeout 處理。
/// retry < 1：重新生成 ephemeral secret，通知 JS 重試。
/// retry >= 1：顯示錯誤畫面，不降級為未加密連線。
pub fn on_timeout() {
    let retries = STATE.with(|s| {
        let mut state = s.borrow_mut();
        let r = state.retry_count;
        state.retry_count += 1;
        r
    });

    if retries < 1 {
        tracing::warn!("ECDH 握手 timeout，執行第 {} 次重試", retries + 1);
        let _ = begin(); // 重新生成 ephemeral secret
        super::js_retry_handshake();
    } else {
        tracing::error!("ECDH 握手重試失敗，顯示錯誤畫面");
        super::js_show_error("無法建立安全連線，請重新整理頁面");
    }
}

/// 用 session_key 加密一則 NetMessage，回傳完整 wire frame bytes。
/// 握手未完成 → Err(HandshakeError::NotCompleted)
pub fn encrypt_outgoing(msg: &NetMessage) -> Result<Vec<u8>, HandshakeError> {
    STATE.with(|s| {
        let state = s.borrow();
        let key = state
            .session_key
            .as_ref()
            .ok_or(HandshakeError::NotCompleted)?;
        let plaintext = bincode::serialize(msg).expect("NetMessage 序列化不應失敗");
        // key: &Zeroizing<[u8; 32]>，deref coercion → &[u8; 32]
        let frame = crypto::encrypt_frame(&plaintext, key).expect("加密不應失敗（金鑰長度已保證）");
        Ok(frame)
    })
}

/// 用 session_key 解密一個加密 wire frame，回傳 NetMessage。
/// 握手未完成 → Err(HandshakeError::NotCompleted)
/// auth tag 驗證失敗 → Err(HandshakeError::DecryptionFailed)
pub fn decrypt_incoming(data: &[u8]) -> Result<NetMessage, HandshakeError> {
    STATE.with(|s| {
        let state = s.borrow();
        let key = state
            .session_key
            .as_ref()
            .ok_or(HandshakeError::NotCompleted)?;
        let plaintext = crypto::decrypt_frame(data, key).map_err(|e| match e {
            crypto::FrameError::AuthFailed => HandshakeError::DecryptionFailed,
            crypto::FrameError::TooShort { needed, available } => {
                HandshakeError::InvalidPayloadLength {
                    declared: needed,
                    available,
                }
            }
        })?;
        bincode::deserialize::<NetMessage>(&plaintext)
            .map_err(|_| HandshakeError::DeserializationFailed)
    })
}

#[cfg(test)]
pub(crate) fn reset_state_for_test() {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.key_pair = None;
        state.client_public_key = None;
        state.completed = false;
        state.retry_count = 0;
        state.session_key = None;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 輔助：模擬 server 端加密流程，產出合法 wire frame
    fn make_server_response_frame(client_pub: &[u8; 32]) -> Vec<u8> {
        let server_pair = EcdhKeyPair::generate();
        let server_pub = server_pair.public_key();
        let salt = build_hkdf_salt(client_pub, &server_pub);
        let shared = server_pair.derive_shared_secret(client_pub).unwrap();
        let key = derive_session_key(&shared, &salt, HKDF_INFO).unwrap();

        // 使用 aes_encrypt 便利 API（回傳 nonce || ciphertext || tag）
        let session_info_placeholder = b"session_info_data";
        let encrypted = crypto::aes_encrypt(&key, session_info_placeholder).unwrap();
        // encrypted = nonce(12) || ciphertext || tag(16)
        let nonce: [u8; 12] = encrypted[..12].try_into().unwrap();
        let encrypted_payload = encrypted[12..].to_vec();

        let msg = NetMessage::ServerHello {
            public_key: server_pub,
            nonce,
            encrypted_payload,
        };
        let payload = bincode::serialize(&msg).unwrap();
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&payload);
        frame
    }

    /// 重置 thread_local 狀態
    fn reset_state() {
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            state.key_pair = None;
            state.client_public_key = None;
            state.completed = false;
            state.retry_count = 0;
            state.session_key = None;
        });
    }

    #[test]
    fn test_begin_returns_32_bytes() {
        reset_state();
        assert_eq!(begin().unwrap().len(), 32);
    }

    #[test]
    fn test_begin_deterministic_length() {
        reset_state();
        let pk1 = begin().unwrap();
        let pk2 = begin().unwrap();
        assert_eq!(pk1.len(), 32);
        assert_eq!(pk2.len(), 32);
        // 不同 ephemeral key（高度機率）
        assert_ne!(pk1, pk2);
    }

    #[test]
    fn test_full_roundtrip() {
        reset_state();
        let client_pub = begin().unwrap();
        let frame = make_server_response_frame(&client_pub);
        let result = complete(&frame).unwrap();
        assert_eq!(result.session_key.len(), 32);
        assert!(is_completed());
    }

    #[test]
    fn test_complete_twice_fails() {
        reset_state();
        let cp = begin().unwrap();
        let f = make_server_response_frame(&cp);
        assert!(complete(&f).is_ok());
        assert!(matches!(
            complete(&f),
            Err(HandshakeError::AlreadyCompleted)
        ));
    }

    #[test]
    fn test_tampered_ciphertext() {
        reset_state();
        let cp = begin().unwrap();
        let mut f = make_server_response_frame(&cp);
        // 竄改 frame 末尾（密文/tag 區域）
        if let Some(last) = f.last_mut() {
            *last ^= 0xFF;
        }
        assert!(matches!(
            complete(&f),
            Err(HandshakeError::DecryptionFailed)
        ));
    }

    #[test]
    fn test_truncated_payload() {
        reset_state();
        let _ = begin().unwrap();
        let mut f = 100u32.to_le_bytes().to_vec();
        f.extend_from_slice(&[0u8; 10]);
        assert!(matches!(
            complete(&f),
            Err(HandshakeError::InvalidPayloadLength { .. })
        ));
    }

    #[test]
    fn test_payload_off_by_one() {
        reset_state();
        let _ = begin().unwrap();
        let mut f = 8u32.to_le_bytes().to_vec();
        f.extend_from_slice(&[0u8; 7]); // 宣告 8，實際 7
        assert!(matches!(
            complete(&f),
            Err(HandshakeError::InvalidPayloadLength { .. })
        ));
    }

    #[test]
    fn test_oversized_payload_len() {
        reset_state();
        let _ = begin().unwrap();
        let mut f = ((MAX_MESSAGE_SIZE + 1) as u32).to_le_bytes().to_vec();
        f.extend_from_slice(&[0u8; 16]);
        assert!(matches!(
            complete(&f),
            Err(HandshakeError::InvalidPayloadLength { .. })
        ));
    }

    #[test]
    fn test_empty_data() {
        reset_state();
        let _ = begin().unwrap();
        assert!(complete(&[]).is_err());
    }

    #[test]
    fn test_short_data_less_than_4_bytes() {
        reset_state();
        let _ = begin().unwrap();
        assert!(complete(&[0, 1, 2]).is_err());
    }

    #[test]
    fn test_retry_count_initial() {
        reset_state();
        STATE.with(|s| assert_eq!(s.borrow().retry_count, 0));
    }

    #[test]
    fn test_handshake_error_display() {
        let err = HandshakeError::InvalidParameterLength {
            field: "server_public_key",
            expected: 32,
            got: 16,
        };
        let msg = format!("{err}");
        assert!(msg.contains("server_public_key"));
        assert!(msg.contains("32"));
        assert!(msg.contains("16"));
    }

    /// 輔助：模擬 server 側，產出合法 ServerHello wire frame 並回傳 server session_key
    fn make_server_response_frame_with_key(client_pub: &[u8; 32]) -> (Vec<u8>, [u8; 32]) {
        let server_pair = EcdhKeyPair::generate();
        let server_pub = server_pair.public_key();
        let salt = build_hkdf_salt(client_pub, &server_pub);
        let shared = server_pair.derive_shared_secret(client_pub).unwrap();
        let server_session_key = derive_session_key(&shared, &salt, HKDF_INFO).unwrap();

        let session_info_placeholder = b"session_info_data";
        let encrypted = crypto::aes_encrypt(&server_session_key, session_info_placeholder).unwrap();
        let nonce: [u8; 12] = encrypted[..12].try_into().unwrap();
        let encrypted_payload = encrypted[12..].to_vec();

        let msg = NetMessage::ServerHello {
            public_key: server_pub,
            nonce,
            encrypted_payload,
        };
        let payload = bincode::serialize(&msg).unwrap();
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&payload);
        (frame, server_session_key)
    }

    #[test]
    fn encrypt_outgoing_before_handshake_fails() {
        reset_state();
        let msg = NetMessage::Ping { timestamp: 0 };
        let result = encrypt_outgoing(&msg);
        assert!(
            matches!(result, Err(HandshakeError::NotCompleted)),
            "握手前 encrypt_outgoing 應回傳 NotCompleted"
        );
    }

    #[test]
    fn decrypt_incoming_before_handshake_fails() {
        reset_state();
        let result = decrypt_incoming(&[0u8; 32]);
        assert!(
            matches!(result, Err(HandshakeError::NotCompleted)),
            "握手前 decrypt_incoming 應回傳 NotCompleted"
        );
    }

    #[test]
    fn session_key_zeroized_on_retry() {
        reset_state();
        // 完成第一次握手
        let pk1 = begin().unwrap();
        let (frame, _) = make_server_response_frame_with_key(&pk1);
        complete(&frame).unwrap();
        assert!(is_completed());

        // retry：begin() 應清零並釋放 session_key
        let _ = begin().unwrap();
        // 握手重置後，session_key 應為 None（is_completed = false）
        assert!(!is_completed());
        // 此時 encrypt_outgoing 應回傳 NotCompleted（session_key 已清零）
        let result = encrypt_outgoing(&NetMessage::Ping { timestamp: 0 });
        assert!(matches!(result, Err(HandshakeError::NotCompleted)));
    }

    #[test]
    fn roundtrip_after_handshake() {
        reset_state();
        let pk = begin().unwrap();
        let (frame, _server_key) = make_server_response_frame_with_key(&pk);
        complete(&frame).unwrap();

        // 加密 → 解密 round-trip
        let msg = NetMessage::Ping { timestamp: 9999 };
        let encrypted_frame = encrypt_outgoing(&msg).expect("加密應成功");
        let decrypted = decrypt_incoming(&encrypted_frame).expect("解密應成功");
        assert_eq!(decrypted, msg, "round-trip 後訊息應相等");
    }

    #[test]
    fn old_key_cannot_decrypt_new_ciphertext() {
        reset_state();
        // 第一次握手
        let pk1 = begin().unwrap();
        let (frame1, _) = make_server_response_frame_with_key(&pk1);
        complete(&frame1).unwrap();

        // 用第一次 session_key 加密一則訊息
        let msg = NetMessage::Ping { timestamp: 42 };
        let ciphertext_from_key1 = encrypt_outgoing(&msg).expect("key1 加密應成功");

        // Retry：清零 key1，重新握手（key2）
        let pk2 = begin().unwrap();
        let (frame2, _) = make_server_response_frame_with_key(&pk2);
        complete(&frame2).unwrap();

        // 用 key2 解密 key1 加密的密文 → DecryptionFailed（replay 不可能成功）
        let result = decrypt_incoming(&ciphertext_from_key1);
        assert!(
            matches!(result, Err(HandshakeError::DecryptionFailed)),
            "舊 key 加密的密文不應被新 key 解密成功"
        );
    }
}
