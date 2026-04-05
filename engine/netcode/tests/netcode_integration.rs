//! Netcode 整合測試
//!
//! 驗證 prediction + rollback + hash validation + connection 在各種條件下的行為。
//!
//! 使用 `cargo test -p netcode --features integration` 執行。

#![cfg(feature = "integration")]

use std::collections::BTreeMap;

use bridge_types::{DeterministicValue, EcsMirror, EntityId, PlayerInput};
use deterministic::{DeterministicRng, SoftF32};
use netcode::hash_validation::{HashValidationResult, HashValidator};
use netcode::prediction::PredictionManager;
use netcode::rollback::RollbackManager;
use netcode::simulation_step::MockSimulationStep;
use netcode::snapshot::{GameSnapshot, InputBuffer, SnapshotBuffer};
use netcode::{ConnectionManager, ConnectionState, MAX_ROLLBACK_DEPTH};
use state_hash::compute_state_hash;

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

/// ceil(latency_ms / 16.67)
fn latency_to_ticks(latency_ms: u64) -> u64 {
    let tick_us = 16667u64;
    let latency_us = latency_ms * 1000;
    (latency_us + tick_us - 1) / tick_us
}

// ── 基礎常數驗證 ──

#[test]
fn max_rollback_depth_is_8() {
    assert_eq!(MAX_ROLLBACK_DEPTH, 8, "MAX_ROLLBACK_DEPTH 應為 8");
}

#[test]
fn latency_100ms_rollback_le_6_frames() {
    let ticks = latency_to_ticks(100);
    assert!(ticks <= 6, "100ms 延遲 rollback <= 6 幀，實際：{}", ticks);
}

#[test]
fn latency_0ms_rollback_0_frames() {
    assert_eq!(latency_to_ticks(0), 0);
}

#[test]
fn latency_16ms_rollback_1_frame() {
    assert_eq!(latency_to_ticks(16), 1);
}

// ── Prediction Manager ──

#[test]
fn prediction_manager_basic_flow() {
    let mut pm = PredictionManager::new();
    assert_eq!(pm.last_confirmed_tick(), 0, "初始 confirmed tick 應為 0");
    assert!(pm.needs_rollback().is_none(), "初始不應需要 rollback");

    // 模擬收到 server state（hash 一致 → 確認）
    let hash = [0xAAu8; 32];
    pm.receive_server_state(5, hash, hash, 10).unwrap();
    assert_eq!(pm.last_confirmed_tick(), 5, "confirmed tick 應更新");

    // hash 不一致 → 觸發 rollback
    let server_hash = [0xBBu8; 32];
    let local_hash = [0xCCu8; 32];
    pm.receive_server_state(6, server_hash, local_hash, 10)
        .unwrap();
    assert!(pm.needs_rollback().is_some(), "hash 不符應觸發 rollback");
}

// ── Snapshot Buffer ──

#[test]
fn snapshot_buffer_push_and_get() {
    let mut buffer = SnapshotBuffer::new();
    buffer.push(5, make_snapshot(5));
    let retrieved = buffer.get(5);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().tick, 5);
}

#[test]
fn snapshot_buffer_ring_buffer_capacity() {
    let mut buffer = SnapshotBuffer::new();
    let capacity = netcode::SNAPSHOT_CAPACITY;

    for i in 0..capacity as u64 {
        buffer.push(i, make_snapshot(i));
    }
    // 新增一個，最舊的應被覆蓋
    buffer.push(capacity as u64, make_snapshot(capacity as u64));

    assert!(buffer.get(0).is_none(), "tick 0 應已被 ring buffer 覆蓋");
    assert!(buffer.get(capacity as u64).is_some(), "最新快照應存在");
}

// ── Hash Validation ──

#[test]
fn hash_validator_consistent_state() {
    let mut validator = HashValidator::new();
    let entities = BTreeMap::new();
    let rng_state = [0u8; 16];
    let hash = compute_state_hash(&entities, &rng_state, 0);

    let result = validator.validate(hash, hash);
    assert!(matches!(result, HashValidationResult::Match));
}

#[test]
fn hash_validator_detects_mismatch() {
    let mut validator = HashValidator::new();
    let result = validator.validate([0xAAu8; 32], [0xBBu8; 32]);
    assert!(!matches!(result, HashValidationResult::Match));
}

// ── Rollback Manager ──

#[test]
fn rollback_within_max_depth_succeeds() {
    let mut rm = RollbackManager::new();
    let step = MockSimulationStep;

    let mut snapshot_buffer = SnapshotBuffer::new();
    for i in 0..=8 {
        snapshot_buffer.push(i, make_snapshot(i));
    }
    let input_buffer = InputBuffer::new();

    let result = rm.rollback_to(3, 8, &mut snapshot_buffer, &input_buffer, &step);
    assert!(result.is_ok(), "深度 5 的 rollback 應成功");
}

#[test]
fn rollback_exceeding_max_depth_fails() {
    let mut rm = RollbackManager::new();
    let step = MockSimulationStep;
    let mut snapshot_buffer = SnapshotBuffer::new();
    let input_buffer = InputBuffer::new();

    let result = rm.rollback_to(5, 14, &mut snapshot_buffer, &input_buffer, &step);
    assert!(result.is_err(), "depth=9 > MAX_ROLLBACK_DEPTH(8) 應失敗");
}

// ── Connection Manager ──

#[test]
fn connection_manager_initial_state() {
    let cm = ConnectionManager::new();
    assert!(matches!(cm.state(), ConnectionState::Connected));
}

// ── Determinism ──

#[test]
fn deterministic_prediction_with_same_seed() {
    let mut rng_a = DeterministicRng::seed_from_u64(42);
    let mut rng_b = DeterministicRng::seed_from_u64(42);
    let values_a: Vec<u32> = (0..100).map(|_| rng_a.next_u32()).collect();
    let values_b: Vec<u32> = (0..100).map(|_| rng_b.next_u32()).collect();
    assert_eq!(values_a, values_b, "同一 seed 的 RNG 序列應完全一致");
}

#[test]
fn state_hash_determinism() {
    // 同一 entities 兩次 hash 應一致
    let entities = BTreeMap::new();
    let rng_state = [1u8; 16];
    let hash_a = compute_state_hash(&entities, &rng_state, 42);
    let hash_b = compute_state_hash(&entities, &rng_state, 42);
    assert_eq!(hash_a, hash_b, "相同狀態的 state hash 應完全一致");
}
