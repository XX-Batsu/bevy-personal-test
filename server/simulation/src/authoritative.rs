//! Server 端權威模擬骨架

use std::collections::BTreeMap;

use bridge_types::{EntityId, PlayerInput};
use deterministic::DeterministicRng;
use netcode::snapshot::GameSnapshot;

/// Server 端權威模擬
///
/// Phase 12 將填充完整遊戲邏輯。
/// 此骨架僅定義介面與基本測試。
pub struct AuthoritativeSimulation {
    /// 當前遊戲狀態
    pub state: GameSnapshot,
    /// 確定性 RNG
    pub rng: DeterministicRng,
    /// 當前 tick
    pub current_tick: u64,
}

impl AuthoritativeSimulation {
    /// 建立新的權威模擬
    pub fn new(initial_state: GameSnapshot, seed: u64) -> Self {
        Self {
            state: initial_state,
            rng: DeterministicRng::seed_from_u64(seed),
            current_tick: 0,
        }
    }

    /// 推進一幀（接收所有玩家輸入）
    pub fn advance(&mut self, _inputs: &BTreeMap<EntityId, Vec<PlayerInput>>) {
        // Phase 12 填充實際遊戲邏輯
        self.current_tick += 1;
        self.state.tick = self.current_tick;
    }

    /// 取得當前 tick
    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// 取得當前狀態的不可變引用
    pub fn state(&self) -> &GameSnapshot {
        &self.state
    }

    /// 取得 RNG 狀態
    pub fn rng_state_bytes(&self) -> [u8; 16] {
        self.rng.state_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::EcsMirror;
    use deterministic::SoftF32;

    fn make_initial_state() -> GameSnapshot {
        GameSnapshot {
            tick: 0,
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 0,
                delta_time: SoftF32::from_f32(0.01667),
            },
            rng_state: [0u8; 16],
        }
    }

    #[test]
    fn new_simulation_starts_at_tick_zero() {
        let sim = AuthoritativeSimulation::new(make_initial_state(), 42);
        assert_eq!(sim.current_tick(), 0);
    }

    #[test]
    fn advance_increments_tick() {
        let mut sim = AuthoritativeSimulation::new(make_initial_state(), 42);
        sim.advance(&BTreeMap::new());
        assert_eq!(sim.current_tick(), 1);
        sim.advance(&BTreeMap::new());
        assert_eq!(sim.current_tick(), 2);
    }

    #[test]
    fn rng_state_bytes_returns_16_bytes() {
        let sim = AuthoritativeSimulation::new(make_initial_state(), 42);
        let bytes = sim.rng_state_bytes();
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn state_returns_current_snapshot() {
        let mut sim = AuthoritativeSimulation::new(make_initial_state(), 42);
        sim.advance(&BTreeMap::new());
        assert_eq!(sim.state().tick, 1);
    }
}
