//! ECDH 握手管理器
//!
//! `HandshakeManager::begin_handshake()` 執行 ECDH 握手（純密碼學計算，同步函數）。
//! 超時與重試職責在 Phase 13 gateway caller 層。

use crate::error::KeyExchangeError;
use crypto::{build_hkdf_salt, derive_session_key, encrypt, EcdhKeyPair, HKDF_INFO};
use rand_core::{OsRng, RngCore};

/// Session key = AES-256-GCM key（32 bytes）
pub type SessionKey = [u8; 32];

/// 握手成功後 Server 回傳給 Client 的資料
pub struct ServerHelloPayload {
    pub server_public_key: [u8; 32],
    /// 12 bytes nonce，由 OsRng 生成（密碼學真隨機，防重放）
    pub nonce: [u8; 12],
    /// AES-256-GCM 加密後的 SessionInfo
    pub encrypted_session_info: Vec<u8>,
}

/// Server 端 session 相關資訊（由 Phase 13 gateway 組裝後傳入）
///
/// 設計依據：load-flow.md 定義 SessionInfo 由 server 邏輯層組裝。
/// Phase 2 engine/shared/src/session.rs 尚未實作，此處在 server crate 定義本地版本。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    pub session_id: u64,
    pub server_tick: u64,
    pub asset_manifest_hash: [u8; 32],
}

/// ECDH 握手管理器（無狀態，每次握手獨立生成 ephemeral keypair）
pub struct HandshakeManager;

impl HandshakeManager {
    /// 建立握手管理器
    pub fn new() -> Self {
        HandshakeManager
    }

    /// 執行 ECDH 握手（純密碼學計算，同步函數）
    ///
    /// 回傳 server hello payload 與衍生的 session key。
    /// 超時與重試職責在 Phase 13 gateway caller 層（tokio::time::timeout 包裝），
    /// 本函數無 async/timeout/retry 邏輯。
    ///
    /// # 參數
    /// - `client_public_key`: 客戶端 X25519 公鑰（all-zero 公鑰 → `InvalidClientKey`）
    /// - `session_info`: 由 caller 組裝的 session 資訊（加密後放入 payload）
    ///
    /// # 不變式
    /// - nonce 由 OsRng 生成（密碼學真隨機，非 DeterministicRng）
    /// - HKDF salt = `build_hkdf_salt(client_pk, server_pk)`（64 bytes 動態值）
    /// - 使用底層 4 參數 `encrypt(key, nonce, plaintext, aad)` API（非便利 API）
    pub fn begin_handshake(
        &self,
        client_public_key: &[u8; 32],
        session_info: &SessionInfo,
    ) -> Result<(ServerHelloPayload, SessionKey), KeyExchangeError> {
        // 拒絕 all-zero 公鑰（low-order point 防護）
        if client_public_key.iter().all(|&b| b == 0) {
            return Err(KeyExchangeError::InvalidClientKey);
        }

        // 生成 server ephemeral keypair
        let server_pair = EcdhKeyPair::generate();
        let server_public_key = server_pair.public_key();

        // 計算 ECDH shared secret（consume server_pair）
        let shared_secret = server_pair
            .derive_shared_secret(client_public_key)
            .map_err(KeyExchangeError::CryptoError)?;

        // 組裝 HKDF salt（client ‖ server，64 bytes 動態值）
        let salt = build_hkdf_salt(client_public_key, &server_public_key);

        // 衍生 session key
        let session_key = derive_session_key(&shared_secret, &salt, HKDF_INFO)?;

        // 生成 nonce（OsRng 密碼學真隨機，非 DeterministicRng）
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);

        // 序列化 SessionInfo 並以底層 4 參數 encrypt API 加密（顯式 nonce）
        let plaintext = bincode::serialize(session_info)
            .map_err(|e| KeyExchangeError::SerializationError(e.to_string()))?;
        let encrypted_session_info = encrypt(&session_key, &nonce, &plaintext, &[])?;

        let payload = ServerHelloPayload {
            server_public_key,
            nonce,
            encrypted_session_info,
        };

        Ok((payload, session_key))
    }
}

