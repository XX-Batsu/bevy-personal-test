//! Property-based 測試 — proptest 驗證 state hash 不變式（256 cases each）
//!
//! 使用 `cargo test -p state_hash --features integration` 執行。

#![cfg(feature = "integration")]

use std::collections::BTreeMap;

use bridge_types::{EntityId, EntityState, MirroredEntity};
use deterministic::{SoftF32, SoftVec3};
use proptest::prelude::*;
use state_hash::compute_state_hash;

fn make_entity(x: f32, y: f32, hp: i64) -> MirroredEntity {
    MirroredEntity {
        position: SoftVec3::new(
            SoftF32::from_f32(x),
            SoftF32::from_f32(y),
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

fn arb_entity_id() -> impl Strategy<Value = EntityId> {
    (0u64..1000).prop_map(EntityId)
}

fn arb_mirrored_entity() -> impl Strategy<Value = MirroredEntity> {
    (any::<f32>(), any::<f32>(), 0i64..1000).prop_map(|(x, y, hp)| make_entity(x, y, hp))
}

fn arb_entities() -> impl Strategy<Value = BTreeMap<EntityId, MirroredEntity>> {
    prop::collection::btree_map(arb_entity_id(), arb_mirrored_entity(), 0..10)
}

fn arb_rng_state() -> impl Strategy<Value = [u8; 16]> {
    prop::array::uniform16(any::<u8>())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Property 1: hash 穩定性
    #[test]
    fn hash_stability(
        entities in arb_entities(),
        rng_state in arb_rng_state(),
        tick in any::<u64>(),
    ) {
        let hash_a = compute_state_hash(&entities, &rng_state, tick);
        let hash_b = compute_state_hash(&entities, &rng_state, tick);
        prop_assert_eq!(hash_a, hash_b, "同一輸入的 hash 不一致");
    }

    /// Property 2: 不同 tick 產生不同 hash
    #[test]
    fn different_ticks_different_hashes(
        entities in arb_entities(),
        rng_state in arb_rng_state(),
        tick_a in 0u64..u64::MAX / 2,
    ) {
        let tick_b = tick_a + 1;
        let hash_a = compute_state_hash(&entities, &rng_state, tick_a);
        let hash_b = compute_state_hash(&entities, &rng_state, tick_b);
        prop_assert_ne!(hash_a, hash_b, "不同 tick 的 hash 不應相同");
    }

    /// Property 3: hash 長度固定 32 bytes
    #[test]
    fn hash_length_32_bytes(
        entities in arb_entities(),
        rng_state in arb_rng_state(),
        tick in any::<u64>(),
    ) {
        let hash = compute_state_hash(&entities, &rng_state, tick);
        prop_assert_eq!(hash.len(), 32);
    }

    /// Property 4: 空 entities hash 確定性
    #[test]
    fn empty_entities_deterministic(
        rng_state in arb_rng_state(),
        tick in any::<u64>(),
    ) {
        let entities = BTreeMap::new();
        let hash_a = compute_state_hash(&entities, &rng_state, tick);
        let hash_b = compute_state_hash(&entities, &rng_state, tick);
        prop_assert_eq!(hash_a, hash_b);
    }

    /// Property 5: BTreeMap 排序保證 hash 穩定
    #[test]
    fn btree_ordering_guarantees_hash_stability(
        id_a in 1u64..100,
        id_b in 101u64..200,
    ) {
        let entity = make_entity(1.0, 2.0, 100);

        let mut map_ab = BTreeMap::new();
        map_ab.insert(EntityId(id_a), entity.clone());
        map_ab.insert(EntityId(id_b), entity.clone());

        let mut map_ba = BTreeMap::new();
        map_ba.insert(EntityId(id_b), entity.clone());
        map_ba.insert(EntityId(id_a), entity);

        let rng_state = [0u8; 16];
        let hash_ab = compute_state_hash(&map_ab, &rng_state, 0);
        let hash_ba = compute_state_hash(&map_ba, &rng_state, 0);
        prop_assert_eq!(hash_ab, hash_ba);
    }
}
