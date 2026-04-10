//! Session replay 錄製與回放（bincode + gzip）。
//! Native only——不需 WASM 相容。

pub mod error;
pub mod playback;
pub mod recorder;

// ── Public API re-exports ──
pub use error::ReplayError;
pub use playback::{FrameStepResult, PlaybackMode, ReplayPlayer, ReplayValidationResult};
pub use recorder::{ReplayFile, ReplayRecorder, REPLAY_FORMAT_VERSION};
// DeterministicSimulation 由 server_types 直接提供，使用者: use server_types::DeterministicSimulation
