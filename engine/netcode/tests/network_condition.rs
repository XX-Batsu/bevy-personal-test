//! Task 20: 網路狀況模擬測試
//!
//! 驗證 netcode 在各種網路條件下的行為：
//! jitter、packet loss、burst loss、多人場景

use std::collections::BTreeMap;

use bridge_types::{EcsMirror, EntityId};
use deterministic::{DeterministicRng, SoftF32};
use netcode::hash_validation::{HashValidationResult, HashValidator};
use netcode::prediction::PredictionManager;
use netcode::rollback::RollbackManager;
use netcode::simulation_step::MockSimulationStep;
use netcode::snapshot::{GameSnapshot, InputBuffer, SnapshotBuffer};
use state_hash::compute_state_hash;

/// 網路條件預設
struct NetworkCondition {
    latency_ms: u64,
    jitter_ms: u64,
}

impl NetworkCondition {
    fn perfect() -> Self {
        Self {
            latency_ms: 0,
            jitter_ms: 0,
        }
    }

    fn jitter_only() -> Self {
        Self {
            latency_ms: 50,
            jitter_ms: 30,
        }
    }

    fn high_latency() -> Self {
        Self {
            latency_ms: 100,
            jitter_ms: 10,
        }
    }
}

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

/// 模擬 latency → tick delay（ceil）
fn latency_to_ticks(latency_ms: u64) -> u64 {
    if latency_ms == 0 {
        0
    } else {
        // ceil(latency / 16.67)
        (latency_ms + 16) / 17
    }
}

// === 測試案例 ===

#[test]
fn perfect_connection_no_rollback() {
    let mut pm = PredictionManager::new();
    let mut sb = SnapshotBuffer::new();
    let mut ib = InputBuffer::new();

    // 模擬 10 tick，server 即時確認（零延遲）
    for tick in 1..=10u64 {
        let snap = make_snapshot(tick);
        pm.apply_input(&[], tick, &mut sb, &mut ib, snap.clone());

        // Server 即時確認（同 tick）
        let hash = compute_state_hash(&snap.ecs_mirror.entities, &snap.rng_state, snap.tick);
        let result = pm.receive_server_state(tick, hash, hash, tick);
        assert!(result.is_ok());
        assert_eq!(pm.needs_rollback(), None);
    }
}

#[test]
fn jitter_causes_delayed_confirmation() {
    let cond = NetworkCondition::jitter_only();
    let delay = latency_to_ticks(cond.latency_ms + cond.jitter_ms); // ceil(80/16.67) ≈ 5

    let mut pm = PredictionManager::new();
    let mut sb = SnapshotBuffer::new();
    let mut ib = InputBuffer::new();

    // 模擬 20 tick
    for tick in 1..=20u64 {
        let snap = make_snapshot(tick);
        pm.apply_input(&[], tick, &mut sb, &mut ib, snap);
    }

    // Server 確認 tick=5（延遲 delay tick 後到達 client）
    let server_snap = make_snapshot(5);
    let hash = compute_state_hash(&server_snap.ecs_mirror.entities, &server_snap.rng_state, 5);
    let current = 5 + delay;
    let result = pm.receive_server_state(5, hash, hash, current);
    assert!(result.is_ok());
    assert_eq!(pm.needs_rollback(), None);
    // 延遲在 MAX_ROLLBACK_DEPTH 以內
    assert!(delay <= 8);
}

#[test]
fn hash_mismatch_triggers_rollback_within_depth() {
    let mut pm = PredictionManager::new();
    let mut sb = SnapshotBuffer::new();
    let mut ib = InputBuffer::new();

    for tick in 1..=10u64 {
        pm.apply_input(&[], tick, &mut sb, &mut ib, make_snapshot(tick));
    }

    // Server 回報 tick=5 的 hash 不同
    let server_hash = [0xFF; 32];
    let local_hash = [0u8; 32];
    let result = pm.receive_server_state(5, server_hash, local_hash, 10);
    assert!(result.is_ok());
    assert_eq!(pm.needs_rollback(), Some(5));
}

#[test]
fn rollback_depth_exceeded_returns_error() {
    let mut pm = PredictionManager::new();
    let mut sb = SnapshotBuffer::new();
    let mut ib = InputBuffer::new();

    pm.apply_input(&[], 1, &mut sb, &mut ib, make_snapshot(1));

    // Server 確認 tick=1，但 client 已在 tick=20（depth=19 > 8）
    let hash = [0u8; 32];
    let result = pm.receive_server_state(1, hash, hash, 20);
    assert!(result.is_err());
}

