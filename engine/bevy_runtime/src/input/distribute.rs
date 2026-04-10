use bevy::prelude::*;
use bridge_types::{DeterministicValue, EntityId, PlayerInput};
use netcode::snapshot::InputBuffer;

use crate::fixed_update::FixedTickCounter;
use crate::frame_orchestrator::{PendingInputs, ScriptInput};

use super::raw_input::RawPlayerInput;

/// 本地玩家 ID — 由玩家進入流程插入。
#[derive(Resource, Clone, Debug)]
pub struct LocalPlayerId(pub EntityId);

/// InputBuffer 的 Bevy Resource 包裝。
#[derive(Resource, Default)]
pub struct InputBufferRes(pub InputBuffer);

/// 去重判斷：frame_number 與上次處理的不同時才分發。
pub fn should_distribute(current_frame: u64, last_frame: u64) -> bool {
    current_frame != last_frame
}

/// 將 RawPlayerInput 轉換為 ScriptInput（JSON 格式）。
pub fn build_script_input(raw: &RawPlayerInput) -> ScriptInput {
    ScriptInput {
        input_type: "player_input".to_string(),
        data: serde_json::json!({
            "keys": raw.keys_pressed,
            "mx": raw.mouse_pos.0,
            "my": raw.mouse_pos.1,
            "mb": raw.mouse_buttons,
        })
        .to_string(),
    }
}

/// 將 RawPlayerInput 打包為兩個 netcode PlayerInput entry。
///
/// - input_type 0: `(keys_pressed << 8) | mouse_buttons`
/// - input_type 1: `(mouse_x as u16 << 16) | mouse_y as u16`
pub fn build_netcode_inputs(
    raw: &RawPlayerInput,
    player_id: EntityId,
    tick: u64,
) -> Vec<PlayerInput> {
    let keys_data = ((raw.keys_pressed as i64) << 8) | raw.mouse_buttons as i64;
    let mouse_data = ((raw.mouse_pos.0 as u16 as i64) << 16) | raw.mouse_pos.1 as u16 as i64;

    vec![
        PlayerInput {
            player_id,
            input_type: 0,
            data: DeterministicValue::Int(keys_data),
            tick,
        },
        PlayerInput {
            player_id,
            input_type: 1,
            data: DeterministicValue::Int(mouse_data),
            tick,
        },
    ]
}

