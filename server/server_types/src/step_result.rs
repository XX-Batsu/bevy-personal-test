//! 模擬步進結果與錯誤類型

use crate::game_event::GameEvent;
use bridge_types::Blake3Hash;

/// 一幀步進結果
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    /// 推進後的 tick 號
    pub tick: u64,
    /// 推進後的 state hash
    pub state_hash: Blake3Hash,
    /// 本幀產生的遊戲事件
    /// TODO(game-logic): 遊戲邏輯實作後此 Vec 才會有內容
    pub events: Vec<GameEvent>,
}

/// 模擬步進錯誤
#[derive(Debug, Clone, PartialEq)]
pub enum SimulationError {
    /// 推進的 tick 號與內部預期不符
    TickMismatch { expected: u64, got: u64 },
    /// RNG 狀態 bytes 長度或格式錯誤
    InvalidRngState,
}

impl std::fmt::Display for SimulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimulationError::TickMismatch { expected, got } => {
                write!(f, "tick 不符：預期 {expected}，實際 {got}")
            }
            SimulationError::InvalidRngState => {
                write!(f, "RNG 狀態還原失敗：bytes 格式錯誤")
            }
        }
    }
}

impl std::error::Error for SimulationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_mismatch_display() {
        let e = SimulationError::TickMismatch {
            expected: 5,
            got: 7,
        };
        assert_eq!(e.to_string(), "tick 不符：預期 5，實際 7");
    }

    #[test]
    fn invalid_rng_state_display() {
        let e = SimulationError::InvalidRngState;
        assert_eq!(e.to_string(), "RNG 狀態還原失敗：bytes 格式錯誤");
    }

    /// 驗證 StepResult 與 GameEvent 的 Clone 與 PartialEq 實作可正常使用，
    /// 防止 GameEvent 遺漏 derive 時的潛在編譯錯誤。
    #[test]
    fn step_result_clone_and_eq() {
        let r = StepResult {
            tick: 1,
            state_hash: [0xAA; 32],
            events: vec![],
        };
        let r2 = r.clone();
        assert_eq!(r, r2);
    }
}
