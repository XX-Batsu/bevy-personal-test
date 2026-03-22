//! ECS 唯讀快照型別
//!
//! `EcsMirror` 與 `MirroredEntity` 為 Bevy ECS → VM 的唯讀快照機制核心型別。
//! Bridge 層在每個邏輯 tick 的 Last schedule 中更新快照，
//! VM 腳本透過 `get_position()` / `get_entity_state()` 讀取。
//!
//! 所有欄位使用確定性型別（SoftF32、SoftVec3、BTreeMap），
//! 確保跨平台 state hash 計算結果一致。

use std::collections::BTreeMap;

use deterministic::{SoftF32, SoftVec3};
use serde::{Deserialize, Serialize};

use crate::{DeterministicValue, EntityId, EntityState};

/// ECS 唯讀快照 — Bridge 層維護，供 VM 讀取
///
/// 資料流：Bevy ECS → EcsMirror（寫入） → VM get_position() / get_entity_state()（讀取）
/// 更新頻率：每邏輯 tick（16.67ms），在 Bevy Last schedule 結束後更新
/// 序列化格式：bincode（內部記憶體表示，非網路傳輸）
///
/// Determinism 保證：
/// - entities 使用 BTreeMap（確定性迭代順序）
/// - delta_time 使用 SoftF32（確定性浮點）
/// - frame_number 為 u64 tick counter（非 wall clock）
///
/// 不使用 #[repr(C)]：含 BTreeMap（heap-allocated），非 FFI-safe，
/// layout 穩定性由 bincode 序列化格式保證
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EcsMirror {
    /// 所有存活 Entity 的鏡像資料，以 EntityId 排序
    pub entities: BTreeMap<EntityId, MirroredEntity>,

    /// 本地玩家 Entity ID
    pub local_player_id: EntityId,

    /// 當前邏輯幀號（0 起始，每 tick +1）
    pub frame_number: u64,

    /// 本幀 delta time（固定 16.67ms = SoftF32::from_native(1.0 / 60.0)）
    pub delta_time: SoftF32,
}