#[test]
fn hash_validator_report_interval() {
    // 每 4 tick 上報一次
    for tick in 0..=20u64 {
        if tick % 4 == 0 {
            assert!(HashValidator::should_report(tick));
        } else {
            assert!(!HashValidator::should_report(tick));
        }
    }
}

#[test]
fn hash_validator_suspicious_detection() {
    let mut hv = HashValidator::new();
    let local = [0xAA; 32];
    let server = [0xBB; 32];

    // 連續 3 次 mismatch → Suspicious
    assert_eq!(hv.validate(local, server), HashValidationResult::Mismatch);
    assert_eq!(hv.validate(local, server), HashValidationResult::Mismatch);
    assert_eq!(hv.validate(local, server), HashValidationResult::Suspicious);
}

#[test]
fn rollback_resimulation_deterministic() {
    let step = MockSimulationStep;
    let mut rm = RollbackManager::new();
    let mut sb = SnapshotBuffer::new();
    let ib = InputBuffer::new();

    sb.push(5, make_snapshot(5));

    // Rollback 兩次，結果應相同
    let r1 = rm.rollback_to(5, 8, &mut sb, &ib, &step).unwrap();

    sb.push(5, make_snapshot(5));
    let r2 = rm.rollback_to(5, 8, &mut sb, &ib, &step).unwrap();

    assert_eq!(r1, r2);
}

#[test]
fn burst_loss_exceeds_rollback_depth() {
    let mut pm = PredictionManager::new();
    let mut sb = SnapshotBuffer::new();
    let mut ib = InputBuffer::new();

    // 模擬 30 tick（代表 burst loss 期間 client 持續前進）
    for tick in 1..=30u64 {
        pm.apply_input(&[], tick, &mut sb, &mut ib, make_snapshot(tick));
    }

    // Server 確認 tick=1（30 tick 延遲 → depth=29 > 8）
    let hash = [0u8; 32];
    let result = pm.receive_server_state(1, hash, hash, 30);
    assert!(result.is_err()); // RollbackDepthExceeded
}

#[test]
fn burst_loss_within_depth_ok() {
    let mut pm = PredictionManager::new();
    let mut sb = SnapshotBuffer::new();
    let mut ib = InputBuffer::new();

    for tick in 1..=10u64 {
        pm.apply_input(&[], tick, &mut sb, &mut ib, make_snapshot(tick));
    }

    // 6 tick burst（在 depth limit 內）
    let hash = [0u8; 32];
    let result = pm.receive_server_state(4, hash, hash, 10); // depth=6 ≤ 8
    assert!(result.is_ok());
}

#[test]
fn multiplayer_consistent_hash() {
    // 兩個 client 使用相同 snapshot → 產生相同 hash
    let snap = make_snapshot(10);
    let hash1 = compute_state_hash(&snap.ecs_mirror.entities, &snap.rng_state, snap.tick);
    let hash2 = compute_state_hash(&snap.ecs_mirror.entities, &snap.rng_state, snap.tick);
    assert_eq!(hash1, hash2);
}

#[test]
fn rng_reproducibility() {
    // 相同 seed → 相同 state_bytes
    let rng1 = DeterministicRng::seed_from_u64(42);
    let rng2 = DeterministicRng::seed_from_u64(42);
    assert_eq!(rng1.state_bytes(), rng2.state_bytes());

    // 不同 seed → 不同 state_bytes
    let rng3 = DeterministicRng::seed_from_u64(99);
    assert_ne!(rng1.state_bytes(), rng3.state_bytes());
}

#[test]
fn latency_to_ticks_computation() {
    assert_eq!(latency_to_ticks(0), 0);
    assert_eq!(latency_to_ticks(16), 1); // ceil(16/16.67) ≈ 1
    assert_eq!(latency_to_ticks(50), 3); // ceil(50/16.67) ≈ 3
    assert_eq!(latency_to_ticks(100), 6); // ceil(100/16.67) = 6
    assert_eq!(latency_to_ticks(133), 8); // ceil(133/16.67) ≈ 8
}

#[test]
fn network_condition_presets() {
    let perfect = NetworkCondition::perfect();
    assert_eq!(perfect.latency_ms, 0);
    assert_eq!(perfect.jitter_ms, 0);

    let high = NetworkCondition::high_latency();
    let high_delay = latency_to_ticks(high.latency_ms + high.jitter_ms);
    assert!(high_delay <= 8); // 110ms → 7 ticks ≤ MAX_ROLLBACK_DEPTH
}
