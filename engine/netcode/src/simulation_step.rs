//! 遊戲邏輯推進介面（SimulationStep trait）與測試用 Mock 實作

use bridge_types::PlayerInput;

use crate::snapshot::GameSnapshot;

/// 遊戲邏輯推進介面，讓 rollback re-simulation 可注入 mock 實作
///
/// 確定性契約：禁止 native float / std::time / HashMap / 系統 RNG / async
/// 相同 state + 相同 inputs → 必須相同結果
pub trait SimulationStep {
    /// 以給定的輸入推進一幀遊戲狀態
    ///
    /// 注意：不遞增 `state.tick`——tick 管理由呼叫端負責
    fn step(&self, state: &mut GameSnapshot, inputs: &[PlayerInput]);
}

/// 測試用 Mock：identity（no-op），驗證 rollback 框架行為
pub struct MockSimulationStep;

impl SimulationStep for MockSimulationStep {
    fn step(&self, _state: &mut GameSnapshot, _inputs: &[PlayerInput]) {}
}

/// 測試用 Mock：累加計數器，驗證 re-simulation 次數
pub struct CountingSimulationStep {
    pub call_count: std::cell::Cell<u64>,
}

impl CountingSimulationStep {
    pub fn new() -> Self {
        Self {
            call_count: std::cell::Cell::new(0),
        }
    }

    pub fn count(&self) -> u64 {
        self.call_count.get()
    }
}

impl Default for CountingSimulationStep {
    fn default() -> Self {
        Self::new()
    }
}

impl SimulationStep for CountingSimulationStep {
    fn step(&self, _state: &mut GameSnapshot, _inputs: &[PlayerInput]) {
        self.call_count.set(self.call_count.get() + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, EcsMirror, EntityId};
    use deterministic::SoftF32;
    use std::collections::BTreeMap;

    fn make_snapshot(tick: u64) -> GameSnapshot {
        GameSnapshot {
            tick,
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: tick,
                delta_time: SoftF32::from_f32(0.01667),
            },
            rng_state: [0u8; 16],
        }
    }

    #[test]
    fn mock_simulation_step_is_identity() {
        let mut state = make_snapshot(10);
        let original = state.clone();
        MockSimulationStep.step(&mut state, &[]);
        assert_eq!(state, original);
    }

    #[test]
    fn mock_simulation_step_with_inputs() {
        let mut state = make_snapshot(5);
        let original = state.clone();
        let input = PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: DeterministicValue::Unit,
            tick: 5,
        };
        MockSimulationStep.step(&mut state, &[input]);
        assert_eq!(state, original);
    }

    #[test]
    fn counting_step_increments_call_count() {
        let counter = CountingSimulationStep::new();
        let mut state = make_snapshot(1);
        for _ in 0..3 {
            counter.step(&mut state, &[]);
        }
        assert_eq!(counter.count(), 3);
    }

    #[test]
    fn counting_step_zero_calls() {
        assert_eq!(CountingSimulationStep::new().count(), 0);
    }

    #[test]
    fn counting_step_does_not_modify_tick() {
        let counter = CountingSimulationStep::new();
        let mut state = make_snapshot(42);
        counter.step(&mut state, &[]);
        assert_eq!(state.tick, 42);
    }

    #[test]
    fn counting_step_does_not_modify_rng_state() {
        let counter = CountingSimulationStep::new();
        let rng = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let mut state = GameSnapshot {
            tick: 1,
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 1,
                delta_time: SoftF32::from_f32(0.01667),
            },
            rng_state: rng,
        };
        counter.step(&mut state, &[]);
        assert_eq!(state.rng_state, rng);
    }
}