/// 單一 Entity 的鏡像資料
///
/// 所有欄位使用確定性型別：
/// - 位置/旋轉/縮放：SoftVec3
/// - 自定義欄位：BTreeMap<String, DeterministicValue>（確定性迭代 + 確定性值）
/// - 狀態：EntityState(u32)
///
/// 不使用 #[repr(C)]：含 BTreeMap<String, DeterministicValue>（heap-allocated），
/// 非 FFI-safe，layout 穩定性由 bincode 序列化格式保證
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MirroredEntity {
    /// 世界座標位置
    pub position: SoftVec3,

    /// 旋轉（Euler angles）
    pub rotation: SoftVec3,

    /// 縮放
    pub scale: SoftVec3,

    /// 當前血量
    pub hp: i64,

    /// 最大血量
    pub max_hp: i64,

    /// Entity 當前狀態
    pub state: EntityState,

    /// 當前播放的動畫 ID（None 表示無動畫）
    pub animation_id: Option<u32>,

    /// 擴展欄位（game logic 值，確定性）
    /// 使用 BTreeMap 確保迭代順序確定性
    /// Value 為 DeterministicValue（含 Str，不含 f64）
    pub custom: BTreeMap<String, DeterministicValue>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use deterministic::SoftF32;

    fn make_soft_vec3(x: f32, y: f32, z: f32) -> SoftVec3 {
        SoftVec3 {
            x: SoftF32::from_f32(x),
            y: SoftF32::from_f32(y),
            z: SoftF32::from_f32(z),
        }
    }

    fn make_entity() -> MirroredEntity {
        let mut custom = BTreeMap::new();
        custom.insert("mana".to_string(), DeterministicValue::Int(50));
        MirroredEntity {
            position: make_soft_vec3(1.0, 2.0, 3.0),
            rotation: make_soft_vec3(0.0, 0.0, 0.0),
            scale: make_soft_vec3(1.0, 1.0, 1.0),
            hp: 80,
            max_hp: 100,
            state: EntityState::IDLE,
            animation_id: None,
            custom,
        }
    }

    #[test]
    fn ecs_mirror_with_entities_round_trip() {
        let mut entities = BTreeMap::new();
        entities.insert(EntityId(1), make_entity());
        entities.insert(EntityId(2), make_entity());
        entities.insert(EntityId(3), make_entity());

        let mirror = EcsMirror {
            entities,
            local_player_id: EntityId(1),
            frame_number: 120,
            delta_time: SoftF32::from_f32(0.01667),
        };

        let bytes = bincode::serialize(&mirror).unwrap();
        let decoded: EcsMirror = bincode::deserialize(&bytes).unwrap();
        assert_eq!(mirror, decoded);
    }

    #[test]
    fn ecs_mirror_empty_round_trip() {
        let mirror = EcsMirror {
            entities: BTreeMap::new(),
            local_player_id: EntityId(0),
            frame_number: 0,
            delta_time: SoftF32::from_f32(0.01667),
        };

        let bytes = bincode::serialize(&mirror).unwrap();
        let decoded: EcsMirror = bincode::deserialize(&bytes).unwrap();
        assert_eq!(mirror, decoded);
    }

    #[test]
    fn mirrored_entity_custom_fields_round_trip() {
        let mut entity = make_entity();
        entity.custom.insert(
            "speed".to_string(),
            DeterministicValue::Float(SoftF32::from_f32(5.5)),
        );
        entity
            .custom
            .insert("alive".to_string(), DeterministicValue::Bool(true));
        entity
            .custom
            .insert("level".to_string(), DeterministicValue::Int(10));

        let bytes = bincode::serialize(&entity).unwrap();
        let decoded: MirroredEntity = bincode::deserialize(&bytes).unwrap();
        assert_eq!(entity, decoded);
    }

    #[test]
    fn ecs_mirror_btreemap_iteration_order_deterministic() {
        let mut entities = BTreeMap::new();
        entities.insert(EntityId(5), make_entity());
        entities.insert(EntityId(1), make_entity());
        entities.insert(EntityId(3), make_entity());

        let keys: Vec<_> = entities.keys().collect();
        assert_eq!(keys, vec![&EntityId(1), &EntityId(3), &EntityId(5)]);
    }

    #[test]
    fn mirrored_entity_empty_custom() {
        let mut entity = make_entity();
        entity.custom = BTreeMap::new();

        let bytes = bincode::serialize(&entity).unwrap();
        let decoded: MirroredEntity = bincode::deserialize(&bytes).unwrap();
        assert_eq!(entity, decoded);
    }

    #[test]
    fn ecs_mirror_single_entity() {
        let mut entities = BTreeMap::new();
        entities.insert(EntityId(42), make_entity());

        let mirror = EcsMirror {
            entities,
            local_player_id: EntityId(42),
            frame_number: 1,
            delta_time: SoftF32::from_f32(0.01667),
        };

        let bytes = bincode::serialize(&mirror).unwrap();
        let decoded: EcsMirror = bincode::deserialize(&bytes).unwrap();
        assert_eq!(mirror, decoded);
    }

    #[test]
    fn mirrored_entity_all_states() {
        for state in [
            EntityState::IDLE,
            EntityState::MOVING,
            EntityState::ATTACKING,
            EntityState::DEAD,
        ] {
            let mut entity = make_entity();
            entity.state = state;

            let bytes = bincode::serialize(&entity).unwrap();
            let decoded: MirroredEntity = bincode::deserialize(&bytes).unwrap();
            assert_eq!(entity, decoded);
        }
    }

    #[test]
    fn mirrored_entity_animation_none_round_trip() {
        let entity = make_entity(); // animation_id = None
        assert_eq!(entity.animation_id, None);
        let bytes = bincode::serialize(&entity).unwrap();
        let decoded: MirroredEntity = bincode::deserialize(&bytes).unwrap();
        assert_eq!(entity, decoded);
    }

    #[test]
    fn mirrored_entity_animation_some_round_trip() {
        let mut entity = make_entity();
        entity.animation_id = Some(42);
        let bytes = bincode::serialize(&entity).unwrap();
        let decoded: MirroredEntity = bincode::deserialize(&bytes).unwrap();
        assert_eq!(entity, decoded);
        assert_eq!(decoded.animation_id, Some(42));
    }
}
