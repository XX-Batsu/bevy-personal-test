//! bridge_types — 跨子系統共用型別定義
//!
//! 此 crate 是 VM、Bridge、Netcode、Replay 等子系統的型別 single source of truth。
//! 僅依賴 `deterministic` crate，不依賴 Bevy。
//!
//! ## 確定性保證
//! - Handle newtype struct 使用 `#[repr(C)]`（EntityId, EffectHandle, SoundHandle, EntityState）
//! - 含 heap-allocated 欄位的 struct/enum 不標 `#[repr(C)]`，序列化穩定性由 bincode 保證
//! - 集合型別使用 `BTreeMap`（非 HashMap）
//! - 浮點使用 `SoftF32`（game logic）或 `f64`（渲染層標註）
//!
//! ## 模組結構
//! - [`handles`] — EntityId, EffectHandle, SoundHandle, EntityState（`#[repr(C)]` newtype）
//! - [`dynamic_value`] — DynamicValue（渲染層值型別，含 f64）
//! - [`deterministic_value`] — DeterministicValue（game logic 值型別，含 SoftF32 + validated_str()）
//! - [`bridge_event`] — BridgeEvent（VM → Bevy 事件，23 variants）
//! - [`errors`] — BridgeError, ScriptError
//! - [`ecs_mirror`] — EcsMirror, MirroredEntity（BTreeMap，確定性迭代）
//! - [`replay`] — ReplayFrame, PlayerInput, Blake3Hash

pub mod bridge_event;
pub mod deterministic_value;
pub mod dynamic_value;
pub mod ecs_mirror;
pub mod errors;
pub mod handles;
pub mod replay;

pub use bridge_event::*;
pub use deterministic_value::*;
pub use dynamic_value::*;
pub use ecs_mirror::*;
pub use errors::*;
pub use handles::*;
pub use replay::*;
