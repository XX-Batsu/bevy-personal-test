use blake3::Hasher;
use bridge_types::{Blake3Hash, DeterministicValue, EntityId, MirroredEntity};
use std::collections::BTreeMap;

/// Hash 單一 entity 的所有欄位至 hasher
///
/// 欄位 hash 順序（固定，與 design doc §10.6 一致）：
///   1. EntityId          — eid.0 (u64 LE)
///   2. Position          — SoftVec3 (3 × u32 LE via to_bits())
///   3. Rotation          — SoftVec3 (3 × u32 LE via to_bits())
///   4. Scale             — SoftVec3 (3 × u32 LE via to_bits())
///   5. HP                — i64 LE
///   6. Max HP            — i64 LE
///   7. EntityState       — state.0 (u32 LE)
///   8. Animation ID      — Option<u32> tag-based: None→0u8 (1B), Some(id)→1u8+u32 LE (5B)
///   9. Custom fields     — BTreeMap key 字母序遍歷，每個 (key, value) 依序 hash
///
/// SoftF32 值透過 `to_bits() -> u32` 轉換後以 LE bytes 參與 hash
/// Custom fields 由 BTreeMap 保證 key 字母序遍歷
pub fn hash_entity(hasher: &mut Hasher, eid: &EntityId, entity: &MirroredEntity) {
    // EntityId
    hasher.update(&eid.0.to_le_bytes());

    // Position (SoftVec3 → 3 × u32 bits via to_bits())
    hasher.update(&entity.position.x.to_bits().to_le_bytes());
    hasher.update(&entity.position.y.to_bits().to_le_bytes());
    hasher.update(&entity.position.z.to_bits().to_le_bytes());

    // Rotation
    hasher.update(&entity.rotation.x.to_bits().to_le_bytes());
    hasher.update(&entity.rotation.y.to_bits().to_le_bytes());
    hasher.update(&entity.rotation.z.to_bits().to_le_bytes());

    // Scale
    hasher.update(&entity.scale.x.to_bits().to_le_bytes());
    hasher.update(&entity.scale.y.to_bits().to_le_bytes());
    hasher.update(&entity.scale.z.to_bits().to_le_bytes());

    // HP, Max HP (i64 LE)
    hasher.update(&entity.hp.to_le_bytes());
    hasher.update(&entity.max_hp.to_le_bytes());

    // EntityState(u32)
    hasher.update(&entity.state.0.to_le_bytes());

    // Animation ID（Option<u32>）→ tag-based 編碼
    // None → 0u8（1 byte），Some(id) → 1u8 + u32 LE（5 bytes）
    // 對齊 design doc 10-state-hash/03-computation-impl.md 系統契約
    match entity.animation_id {
        None => {
            hasher.update(&[0u8]);
        }
        Some(id) => {
            hasher.update(&[1u8]);
            hasher.update(&id.to_le_bytes());
        }
    }

    // Custom fields（BTreeMap 保證 key 字母序）
    for (key, value) in entity.custom.iter() {
        hasher.update(key.as_bytes());
        hash_deterministic_value(hasher, value);
    }
}

/// 計算整體遊戲狀態的 blake3 hash
///
/// `rng_state_bytes` 型別為 `&[u8; 16]` 而非 `&[u8]`：
/// 編譯期保證長度正確，避免 release build 中 `debug_assert` 不執行導致
/// 長度錯誤 silent 產生錯誤 hash。上游 `DeterministicRng::state_bytes()`
/// 回傳 `[u8; 16]`，型別自然匹配。
pub fn compute_state_hash(
    entities: &BTreeMap<EntityId, MirroredEntity>,
    rng_state_bytes: &[u8; 16],
    tick: u64,
) -> Blake3Hash {
    let mut hasher = blake3::Hasher::new();

    // 1. Tick number（u64 LE，固定長度 8 bytes）
    hasher.update(&tick.to_le_bytes());

    // 2. RNG state（16 bytes: state u64 LE + increment u64 LE）
    hasher.update(rng_state_bytes);

    // 3. Entities — BTreeMap 保證按 EntityId 升序遍歷
    for (eid, entity) in entities.iter() {
        hash_entity(&mut hasher, eid, entity);
    }

    *hasher.finalize().as_bytes()
}

