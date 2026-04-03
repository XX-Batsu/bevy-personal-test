use bridge_types::replay::PlayerInput;
use serde::{Deserialize, Serialize};

/// OTA 更新確認狀態
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AckStatus {
    /// 更新成功套用
    Ok,
    /// 需要回滾到前一版本
    Rollback,
    /// 未知狀態（相容性保留）
    Unknown,
}

/// 斷線原因，用於 Disconnect 訊息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisconnectReason {
    /// 正常斷線（玩家主動離開）
    Normal,
    /// 連線逾時
    Timeout,
    /// 版本不相容
    VersionMismatch,
    /// 偵測到作弊行為
    CheatDetected,
    /// 伺服器關閉
    ServerShutdown,
}

/// 所有網路訊息的頂層 enum
///
/// Wire format 由 `codec` 模組處理：`[4 bytes LE length] | [bincode(NetMessage)]`
/// 訊息類型辨識由 bincode variant tag 自動處理，不需要額外的 msg_type byte。
///
/// # Variant 順序穩定性
///
/// serde bincode 使用 variant 宣告順序（0-based index）作為序列化 tag。
/// 新增 variant 只能追加在末尾，不得插入或重新排列既有 variant，
/// 否則會破壞 client/server 間的 wire format 相容性。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NetMessage {
    /// 客戶端發起握手，攜帶 X25519 公鑰
    ClientHello { public_key: [u8; 32] },

    /// 伺服器回應握手，攜帶公鑰與加密 payload
    ServerHello {
        public_key: [u8; 32],
        encrypted_payload: Vec<u8>,
        nonce: [u8; 12],
    },

    /// 客戶端確認金鑰交換完成
    KeyConfirm {
        encrypted_ack: Vec<u8>,
        nonce: [u8; 12],
    },

    /// 遊戲輸入（走 Unreliable 通道）
    GameInput { tick: u64, inputs: Vec<PlayerInput> },

    /// 狀態快照（伺服器推送給重連客戶端）
    StateSnapshot {
        tick: u64,
        ecs_mirror: Vec<u8>,
        rng_state: [u8; 16],
    },

    /// 狀態 hash 驗證（走 Unreliable 通道）
    HashCheck { tick: u64, hash: [u8; 32] },

    /// 完整狀態同步（斷線重連後的完整重建）
    FullSync {
        tick: u64,
        snapshot: Vec<u8>,
        rng_state: [u8; 16],
    },

    /// OTA 更新資料分片
    OtaChunk {
        update_id: u64,
        seq: u16,
        total: u16,
        data: Vec<u8>,
    },

    /// OTA 更新確認回應
    OtaAck {
        update_id: u64,
        status: AckStatus,
        version_hash: [u8; 32],
        tick: u64,
    },

    /// 協議版本不相容通知
    VersionMismatch,

    /// 心跳請求（走 Unreliable 通道）
    Ping { timestamp: u64 },

    /// 心跳回應（走 Unreliable 通道）
    Pong { timestamp: u64 },

    /// 斷線通知
    Disconnect { reason: DisconnectReason },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_round_trip() {
        let msg = NetMessage::ClientHello {
            public_key: [42u8; 32],
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn server_hello_round_trip() {
        let msg = NetMessage::ServerHello {
            public_key: [1u8; 32],
            encrypted_payload: vec![10, 20, 30],
            nonce: [5u8; 12],
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn game_input_round_trip() {
        let msg = NetMessage::GameInput {
            tick: 100,
            inputs: vec![],
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn hash_check_round_trip() {
        let msg = NetMessage::HashCheck {
            tick: 200,
            hash: [0xAB; 32],
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn ota_chunk_round_trip() {
        let msg = NetMessage::OtaChunk {
            update_id: 123u64,
            seq: 0u16,
            total: 3u16,
            data: vec![0u8; 1024],
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn disconnect_reason_round_trip() {
        let msg = NetMessage::Disconnect {
            reason: DisconnectReason::CheatDetected,
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn ota_ack_round_trip() {
        let msg = NetMessage::OtaAck {
            update_id: u64::MAX,
            status: AckStatus::Rollback,
            version_hash: [0xFF; 32],
            tick: 999,
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn ping_pong_round_trip() {
        let ping = NetMessage::Ping {
            timestamp: u64::MAX,
        };
        let pong = NetMessage::Pong { timestamp: 0 };
        let ping_bytes = bincode::serialize(&ping).unwrap();
        let pong_bytes = bincode::serialize(&pong).unwrap();
        assert_eq!(ping, bincode::deserialize(&ping_bytes).unwrap());
        assert_eq!(pong, bincode::deserialize(&pong_bytes).unwrap());
    }

    #[test]
    fn all_variants_serializable() {
        // 確保每個 variant 都能序列化，不會 panic
        let messages = vec![
            NetMessage::ClientHello {
                public_key: [0; 32],
            },
            NetMessage::ServerHello {
                public_key: [0; 32],
                encrypted_payload: vec![],
                nonce: [0; 12],
            },
            NetMessage::KeyConfirm {
                encrypted_ack: vec![],
                nonce: [0; 12],
            },
            NetMessage::GameInput {
                tick: 0,
                inputs: vec![],
            },
            NetMessage::StateSnapshot {
                tick: 0,
                ecs_mirror: vec![],
                rng_state: [0u8; 16],
            },
            NetMessage::HashCheck {
                tick: 0,
                hash: [0; 32],
            },
            NetMessage::FullSync {
                tick: 0,
                snapshot: vec![],
                rng_state: [0u8; 16],
            },
            NetMessage::OtaChunk {
                update_id: 0u64,
                seq: 0u16,
                total: 0u16,
                data: vec![],
            },
            NetMessage::OtaAck {
                update_id: 0u64,
                status: AckStatus::Ok,
                version_hash: [0; 32],
                tick: 0,
            },
            NetMessage::VersionMismatch,
            NetMessage::Ping { timestamp: 0 },
            NetMessage::Pong { timestamp: 0 },
            NetMessage::Disconnect {
                reason: DisconnectReason::Normal,
            },
        ];
        for msg in &messages {
            let bytes = bincode::serialize(msg).unwrap();
            let _: NetMessage = bincode::deserialize(&bytes).unwrap();
        }
    }
}
