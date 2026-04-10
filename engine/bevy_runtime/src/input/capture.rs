use bevy::input::mouse::MouseButton;
use bevy::input::ButtonInput;
use bevy::prelude::*;

use super::mapping::InputMapping;
use super::raw_input::RawPlayerInput;

/// 計算按鍵 bitmask：遍歷映射，檢查每個動作是否有對應按鍵被按下。
pub fn compute_keys_pressed(keyboard: &ButtonInput<KeyCode>, mapping: &InputMapping) -> u32 {
    let mut keys: u32 = 0;
    for (&action, key_codes) in mapping.bindings.iter() {
        if key_codes.iter().any(|k| keyboard.pressed(*k)) {
            keys |= 1 << (action as u32);
        }
    }
    keys
}

/// 計算滑鼠座標：f32 → i16 clamp。cursor 不在視窗內時保留前一幀值。
pub fn compute_mouse_pos(cursor_pos: Option<(f32, f32)>, prev: (i16, i16)) -> (i16, i16) {
    match cursor_pos {
        Some((x, y)) => (
            x.clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            y.clamp(i16::MIN as f32, i16::MAX as f32) as i16,
        ),
        None => prev,
    }
}

/// 計算滑鼠按鍵 bitmask：bit 0=Left, 1=Right, 2=Middle。
pub fn compute_mouse_buttons(mouse_buttons: &ButtonInput<MouseButton>) -> u8 {
    let mut mb: u8 = 0;
    if mouse_buttons.pressed(MouseButton::Left) {
        mb |= 1;
    }
    if mouse_buttons.pressed(MouseButton::Right) {
        mb |= 2;
    }
    if mouse_buttons.pressed(MouseButton::Middle) {
        mb |= 4;
    }
    mb
}

/// PreUpdate 系統：每個 render frame 採集一次硬體輸入，寫入 RawPlayerInput。
/// 無狀態守衛 — 任何 AppState 都執行。
/// 使用 Option<Res<...>> 以支援 headless / MinimalPlugins 環境。
pub fn input_capture_system(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mouse_btn: Option<Res<ButtonInput<MouseButton>>>,
    windows: Query<&Window>,
    mapping: Res<InputMapping>,
    mut raw_input: ResMut<RawPlayerInput>,
) {
    // headless 環境無輸入 resource → 僅遞增 frame_number
    let Some(keyboard) = keyboard else {
        raw_input.frame_number += 1;
        return;
    };
    let Some(mouse_btn) = mouse_btn else {
        raw_input.frame_number += 1;
        return;
    };

    raw_input.keys_pressed = compute_keys_pressed(&keyboard, &mapping);
    raw_input.mouse_buttons = compute_mouse_buttons(&mouse_btn);

    if let Ok(window) = windows.get_single() {
        let cursor_pos = window.cursor_position().map(|v| (v.x, v.y));
        raw_input.mouse_pos = compute_mouse_pos(cursor_pos, raw_input.mouse_pos);
    }

    raw_input.frame_number += 1;

    if raw_input.keys_pressed != 0 || raw_input.mouse_buttons != 0 {
        tracing::debug!(
            frame = raw_input.frame_number,
            keys = format!("{:#06b}", raw_input.keys_pressed),
            mouse_pos = ?raw_input.mouse_pos,
            mouse_buttons = raw_input.mouse_buttons,
            "輸入採集"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::mapping::{InputAction, InputMapping};
    use bevy::input::ButtonInput;
    use bevy::prelude::KeyCode;

    /// 輔助：建立含指定按下鍵的 ButtonInput
    fn make_keyboard(pressed: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut kb = ButtonInput::default();
        for &key in pressed {
            kb.press(key);
        }
        kb
    }

    #[test]
    fn 單鍵按下設定正確_bit() {
        let kb = make_keyboard(&[KeyCode::KeyW]);
        let mapping = InputMapping::default();
        let keys = compute_keys_pressed(&kb, &mapping);
        assert_eq!(keys, 1 << InputAction::MoveUp as u32);
    }

    #[test]
    fn 多鍵同時按下設定多個_bit() {
        let kb = make_keyboard(&[KeyCode::KeyW, KeyCode::Space]);
        let mapping = InputMapping::default();
        let keys = compute_keys_pressed(&kb, &mapping);
        let expected = (1 << InputAction::MoveUp as u32) | (1 << InputAction::Attack as u32);
        assert_eq!(keys, expected);
    }

    #[test]
    fn 無鍵按下返回零() {
        let kb = make_keyboard(&[]);
        let mapping = InputMapping::default();
        let keys = compute_keys_pressed(&kb, &mapping);
        assert_eq!(keys, 0);
    }

    #[test]
    fn 備用鍵觸發同一動作() {
        // ArrowUp 也是 MoveUp
        let kb = make_keyboard(&[KeyCode::ArrowUp]);
        let mapping = InputMapping::default();
        let keys = compute_keys_pressed(&kb, &mapping);
        assert_eq!(keys, 1 << InputAction::MoveUp as u32);
    }

    #[test]
    fn 未映射按鍵不影響_bitmask() {
        let kb = make_keyboard(&[KeyCode::KeyZ]); // Z 不在預設映射中
        let mapping = InputMapping::default();
        let keys = compute_keys_pressed(&kb, &mapping);
        assert_eq!(keys, 0);
    }

    #[test]
    fn 滑鼠座標_clamp_正常值() {
        let pos = Some((320.0_f32, 240.0_f32));
        let prev = (0i16, 0i16);
        let result = compute_mouse_pos(pos, prev);
        assert_eq!(result, (320, 240));
    }

    #[test]
    fn 滑鼠座標_clamp_超出上界() {
        let pos = Some((40000.0_f32, 40000.0_f32));
        let prev = (0, 0);
        let result = compute_mouse_pos(pos, prev);
        assert_eq!(result, (i16::MAX, i16::MAX));
    }

    #[test]
    fn cursor_離開視窗保留前一幀值() {
        let pos: Option<(f32, f32)> = None;
        let prev = (100i16, 200i16);
        let result = compute_mouse_pos(pos, prev);
        assert_eq!(result, (100, 200));
    }

    #[test]
    fn 滑鼠按鍵_bitmask() {
        let mut mb = ButtonInput::default();
        mb.press(bevy::input::mouse::MouseButton::Left);
        mb.press(bevy::input::mouse::MouseButton::Middle);
        let result = compute_mouse_buttons(&mb);
        assert_eq!(result, 0b101); // bit 0 = Left, bit 2 = Middle
    }

    #[test]
    fn 無滑鼠按鍵返回零() {
        let mb = ButtonInput::default();
        let result = compute_mouse_buttons(&mb);
        assert_eq!(result, 0);
    }

    #[test]
    fn 滑鼠座標_clamp_超出下界() {
        let pos = Some((-40000.0_f32, -40000.0_f32));
        let prev = (0, 0);
        let result = compute_mouse_pos(pos, prev);
        assert_eq!(result, (i16::MIN, i16::MIN));
    }
}