/// FixedUpdate 分發系統：讀取 RawPlayerInput，分發至 script engine 與 netcode。
/// 僅在 AppState::InGame 執行（由 InputPlugin 設定 run_if）。
pub fn input_distribution_system(
    raw_input: Res<RawPlayerInput>,
    mut last_frame: Local<u64>,
    tick: Res<FixedTickCounter>,
    mut pending_inputs: ResMut<PendingInputs>,
    local_player_id: Option<Res<LocalPlayerId>>,
    input_buffer: Option<ResMut<InputBufferRes>>,
) {
    if !should_distribute(raw_input.frame_number, *last_frame) {
        return;
    }
    *last_frame = raw_input.frame_number;

    let current_tick = tick.count;

    // Script 路徑
    pending_inputs.push(build_script_input(&raw_input));

    // Netcode 路徑
    let Some(local_player) = local_player_id else {
        // 使用 static flag 避免每 tick 重複 warn
        use std::sync::atomic::{AtomicBool, Ordering};
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!("本地玩家 ID 尚未設定，跳過 netcode 輸入分發");
        }
        return;
    };

    debug_assert!(raw_input.mouse_pos.0 >= 0, "視窗座標 x 不應為負");
    debug_assert!(raw_input.mouse_pos.1 >= 0, "視窗座標 y 不應為負");

    let entries = build_netcode_inputs(&raw_input, local_player.0, current_tick);

    if let Some(mut buffer) = input_buffer {
        buffer.0.insert(current_tick, entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::raw_input::RawPlayerInput;
    use bridge_types::{DeterministicValue, EntityId};

    // === Script 路徑 ===

    #[test]
    fn 轉換為_script_input_格式正確() {
        let raw = RawPlayerInput {
            frame_number: 1,
            keys_pressed: 0b1010,
            mouse_pos: (320, 240),
            mouse_buttons: 0b011,
        };
        let script_input = build_script_input(&raw);
        assert_eq!(script_input.input_type, "player_input");
        let data: serde_json::Value = serde_json::from_str(&script_input.data).unwrap();
        assert_eq!(data["keys"], 0b1010);
        assert_eq!(data["mx"], 320);
        assert_eq!(data["my"], 240);
        assert_eq!(data["mb"], 0b011);
    }

    // === Netcode bit packing ===

    #[test]
    fn bit_packing_全零輸入() {
        let raw = RawPlayerInput::default();
        let pid = EntityId(42);
        let tick = 100;
        let entries = build_netcode_inputs(&raw, pid, tick);
        assert_eq!(entries.len(), 2);

        // Entry 0: keys + mouse_buttons
        assert_eq!(entries[0].player_id, pid);
        assert_eq!(entries[0].input_type, 0);
        assert_eq!(entries[0].data, DeterministicValue::Int(0));
        assert_eq!(entries[0].tick, 100);

        // Entry 1: mouse pos
        assert_eq!(entries[1].input_type, 1);
        assert_eq!(entries[1].data, DeterministicValue::Int(0));
    }

    #[test]
    fn bit_packing_全按下() {
        let raw = RawPlayerInput {
            frame_number: 1,
            keys_pressed: 0xFFFF_FFFF,
            mouse_pos: (i16::MAX, i16::MAX),
            mouse_buttons: 0x07,
        };
        let pid = EntityId(1);
        let entries = build_netcode_inputs(&raw, pid, 50);

        // Entry 0: (0xFFFF_FFFF << 8) | 0x07
        let expected_keys = ((0xFFFF_FFFFu32 as i64) << 8) | 0x07i64;
        assert_eq!(entries[0].data, DeterministicValue::Int(expected_keys));

        // Entry 1: (i16::MAX as u16 << 16) | i16::MAX as u16
        let mx = i16::MAX as u16;
        let expected_mouse = ((mx as i64) << 16) | mx as i64;
        assert_eq!(entries[1].data, DeterministicValue::Int(expected_mouse));
    }

    #[test]
    fn bit_packing_round_trip_鍵與滑鼠按鍵() {
        let keys_pressed: u32 = 0b0000_0011_0000_0101; // MoveUp + MoveLeft + Skill1 + Skill2
        let mouse_buttons: u8 = 0b101; // Left + Middle
        let packed = ((keys_pressed as i64) << 8) | mouse_buttons as i64;

        // 解碼
        let decoded_keys = (packed >> 8) as u32;
        let decoded_mb = (packed & 0xFF) as u8;
        assert_eq!(decoded_keys, keys_pressed);
        assert_eq!(decoded_mb, mouse_buttons);
    }

    #[test]
    fn bit_packing_round_trip_滑鼠座標() {
        let mx: i16 = -100;
        let my: i16 = 500;
        let packed = ((mx as u16 as i64) << 16) | my as u16 as i64;

        // 解碼
        let decoded_mx = ((packed >> 16) & 0xFFFF) as u16 as i16;
        let decoded_my = (packed & 0xFFFF) as u16 as i16;
        assert_eq!(decoded_mx, mx);
        assert_eq!(decoded_my, my);
    }

    // === 去重 ===

    #[test]
    fn 去重_同一_frame_number_應跳過() {
        assert!(!should_distribute(5, 5)); // 已處理過
    }

    #[test]
    fn 去重_新_frame_number_應處理() {
        assert!(should_distribute(6, 5)); // 新 frame
    }

    #[test]
    fn 去重_首次呼叫_frame_0_應處理() {
        // frame_number 從 1 開始（capture 先 +1 再寫入），last_frame 初始 0
        assert!(should_distribute(1, 0));
    }
}
