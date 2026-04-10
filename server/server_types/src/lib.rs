//! server_types — Server 端共用類型、trait 與錯誤定義
//!
//! ## 模組結構
//! - [`game_event`] — GameEvent（空 enum，待遊戲類型確認後擴充）
//! - [`step_result`] — StepResult、SimulationError
//! - [`hash_result`] — HashCheckResult
//! - [`simulation_trait`] — DeterministicSimulation trait

pub mod game_event;
pub mod hash_result;
pub mod simulation_trait;
pub mod step_result;

pub use game_event::GameEvent;
pub use hash_result::HashCheckResult;
pub use simulation_trait::DeterministicSimulation;
pub use step_result::{SimulationError, StepResult};
