//! 模擬時鐘模組 — 固定 16.67ms (60Hz) 的 tick counter。
//!
//! 遊戲邏輯的所有時間推進皆透過 [`SimulationClock`]。
//! 不使用 `std::time`（確定性規則），不依賴 Bevy `FixedUpdate` schedule（Phase 9 範疇）。
//!
//! # 設計依據
//! - `docs/design/architecture/08-deterministic-runtime/fixed-timestep.md`

use serde::{Deserialize, Serialize};

/// 模擬時鐘 — 固定 16.67ms (60Hz) 的 tick counter。
///
/// 遊戲邏輯的所有時間推進皆透過此結構。
/// 不使用 std::time（確定性規則），不依賴 Bevy FixedUpdate schedule（Phase 9 範疇）。
///
/// # 設計依據
/// - `docs/design/architecture/08-deterministic-runtime/fixed-timestep.md`
/// - Tick 間隔：16.67ms = 16667μs（60Hz）
/// - Tick Counter：`u64`，每局從 0 開始遞增
///
/// # 使用範例
/// ```rust
/// use deterministic::SimulationClock;
///
/// let mut clock = SimulationClock::new();
/// assert_eq!(clock.tick(), 0);
///
/// clock.advance();
/// assert_eq!(clock.tick(), 1);
/// assert_eq!(clock.elapsed_micros(), 16667);
///
/// clock.reset();
/// assert_eq!(clock.tick(), 0);
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[repr(C)]
pub struct SimulationClock {
    /// 當前 tick 數，從 0 開始。
    ///
    /// 每次 `advance()` 遞增 1。
    /// 使用 `u64` 確保不會在合理遊戲時間內溢位
    /// （u64::MAX / 60Hz ≈ 97 億年）。
    pub tick: u64,
}

impl SimulationClock {
    /// 每 tick 間隔（微秒）：16.67ms = 16667μs。
    ///
    /// 設計文件規定邏輯 tick 固定 60Hz：
    /// 1_000_000μs / 60 ≈ 16666.67μs，取整為 16667μs。
    ///
    /// 以常數定義而非硬編碼，便於：
    /// - 測試驗證（`test_tick_interval_micros`）
    /// - 下游模組引用（`SimulationClock::TICK_INTERVAL_MICROS`）
    pub const TICK_INTERVAL_MICROS: u64 = 16667;

    /// 建立新的模擬時鐘，tick 從 0 開始。
    #[must_use]
    pub fn new() -> Self {
        Self { tick: 0 }
    }

    /// 取得當前 tick 數。
    ///
    /// tick 從 0 開始，每次 `advance()` 遞增 1。
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// 推進一個 tick。
    ///
    /// 每次呼叫令 `self.tick = self.tick.saturating_add(1)`。
    /// 此方法對應 Bevy FixedUpdate 中的一次邏輯步進（Phase 9 整合）。
    pub fn advance(&mut self) {
        self.tick = self.tick.saturating_add(1);
    }

    /// 取得每 tick 間隔（微秒）。
    ///
    /// 回傳 `Self::TICK_INTERVAL_MICROS`（16667μs）。
    /// 透過實例方法提供，讓呼叫端不需要知道常數名稱。
    #[must_use]
    pub fn tick_interval_micros(&self) -> u64 {
        Self::TICK_INTERVAL_MICROS
    }

    /// 取得自 tick 0 以來的經過時間（微秒）。
    ///
    /// 計算方式：`self.tick * TICK_INTERVAL_MICROS`。
    ///
    /// 範例對照：
    /// - 1 tick  → 16,667μs  ≈ 16.67ms
    /// - 60 ticks → 1,000,020μs ≈ 1.00002s
    /// - 3600 ticks → 60,001,200μs ≈ 60s（1 分鐘）
    #[must_use]
    pub fn elapsed_micros(&self) -> u64 {
        self.tick * Self::TICK_INTERVAL_MICROS
    }

    /// 重設時鐘至 tick 0。
    ///
    /// 用於：
    /// - 局間重置（新一局開始）
    /// - Rollback 至起始狀態
    /// - Replay 重播前的初始化
    pub fn reset(&mut self) {
        self.tick = 0;
    }

    /// 序列化時鐘狀態為 8 bytes（tick 的 little-endian 表示）。
    ///
    /// 用途：state hash 計算。
    #[must_use]
    pub fn state_bytes(&self) -> [u8; 8] {
        self.tick.to_le_bytes()
    }
}

/// Default 實作委派至 `new()`。
///
/// 確保 `SimulationClock::default()` 與 `SimulationClock::new()` 行為完全一致。
/// 提供 Default 以支援 Bevy 的 `app.init_resource::<SimulationClock>()`。
impl Default for SimulationClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_tick_is_zero() {
        let clock = SimulationClock::new();
        assert_eq!(clock.tick(), 0);
    }

    #[test]
    fn test_advance_increments_tick() {
        let mut clock = SimulationClock::new();
        clock.advance();
        assert_eq!(clock.tick(), 1);
        clock.advance();
        assert_eq!(clock.tick(), 2);
    }

    #[test]
    fn test_tick_interval_micros() {
        let clock = SimulationClock::new();
        assert_eq!(clock.tick_interval_micros(), 16667);
    }

    #[test]
    fn test_elapsed_micros_after_60_ticks() {
        let mut clock = SimulationClock::new();
        for _ in 0..60 {
            clock.advance();
        }
        // 60Hz × 16667μs = 1,000,020μs ≈ 1 秒
        assert_eq!(clock.elapsed_micros(), 60 * 16667);
    }

    #[test]
    fn test_reset() {
        let mut clock = SimulationClock::new();
        for _ in 0..10 {
            clock.advance();
        }
        assert_eq!(clock.tick(), 10); // 前置條件確認
        clock.reset();
        assert_eq!(clock.tick(), 0);
        assert_eq!(clock.elapsed_micros(), 0);
    }

    #[test]
    fn test_default_trait() {
        let clock = SimulationClock::default();
        assert_eq!(clock.tick(), 0);
    }

    #[test]
    fn test_state_bytes() {
        let mut clock = SimulationClock::new();
        assert_eq!(clock.state_bytes(), [0u8; 8]);
        clock.advance();
        assert_eq!(clock.state_bytes(), 1u64.to_le_bytes());
    }
}
