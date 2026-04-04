use crate::message::NetMessage;

/// 單一訊息最大允許 payload 大小（防止 DoS 記憶體耗盡）
///
/// 對齊 wire-format.md §量化約束。
/// FullSyncPacket（含完整 state snapshot）為最大合法訊息，
/// 4 MB 留有足夠裕度。
pub const MAX_MESSAGE_SIZE: u32 = 4 * 1024 * 1024; // 4 MB

/// Codec 錯誤類型
#[derive(Debug)]
pub enum CodecError {
    /// frame 資料不足，無法解碼（需要更多 bytes）
    InsufficientData,
    /// payload 超過 MAX_MESSAGE_SIZE 上限
    MessageTooLarge { size: u32, max: u32 },
    /// bincode 序列化失敗
    Serialize(bincode::Error),
    /// bincode 反序列化失敗（收到損毀的資料）
    Deserialize(bincode::Error),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientData => write!(f, "資料不足，無法解碼"),
            Self::MessageTooLarge { size, max } => {
                write!(f, "訊息過大（{} bytes，上限 {} bytes）", size, max)
            }
            Self::Serialize(e) => write!(f, "序列化錯誤: {}", e),
            Self::Deserialize(e) => write!(f, "反序列化錯誤: {}", e),
        }
    }
}

