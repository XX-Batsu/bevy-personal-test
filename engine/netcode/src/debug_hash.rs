//! Debug 用 per-component state hash（§10.5）
//!
//! Feature-gated: 僅在 `debug-mode` feature 啟用時可用。

#[cfg(feature = "debug-mode")]
use std::collections::BTreeMap;

#[cfg(feature = "debug-mode")]
use bridge_types::{Blake3Hash, DeterministicValue, EntityId, MirroredEntity};

#[cfg(feature = "debug-mode")]
use state_hash::compute_state_hash;

/// Per-component debug hash，用於快速定位 desync 來源
#[cfg(feature = "debug-mode")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugStateHash {
    /// 完整 state hash（必須與 compute_state_hash() bit-identical）
    pub total: Blake3Hash,
    /// 所有 entity 位置的 hash
    pub positions_hash: Blake3Hash,
    /// 所有 entity 狀態的 hash
    pub states_hash: Blake3Hash,
    /// RNG state 的 hash
    pub rng_hash: Blake3Hash,
    /// tick 的 hash
    pub tick_hash: Blake3Hash,
    /// 每個 entity 的個別 hash
    pub per_entity_hash: BTreeMap<EntityId, Blake3Hash>,
}

/// Debug hash 比對結果
#[cfg(feature = "debug-mode")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugHashDiff {
    pub positions_mismatch: bool,
    pub states_mismatch: bool,
    pub rng_mismatch: bool,
    pub tick_mismatch: bool,
    pub mismatched_entities: Vec<EntityId>,
}

/// Hash `DeterministicValue` 的各 variant（含 type tag）
///
/// Type tag 為系統契約：0=Int, 1=Float, 2=Bool, 3=Unit, 4=Str
/// 對齊 state_hash crate 的 hash_deterministic_value（該函式為 crate-private）
#[cfg(feature = "debug-mode")]
fn hash_deterministic_value_into(hasher: &mut blake3::Hasher, value: &DeterministicValue) {
    match value {
        DeterministicValue::Int(v) => {
            hasher.update(&[0u8]);
            hasher.update(&v.to_le_bytes());
        }
        DeterministicValue::Float(v) => {
            hasher.update(&[1u8]);
            hasher.update(&v.to_bits().to_le_bytes());
        }
        DeterministicValue::Bool(v) => {
            hasher.update(&[2u8]);
            hasher.update(&[*v as u8]);
        }
        DeterministicValue::Unit => {
            hasher.update(&[3u8]);
        }
        DeterministicValue::Str(v) => {
            hasher.update(&[4u8]);
            hasher.update(&(v.len() as u32).to_le_bytes());
            hasher.update(v.as_bytes());
        }
    }
}

/// 計算 per-component debug hash
///
/// total 必須與 compute_state_hash() bit-identical
#[cfg(feature = "debug-mode")]
pub fn compute_debug_hash(
    entities: &BTreeMap<EntityId, MirroredEntity>,
    rng_state_bytes: &[u8; 16],
    tick: u64,
) -> DebugStateHash {
    // total hash（必須與 compute_state_hash 一致）
    let total = compute_state_hash(entities, rng_state_bytes, tick);

    // tick hash
    let tick_hash = blake3::hash(&tick.to_le_bytes()).into();

    // rng hash
    let rng_hash = blake3::hash(rng_state_bytes).into();

    // positions hash（含 position + rotation + scale）
    let mut pos_hasher = blake3::Hasher::new();
    for (eid, entity) in entities.iter() {
        pos_hasher.update(&eid.0.to_le_bytes());
        pos_hasher.update(&entity.position.x.to_bits().to_le_bytes());
        pos_hasher.update(&entity.position.y.to_bits().to_le_bytes());
        pos_hasher.update(&entity.position.z.to_bits().to_le_bytes());
        pos_hasher.update(&entity.rotation.x.to_bits().to_le_bytes());
        pos_hasher.update(&entity.rotation.y.to_bits().to_le_bytes());
        pos_hasher.update(&entity.rotation.z.to_bits().to_le_bytes());
        pos_hasher.update(&entity.scale.x.to_bits().to_le_bytes());
        pos_hasher.update(&entity.scale.y.to_bits().to_le_bytes());
        pos_hasher.update(&entity.scale.z.to_bits().to_le_bytes());
    }
    let positions_hash: Blake3Hash = pos_hasher.finalize().into();

    // states hash（含 state + hp + max_hp + animation_id + custom fields）
    let mut state_hasher = blake3::Hasher::new();
    for (eid, entity) in entities.iter() {
        state_hasher.update(&eid.0.to_le_bytes());
        state_hasher.update(&entity.state.0.to_le_bytes());
        state_hasher.update(&entity.hp.to_le_bytes());
        state_hasher.update(&entity.max_hp.to_le_bytes());
        // animation_id（tag-based：None→0u8, Some(id)→1u8+u32 LE）
        match entity.animation_id {
            None => {
                state_hasher.update(&[0u8]);
            }
            Some(id) => {
                state_hasher.update(&[1u8]);
                state_hasher.update(&id.to_le_bytes());
            }
        }
        // custom fields（BTreeMap 保證 key 字母序）
        for (key, value) in entity.custom.iter() {
            state_hasher.update(key.as_bytes());
            hash_deterministic_value_into(&mut state_hasher, value);
        }
    }
    let states_hash: Blake3Hash = state_hasher.finalize().into();

    // per-entity hash
    let mut per_entity_hash = BTreeMap::new();
    for (eid, entity) in entities.iter() {
        let mut h = blake3::Hasher::new();
        h.update(&eid.0.to_le_bytes());
        // position
        h.update(&entity.position.x.to_bits().to_le_bytes());
        h.update(&entity.position.y.to_bits().to_le_bytes());
        h.update(&entity.position.z.to_bits().to_le_bytes());
        // rotation
        h.update(&entity.rotation.x.to_bits().to_le_bytes());
        h.update(&entity.rotation.y.to_bits().to_le_bytes());
        h.update(&entity.rotation.z.to_bits().to_le_bytes());
        // scale
        h.update(&entity.scale.x.to_bits().to_le_bytes());
        h.update(&entity.scale.y.to_bits().to_le_bytes());
        h.update(&entity.scale.z.to_bits().to_le_bytes());
        // hp, max_hp, state
        h.update(&entity.hp.to_le_bytes());
        h.update(&entity.max_hp.to_le_bytes());
        h.update(&entity.state.0.to_le_bytes());
        // animation_id
        match entity.animation_id {
            None => {
                h.update(&[0u8]);
            }
            Some(id) => {
                h.update(&[1u8]);
                h.update(&id.to_le_bytes());
            }
        }
        // custom fields（BTreeMap 保證 key 字母序）
        for (key, value) in entity.custom.iter() {
            h.update(key.as_bytes());
            hash_deterministic_value_into(&mut h, value);
        }
        per_entity_hash.insert(*eid, h.finalize().into());
    }

    DebugStateHash {
        total,
        positions_hash,
        states_hash,
        rng_hash,
        tick_hash,
        per_entity_hash,
    }
}

