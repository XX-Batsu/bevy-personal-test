//! Shadow VM 統一錯誤型別
//!
//! `ShadowVmError` 覆蓋所有 shadow_vm crate 內部模組的失敗路徑。
//! Worker 端的錯誤透過 `ShadowStatus::Error(String)` 轉為字串後嵌入 `ShadowResponse` 回傳；
//! 主線程端的錯誤直接在本地處理。

/// Shadow VM 統一錯誤型別（10 variants）
#[derive(Debug)]
pub enum ShadowVmError {
    /// bytecode 格式錯誤或反序列化失敗
    BytecodeLoadFailed(String),
    /// Rhai Engine 初始化失敗（如記憶體不足）
    EngineInitFailed(String),
    /// rng_state 長度不為 16 bytes
    RngStateInvalid {
        expected_len: usize,
        actual_len: usize,
    },
    /// 重播超時（超過效能預算）
    ReplayTimeout { tick: u64, elapsed_ms: u64 },
    /// Rhai script 執行期錯誤
    ScriptError { tick: u64, detail: String },
    /// bincode 反序列化失敗
    DeserializationFailed(String),
    /// bincode 序列化失敗
    SerializationFailed(String),
    /// postMessage 或 Worker 通訊失敗（Worker 未初始化、訊息無法傳遞）
    CommunicationFailed(String),
    /// 反壓佇列已滿（pending_count ≥ MAX_PENDING_REQUESTS）
    QueueFull,
    /// Worker 重啟次數超過速率限制（MAX_RESTARTS_PER_WINDOW）
    TooManyRestarts,
}

impl std::fmt::Display for ShadowVmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BytecodeLoadFailed(msg) => write!(f, "Bytecode 載入失敗：{msg}"),
            Self::EngineInitFailed(msg) => write!(f, "Rhai Engine 初始化失敗：{msg}"),
            Self::RngStateInvalid {
                expected_len,
                actual_len,
            } => write!(
                f,
                "RNG state 長度無效：預期 {expected_len} bytes，實際 {actual_len} bytes"
            ),
            Self::ReplayTimeout { tick, elapsed_ms } => {
                write!(f, "Tick {tick} 重播超時：{elapsed_ms}ms")
            }
            Self::ScriptError { tick, detail } => {
                write!(f, "Tick {tick} Script 執行錯誤：{detail}")
            }
            Self::DeserializationFailed(msg) => write!(f, "反序列化失敗：{msg}"),
            Self::SerializationFailed(msg) => write!(f, "序列化失敗：{msg}"),
            Self::CommunicationFailed(msg) => write!(f, "通訊失敗：{msg}"),
            Self::QueueFull => write!(f, "Shadow VM 反壓佇列已滿"),
            Self::TooManyRestarts => write!(f, "Worker 重啟次數超過速率限制"),
        }
    }
}

impl std::error::Error for ShadowVmError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytecode_load_failed_display() {
        let err = ShadowVmError::BytecodeLoadFailed("無效格式".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Bytecode 載入失敗"));
        assert!(msg.contains("無效格式"));
    }

    #[test]
    fn engine_init_failed_display() {
        let err = ShadowVmError::EngineInitFailed("記憶體不足".to_string());
        let msg = err.to_string();
        assert!(msg.contains("初始化失敗"));
        assert!(msg.contains("記憶體不足"));
    }

    #[test]
    fn rng_state_invalid_display() {
        let err = ShadowVmError::RngStateInvalid {
            expected_len: 16,
            actual_len: 8,
        };
        let msg = err.to_string();
        assert!(msg.contains("16"));
        assert!(msg.contains("8"));
    }

    #[test]
    fn replay_timeout_display() {
        let err = ShadowVmError::ReplayTimeout {
            tick: 100,
            elapsed_ms: 25,
        };
        let msg = err.to_string();
        assert!(msg.contains("100"));
        assert!(msg.contains("25"));
    }

    #[test]
    fn script_error_display() {
        let err = ShadowVmError::ScriptError {
            tick: 42,
            detail: "除以零".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("42"));
        assert!(msg.contains("除以零"));
    }

    #[test]
    fn deserialization_failed_display() {
        let err = ShadowVmError::DeserializationFailed("invalid tag".into());
        assert!(err.to_string().contains("反序列化失敗"));
    }

    #[test]
    fn serialization_failed_display() {
        let err = ShadowVmError::SerializationFailed("buffer overflow".into());
        assert!(err.to_string().contains("序列化失敗"));
    }

    #[test]
    fn communication_failed_display() {
        let err = ShadowVmError::CommunicationFailed("postMessage 失敗".into());
        assert!(err.to_string().contains("通訊失敗"));
        assert!(err.to_string().contains("postMessage 失敗"));
    }

    #[test]
    fn queue_full_display() {
        let err = ShadowVmError::QueueFull;
        assert!(err.to_string().contains("反壓佇列已滿"));
    }

    #[test]
    fn too_many_restarts_display() {
        let err = ShadowVmError::TooManyRestarts;
        assert!(err.to_string().contains("重啟次數超過速率限制"));
    }

    #[test]
    fn all_variants_constructable() {
        let _e1 = ShadowVmError::BytecodeLoadFailed("x".into());
        let _e2 = ShadowVmError::EngineInitFailed("x".into());
        let _e3 = ShadowVmError::RngStateInvalid {
            expected_len: 16,
            actual_len: 0,
        };
        let _e4 = ShadowVmError::ReplayTimeout {
            tick: 0,
            elapsed_ms: 0,
        };
        let _e5 = ShadowVmError::ScriptError {
            tick: 0,
            detail: "x".into(),
        };
        let _e6 = ShadowVmError::DeserializationFailed("x".into());
        let _e7 = ShadowVmError::SerializationFailed("x".into());
        let _e8 = ShadowVmError::CommunicationFailed("x".into());
        let _e9 = ShadowVmError::QueueFull;
        let _e10 = ShadowVmError::TooManyRestarts;
    }

    #[test]
    fn error_trait_object_conversion() {
        let err = ShadowVmError::BytecodeLoadFailed("test".into());
        // 驗證可轉型為 Box<dyn Error>
        let boxed: Box<dyn std::error::Error> = Box::new(err);
        // 驗證 downcast_ref 可還原原始型別
        assert!(boxed.downcast_ref::<ShadowVmError>().is_some());
    }
}
