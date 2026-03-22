//! 錯誤型別定義
//!
//! `BridgeError` — Bridge API 呼叫錯誤，用於 Rhai 腳本中表現為 runtime error。
//! `ScriptError` — 腳本執行錯誤，由 VM runtime 產生，Bridge 層捕獲並記錄。
//!
//! 兩者分屬不同錯誤域：
//! - BridgeError：單次 Bridge API 呼叫失敗（Phase 6 轉為 `Box<EvalAltResult>`）
//! - ScriptError：腳本整體執行失敗（Phase 8 ScriptManager 收集並記錄）

use serde::{Deserialize, Serialize};

use crate::EntityId;

/// Bridge API 呼叫錯誤
/// 在 Rhai 腳本中表現為 runtime error（見 03-bridge-api/entity-operations.md 錯誤處理）
///
/// 錯誤處理策略：
/// - EntityNotFound: 回傳 () 並 log warning（不中斷腳本）
/// - InvalidHandle: 靜默忽略（fire-and-forget 語義）
/// - InvalidParameter: 中斷該次 API 呼叫，log error
/// - ScriptDisabled: 中斷整個腳本執行
/// - StringTooLong: 字串長度超過上限，由 validated_str() 觸發
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BridgeError {
    /// Entity ID 不存在或已 despawn
    EntityNotFound(EntityId),

    /// EffectHandle / SoundHandle 無效或已過期
    InvalidHandle,

    /// 參數驗證失敗（座標 NaN/Inf、負數 scale 等）
    InvalidParameter(String),

    /// 腳本已被停用（管理指令或安全機制觸發）
    ScriptDisabled,

    /// 字串長度超過上限（DeterministicValue::Str 的 4096 bytes 限制）
    /// 由 validated_str() 建構函式觸發（見 task-02）
    StringTooLong { len: usize, max: usize },
}