/// 比對兩個 DebugStateHash，回傳差異
///
/// 若 total 相同，回傳 None
#[cfg(feature = "debug-mode")]
pub fn diff_debug_hashes(local: &DebugStateHash, remote: &DebugStateHash) -> Option<DebugHashDiff> {
    if local.total == remote.total {
        return None;
    }

    let mut mismatched_entities = Vec::new();

    // 比對 per-entity
    for (eid, local_hash) in &local.per_entity_hash {
        match remote.per_entity_hash.get(eid) {
            Some(remote_hash) if local_hash != remote_hash => {
                mismatched_entities.push(*eid);
            }
            None => {
                mismatched_entities.push(*eid);
            }
            _ => {}
        }
    }
    // 檢查 remote 有但 local 沒有的 entity
    for eid in remote.per_entity_hash.keys() {
        if !local.per_entity_hash.contains_key(eid) {
            mismatched_entities.push(*eid);
        }
    }

    Some(DebugHashDiff {
        positions_mismatch: local.positions_hash != remote.positions_hash,
        states_mismatch: local.states_hash != remote.states_hash,
        rng_mismatch: local.rng_hash != remote.rng_hash,
        tick_mismatch: local.tick_hash != remote.tick_hash,
        mismatched_entities,
    })
}

/// 將 DebugStateHash 以 hex 格式輸出至 tracing log
#[cfg(feature = "debug-mode")]
pub fn log_debug_hash(debug_hash: &DebugStateHash, tick: u64) {
    tracing::debug!(
        tick = tick,
        total = hex::encode(debug_hash.total),
        positions = hex::encode(debug_hash.positions_hash),
        states = hex::encode(debug_hash.states_hash),
        rng = hex::encode(debug_hash.rng_hash),
        tick_hash = hex::encode(debug_hash.tick_hash),
        entity_count = debug_hash.per_entity_hash.len(),
        "Debug state hash"
    );
}

#[cfg(all(test, feature = "debug-mode"))]
mod tests {
    use super::*;
    use bridge_types::EntityState;
    use deterministic::{SoftF32, SoftVec3};

    fn make_soft_vec3(x: f32, y: f32, z: f32) -> SoftVec3 {
        SoftVec3 {
            x: SoftF32::from_f32(x),
            y: SoftF32::from_f32(y),
            z: SoftF32::from_f32(z),
        }
    }

    fn make_entity() -> MirroredEntity {
        MirroredEntity {
            position: make_soft_vec3(1.0, 2.0, 3.0),
            rotation: make_soft_vec3(0.0, 0.0, 0.0),
            scale: make_soft_vec3(1.0, 1.0, 1.0),
            hp: 100,
            max_hp: 100,
            state: EntityState::IDLE,
            animation_id: None,
            custom: BTreeMap::new(),
        }
    }

    #[test]
    fn debug_hash_total_matches_compute_state_hash() {
        let mut entities = BTreeMap::new();
        entities.insert(EntityId(1), make_entity());
        let rng = [0u8; 16];
        let tick = 42;

        let debug = compute_debug_hash(&entities, &rng, tick);
        let expected = compute_state_hash(&entities, &rng, tick);
        assert_eq!(debug.total, expected);
    }

