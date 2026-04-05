//! 時間來源抽象模組
//!
//! 定義 [`Clock`] trait，提供基礎設施計時（腳本超時偵測、效能診斷）。
//! 遊戲邏輯不使用此 trait（使用 [`SimulationClock`](crate::SimulationClock) tick counter）。
//!
//! # 設計依據
//! - `docs/design/architecture/02-concurrency/parallel-architecture.md`
//! - README.md：「Infrastructure timing must use the Clock trait」

/// 時間來源抽象
///
/// 提供微秒級壁鐘時間，用於基礎設施計時（腳本超時、效能診斷）。
/// 不用於遊戲邏輯（遊戲邏輯使用 SimulationClock tick counter）。
///
/// # 實作
/// - [`NativeClock`]：Native 平台（`std::time::Instant`）
/// - `WasmClock`：WASM 平台（`web_sys::Performance.now()`）
/// - 測試可注入 `MockClock` 控制時間推進
pub trait Clock: Send + Sync {
    /// 回傳當前時間（微秒，µs）
    ///
    /// 單位：微秒（1 ms = 1,000 µs）。
    /// 時間基準為實作自行定義的 epoch（如 Instant 建構時刻）。
    fn now_micros(&self) -> u64;
}

/// Native 平台時間來源（非 WASM）
///
/// 使用 `std::time::Instant` 作為時間來源。
/// 內部保存建構時的 epoch，`now_micros()` 回傳自 epoch 起經過的微秒數。
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeClock {
    epoch: std::time::Instant,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeClock {
    /// 建立 NativeClock，以當前時刻為 epoch
    pub fn new() -> Self {
        Self {
            epoch: std::time::Instant::now(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for NativeClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Clock for NativeClock {
    fn now_micros(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }
}

/// WASM 平台時間來源
///
/// 使用 `js_sys::Date::now()` 作為時間來源（毫秒精度）。
/// 在 main thread 與 Web Worker 環境中均可使用。
/// 建構時記錄 epoch，`now_micros()` 回傳自 epoch 起經過的微秒數。
#[cfg(target_arch = "wasm32")]
pub struct WasmClock {
    epoch_ms: f64,
}

#[cfg(target_arch = "wasm32")]
impl WasmClock {
    /// 建立 WasmClock，以當前 Date.now() 為 epoch
    pub fn new() -> Self {
        Self {
            epoch_ms: js_sys::Date::now(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for WasmClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
impl Clock for WasmClock {
    fn now_micros(&self) -> u64 {
        let elapsed_ms = js_sys::Date::now() - self.epoch_ms;
        // Date.now() 精度為毫秒，轉換為微秒（× 1000）
        (elapsed_ms * 1000.0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_clock_returns_increasing_values() {
        let clock = NativeClock::new();
        let t1 = clock.now_micros();
        // 做一些工作確保時間推進
        let mut sum = 0u64;
        for i in 0..1000 {
            sum = sum.wrapping_add(i);
        }
        let _ = sum;
        let t2 = clock.now_micros();
        assert!(t2 >= t1, "時間應單調遞增");
    }

    #[test]
    fn native_clock_default_trait() {
        let _clock = NativeClock::default();
    }
}