/// 腳本執行錯誤
/// 由 VM runtime 產生，Bridge 層捕獲並記錄
///
/// 對齊上游 bridge-events.md / timeout-behavior.md 定義：
/// - 管理層 variant（Timeout/OperationLimit/RuntimeError）包含 script_id + tick，
///   由 Phase 8 ScriptManager 在捕獲 VM 層錯誤後補充上下文
/// - VM 層 variant（CompileError/BudgetExhausted）不含 script_id + tick，
///   由 Phase 6 SandboxedEngine 直接產生
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScriptError {
    // ── 管理層 variant（Phase 8 ScriptManager 產生，含上下文） ──
    /// 執行超時（單次 callback 2ms 上限）
    /// elapsed_ms: f64 為診斷資料，非 game logic
    Timeout {
        script_id: String,
        elapsed_ms: f64,
        tick: u64,
    },

    /// 操作數上限（50,000 ops）
    OperationLimit {
        script_id: String,
        ops: u64,
        tick: u64,
    },

    /// 腳本執行期錯誤（除零、型別不符等）
    RuntimeError {
        script_id: String,
        message: String,
        tick: u64,
    },

    // ── VM 層 variant（Phase 6 SandboxedEngine 產生，不含上下文） ──
    /// Rhai AST 編譯失敗（語法錯誤）
    /// Phase 6 SandboxedEngine::execute() 在 compile 階段產生
    CompileError(String),

    /// 幀預算耗盡（remaining_ms ≤ 0）
    /// Phase 6 execute_with_budget() 在預算檢查時產生
    BudgetExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BridgeError 測試 ──

    #[test]
    fn bridge_error_bincode_round_trip() {
        let errors = vec![
            BridgeError::EntityNotFound(EntityId(42)),
            BridgeError::EntityNotFound(EntityId(0)),
            BridgeError::EntityNotFound(EntityId(u64::MAX)),
            BridgeError::InvalidHandle,
            BridgeError::InvalidParameter("NaN coordinate".to_string()),
            BridgeError::InvalidParameter(String::new()),
            BridgeError::ScriptDisabled,
            BridgeError::StringTooLong {
                len: 5000,
                max: 4096,
            },
        ];
        for err in errors {
            let bytes = bincode::serialize(&err).unwrap();
            let decoded: BridgeError = bincode::deserialize(&bytes).unwrap();
            assert_eq!(err, decoded);
        }
    }

    #[test]
    fn bridge_error_entity_not_found_zero() {
        let err = BridgeError::EntityNotFound(EntityId(0));
        let bytes = bincode::serialize(&err).unwrap();
        let decoded: BridgeError = bincode::deserialize(&bytes).unwrap();
        assert_eq!(err, decoded);
    }

    #[test]
    fn bridge_error_entity_not_found_max() {
        let err = BridgeError::EntityNotFound(EntityId(u64::MAX));
        let bytes = bincode::serialize(&err).unwrap();
        let decoded: BridgeError = bincode::deserialize(&bytes).unwrap();
        assert_eq!(err, decoded);
    }

    #[test]
    fn bridge_error_invalid_param_long() {
        let err = BridgeError::InvalidParameter("x".repeat(10_000));
        let bytes = bincode::serialize(&err).unwrap();
        let decoded: BridgeError = bincode::deserialize(&bytes).unwrap();
        assert_eq!(err, decoded);
    }

    #[test]
    fn bridge_error_string_too_long() {
        let err = BridgeError::StringTooLong {
            len: 5000,
            max: 4096,
        };
        let bytes = bincode::serialize(&err).unwrap();
        let decoded: BridgeError = bincode::deserialize(&bytes).unwrap();
        assert_eq!(err, decoded);
    }

    #[test]
    fn bridge_error_debug_format() {
        // 驗證所有 5 個 variant 的 Debug format 不 panic
        let errors: Vec<BridgeError> = vec![
            BridgeError::EntityNotFound(EntityId(1)),
            BridgeError::InvalidHandle,
            BridgeError::InvalidParameter("test".to_string()),
            BridgeError::ScriptDisabled,
            BridgeError::StringTooLong {
                len: 5000,
                max: 4096,
            },
        ];
        for err in &errors {
            let _ = format!("{:?}", err);
        }
    }

    #[test]
    fn bridge_error_clone_eq() {
        let err = BridgeError::EntityNotFound(EntityId(1));
        assert_eq!(err.clone(), err);
    }

    #[test]
    fn bridge_error_bincode_truncated() {
        let err = BridgeError::EntityNotFound(EntityId(42));
        let bytes = bincode::serialize(&err).unwrap();
        let truncated = &bytes[..bytes.len() / 2];
        assert!(bincode::deserialize::<BridgeError>(truncated).is_err());
    }

    #[test]
    fn bridge_error_bincode_empty() {
        assert!(bincode::deserialize::<BridgeError>(&[]).is_err());
    }

    // ── ScriptError 測試 ──

    #[test]
    fn script_error_bincode_round_trip() {
        let errors = vec![
            ScriptError::Timeout {
                script_id: "s1".into(),
                elapsed_ms: 2.5,
                tick: 100,
            },
            ScriptError::Timeout {
                script_id: "s1".into(),
                elapsed_ms: 0.0,
                tick: 0,
            },
            ScriptError::Timeout {
                script_id: "s1".into(),
                elapsed_ms: 2.0,
                tick: 60,
            },
            ScriptError::Timeout {
                script_id: "s1".into(),
                elapsed_ms: 1000.0,
                tick: u64::MAX,
            },
            ScriptError::OperationLimit {
                script_id: "s2".into(),
                ops: 50_000,
                tick: 42,
            },
            ScriptError::OperationLimit {
                script_id: "s2".into(),
                ops: 50_001,
                tick: 42,
            },
            ScriptError::RuntimeError {
                script_id: "s3".into(),
                message: "division by zero".into(),
                tick: 10,
            },
            ScriptError::RuntimeError {
                script_id: "s3".into(),
                message: String::new(),
                tick: 0,
            },
            ScriptError::Timeout {
                script_id: String::new(),
                elapsed_ms: 1.0,
                tick: 0,
            },
            ScriptError::CompileError("unexpected token '}'".to_string()),
            ScriptError::CompileError(String::new()),
            ScriptError::BudgetExhausted,
        ];
        for err in errors {
            let bytes = bincode::serialize(&err).unwrap();
            let decoded: ScriptError = bincode::deserialize(&bytes).unwrap();
            assert_eq!(err, decoded);
        }
    }

    #[test]
    fn script_error_debug_format() {
        // 驗證所有 5 個 variant 的 Debug format 不 panic
        let errors: Vec<ScriptError> = vec![
            ScriptError::Timeout {
                script_id: "s1".into(),
                elapsed_ms: 2.5,
                tick: 100,
            },
            ScriptError::OperationLimit {
                script_id: "s2".into(),
                ops: 50_000,
                tick: 42,
            },
            ScriptError::RuntimeError {
                script_id: "s3".into(),
                message: "error".into(),
                tick: 10,
            },
            ScriptError::CompileError("syntax error".to_string()),
            ScriptError::BudgetExhausted,
        ];
        for err in &errors {
            let _ = format!("{:?}", err);
        }
    }

    #[test]
    fn script_error_clone_eq() {
        let err = ScriptError::Timeout {
            script_id: "s1".into(),
            elapsed_ms: 2.5,
            tick: 100,
        };
        assert_eq!(err.clone(), err);
    }

    #[test]
    fn script_error_bincode_truncated() {
        let err = ScriptError::Timeout {
            script_id: "s1".into(),
            elapsed_ms: 2.5,
            tick: 100,
        };
        let bytes = bincode::serialize(&err).unwrap();
        let truncated = &bytes[..bytes.len() / 2];
        assert!(bincode::deserialize::<ScriptError>(truncated).is_err());
    }

    #[test]
    fn script_error_bincode_empty() {
        assert!(bincode::deserialize::<ScriptError>(&[]).is_err());
    }
}
