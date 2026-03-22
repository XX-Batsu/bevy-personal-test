//! Handle types 與 Entity 狀態定義
//!
//! 定義所有跨子系統共用的識別碼型別：EntityId、EffectHandle、SoundHandle、EntityState。
//! 這些型別為 VM、Bridge、Netcode、Replay 等子系統的 single source of truth。

use serde::{Deserialize, Serialize};

/// Entity 唯一識別碼
/// 用於所有跨子系統的 Entity 引用（VM、Bridge、Netcode、Replay）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct EntityId(pub u64);

/// 特效 handle — `play_vfx()` 回傳，`stop_vfx()` 傳入
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct EffectHandle(pub u32);

/// 音效 handle — `play_sound()` / `play_sound_at()` 回傳，`stop_sound()` 傳入
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct SoundHandle(pub u32);

/// Entity 狀態 ID（以關聯常數定義具體狀態）
/// 用於 MirroredEntity.state 欄位，確保跨子系統狀態一致
///
/// 常數值為系統契約，參與 State Hash 計算，不可修改既有值。
/// 新增狀態必須使用未占用的 `u32` 值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct EntityState(pub u32);

impl EntityState {
    pub const IDLE: Self = Self(0);
    pub const MOVING: Self = Self(1);
    pub const ATTACKING: Self = Self(2);
    pub const DEAD: Self = Self(3);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_id_bincode_round_trip() {
        let id = EntityId(42);
        let bytes = bincode::serialize(&id).unwrap();
        let decoded: EntityId = bincode::deserialize(&bytes).unwrap();
        assert_eq!(id, decoded);
    }

    #[test]
    fn entity_id_zero() {
        let id = EntityId(0);
        let bytes = bincode::serialize(&id).unwrap();
        let decoded: EntityId = bincode::deserialize(&bytes).unwrap();
        assert_eq!(id, decoded);
    }

    #[test]
    fn entity_id_max() {
        let id = EntityId(u64::MAX);
        let bytes = bincode::serialize(&id).unwrap();
        let decoded: EntityId = bincode::deserialize(&bytes).unwrap();
        assert_eq!(id, decoded);
    }

    #[test]
    fn effect_handle_bincode_round_trip() {
        let handle = EffectHandle(7);
        let bytes = bincode::serialize(&handle).unwrap();
        let decoded: EffectHandle = bincode::deserialize(&bytes).unwrap();
        assert_eq!(handle, decoded);
    }

    #[test]
    fn sound_handle_bincode_round_trip() {
        let handle = SoundHandle(99);
        let bytes = bincode::serialize(&handle).unwrap();
        let decoded: SoundHandle = bincode::deserialize(&bytes).unwrap();
        assert_eq!(handle, decoded);
    }

    #[test]
    fn entity_state_bincode_round_trip() {
        for state in [
            EntityState::IDLE,
            EntityState::MOVING,
            EntityState::ATTACKING,
            EntityState::DEAD,
        ] {
            let bytes = bincode::serialize(&state).unwrap();
            let decoded: EntityState = bincode::deserialize(&bytes).unwrap();
            assert_eq!(state, decoded);
        }
    }

    #[test]
    fn entity_id_ord_for_btreemap() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert(EntityId(3), "c");
        map.insert(EntityId(1), "a");
        map.insert(EntityId(2), "b");
        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&EntityId(1), &EntityId(2), &EntityId(3)]);
    }

    #[test]
    fn entity_state_custom_value() {
        let state = EntityState(100);
        let bytes = bincode::serialize(&state).unwrap();
        let decoded: EntityState = bincode::deserialize(&bytes).unwrap();
        assert_eq!(state, decoded);
    }

    #[test]
    fn entity_id_copy_semantics() {
        let a = EntityId(1);
        let b = a; // Copy
        assert_eq!(a, b); // a 仍可使用（非 move）
    }

    #[test]
    fn effect_handle_copy_semantics() {
        let a = EffectHandle(1);
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn bincode_truncated_entity_id() {
        let id = EntityId(42);
        let bytes = bincode::serialize(&id).unwrap();
        // 截斷為不完整的 4 bytes（EntityId 需要 8 bytes）
        let truncated = &bytes[..4];
        assert!(bincode::deserialize::<EntityId>(truncated).is_err());
    }

    #[test]
    fn bincode_empty_input() {
        assert!(bincode::deserialize::<EntityId>(&[]).is_err());
    }

    #[test]
    fn entity_id_size_of() {
        assert_eq!(std::mem::size_of::<EntityId>(), 8);
    }

    #[test]
    fn entity_state_size_of() {
        assert_eq!(std::mem::size_of::<EntityState>(), 4);
    }

    #[test]
    fn effect_handle_max() {
        let handle = EffectHandle(u32::MAX);
        let bytes = bincode::serialize(&handle).unwrap();
        let decoded: EffectHandle = bincode::deserialize(&bytes).unwrap();
        assert_eq!(handle, decoded);
    }

    #[test]
    fn sound_handle_max() {
        let handle = SoundHandle(u32::MAX);
        let bytes = bincode::serialize(&handle).unwrap();
        let decoded: SoundHandle = bincode::deserialize(&bytes).unwrap();
        assert_eq!(handle, decoded);
    }

    #[test]
    fn entity_state_constants_values() {
        assert_eq!(EntityState::IDLE.0, 0);
        assert_eq!(EntityState::MOVING.0, 1);
        assert_eq!(EntityState::ATTACKING.0, 2);
        assert_eq!(EntityState::DEAD.0, 3);
    }
}
