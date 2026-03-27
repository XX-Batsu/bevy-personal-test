//! BridgePlugin — 整合 flush、EcsMirror 同步、fallback、HandleRegistry sweep。

use bevy::prelude::*;
use deterministic::NativeClock;
use vm_runtime::HandleRegistry;

use crate::event_queue::{BridgeDiagnostics, BridgeEntityMap, BridgeEventQueue};
use crate::fallback::{
    fallback_animation_system, fallback_event_system, fallback_movement_system, fallback_ui_system,
};
use crate::flush::{flush_bridge_events, ClockResource};
use crate::handle_sweep::{sweep_handle_registry, HandleRegistryResource, SimulationClock};
use crate::mirror_sync::{update_ecs_mirror, EcsMirrorResource, SharedBridgeState};
use crate::schedule::GameFixedSet;

/// Bridge Plugin：整合 flush、EcsMirror 同步、fallback、HandleRegistry sweep。
///
/// **先決條件**：呼叫端須在加入此 Plugin 前先 configure `GameFixedSet` chain。
/// `bevy_runtime::GamePlugin` 負責此配置。
pub struct BridgePlugin {
    /// 注入 VM 側的 BridgeState（Arc<Mutex<BridgeState>>）
    pub bridge_state: SharedBridgeState,
}

impl Plugin for BridgePlugin {
    fn build(&self, app: &mut App) {
        // ─── Resources ─────────────────────────────────────────────
        app.init_resource::<BridgeEventQueue>();
        app.init_resource::<BridgeEntityMap>();
        app.init_resource::<BridgeDiagnostics>();
        app.init_resource::<SimulationClock>();
        app.init_resource::<EcsMirrorResource>();
        app.insert_resource(self.bridge_state.clone());
        app.insert_resource(ClockResource(Box::new(NativeClock::new())));
        app.insert_resource(HandleRegistryResource(HandleRegistry::new()));

        // ─── FixedUpdate 系統（依賴 GameFixedSet chain 已由 GamePlugin 配置）───
        app.add_systems(
            FixedUpdate,
            flush_bridge_events.in_set(GameFixedSet::FlushBridgeEvents),
        );
        app.add_systems(
            FixedUpdate,
            (
                update_ecs_mirror,
                fallback_animation_system,
                fallback_ui_system,
                fallback_event_system,
                fallback_movement_system,
            )
                .in_set(GameFixedSet::UpdateEcsMirror),
        );

        // ─── Last schedule：HandleRegistry sweep（每 60 frames）───
        app.add_systems(Last, sweep_handle_registry);
    }
}
