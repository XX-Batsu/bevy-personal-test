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
