//! 共用的 FixedUpdate 系統排序標籤。
//!
//! `GameFixedSet` 定義於此（vm_bevy_bridge），供 bevy_runtime 與 BridgePlugin 共用，
//! 避免循環依賴（bevy_runtime → vm_bevy_bridge，不可反向）。

use bevy::prelude::*;

/// FixedUpdate 系統鏈中的系統排序標籤。
///
/// 六個 set 以 `.chain()` 強制依序執行：
/// `ProcessInputs → RecognizeGestures → RunScripts → FlushBridgeEvents → UpdateEcsMirror → ComputeStateHash`
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GameFixedSet {
    ProcessInputs,
    RecognizeGestures,
    RunScripts,
    FlushBridgeEvents,
    UpdateEcsMirror,
    ComputeStateHash,
}

/// 測試用 helper：configure GameFixedSet chain（一般由 `bevy_runtime::GamePlugin` 呼叫）。
///
/// 不加 `#[cfg(test)]` gate（雖名 `_for_tests`），因 bevy_runtime crate 內
/// `tests/camera_phase_b_e2e.rs` 整合測試需透過 dependency 存取。
pub fn configure_game_fixed_set_for_tests(app: &mut App) {
    app.configure_sets(
        FixedUpdate,
        (
            GameFixedSet::ProcessInputs,
            GameFixedSet::RecognizeGestures,
            GameFixedSet::RunScripts,
            GameFixedSet::FlushBridgeEvents,
            GameFixedSet::UpdateEcsMirror,
            GameFixedSet::ComputeStateHash,
        )
            .chain(),
    );
}
