//! Session replay 錄製與回放（bincode + gzip）。
//! Native only——不需 WASM 相容。

pub mod error;
pub mod playback;
pub mod recorder;

// ── Public API re-exports ──
pub use error::ReplayError;
pub use playback::{
    DeterministicSimulation, FrameStepResult, PlaybackMode, ReplayPlayer, ReplayValidationResult,
};
pub use recorder::{ReplayFile, ReplayRecorder, REPLAY_FORMAT_VERSION};