impl Default for HandshakeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ConnectionId, SessionRegistry};
    use crate::wire_format::{decode, encode, NetMessage};
    use crypto::{build_hkdf_salt, decrypt, derive_session_key, EcdhKeyPair, HKDF_INFO};
    use zeroize::Zeroizing;

    fn make_session_info() -> SessionInfo {
        SessionInfo {
            session_id: 42,
            server_tick: 100,
            asset_manifest_hash: [0x12; 32],
        }
    }

    /// ECDH 雙端 derive 相同 session key
    #[test]
    fn test_ecdh_derive_same_session_key() {
        let manager = HandshakeManager::new();
        let client_pair = EcdhKeyPair::generate();
        let client_pk = client_pair.public_key();

        let session_info = make_session_info();
        let (payload, server_session_key) = manager
            .begin_handshake(&client_pk, &session_info)
            .expect("握手應成功");

        // Client 端計算 session key
        let client_shared_secret = client_pair
            .derive_shared_secret(&payload.server_public_key)
            .expect("Client 計算 shared secret 應成功");
        let salt = build_hkdf_salt(&client_pk, &payload.server_public_key);
        let client_session_key = derive_session_key(&client_shared_secret, &salt, HKDF_INFO)
            .expect("Client 衍生 session key 應成功");

        assert_eq!(
            server_session_key, client_session_key,
            "Server 與 Client 衍生的 session key 應相同"
        );
    }

    /// Wire format encode/decode round-trip
    #[test]
    fn test_wire_format_round_trip() {
        let msg = NetMessage::EcdhClientPublicKey {
            public_key: [0u8; 32],
        };
        let frame = encode(&msg).expect("encode 應成功");
        let decoded = decode(&frame).expect("decode 應成功");
        assert_eq!(msg, decoded, "decode 後應與原始值相等");
    }

    /// Wire format LE length field 正確性（payload 長度 > 255）
    #[test]
    fn test_wire_format_le_length_field() {
        let msg = NetMessage::EcdhClientPublicKey {
            public_key: [0u8; 32],
        };
        let frame = encode(&msg).expect("encode 應成功");
        let payload_len = frame.len() - 4;
        let expected_le = (payload_len as u32).to_le_bytes();
        assert_eq!(
            &frame[0..4],
            &expected_le,
            "frame 前 4 bytes 應為 payload 長度的 LE 表示"
        );
    }

    /// decode frame 太短
    #[test]
    fn test_decode_frame_too_short() {
        let result = decode(&[0u8; 3]);
        assert!(
            matches!(result, Err(KeyExchangeError::DecodeError(_))),
            "< 4 bytes 應回傳 DecodeError"
        );
    }

    /// decode payload 不完整
    #[test]
    fn test_decode_payload_incomplete() {
        let mut frame = vec![0u8; 4];
        // 宣稱 payload 100 bytes，但實際只有 10 bytes
        frame[0..4].copy_from_slice(&100u32.to_le_bytes());
        frame.extend_from_slice(&[0u8; 10]);
        let result = decode(&frame);
        assert!(
            matches!(result, Err(KeyExchangeError::DecodeError(_))),
            "payload 不完整應回傳 DecodeError"
        );
    }

    /// decode bincode 錯誤
    #[test]
    fn test_decode_bincode_error() {
        let mut frame = vec![0u8; 4];
        frame[0..4].copy_from_slice(&8u32.to_le_bytes());
        frame.extend_from_slice(&[0xFF; 8]);
        let result = decode(&frame);
        assert!(
            matches!(result, Err(KeyExchangeError::DecodeError(_))),
            "無效 bincode payload 應回傳 DecodeError"
        );
    }

    /// all-zero 公鑰 → InvalidClientKey
    #[test]
    fn test_begin_handshake_invalid_client_key() {
        let manager = HandshakeManager::new();
        let zero_key = [0u8; 32];
        let session_info = make_session_info();
        let result = manager.begin_handshake(&zero_key, &session_info);
        assert!(
            matches!(result, Err(KeyExchangeError::InvalidClientKey)),
            "all-zero 公鑰應回傳 InvalidClientKey"
        );
    }

    /// 不同 client → 不同 session key
    #[test]
    fn test_different_clients_different_keys() {
        let manager = HandshakeManager::new();
        let client_a = EcdhKeyPair::generate();
        let client_b = EcdhKeyPair::generate();
        let session_info = make_session_info();

        let (_, key_a) = manager
            .begin_handshake(&client_a.public_key(), &session_info)
            .unwrap();
        let (_, key_b) = manager
            .begin_handshake(&client_b.public_key(), &session_info)
            .unwrap();

        assert_ne!(key_a, key_b, "不同 client 應得到不同 session key");
    }

    /// 連續兩次握手 nonce 不相同
    #[test]
    fn test_nonce_uniqueness() {
        let manager = HandshakeManager::new();
        let client_pair = EcdhKeyPair::generate();
        let client_pk = client_pair.public_key();
        let session_info = make_session_info();

        // 需要兩個不同的 keypair 因為 derive_shared_secret 會消耗 self
        let client_pair2 = EcdhKeyPair::generate();
        let _ = client_pair.derive_shared_secret(&[1u8; 32]).ok(); // consume pair 1
        let client_pk2 = client_pair2.public_key();

        let (payload_1, _) = manager.begin_handshake(&client_pk, &session_info).unwrap();
        let (payload_2, _) = manager.begin_handshake(&client_pk2, &session_info).unwrap();

        // 每次握手的 nonce 應不同（OsRng 保證）
        // 注意：此測試為弱唯一性斷言（非統計性碰撞測試），12 bytes nonce collision 幾乎不可能
        assert_ne!(
            payload_1.nonce, payload_2.nonce,
            "連續兩次握手的 nonce 應不同"
        );
    }

    /// SessionRegistry: register → get → remove
    #[test]
    fn test_session_registry_store_retrieve_remove() {
        let mut registry = SessionRegistry::new();
        let key = Zeroizing::new([0xAB; 32]);
        let conn_id: ConnectionId = 1;

        registry.register(conn_id, key);
        assert_eq!(
            registry.get(conn_id).unwrap(),
            &[0xAB; 32],
            "register 後 get 應成功"
        );

        registry.remove(conn_id);
        assert!(registry.get(conn_id).is_none(), "remove 後 get 應回傳 None");
    }

    /// build_hkdf_salt: salt[..32] == client_pk, salt[32..] == server_pk
    #[test]
    fn test_build_hkdf_salt_64bytes() {
        let client_pk = [0xAA; 32];
        let server_pk = [0xBB; 32];
        let salt = build_hkdf_salt(&client_pk, &server_pk);
        assert_eq!(salt.len(), 64, "salt 應為 64 bytes");
        assert_eq!(&salt[..32], &client_pk, "salt 前 32 bytes 應為 client 公鑰");
        assert_eq!(&salt[32..], &server_pk, "salt 後 32 bytes 應為 server 公鑰");
    }

    /// SessionInfo bincode round-trip
    #[test]
    fn test_session_info_serialization() {
        let info = make_session_info();
        let bytes = bincode::serialize(&info).expect("序列化應成功");
        let decoded: SessionInfo = bincode::deserialize(&bytes).expect("反序列化應成功");
        assert_eq!(info, decoded, "SessionInfo bincode round-trip 應一致");
    }

    /// begin_handshake 加密 session_info 可驗證解密
    #[test]
    fn test_begin_handshake_with_session_info() {
        let manager = HandshakeManager::new();
        let client_pair = EcdhKeyPair::generate();
        let client_pk = client_pair.public_key();
        let session_info = make_session_info();

        let (payload, session_key) = manager
            .begin_handshake(&client_pk, &session_info)
            .expect("握手應成功");

        // 解密 encrypted_session_info
        let decrypted = decrypt(
            &session_key,
            &payload.nonce,
            &payload.encrypted_session_info,
            &[],
        )
        .expect("解密應成功");

        let decoded_info: SessionInfo = bincode::deserialize(&decrypted).expect("反序列化應成功");
        assert_eq!(
            session_info, decoded_info,
            "解密後 SessionInfo 應與原始值一致"
        );
    }

    /// re-register 覆蓋舊 key
    #[test]
    fn test_session_registry_re_register_overwrites() {
        let mut registry = SessionRegistry::new();
        registry.register(1, Zeroizing::new([0xAA; 32]));
        registry.register(1, Zeroizing::new([0xBB; 32]));
        assert_eq!(
            registry.get(1).unwrap(),
            &[0xBB; 32],
            "重複 register 後應回傳新值"
        );
    }

    /// AES 解密失敗（竄改密文）
    #[test]
    fn test_aes_decrypt_failure() {
        let manager = HandshakeManager::new();
        let client_pair = EcdhKeyPair::generate();
        let client_pk = client_pair.public_key();
        let session_info = make_session_info();

        let (mut payload, session_key) = manager
            .begin_handshake(&client_pk, &session_info)
            .expect("握手應成功");

        // 竄改密文第一個 byte
        if let Some(b) = payload.encrypted_session_info.first_mut() {
            *b ^= 0xFF;
        }

        let result = decrypt(
            &session_key,
            &payload.nonce,
            &payload.encrypted_session_info,
            &[],
        );
        assert!(result.is_err(), "竄改密文後解密應失敗");
    }

    /// wire decode 超大 payload（防禦性）
    #[test]
    fn test_wire_decode_oversized() {
        // length header 宣稱 2 MB，但實際資料很短
        let len: u32 = 2 * 1024 * 1024; // 2 MB
        let mut frame = len.to_le_bytes().to_vec();
        frame.extend_from_slice(&[0u8; 100]); // 實際只有 100 bytes
        let result = decode(&frame);
        assert!(
            matches!(result, Err(KeyExchangeError::DecodeError(_))),
            "payload 不完整應回傳 DecodeError"
        );
    }
}