impl std::error::Error for CodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) | Self::Deserialize(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

/// 編碼訊息為 length-prefix frame
///
/// Wire format: `[4 bytes LE u32 length] | [bincode(NetMessage)]`
///
/// # Errors
/// - `CodecError::Serialize` — bincode 序列化失敗（極罕見）
/// - `CodecError::MessageTooLarge` — 序列化後 payload 超過 `MAX_MESSAGE_SIZE`（4 MB）
pub fn encode(msg: &NetMessage) -> Result<Vec<u8>, CodecError> {
    let payload = bincode::serialize(msg).map_err(CodecError::Serialize)?;
    let len: u32 = payload
        .len()
        .try_into()
        .map_err(|_| CodecError::MessageTooLarge {
            size: u32::MAX,
            max: MAX_MESSAGE_SIZE,
        })?;
    if len > MAX_MESSAGE_SIZE {
        return Err(CodecError::MessageTooLarge {
            size: len,
            max: MAX_MESSAGE_SIZE,
        });
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// 從完整 frame 解碼訊息
///
/// 輸入必須包含 4-byte length prefix（後接 payload）。
/// 若 frame 後方有多餘 bytes，將被忽略（串流解碼情境）。
///
/// # Errors
/// - `CodecError::InsufficientData` — `frame.len() < 4` 或 payload 不完整
/// - `CodecError::MessageTooLarge` — length header 宣告超過 `MAX_MESSAGE_SIZE`
/// - `CodecError::Deserialize` — bincode 反序列化失敗
pub fn decode(frame: &[u8]) -> Result<NetMessage, CodecError> {
    if frame.len() < 4 {
        return Err(CodecError::InsufficientData);
    }
    let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
    if len > MAX_MESSAGE_SIZE {
        return Err(CodecError::MessageTooLarge {
            size: len,
            max: MAX_MESSAGE_SIZE,
        });
    }
    let len = len as usize;
    if frame.len() < 4 + len {
        return Err(CodecError::InsufficientData);
    }
    bincode::deserialize(&frame[4..4 + len]).map_err(CodecError::Deserialize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::NetMessage;

    #[test]
    fn encode_decode_round_trip() {
        let msg = NetMessage::Ping { timestamp: 12345 };
        let frame = encode(&msg).unwrap();
        let decoded = decode(&frame).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn frame_has_4_byte_length_prefix() {
        let msg = NetMessage::Ping { timestamp: 0 };
        let frame = encode(&msg).unwrap();
        let len = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(len, frame.len() - 4);
    }

    #[test]
    fn decode_truncated_frame_fails() {
        let msg = NetMessage::Ping { timestamp: 0 };
        let frame = encode(&msg).unwrap();
        let truncated = &frame[..frame.len() - 2];
        // 截斷 frame 應回傳 InsufficientData（payload 長度不足）
        assert!(matches!(
            decode(truncated),
            Err(CodecError::InsufficientData)
        ));
    }

    #[test]
    fn large_message_round_trip() {
        let msg = NetMessage::OtaChunk {
            update_id: 0u64,
            seq: 0u16,
            total: 1u16,
            data: vec![0xAB; 65536],
        };
        let frame = encode(&msg).unwrap();
        let decoded = decode(&frame).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn frame_has_correct_le_bytes() {
        // payload 長度 > 255 時，驗證 frame[0..4] 的具體 LE 位元組值
        let msg = NetMessage::OtaChunk {
            update_id: 0u64,
            seq: 0u16,
            total: 1u16,
            data: vec![0xAB; 300],
        };
        let frame = encode(&msg).unwrap();
        let payload_len = (frame.len() - 4) as u32;
        let expected_bytes = payload_len.to_le_bytes();
        assert_eq!(frame[0], expected_bytes[0]);
        assert_eq!(frame[1], expected_bytes[1]);
        assert_eq!(frame[2], expected_bytes[2]);
        assert_eq!(frame[3], expected_bytes[3]);
    }

    #[test]
    fn decode_empty_frame_returns_insufficient_data() {
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn decode_only_length_prefix_returns_insufficient_data() {
        // 宣告有 5 bytes payload 但無 payload
        assert!(decode(&[5, 0, 0, 0]).is_err());
    }

    #[test]
    fn decode_corrupted_payload_returns_deserialize_error() {
        // 正確的 length prefix 指向 4 bytes payload，但 payload 為亂碼
        let mut frame = Vec::new();
        frame.extend_from_slice(&4u32.to_le_bytes());
        frame.extend_from_slice(&[0xFF, 0xFE, 0xFD, 0xFC]);
        assert!(decode(&frame).is_err());
    }

    #[test]
    fn encode_unit_variant_round_trip() {
        let msg = NetMessage::VersionMismatch;
        let frame = encode(&msg).unwrap();
        let decoded = decode(&frame).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn decode_ignores_trailing_bytes() {
        // 串流解碼情境：frame 後方有多餘 bytes 不應導致失敗
        let msg = NetMessage::Ping { timestamp: 42 };
        let mut frame = encode(&msg).unwrap();
        frame.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let decoded = decode(&frame).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn decode_zero_length_payload() {
        // frame 恰好 4 bytes（length=0），payload 長度為零
        // bincode 無法從空 payload 反序列化 NetMessage，應返回 Deserialize 錯誤
        let frame = 0u32.to_le_bytes();
        assert!(decode(&frame).is_err());
    }

    #[test]
    fn decode_empty_frame_fails() {
        assert!(matches!(decode(&[]), Err(CodecError::InsufficientData)));
    }

    #[test]
    fn decode_only_length_prefix_fails() {
        // 宣告 5 bytes payload，但無 payload
        assert!(matches!(
            decode(&[5, 0, 0, 0]),
            Err(CodecError::InsufficientData)
        ));
    }

    #[test]
    fn decode_corrupted_payload_fails() {
        // 正確 length header，但 payload 為亂碼
        let mut frame = vec![4, 0, 0, 0]; // length = 4
        frame.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(matches!(decode(&frame), Err(CodecError::Deserialize(_))));
    }

    #[test]
    fn decode_oversized_length_header_fails() {
        // length header 宣告超過 MAX_MESSAGE_SIZE
        let size = MAX_MESSAGE_SIZE + 1;
        let frame_header = size.to_le_bytes();
        assert!(matches!(
            decode(&frame_header),
            Err(CodecError::MessageTooLarge { .. })
        ));
    }

    #[test]
    fn encode_version_mismatch_unit_variant() {
        let msg = NetMessage::VersionMismatch;
        let frame = encode(&msg).unwrap();
        // unit variant 的 payload 為 bincode variant index（u32 LE = 4 bytes）
        assert_eq!(frame.len(), 4 + (frame.len() - 4)); // length header 正確
        let decoded = decode(&frame).unwrap();
        assert_eq!(msg, decoded);
    }
}
