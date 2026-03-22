//! 確定性運算 crate — 提供跨平台 bit-exact 的浮點、向量、RNG、時鐘型別。
//! 此 crate 無 Bevy 依賴，供所有下游子系統使用。
//!
//! # 模組概覽
//!
//! - [`soft_float`] — `SoftF32` 確定性浮點型別（基於 softfloat 1.0 pure Rust）
//! - [`soft_vec`] — `SoftVec2` / `SoftVec3` 確定性向量型別
//! - [`deterministic_rng`] — `DeterministicRng` PCG-XSH-RR 確定性隨機數產生器
//! - [`simulation_clock`] — `SimulationClock` 固定 16.67ms 模擬時鐘
//!
//! # 使用範例
//!
//! ```rust
//! use deterministic::{SoftF32, SoftVec2, DeterministicRng, SimulationClock};
//!
//! let pos = SoftVec2::new(SoftF32::from_f32(1.0), SoftF32::from_f32(2.0));
//! let vel = SoftVec2::new(SoftF32::from_f32(0.5), SoftF32::from_f32(-0.3));
//! let dt = SoftF32::from_f32(1.0 / 60.0);
//! let new_pos = pos + vel.scale(dt);
//! ```

pub mod deterministic_rng;
pub mod simulation_clock;
pub mod soft_float;
pub mod soft_vec;

// ── 便利 re-exports ──
// 下游 crate 可直接 `use deterministic::SoftF32;` 而不需指定子模組。

pub use deterministic_rng::DeterministicRng;
pub use simulation_clock::SimulationClock;
pub use soft_float::SoftF32;
pub use soft_vec::{SoftVec2, SoftVec3};
