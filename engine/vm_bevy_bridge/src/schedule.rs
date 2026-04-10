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
