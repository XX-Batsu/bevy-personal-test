//! Phase 11 確定性 property-based tests
//!
//! 使用 proptest 驗證 netcode 的確定性契約：
//! 相同 seed + 相同 input → 相同 state hash

use proptest::prelude::*;

use bridge_types::{Blake3Hash, DeterministicValue, EcsMirror, EntityId, PlayerInput};
use deterministic::{DeterministicRng, SoftF32};
use netcode::simulation_step::{MockSimulationStep, SimulationStep};
use netcode::snapshot::GameSnapshot;
use state_hash::compute_state_hash;
use std::collections::BTreeMap;

/// 構造 EcsMirror（無 Default impl，需手動）
fn empty_ecs_mirror() -> EcsMirror {
    EcsMirror {
        entities: BTreeMap::new(),
        local_player_id: EntityId(0),
        frame_number: 0,
        delta_time: SoftF32::from_f32(0.01667),
    }
}

/// 隨機 PlayerInput 策略
fn arb_player_input() -> impl Strategy<Value = PlayerInput> {
    (any::<u64>(), any::<i64>(), any::<i64>(), any::<u64>()).prop_map(
        |(id_val, input_type, data_val, tick)| PlayerInput {
            player_id: EntityId(id_val),
            input_type,
            data: DeterministicValue::Int(data_val),
            tick,
        },
    )
}

/// 隨機 input 序列策略（1-500 幀，每幀 0-4 個輸入）
fn arb_input_sequence() -> impl Strategy<Value = Vec<Vec<PlayerInput>>> {
    prop::collection::vec(prop::collection::vec(arb_player_input(), 0..4), 1..=500)
}

/// 確定性模擬：以固定 seed 初始化，逐幀 step，回傳最終 state hash
fn replay_and_hash<S: SimulationStep>(
    seed: u64,
    inputs: &[Vec<PlayerInput>],
    step_fn: &S,
) -> Blake3Hash {
    let rng = DeterministicRng::seed_from_u64(seed);
    let mut state = GameSnapshot {
        tick: 0,
        ecs_mirror: empty_ecs_mirror(),
        rng_state: rng.state_bytes(),
    };

    for (i, frame_inputs) in inputs.iter().enumerate() {
        let tick = (i as u64) + 1;
        step_fn.step(&mut state, frame_inputs);
        state.tick = tick;
    }

    compute_state_hash(&state.ecs_mirror.entities, &state.rng_state, state.tick)
}

/// Rollback 模擬：replay 至中間 tick，存快照，繼續至末尾；
/// 再從快照 re-simulate 至末尾，回傳最終 hash
fn replay_with_rollback<S: SimulationStep>(
    seed: u64,
    inputs: &[Vec<PlayerInput>],
    step_fn: &S,
) -> Blake3Hash {
    let rng = DeterministicRng::seed_from_u64(seed);
    let mut state = GameSnapshot {
        tick: 0,
        ecs_mirror: empty_ecs_mirror(),
        rng_state: rng.state_bytes(),
    };

    let midpoint = inputs.len() / 2;

    // Phase 1: replay 至 midpoint
    for (i, frame_inputs) in inputs[..midpoint].iter().enumerate() {
        let tick = (i as u64) + 1;
        step_fn.step(&mut state, frame_inputs);
        state.tick = tick;
    }
    let saved_snapshot = state.clone();

    // Phase 2: 繼續至末尾
    for (i, frame_inputs) in inputs[midpoint..].iter().enumerate() {
        let tick = (midpoint as u64) + (i as u64) + 1;
        step_fn.step(&mut state, frame_inputs);
        state.tick = tick;
    }

    // Phase 3: 從快照 rollback re-simulate
    let mut rollback_state = saved_snapshot;
    for (i, frame_inputs) in inputs[midpoint..].iter().enumerate() {
        let tick = (midpoint as u64) + (i as u64) + 1;
        step_fn.step(&mut rollback_state, frame_inputs);
        rollback_state.tick = tick;
    }

    compute_state_hash(
        &rollback_state.ecs_mirror.entities,
        &rollback_state.rng_state,
        rollback_state.tick,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 同 seed 同 input → 同 hash（基本確定性）
    #[test]
    fn same_seed_same_input_same_hash(
        seed in any::<u64>(),
        inputs in arb_input_sequence(),
    ) {
        let step = MockSimulationStep;
        let hash_a = replay_and_hash(seed, &inputs, &step);
        let hash_b = replay_and_hash(seed, &inputs, &step);
        prop_assert_eq!(hash_a, hash_b);
    }

    /// 不同 seed → 不同 hash
    #[test]
    fn different_seed_different_hash(
        seed_a in any::<u64>(),
        seed_b in any::<u64>(),
        inputs in arb_input_sequence(),
    ) {
        prop_assume!(seed_a != seed_b);
        let step = MockSimulationStep;
        let hash_a = replay_and_hash(seed_a, &inputs, &step);
        let hash_b = replay_and_hash(seed_b, &inputs, &step);
        prop_assert_ne!(hash_a, hash_b);
    }

    /// rollback re-simulation 與直接 replay 產生相同 hash
    #[test]
    fn rollback_determinism(
        seed in any::<u64>(),
        inputs in arb_input_sequence(),
    ) {
        let step = MockSimulationStep;
        let direct_hash = replay_and_hash(seed, &inputs, &step);
        let rollback_hash = replay_with_rollback(seed, &inputs, &step);
        prop_assert_eq!(direct_hash, rollback_hash);
    }

    /// 空 input 序列產生一致 hash
    #[test]
    fn empty_inputs_deterministic(seed in any::<u64>()) {
        let step = MockSimulationStep;
        let inputs: Vec<Vec<PlayerInput>> = vec![vec![]];
        let hash_a = replay_and_hash(seed, &inputs, &step);
        let hash_b = replay_and_hash(seed, &inputs, &step);
        prop_assert_eq!(hash_a, hash_b);
    }

    /// input.tick 欄位不影響確定性（MockSimulationStep 不讀取 tick）
    #[test]
    fn input_tick_value_does_not_affect_determinism(
        seed in any::<u64>(),
        inputs in arb_input_sequence(),
    ) {
        let step = MockSimulationStep;
        let hash_a = replay_and_hash(seed, &inputs, &step);
        // 將所有 input.tick 歸零後重新 replay
        let zeroed: Vec<Vec<PlayerInput>> = inputs
            .iter()
            .map(|frame| {
                frame
                    .iter()
                    .map(|inp| PlayerInput {
                        player_id: inp.player_id,
                        input_type: inp.input_type,
                        data: inp.data.clone(),
                        tick: 0,
                    })
                    .collect()
            })
            .collect();
        let hash_b = replay_and_hash(seed, &zeroed, &step);
        prop_assert_eq!(hash_a, hash_b);
    }
}

#[cfg(feature = "deterministic-replay")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Feature-gated: 完整確定性 replay 驗證（CI 專用）
    #[test]
    fn deterministic_replay_full(
        seed in any::<u64>(),
        inputs in arb_input_sequence(),
    ) {
        let step = MockSimulationStep;
        let hash_a = replay_and_hash(seed, &inputs, &step);
        let hash_b = replay_and_hash(seed, &inputs, &step);
        let hash_rollback = replay_with_rollback(seed, &inputs, &step);
        prop_assert_eq!(hash_a, hash_b);
        prop_assert_eq!(hash_a, hash_rollback);
    }
}
