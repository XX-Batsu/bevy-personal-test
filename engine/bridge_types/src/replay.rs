//! Replay 系統核心型別
//!
//! 定義 `ReplayFrame`、`PlayerInput` 與 `Blake3Hash`，
//! 用於確定性 replay 記錄與回放驗證。
//!
//! 所有欄位皆為確定性型別（SoftF32 / DeterministicValue），
//! 不含 HashMap/HashSet，確保跨平台 replay 一致性。
//!
//! 參考：architecture/09-deterministic-replay/replay-log-format.md

use serde::{Deserialize, Serialize};

use crate::{DeterministicValue, EntityId};

/// blake3 hash 結果（32 bytes）
/// 用於 ReplayFrame.state_hash 和 Phase 3 state hash 計算
pub type Blake3Hash = [u8; 32];

/// 玩家輸入（確定性資料，用於 replay）
///
/// 每個 PlayerInput 記錄一個玩家在某 tick 的操作。
/// data 使用 DeterministicValue（非 DynamicValue），確保 replay 確定性。
///
/// 注意：design doc 09-deterministic-replay/replay-log-format.md 使用 `data: Vec<u8>`（raw bincode bytes）
/// Phase 2 Plan 改用 `data: DeterministicValue` 提供型別安全，
/// 序列化時由 bincode 處理，效果等價但更安全。
///
/// 不標 #[repr(C)]：data 為 DeterministicValue（含 Str(String)，heap-allocated），
/// 序列化穩定性由 bincode 格式保證。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerInput {
    /// 操作的玩家
    pub player_id: EntityId,

    /// 輸入類型（由遊戲邏輯定義具體語義）
    pub input_type: i64,

    /// 輸入資料（確定性值）
    ///
    /// 當 data 為 Str variant 時，production code 中必須透過
    /// DeterministicValue::validated_str() 建構（4096 bytes 上限），
    /// 直接 Str(...) 僅限測試使用。
    pub data: DeterministicValue,

    /// 此輸入對應的 tick
    pub tick: u64,
}

/// Replay 單幀記錄
///
/// 每個邏輯 tick 產生一筆 ReplayFrame，記錄：
/// - 所有玩家的輸入
/// - RNG 完整狀態（用於 rollback 還原）
/// - 幀結束時的 state hash（用於 desync 偵測）
///
/// rng_state 格式：16 bytes = PCG state (u64 LE) + PCG increment (u64 LE)
/// 對應 Phase 1 的 DeterministicRng::state_bytes() 輸出
///
/// 不標 #[repr(C)]：inputs 為 Vec<PlayerInput>（heap-allocated），
/// 序列化穩定性由 bincode 格式保證。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayFrame {
    /// 邏輯 tick 編號
    pub tick: u64,

    /// 本 tick 所有玩家的輸入（必須按 player_id 升序排列）
    pub inputs: Vec<PlayerInput>,

    /// 完整 PCG 狀態（state: u64 LE + increment: u64 LE = 16 bytes）
    /// 用於 rollback 時從此 tick 精確還原 RNG 狀態
    pub rng_state: [u8; 16],

    /// 本 tick 結束時的 blake3 state hash
    /// 用於 desync 偵測和 replay 驗證
    pub state_hash: Blake3Hash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_input_bincode_round_trip() {
        let input = PlayerInput {
            player_id: EntityId(1),
            input_type: 3,
            data: DeterministicValue::Int(42),
            tick: 100,
        };
        let bytes = bincode::serialize(&input).unwrap();
        let decoded: PlayerInput = bincode::deserialize(&bytes).unwrap();
        assert_eq!(input, decoded);
    }

    #[test]
    fn player_input_with_softf32() {
        use deterministic::SoftF32;
        let input = PlayerInput {
            player_id: EntityId(1),
            input_type: 1,
            data: DeterministicValue::Float(SoftF32::from_f32(3.14)),
            tick: 1,
        };
        let bytes = bincode::serialize(&input).unwrap();
        let decoded: PlayerInput = bincode::deserialize(&bytes).unwrap();
        assert_eq!(input, decoded);
    }

    #[test]
    fn replay_frame_bincode_round_trip() {
        let frame = ReplayFrame {
            tick: 60,
            inputs: vec![
                PlayerInput {
                    player_id: EntityId(1),
                    input_type: 1,
                    data: DeterministicValue::Int(10),
                    tick: 60,
                },
                PlayerInput {
                    player_id: EntityId(2),
                    input_type: 2,
                    data: DeterministicValue::Bool(true),
                    tick: 60,
                },
            ],
            rng_state: [
                0xEF, 0xBE, 0xAD, 0xDE, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ],
            state_hash: [0u8; 32],
        };
        let bytes = bincode::serialize(&frame).unwrap();
        let decoded: ReplayFrame = bincode::deserialize(&bytes).unwrap();
        assert_eq!(frame, decoded);
    }

    #[test]
    fn replay_frame_empty_inputs() {
        let frame = ReplayFrame {
            tick: 0,
            inputs: vec![],
            rng_state: [0u8; 16],
            state_hash: [0u8; 32],
        };
        let bytes = bincode::serialize(&frame).unwrap();
        let decoded: ReplayFrame = bincode::deserialize(&bytes).unwrap();
        assert_eq!(frame, decoded);
        assert!(decoded.inputs.is_empty());
    }

    #[test]
    fn replay_frame_rng_state_16_bytes() {
        let rng_state: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let frame = ReplayFrame {
            tick: 999,
            inputs: vec![],
            rng_state,
            state_hash: [0u8; 32],
        };
        let bytes = bincode::serialize(&frame).unwrap();
        let decoded: ReplayFrame = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.rng_state, rng_state);
    }

    #[test]
    fn blake3_hash_zero() {
        let frame = ReplayFrame {
            tick: 1,
            inputs: vec![],
            rng_state: [0u8; 16],
            state_hash: [0u8; 32],
        };
        let bytes = bincode::serialize(&frame).unwrap();
        let decoded: ReplayFrame = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.state_hash, [0u8; 32]);
    }

    #[test]
    fn blake3_hash_all_ff() {
        let frame = ReplayFrame {
            tick: 1,
            inputs: vec![],
            rng_state: [0u8; 16],
            state_hash: [0xFF; 32],
        };
        let bytes = bincode::serialize(&frame).unwrap();
        let decoded: ReplayFrame = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.state_hash, [0xFF; 32]);
    }

    #[test]
    fn player_input_str_data() {
        let input = PlayerInput {
            player_id: EntityId(5),
            input_type: 10,
            data: DeterministicValue::Str("move_forward".to_string()),
            tick: 42,
        };
        let bytes = bincode::serialize(&input).unwrap();
        let decoded: PlayerInput = bincode::deserialize(&bytes).unwrap();
        assert_eq!(input, decoded);
    }
}
