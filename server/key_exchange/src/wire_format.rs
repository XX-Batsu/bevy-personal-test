//! Wire format 編解碼：`[4 bytes LE length] | [bincode(NetMessage)]`
//!
//! 對齊上游 docs/design/architecture/11-wasm-loader/wire-format.md

use crate::error::KeyExchangeError;

/// NetMessage placeholder（Phase 13 engine/protocol 尚未實作，本模組使用最小定義）
/// 欄位名對齊上游 wire-format.md `NetMessage` enum 定義
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum NetMessage {
    /// ECDH Client Public Key（Step 2：Client → Server）
    EcdhClientPublicKey { public_key: [u8; 32] },
    /// ECDH Server Response（Step 3：Server → Client）
    EcdhServerResponse {
        server_public_key: [u8; 32],
        nonce: [u8; 12],
        encrypted_session_info: Vec<u8>,
    },
}

/// 將 NetMessage 編碼為 `[4 bytes LE length] | [bincode(NetMessage)]`
pub fn encode(msg: &NetMessage) -> Result<Vec<u8>, KeyExchangeError> {
    let payload =
        bincode::serialize(msg).map_err(|e| KeyExchangeError::SerializationError(e.to_string()))?;
    let len = payload.len() as u32;
    let mut frame = len.to_le_bytes().to_vec();
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// 從 `[4 bytes LE length] | [bincode(NetMessage)]` 格式解碼
pub fn decode(bytes: &[u8]) -> Result<NetMessage, KeyExchangeError> {
    if bytes.len() < 4 {
        return Err(KeyExchangeError::DecodeError("frame 太短".into()));
    }
    let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    if bytes.len() < 4 + len {
        return Err(KeyExchangeError::DecodeError("payload 不完整".into()));
    }
    bincode::deserialize(&bytes[4..4 + len])
        .map_err(|e| KeyExchangeError::DecodeError(e.to_string()))
}
