use bevy::prelude::*;

/// 每個 render frame 採集一次的原始硬體輸入。
/// 全整數欄位，無浮點數，符合確定性規則。
/// 此結構為幀間暫存 Resource，不序列化至 replay log。
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct RawPlayerInput {
    /// Render frame 計數器（PreUpdate 每幀遞增），供分發層去重。
    pub frame_number: u64,
    /// 按鍵 bitmask，由 InputMapping 決定每個 bit 對應的動作。
    pub keys_pressed: u32,
    /// 視窗像素座標，原點左上角。
    pub mouse_pos: (i16, i16),
    /// 滑鼠按鍵 bitmask：bit 0=Left, 1=Right, 2=Middle。
    pub mouse_buttons: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_所有欄位為零() {
        let input = RawPlayerInput::default();
        assert_eq!(input.frame_number, 0);
        assert_eq!(input.keys_pressed, 0);
        assert_eq!(input.mouse_pos, (0, 0));
        assert_eq!(input.mouse_buttons, 0);
    }

    #[test]
    fn clone_產生獨立副本() {
        let input = RawPlayerInput {
            keys_pressed: 0b1010,
            frame_number: 42,
            ..Default::default()
        };
        let cloned = input.clone();
        assert_eq!(cloned.keys_pressed, 0b1010);
        assert_eq!(cloned.frame_number, 42);
    }
}
