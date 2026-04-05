//! 測試基礎設施：網路條件模擬工具

pub mod conditioner;

pub use conditioner::{BurstLossSimulator, ConditionedPacket, NetworkCondition};
