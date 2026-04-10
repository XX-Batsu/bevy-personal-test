//! 確定性模擬介面（從 replay_engine 遷移至此）

use std::collections::BTreeMap;

use bridge_types::{Blake3Hash, EntityId, PlayerInput};

/// 確定性模擬介面。
///
/// 此 trait 定義模擬引擎的最小介面：
/// - 還原 RNG 狀態（用於 replay 逐幀驗證）
/// - 推進一幀
/// - 計算 state hash
///
/// `replay_engine` 與 `simulation` 均依賴此 trait，透過
/// `server_types` 共享，避免循環依賴。
pub trait DeterministicSimulation {
    type Error: std::fmt::Display;

    /// 還原 RNG 狀態（從 &[u8; 16] 反序列化）
    fn restore_rng(&mut self, rng_state: &[u8; 16]);

    /// 以給定輸入推進一幀
    /// inputs: 每位玩家本幀的輸入序列（Vec 保留輸入順序）
    fn step(&mut self, inputs: &BTreeMap<EntityId, Vec<PlayerInput>>) -> Result<(), Self::Error>;

    /// 計算當前 state hash
    fn compute_state_hash(&self) -> Blake3Hash;
}