    #[test]
    fn debug_hash_empty_entities() {
        let entities = BTreeMap::new();
        let rng = [0u8; 16];
        let debug = compute_debug_hash(&entities, &rng, 0);
        let expected = compute_state_hash(&entities, &rng, 0);
        assert_eq!(debug.total, expected);
        assert!(debug.per_entity_hash.is_empty());
    }

    #[test]
    fn diff_same_hashes_returns_none() {
        let entities = BTreeMap::new();
        let rng = [0u8; 16];
        let hash = compute_debug_hash(&entities, &rng, 0);
        assert_eq!(diff_debug_hashes(&hash, &hash), None);
    }

    #[test]
    fn diff_different_tick_detected() {
        let entities = BTreeMap::new();
        let rng = [0u8; 16];
        let h1 = compute_debug_hash(&entities, &rng, 1);
        let h2 = compute_debug_hash(&entities, &rng, 2);
        let diff = diff_debug_hashes(&h1, &h2).unwrap();
        assert!(diff.tick_mismatch);
        assert!(!diff.rng_mismatch);
    }

    #[test]
    fn diff_different_rng_detected() {
        let entities = BTreeMap::new();
        let h1 = compute_debug_hash(&entities, &[0u8; 16], 0);
        let h2 = compute_debug_hash(&entities, &[1u8; 16], 0);
        let diff = diff_debug_hashes(&h1, &h2).unwrap();
        assert!(diff.rng_mismatch);
        assert!(!diff.tick_mismatch);
    }

    #[test]
    fn diff_different_positions_detected() {
        let mut e1 = BTreeMap::new();
        e1.insert(EntityId(1), make_entity());
        let mut e2 = BTreeMap::new();
        let mut entity2 = make_entity();
        entity2.position = make_soft_vec3(9.0, 9.0, 9.0);
        e2.insert(EntityId(1), entity2);

        let rng = [0u8; 16];
        let h1 = compute_debug_hash(&e1, &rng, 0);
        let h2 = compute_debug_hash(&e2, &rng, 0);
        let diff = diff_debug_hashes(&h1, &h2).unwrap();
        assert!(diff.positions_mismatch);
        assert!(diff.mismatched_entities.contains(&EntityId(1)));
    }

    #[test]
    fn per_entity_hash_unique_per_entity() {
        let mut entities = BTreeMap::new();
        entities.insert(EntityId(1), make_entity());
        let mut entity2 = make_entity();
        entity2.hp = 50;
        entities.insert(EntityId(2), entity2);

        let debug = compute_debug_hash(&entities, &[0u8; 16], 0);
        assert_eq!(debug.per_entity_hash.len(), 2);
        assert_ne!(
            debug.per_entity_hash[&EntityId(1)],
            debug.per_entity_hash[&EntityId(2)]
        );
    }

    #[test]
    fn positions_hash_includes_rotation_scale() {
        let rng = [0u8; 16];

        // 基準 entity
        let mut e1 = BTreeMap::new();
        e1.insert(EntityId(1), make_entity());
        let h1 = compute_debug_hash(&e1, &rng, 0);

        // 變更 rotation → positions_hash 必須不同
        let mut e2 = BTreeMap::new();
        let mut entity_rot = make_entity();
        entity_rot.rotation = make_soft_vec3(0.0, 90.0, 0.0);
        e2.insert(EntityId(1), entity_rot);
        let h2 = compute_debug_hash(&e2, &rng, 0);
        assert_ne!(h1.positions_hash, h2.positions_hash);

        // 變更 scale → positions_hash 必須不同
        let mut e3 = BTreeMap::new();
        let mut entity_scale = make_entity();
        entity_scale.scale = make_soft_vec3(2.0, 2.0, 2.0);
        e3.insert(EntityId(1), entity_scale);
        let h3 = compute_debug_hash(&e3, &rng, 0);
        assert_ne!(h1.positions_hash, h3.positions_hash);
    }

    #[test]
    fn states_hash_includes_animation_id() {
        let rng = [0u8; 16];

        // 基準 entity（animation_id = None）
        let mut e1 = BTreeMap::new();
        e1.insert(EntityId(1), make_entity());
        let h1 = compute_debug_hash(&e1, &rng, 0);

        // 變更 animation_id → states_hash 必須不同
        let mut e2 = BTreeMap::new();
        let mut entity_anim = make_entity();
        entity_anim.animation_id = Some(5);
        e2.insert(EntityId(1), entity_anim);
        let h2 = compute_debug_hash(&e2, &rng, 0);
        assert_ne!(h1.states_hash, h2.states_hash);

        // Some(0) vs Some(5) → states_hash 必須不同
        let mut e3 = BTreeMap::new();
        let mut entity_anim0 = make_entity();
        entity_anim0.animation_id = Some(0);
        e3.insert(EntityId(1), entity_anim0);
        let h3 = compute_debug_hash(&e3, &rng, 0);
        assert_ne!(h2.states_hash, h3.states_hash);
    }
}
