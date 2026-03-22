//! 確定性值型別
//!
//! `DeterministicValue` 用於 game logic 資料（PlayerInput、ReplayFrame、EcsMirror）。
//! 不含 f64（game logic 必須使用 SoftF32），確保跨平台確定性。
//!
//! Type tag 為系統契約（見 10-state-hash/01-algorithm.md）：
//! 0=Int, 1=Float, 2=Bool, 3=Unit, 4=Str
//! Variant 宣告順序必須與 type tag 一致，不可調動。

use deterministic::SoftF32;
use serde::{Deserialize, Serialize};

use crate::errors::BridgeError;

/// Str variant 長度上限（與 sandbox 字串限制一致）
pub const DETERMINISTIC_STR_MAX_BYTES: usize = 4096;

/// 確定性值型別 — 用於 game logic 資料
///
/// 使用場景：ReplayFrame、PlayerInput、EcsMirror.custom（game logic 值）
///
/// 不含 f64（game logic 必須使用 SoftF32）。
/// Str variant 有 4,096 byte 長度上限（與 sandbox 限制一致），
/// 透過 `validated_str()` 建構函式在建立時驗證。
///
/// Type tag 為系統契約（見 10-state-hash/01-algorithm.md）：
/// 0=Int, 1=Float, 2=Bool, 3=Unit, 4=Str
/// **Variant 宣告順序必須與 type tag 一致，不可調動。**
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeterministicValue {
    /// Type tag 0
    Int(i64),
    /// Type tag 1
    Float(SoftF32),
    /// Type tag 2
    Bool(bool),
    /// Type tag 3
    Unit,
    /// Type tag 4 — 上限 4096 bytes，production code 必須透過 `validated_str()` 建構
    Str(String),
}

impl DeterministicValue {
    /// 建立 Str variant 並驗證長度上限。
    /// 超過 4096 bytes 回傳 `Err(BridgeError::StringTooLong)`，
    /// 呼叫端記錄 `tracing::warn!` 並降級為 Unit。
    pub fn validated_str(s: String) -> Result<Self, BridgeError> {
        if s.len() > DETERMINISTIC_STR_MAX_BYTES {
            return Err(BridgeError::StringTooLong {
                len: s.len(),
                max: DETERMINISTIC_STR_MAX_BYTES,
            });
        }
        Ok(Self::Str(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_value_bincode_round_trip() {
        let values = vec![
            DeterministicValue::Int(-100),
            DeterministicValue::Float(SoftF32::from_f32(1.5)),
            DeterministicValue::Bool(false),
            DeterministicValue::Str("test".to_string()),
            DeterministicValue::Unit,
        ];
        for val in values {
            let bytes = bincode::serialize(&val).unwrap();
            let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
            assert_eq!(val, decoded);
        }
    }

    #[test]
    fn deterministic_value_zero_float() {
        let val = DeterministicValue::Float(SoftF32::ZERO);
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn deterministic_value_empty_str() {
        let val = DeterministicValue::Str(String::new());
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn deterministic_value_validated_str_ok() {
        let result = DeterministicValue::validated_str("hello".to_string());
        assert_eq!(
            result.unwrap(),
            DeterministicValue::Str("hello".to_string())
        );
    }

    #[test]
    fn deterministic_value_validated_str_at_limit() {
        let s = "a".repeat(DETERMINISTIC_STR_MAX_BYTES); // 剛好 4096 bytes
        let result = DeterministicValue::validated_str(s.clone());
        assert_eq!(result.unwrap(), DeterministicValue::Str(s));
    }

    #[test]
    fn deterministic_value_validated_str_over_limit() {
        let s = "a".repeat(DETERMINISTIC_STR_MAX_BYTES + 1); // 4097 bytes
        let result = DeterministicValue::validated_str(s);
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeError::StringTooLong { len, max } => {
                assert_eq!(len, 4097);
                assert_eq!(max, 4096);
            }
            other => panic!("預期 StringTooLong，實際為 {:?}", other),
        }
    }

    #[test]
    fn deterministic_value_from_f64_round_trip() {
        // f32 可精確表示的值，from_f64 → to_f64 應無損
        let soft = SoftF32::from_f64(1.5);
        assert!((soft.to_f64() - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn deterministic_value_from_f64_precision_loss() {
        // f64 精度超過 f32，截斷後應與 from_f32 結果一致
        let from_f64 = SoftF32::from_f64(1.0000001_f64);
        let from_f32 = SoftF32::from_f32(1.0000001_f32);
        assert_eq!(from_f64.to_bits(), from_f32.to_bits());
    }

    #[test]
    fn deterministic_value_from_f64_large() {
        // f64::MAX 超出 f32 範圍，應轉為 Inf
        let soft = SoftF32::from_f64(f64::MAX);
        assert_eq!(soft.to_bits(), 0x7F80_0000); // +Inf
    }

    #[test]
    fn deterministic_value_not_copy() {
        // 驗證 DeterministicValue 不 impl Copy（含 Str variant）
        // 若不慎加上 Copy derive，此測試將編譯失敗
        let val = DeterministicValue::Str("owned".to_string());
        let moved = val; // move，非 copy
                         // val 在此不可用 — 驗證 move 語義
        assert_eq!(moved, DeterministicValue::Str("owned".to_string()));
    }
}
