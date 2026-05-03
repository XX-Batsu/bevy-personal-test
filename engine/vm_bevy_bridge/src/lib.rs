//! VM-ECS 橋接層 — BridgeEventQueue、flush 系統、EcsMirror 同步、實體映射。
//!
//! 提供 bridge 系統與資源，由 `bevy_runtime::BridgePlugin` 整合至 Bevy App。

pub mod event_queue;
pub mod fallback;
pub mod flush;
pub mod handle_sweep;
pub mod mirror_sync;
pub mod schedule;

pub use event_queue::{BridgeDiagnostics, BridgeEntityMap, BridgeEventQueue, GameEntityTypeId};
pub use fallback::{
    fallback_animation_system, fallback_event_system, fallback_movement_system, fallback_ui_system,
    AnimationDefault, ScriptDisabled, ServerPosition,
};
pub use flush::{
    flush_bridge_events, BlendAnimationRequest, ClockResource, GameEntityBundle,
    PlayAnimationRequest, VfxPlaceholder,
};
pub use handle_sweep::{sweep_handle_registry, HandleRegistryResource, SimulationClock};
pub use mirror_sync::{update_ecs_mirror, EcsMirrorResource, SharedBridgeState};
pub use schedule::{configure_game_fixed_set_for_tests, GameFixedSet};

mod plugin;
pub use plugin::BridgePlugin;
