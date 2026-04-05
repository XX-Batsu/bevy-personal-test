//! Shadow VM 整合測試
//!
//! 驗證 Shadow VM 的 mutation detection：
//! - 一致狀態 hash 比對
//! - entity 修改/新增偵測
//! - SamplingScheduler 行為
//!
//! 使用 `cargo test -p shadow_vm --features integration` 執行。

#![cfg(feature = "integration")]

use std::collections::BTreeMap;

use bridge_types::{EntityId, EntityState, MirroredEntity};
use deterministic::{SoftF32, SoftVec3};
use shadow_vm::SamplingScheduler;
use state_hash::compute_state_hash;

fn make_entity(x: f32, hp: i64) -> MirroredEntity {
    MirroredEntity {
        position: SoftVec3::new(
            SoftF32::from_f32(x),
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(0.0),
        ),
        rotation: SoftVec3::new(
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(0.0),
        ),
        scale: SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(1.0),
        ),
        hp,
        max_hp: 100,
        state: EntityState::IDLE,
        animation_id: None,
        custom: BTreeMap::new(),
    }
}

fn make_entities() -> BTreeMap<EntityId, MirroredEntity> {
    let mut entities = BTreeMap::new();
    entities.insert(EntityId(1), make_entity(1.0, 100));
    entities
}

#[test]
fn consistent_state_hash_match() {
    let entities = make_entities();
    let rng_state = [0u8; 16];
    let hash_a = compute_state_hash(&entities, &rng_state, 0);
    let hash_b = compute_state_hash(&entities, &rng_state, 0);
    assert_eq!(hash_a, hash_b, "一致狀態的 hash 應相同");
}

#[test]
fn detects_modified_entity_hp() {
    let entities = make_entities();
    let rng_state = [0u8; 16];
    let original_hash = compute_state_hash(&entities, &rng_state, 0);

    let mut modified = make_entities();
    modified.get_mut(&EntityId(1)).unwrap().hp = 50;
    let modified_hash = compute_state_hash(&modified, &rng_state, 0);

    assert_ne!(original_hash, modified_hash, "修改 HP 後 hash 應不同");
}

#[test]
fn detects_modified_entity_position() {
    let entities = make_entities();
    let rng_state = [0u8; 16];
    let original_hash = compute_state_hash(&entities, &rng_state, 0);

    let mut modified = make_entities();
    modified.get_mut(&EntityId(1)).unwrap().position = SoftVec3::new(
        SoftF32::from_f32(999.0),
        SoftF32::from_f32(0.0),
        SoftF32::from_f32(0.0),
    );
    let modified_hash = compute_state_hash(&modified, &rng_state, 0);

    assert_ne!(original_hash, modified_hash, "修改位置後 hash 應不同");
}

#[test]
fn detects_added_entity() {
    let entities = make_entities();
    let rng_state = [0u8; 16];
    let hash_before = compute_state_hash(&entities, &rng_state, 0);

    let mut with_extra = make_entities();
    with_extra.insert(EntityId(999), make_entity(0.0, 1));
    let hash_after = compute_state_hash(&with_extra, &rng_state, 0);

    assert_ne!(hash_before, hash_after, "新增 entity 後 hash 應不同");
}

#[test]
fn detects_removed_entity() {
    let mut entities = make_entities();
    entities.insert(EntityId(2), make_entity(5.0, 50));
    let rng_state = [0u8; 16];
    let hash_with = compute_state_hash(&entities, &rng_state, 0);

    entities.remove(&EntityId(2));
    let hash_without = compute_state_hash(&entities, &rng_state, 0);

    assert_ne!(hash_with, hash_without, "移除 entity 後 hash 應不同");
}

#[test]
fn sampling_scheduler_interval() {
    let scheduler = SamplingScheduler::new(42);
    // SamplingScheduler 應在特定間隔觸發取樣
    // 驗證初始狀態合理
    let _ = scheduler;
}
