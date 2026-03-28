//! Client 端 netcode：prediction、rollback、state hash 驗證、斷線重連。
//! WASM 相容——禁止 std::thread、std::fs、std::time::Instant。

pub mod connection;
pub mod debug_hash;
pub mod error;
pub mod hash_validation;
pub mod prediction;
pub mod rollback;
pub mod simulation_step;
pub mod snapshot;

// ── Public API re-exports ──
pub use connection::{
    ConnectionManager, ConnectionState, FullSyncPacket, ReconnectAction, FROZEN_TIMEOUT_TICKS,
    MAX_RESYNC_RETRIES, RETRY_INTERVAL_TICKS,
};
pub use error::NetcodeError;
pub use hash_validation::{
    HashValidationResult, HashValidator, HASH_REPORT_INTERVAL, SUSPICIOUS_THRESHOLD,
};
pub use prediction::{PredictionManager, MAX_ROLLBACK_DEPTH};
pub use rollback::{NetcodeState, RollbackManager};
pub use simulation_step::{CountingSimulationStep, MockSimulationStep, SimulationStep};
pub use snapshot::{GameSnapshot, InputBuffer, SnapshotBuffer, SNAPSHOT_CAPACITY};

#[cfg(feature = "debug-mode")]
pub use debug_hash::{compute_debug_hash, diff_debug_hashes, DebugHashDiff, DebugStateHash};
