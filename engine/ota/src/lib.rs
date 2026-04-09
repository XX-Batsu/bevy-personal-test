//! OTA 更新管理 — 統一 asset + script 差異計算與下載排程。

pub mod error;
pub mod ota_manager;
pub mod priority;

pub use error::OtaError;
pub use ota_manager::OtaManager;
pub use priority::{OtaItem, Priority};