/// Hash `DeterministicValue` 的各 variant（含 type tag）
///
/// Type tag 為系統契約：0=Int, 1=Float, 2=Bool, 3=Unit, 4=Str
/// 詳見 `10-state-hash/03-computation-impl.md`
fn hash_deterministic_value(hasher: &mut Hasher, value: &DeterministicValue) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, EntityId, EntityState, MirroredEntity};
    use deterministic::{SoftF32, SoftVec3};
    use std::collections::BTreeMap;

    fn soft_vec3(x: f32, y: f32, z: f32) -> SoftVec3 {
        SoftVec3 {
            x: SoftF32::from_f32(x),
            y: SoftF32::from_f32(y),
            z: SoftF32::from_f32(z),
        }
    }

    fn make_entity() -> MirroredEntity {
        MirroredEntity {
            position: soft_vec3(1.0, 2.0, 3.0),
            rotation: soft_vec3(0.0, 0.0, 0.0),
            scale: soft_vec3(1.0, 1.0, 1.0),
            hp: 80,
            max_hp: 100,
            state: EntityState::IDLE,
            animation_id: None,
            custom: BTreeMap::new(),
        }
    }

    /// 測試 helper：計算單一 entity 的 hash
    fn hash_one(eid: &EntityId, entity: &MirroredEntity) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        hash_entity(&mut h, eid, entity);
        h.finalize()
    }

    /// 測試 helper：手動按欄位順序計算期望 hash（用於 bit-level 驗證）
    fn expected_hash_for(eid: &EntityId, entity: &MirroredEntity) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        h.update(&eid.0.to_le_bytes());
        for v in [&entity.position, &entity.rotation, &entity.scale] {
            h.update(&v.x.to_bits().to_le_bytes());
            h.update(&v.y.to_bits().to_le_bytes());
            h.update(&v.z.to_bits().to_le_bytes());
        }
        h.update(&entity.hp.to_le_bytes());
        h.update(&entity.max_hp.to_le_bytes());
        h.update(&entity.state.0.to_le_bytes());
        match entity.animation_id {
            None => {
                h.update(&[0u8]);
            }
            Some(id) => {
                h.update(&[1u8]);
                h.update(&id.to_le_bytes());
            }
        }
        for (key, value) in entity.custom.iter() {
            h.update(key.as_bytes());
            hash_deterministic_value(&mut h, value);
        }
        h.finalize()
    }

    // === 基本確定性 ===

    #[test]
    fn same_entity_same_hash() {
        let eid = EntityId(1);
        let entity = make_entity();
        assert_eq!(hash_one(&eid, &entity), hash_one(&eid, &entity));
    }

    #[test]
    fn different_position_different_hash() {
        let eid = EntityId(1);
        let e1 = make_entity();
        let mut e2 = make_entity();
        e2.position = soft_vec3(9.0, 8.0, 7.0);
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn different_hp_different_hash() {
        let eid = EntityId(1);
        let e1 = make_entity();
        let mut e2 = make_entity();
        e2.hp = 50;
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn different_rotation_different_hash() {
        let eid = EntityId(1);
        let e1 = make_entity();
        let mut e2 = make_entity();
        e2.rotation = soft_vec3(0.0, 90.0, 0.0);
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn different_scale_different_hash() {
        let eid = EntityId(1);
        let e1 = make_entity();
        let mut e2 = make_entity();
        e2.scale = soft_vec3(2.0, 2.0, 2.0);
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn different_state_different_hash() {
        let eid = EntityId(1);
        let e1 = make_entity();
        let mut e2 = make_entity();
        e2.state = EntityState::ATTACKING;
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn different_animation_id_different_hash() {
        let eid = EntityId(1);
        let e1 = make_entity(); // animation_id = None
        let mut e2 = make_entity();
        e2.animation_id = Some(5);
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    // === Custom Fields ===

    #[test]
    fn entity_with_custom_fields_deterministic() {
        let eid = EntityId(1);
        let mut entity = make_entity();
        entity
            .custom
            .insert("mana".to_string(), DeterministicValue::Int(50));
        entity.custom.insert(
            "speed".to_string(),
            DeterministicValue::Float(SoftF32::from_f32(3.5)),
        );
        assert_eq!(hash_one(&eid, &entity), hash_one(&eid, &entity));
    }

    #[test]
    fn custom_field_str_hashed() {
        let eid = EntityId(1);
        let mut entity = make_entity();
        entity.custom.insert(
            "name".to_string(),
            DeterministicValue::Str("warrior".to_string()),
        );
        assert_eq!(hash_one(&eid, &entity), hash_one(&eid, &entity));
    }

    #[test]
    fn different_custom_value_different_hash() {
        let eid = EntityId(1);
        let mut e1 = make_entity();
        e1.custom
            .insert("mana".to_string(), DeterministicValue::Int(50));
        let mut e2 = make_entity();
        e2.custom
            .insert("mana".to_string(), DeterministicValue::Int(100));
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn custom_field_presence_affects_hash() {
        let eid = EntityId(1);
        let e1 = make_entity(); // custom: empty
        let mut e2 = make_entity();
        e2.custom
            .insert("buff".to_string(), DeterministicValue::Bool(true));
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    // === DeterministicValue Type Tag 驗證 ===

    #[test]
    fn custom_field_bool_hashed() {
        let eid = EntityId(1);
        let mut e1 = make_entity();
        e1.custom
            .insert("active".to_string(), DeterministicValue::Bool(true));
        let mut e2 = make_entity();
        e2.custom
            .insert("active".to_string(), DeterministicValue::Bool(false));
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn custom_field_unit_hashed() {
        let eid = EntityId(1);
        let e1 = make_entity(); // custom: empty
        let mut e2 = make_entity();
        e2.custom
            .insert("marker".to_string(), DeterministicValue::Unit);
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn custom_field_str_empty_hashed() {
        let eid = EntityId(1);
        let mut e1 = make_entity();
        e1.custom
            .insert("name".to_string(), DeterministicValue::Str("".to_string()));
        let mut e2 = make_entity();
        e2.custom
            .insert("name".to_string(), DeterministicValue::Str("a".to_string()));
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    #[test]
    fn type_tag_int_verified_via_entity() {
        // Int(0) 和 Unit 必須透過不同 type tag（0 vs 3）產生不同 hash
        let eid = EntityId(1);
        let mut e1 = make_entity();
        e1.custom
            .insert("val".to_string(), DeterministicValue::Int(0));
        let mut e2 = make_entity();
        e2.custom
            .insert("val".to_string(), DeterministicValue::Unit);
        assert_ne!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    // === BTreeMap 確定性驗證 ===

    #[test]
    fn custom_field_insertion_order_irrelevant() {
        let eid = EntityId(1);
        let mut e1 = make_entity();
        e1.custom
            .insert("mana".to_string(), DeterministicValue::Int(50));
        e1.custom.insert(
            "speed".to_string(),
            DeterministicValue::Float(SoftF32::from_f32(3.5)),
        );
        // 反序插入
        let mut e2 = make_entity();
        e2.custom.insert(
            "speed".to_string(),
            DeterministicValue::Float(SoftF32::from_f32(3.5)),
        );
        e2.custom
            .insert("mana".to_string(), DeterministicValue::Int(50));
        assert_eq!(hash_one(&eid, &e1), hash_one(&eid, &e2));
    }

    // === Bit-Level 驗證 ===

    #[test]
    fn soft_f32_hashed_as_u32_bits() {
        let val = SoftF32::from_f32(1.5);
        assert_eq!(val.to_bits(), 0x3FC0_0000_u32); // IEEE 754: 1.5f32
        let eid = EntityId(1);
        let mut entity = make_entity();
        entity.position.x = val;
        assert_eq!(hash_one(&eid, &entity), expected_hash_for(&eid, &entity));
    }

    #[test]
    fn animation_id_tag_based_encoding() {
        let eid = EntityId(1);
        let mut entity = make_entity();
        entity.animation_id = Some(42);
        // expected_hash_for 使用 tag-based: [1u8] + 42u32 LE
        assert_eq!(hash_one(&eid, &entity), expected_hash_for(&eid, &entity));
    }

    // === Game State Hashing 測試（Task 04） ===

    /// 測試 helper：建構 RNG state bytes（state u64 LE + increment u64 LE）
    fn make_rng_bytes(state: u64, increment: u64) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[..8].copy_from_slice(&state.to_le_bytes());
        buf[8..].copy_from_slice(&increment.to_le_bytes());
        buf
    }

    #[test]
    fn same_state_same_hash() {
        let mut entities = BTreeMap::new();
        entities.insert(EntityId(1), make_entity());
        entities.insert(EntityId(2), make_entity());

        let rng_bytes = make_rng_bytes(12345, 67890);
        let tick = 100u64;

        let hash1 = compute_state_hash(&entities, &rng_bytes, tick);
        let hash2 = compute_state_hash(&entities, &rng_bytes, tick);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn different_tick_different_hash() {
        let entities = BTreeMap::new();
        let rng_bytes = make_rng_bytes(0, 0);

        let hash1 = compute_state_hash(&entities, &rng_bytes, 1);
        let hash2 = compute_state_hash(&entities, &rng_bytes, 2);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn different_rng_state_different_hash() {
        let entities = BTreeMap::new();
        let rng1 = make_rng_bytes(100, 200);
        let rng2 = make_rng_bytes(999, 200);

        let hash1 = compute_state_hash(&entities, &rng1, 1);
        let hash2 = compute_state_hash(&entities, &rng2, 1);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn insertion_order_does_not_affect_hash() {
        let rng_bytes = make_rng_bytes(0, 0);

        // 插入順序 1: 1, 2, 3
        let mut entities1 = BTreeMap::new();
        entities1.insert(EntityId(1), make_entity());
        entities1.insert(EntityId(2), make_entity());
        entities1.insert(EntityId(3), make_entity());

        // 插入順序 2: 3, 1, 2
        let mut entities2 = BTreeMap::new();
        entities2.insert(EntityId(3), make_entity());
        entities2.insert(EntityId(1), make_entity());
        entities2.insert(EntityId(2), make_entity());

        let hash1 = compute_state_hash(&entities1, &rng_bytes, 1);
        let hash2 = compute_state_hash(&entities2, &rng_bytes, 1);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn empty_mirror_produces_valid_hash() {
        let entities = BTreeMap::new();
        let rng_bytes = make_rng_bytes(0, 0);

        let hash = compute_state_hash(&entities, &rng_bytes, 0);
        assert_ne!(hash, [0u8; 32]); // 不應為全零
    }

    #[test]
    fn hash_order_is_tick_rng_entities() {
        // 驗證 hash 順序：tick → RNG → entities
        let mut entities = BTreeMap::new();
        entities.insert(EntityId(1), make_entity());

        let rng_bytes = make_rng_bytes(42, 99);
        let tick = 7u64;

        let hash = compute_state_hash(&entities, &rng_bytes, tick);

        // 手動按 tick → RNG → entities 順序計算
        let mut expected = blake3::Hasher::new();
        expected.update(&tick.to_le_bytes());
        expected.update(&rng_bytes);
        hash_entity(&mut expected, &EntityId(1), &entities[&EntityId(1)]);
        let expected_hash: Blake3Hash = *expected.finalize().as_bytes();

        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn animation_id_none_vs_some0_different_hash() {
        // animation_id: None 與 Some(0) 必須產生不同 hash
        // None → [0u8]（1 byte），Some(0) → [1u8] + 0u32 LE（5 bytes）
        let rng_bytes = make_rng_bytes(0, 0);

        let mut entities1 = BTreeMap::new();
        let mut e1 = make_entity();
        e1.animation_id = None;
        entities1.insert(EntityId(1), e1);

        let mut entities2 = BTreeMap::new();
        let mut e2 = make_entity();
        e2.animation_id = Some(0);
        entities2.insert(EntityId(1), e2);

        let hash1 = compute_state_hash(&entities1, &rng_bytes, 1);
        let hash2 = compute_state_hash(&entities2, &rng_bytes, 1);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn animation_id_some0_vs_some42_different_hash() {
        // animation_id: Some(0) 與 Some(42) 必須產生不同 hash
        let rng_bytes = make_rng_bytes(0, 0);

        let mut entities1 = BTreeMap::new();
        let mut e1 = make_entity();
        e1.animation_id = Some(0);
        entities1.insert(EntityId(1), e1);

        let mut entities2 = BTreeMap::new();
        let mut e2 = make_entity();
        e2.animation_id = Some(42);
        entities2.insert(EntityId(1), e2);

        let hash1 = compute_state_hash(&entities1, &rng_bytes, 1);
        let hash2 = compute_state_hash(&entities2, &rng_bytes, 1);
        assert_ne!(hash1, hash2);
    }
}
