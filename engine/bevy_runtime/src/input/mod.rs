pub mod capture;
pub mod distribute;
pub mod gesture;
pub mod mapping;
pub mod raw_input;

use bevy::prelude::*;

use capture::input_capture_system;
use distribute::input_distribution_system;
use gesture::{
    gesture_recognition_system, reset_gesture_recognizers, GestureEvent, GestureRecognizers,
};
use mapping::InputMapping;
use raw_input::RawPlayerInput;

use crate::frame_orchestrator::PendingInputs;
use crate::welcome::AppState;
use vm_bevy_bridge::schedule::GameFixedSet;

/// 輸入捕獲、分發與手勢識別 Plugin。
///
/// - PreUpdate：每個 render frame 採集鍵盤/滑鼠 → RawPlayerInput
/// - FixedUpdate (ProcessInputs, InGame)：分發至 script engine + netcode
/// - FixedUpdate (RecognizeGestures, InGame)：手勢識別狀態機
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RawPlayerInput>();
        app.init_resource::<InputMapping>();
        app.init_resource::<GestureRecognizers>();
        // PendingInputs 由此處初始化（gesture_recognition_system 向其推送事件）。
        // 資源的 flush（清空）由 frame_orchestrator 的 FlushBridgeEvents 系統負責。
        // 若在測試或其他情境中只掛載 InputPlugin 而未掛載 FrameOrchestratorPlugin，
        // 需自行確保 PendingInputs 會被定期清空，否則記憶體將無限增長。
        app.init_resource::<PendingInputs>();
        app.add_event::<GestureEvent>();

        app.add_systems(PreUpdate, input_capture_system);

        app.add_systems(
            FixedUpdate,
            input_distribution_system
                .in_set(GameFixedSet::ProcessInputs)
                .run_if(in_state(AppState::InGame)),
        );

        app.add_systems(
            FixedUpdate,
            gesture_recognition_system
                .in_set(GameFixedSet::RecognizeGestures)
                .run_if(in_state(AppState::InGame)),
        );

        app.add_systems(OnExit(AppState::InGame), reset_gesture_recognizers);
    }
}
