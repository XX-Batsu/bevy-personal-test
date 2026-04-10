use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 遊戲輸入動作。Discriminant 即 bitmask bit 位（0..31）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum InputAction {
    MoveUp = 0,
    MoveDown = 1,
    MoveLeft = 2,
    MoveRight = 3,
    Attack = 4,
    Skill1 = 5,
    Skill2 = 6,
    Skill3 = 7,
    Interact = 8,
    Dodge = 9,
}

/// 動作 → 按鍵映射。使用 BTreeMap 保證確定性遍歷順序。
#[derive(Resource, Clone, Debug)]
pub struct InputMapping {
    pub bindings: BTreeMap<InputAction, Vec<KeyCode>>,
}

impl Default for InputMapping {
    fn default() -> Self {
        let mut bindings = BTreeMap::new();
        bindings.insert(InputAction::MoveUp, vec![KeyCode::KeyW, KeyCode::ArrowUp]);
        bindings.insert(
            InputAction::MoveDown,
            vec![KeyCode::KeyS, KeyCode::ArrowDown],
        );
        bindings.insert(
            InputAction::MoveLeft,
            vec![KeyCode::KeyA, KeyCode::ArrowLeft],
        );
        bindings.insert(
            InputAction::MoveRight,
            vec![KeyCode::KeyD, KeyCode::ArrowRight],
        );
        bindings.insert(InputAction::Attack, vec![KeyCode::Space]);
        bindings.insert(InputAction::Skill1, vec![KeyCode::Digit1]);
        bindings.insert(InputAction::Skill2, vec![KeyCode::Digit2]);
        bindings.insert(InputAction::Skill3, vec![KeyCode::Digit3]);
        bindings.insert(InputAction::Interact, vec![KeyCode::KeyE]);
        bindings.insert(InputAction::Dodge, vec![KeyCode::ShiftLeft]);
        Self { bindings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 預設映射包含所有動作() {
        let mapping = InputMapping::default();
        let actions = [
            InputAction::MoveUp,
            InputAction::MoveDown,
            InputAction::MoveLeft,
            InputAction::MoveRight,
            InputAction::Attack,
            InputAction::Skill1,
            InputAction::Skill2,
            InputAction::Skill3,
            InputAction::Interact,
            InputAction::Dodge,
        ];
        for action in &actions {
            assert!(
                mapping.bindings.contains_key(action),
                "{:?} 缺少預設綁定",
                action
            );
        }
    }

    #[test]
    fn 每個動作至少綁定一個按鍵() {
        let mapping = InputMapping::default();
        for (action, keys) in &mapping.bindings {
            assert!(!keys.is_empty(), "{:?} 綁定了空的按鍵列表", action);
        }
    }

    #[test]
    fn btreemap_遍歷順序與_discriminant_一致() {
        let mapping = InputMapping::default();
        let keys: Vec<InputAction> = mapping.bindings.keys().copied().collect();
        for window in keys.windows(2) {
            assert!(
                (window[0] as u32) < (window[1] as u32),
                "BTreeMap 順序應遞增：{:?} >= {:?}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn action_discriminant_即_bit位() {
        assert_eq!(InputAction::MoveUp as u32, 0);
        assert_eq!(InputAction::MoveDown as u32, 1);
        assert_eq!(InputAction::MoveLeft as u32, 2);
        assert_eq!(InputAction::MoveRight as u32, 3);
        assert_eq!(InputAction::Attack as u32, 4);
        assert_eq!(InputAction::Skill1 as u32, 5);
        assert_eq!(InputAction::Skill2 as u32, 6);
        assert_eq!(InputAction::Skill3 as u32, 7);
        assert_eq!(InputAction::Interact as u32, 8);
        assert_eq!(InputAction::Dodge as u32, 9);
    }

    #[test]
    fn 所有_discriminant_小於32() {
        let actions = [
            InputAction::MoveUp,
            InputAction::MoveDown,
            InputAction::MoveLeft,
            InputAction::MoveRight,
            InputAction::Attack,
            InputAction::Skill1,
            InputAction::Skill2,
            InputAction::Skill3,
            InputAction::Interact,
            InputAction::Dodge,
        ];
        for action in &actions {
            assert!(
                (*action as u32) < 32,
                "{:?} 的 discriminant {} 超出 u32 bitmask 範圍",
                action,
                *action as u32
            );
        }
    }

    #[test]
    fn 自定義映射修改綁定後_capture_行為正確() {
        use crate::input::capture::compute_keys_pressed;
        use bevy::input::ButtonInput;

        let mut mapping = InputMapping::default();
        // 將 MoveUp 改綁到 KeyZ
        mapping
            .bindings
            .insert(InputAction::MoveUp, vec![KeyCode::KeyZ]);

        let mut kb = ButtonInput::default();
        kb.press(KeyCode::KeyZ);

        let keys = compute_keys_pressed(&kb, &mapping);
        assert_eq!(keys, 1 << InputAction::MoveUp as u32, "Z 鍵應觸發 MoveUp");

        // W 鍵不再觸發 MoveUp
        let mut kb2 = ButtonInput::default();
        kb2.press(KeyCode::KeyW);
        let keys2 = compute_keys_pressed(&kb2, &mapping);
        assert_eq!(keys2, 0, "W 鍵已解綁，不應觸發任何動作");
    }
}
