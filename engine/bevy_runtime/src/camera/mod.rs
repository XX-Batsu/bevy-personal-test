//! 遊戲攝影機系統 — ECS-heavy component-driven 架構。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md`
//!
//! # 架構
//! - 每個功能為獨立 Component，掛在攝影機 entity 上按需組合
//! - 11 個系統在 `Update` 以 `.chain()` 保證順序（Phase A）
//! - PreUpdate 2 個 BridgeCamera marker register/unregister 系統（Phase B）
//! - FixedUpdate 1 個 flush_camera_ops 系統（Phase B；GameFixedSet::FlushBridgeEvents stage）
//! - 所有計算使用 native `f32`（渲染層，不受 SoftF32 約束）

pub mod bounds;
pub mod camera_flush;
pub mod camera_registry;
pub mod cinematic;
pub mod components;
pub mod cursor;
pub mod events;
pub mod follow;
pub mod override_stack;
pub mod shake;
pub mod zoom;

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use vm_bevy_bridge::GameFixedSet;

pub use bounds::camera_bounds_system;
pub use camera_flush::flush_camera_ops;
pub use camera_registry::{
    bridge_camera_register_system, bridge_camera_unregister_system, BridgeCamera, CameraRegistry,
    RegisterError,
};
pub use cinematic::{
    on_cinematic_added, on_cinematic_removed, CameraCinematic, CameraCinematicShadow,
};
pub use components::{
    decay_factor, CameraBounds, CameraFollow, CameraLookAhead, CameraTarget, CameraZoom,
    PreviousTargetPosition, DEFAULT_CINEMATIC_SPEED, DEFAULT_FOLLOW_SPEED,
    DEFAULT_LOOK_AHEAD_MAX_OFFSET, DEFAULT_MOUSE_WEIGHT, DEFAULT_SHAKE_DECAY_RATE,
    DEFAULT_SHAKE_DIRECTION_BIAS, DEFAULT_SHAKE_MAX_STRENGTH_RATIO,
    DEFAULT_SHAKE_PERPENDICULAR_DAMPING, DEFAULT_VELOCITY_WEIGHT, DEFAULT_VIEWPORT_HEIGHT,
    DEFAULT_ZOOM_MAX, DEFAULT_ZOOM_MIN, DEFAULT_ZOOM_SCROLL_SPEED, DEFAULT_ZOOM_SPEED,
    SHAKE_FREQUENCY,
};
pub use cursor::{cursor_position_system, viewport_to_world, CursorWorldPosition};
pub use events::{CameraOverrideOp, CameraOverrideRequest, CameraShakeOp, CameraShakeRequest};
pub use follow::{camera_follow_system, camera_look_ahead_system, update_previous_target_system};
pub use override_stack::{
    override_apply_system, override_event_handler_system, override_stack_tick_system,
    CameraOverrideStack, Override, OverrideId, PushOverrideParams,
};
pub use shake::{
    shake_apply_system, shake_event_handler_system, CameraShake, ShakeEntry, ShakeId, ShakeParams,
    ShakeSeedCounter,
};
pub use zoom::camera_zoom_system;

/// 攝影機 plugin。
///
/// **先決條件**: app 必須已 insert `vm_bevy_bridge::SharedBridgeState`
/// resource（一般由 GamePlugin 透過 BridgePlugin 設置），否則 `flush_camera_ops`
/// 在執行時 panic（找不到 SharedBridgeState resource）。
///
/// CameraPlugin 自動註冊：
/// - Phase A events（CameraOverrideRequest / CameraShakeRequest）
/// - Phase B resources（CameraRegistry, ShakeSeedCounter）
/// - Phase B systems（PreUpdate: register/unregister; FixedUpdate.FlushBridgeEvents: flush_camera_ops）
/// - Phase A observers（cinematic on_add / on_remove）
/// - Phase A 主系統 chain（Update）
///
/// PreUpdate 系統（2 個，`.chain()`）：
/// ```text
/// bridge_camera_register → bridge_camera_unregister
/// ```
///
/// FixedUpdate 系統（1 個，於 `GameFixedSet::FlushBridgeEvents`）：
/// ```text
/// flush_camera_ops
/// ```
///
/// Update 系統鏈（`.chain()`，共 11 個系統）：
/// ```text
/// override_event_handler → shake_event_handler → override_stack_tick
/// → follow → look_ahead → update_previous_target → override_apply
/// → zoom → bounds → shake_apply → cursor
/// ```
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<MouseWheel>()
            .add_event::<events::CameraOverrideRequest>()
            .add_event::<events::CameraShakeRequest>()
            .init_resource::<CursorWorldPosition>()
            .init_resource::<shake::ShakeSeedCounter>()
            .init_resource::<camera_registry::CameraRegistry>()
            .add_observer(cinematic::on_cinematic_added)
            .add_observer(cinematic::on_cinematic_removed)
            .add_systems(
                PreUpdate,
                (
                    camera_registry::bridge_camera_register_system,
                    camera_registry::bridge_camera_unregister_system,
                )
                    .chain(),
            )
            .add_systems(
                FixedUpdate,
                camera_flush::flush_camera_ops.in_set(GameFixedSet::FlushBridgeEvents),
            )
            .add_systems(
                Update,
                (
                    override_stack::override_event_handler_system,
                    shake::shake_event_handler_system,
                    override_stack::override_stack_tick_system,
                    camera_follow_system,
                    camera_look_ahead_system,
                    update_previous_target_system,
                    override_stack::override_apply_system,
                    camera_zoom_system,
                    camera_bounds_system,
                    shake::shake_apply_system,
                    cursor_position_system,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use vm_bevy_bridge::SharedBridgeState;
    use vm_runtime::BridgeState;

    /// 建立測試用 App：含 SharedBridgeState、GameFixedSet configure、ManualDuration。
    fn build_test_app_with_fixed_update(secs: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(secs),
        ));
        vm_bevy_bridge::configure_game_fixed_set_for_tests(&mut app);
        app.insert_resource(SharedBridgeState(Arc::new(Mutex::new(BridgeState::new()))));
        app
    }

    #[test]
    fn camera_plugin_建構不_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(CameraPlugin);
        app.update();
    }

    #[test]
    fn camera_plugin_註冊_cursor_world_position_resource() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(CameraPlugin);
        app.update();

        assert!(
            app.world().get_resource::<CursorWorldPosition>().is_some(),
            "CursorWorldPosition resource 應存在"
        );
    }

    /// 驗證 CameraCinematic 存在時 follow/look-ahead 不執行。
    #[test]
    fn cinematic_接管_follow_不執行() {
        let mut app = build_test_app_with_fixed_update(1.0 / 60.0);
        app.add_plugins(CameraPlugin);
        app.update(); // 初始化

        // 目標在 (100, 100)
        app.world_mut()
            .spawn((CameraTarget, Transform::from_xyz(100.0, 100.0, 0.0)));

        // 攝影機在原點 + CameraCinematic 指向 (500, 500)
        let cam = app
            .world_mut()
            .spawn((
                CameraFollow::default(),
                CameraLookAhead::default(),
                PreviousTargetPosition::default(),
                CameraCinematic {
                    target_position: Vec2::new(500.0, 500.0),
                    target_zoom: 720.0,
                    speed: DEFAULT_CINEMATIC_SPEED,
                    priority: 1000,
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        app.update();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();

        assert!(
            pos.x > 0.0 && pos.y > 0.0,
            "cinematic 應朝 (500,500) 移動，實際: {pos:?}"
        );
        assert!(
            pos.x > 30.0,
            "cinematic 目標 500 vs follow 目標 100，位置應 > 30，實際: {}",
            pos.x
        );
    }

    /// cinematic + bounds：cinematic 目標在 bounds 外 → 被 clamp。
    #[test]
    fn cinematic_受_bounds_限制() {
        let mut app = build_test_app_with_fixed_update(1.0 / 60.0);
        app.add_plugins(CameraPlugin);
        app.update();

        let cam = app
            .world_mut()
            .spawn((
                CameraCinematic {
                    target_position: Vec2::new(9999.0, 9999.0),
                    target_zoom: 720.0,
                    speed: 100.0,
                    priority: 1000,
                },
                CameraBounds {
                    min: Vec2::new(-100.0, -100.0),
                    max: Vec2::new(100.0, 100.0),
                    clamp_shake: false,
                },
                Projection::Orthographic(bevy::render::camera::OrthographicProjection {
                    scaling_mode: bevy::render::camera::ScalingMode::FixedVertical {
                        viewport_height: 100.0,
                    },
                    ..bevy::render::camera::OrthographicProjection::default_2d()
                }),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        for _ in 0..60 {
            app.update();
        }

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();

        // half_h = 50, half_w ≈ 88.9
        // Y clamp: max = 100 - 50 = 50
        // X clamp: max = 100 - 88.9 ≈ 11.1
        assert!(
            pos.x <= 12.0 && pos.y <= 51.0,
            "cinematic 目標在 bounds 外應被 clamp，實際: {pos:?}"
        );
    }

    /// 驗證移除 CameraCinematic 後 follow 恢復。
    #[test]
    fn cinematic_移除後_follow_恢復() {
        let mut app = build_test_app_with_fixed_update(1.0 / 60.0);
        app.add_plugins(CameraPlugin);
        app.update();

        app.world_mut()
            .spawn((CameraTarget, Transform::from_xyz(100.0, 0.0, 0.0)));

        let cam = app
            .world_mut()
            .spawn((
                CameraFollow { speed: 100.0 },
                PreviousTargetPosition::default(),
                CameraCinematic {
                    target_position: Vec2::new(-500.0, 0.0),
                    target_zoom: 720.0,
                    speed: DEFAULT_CINEMATIC_SPEED,
                    priority: 1000,
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        app.update();

        let pos_cinematic = app.world().get::<Transform>(cam).unwrap().translation.x;
        assert!(
            pos_cinematic < 0.0,
            "cinematic 目標 -500，位置應 < 0，實際: {pos_cinematic}"
        );

        app.world_mut().entity_mut(cam).remove::<CameraCinematic>();

        for _ in 0..60 {
            app.update();
        }

        let pos_follow = app.world().get::<Transform>(cam).unwrap().translation.x;
        assert!(
            (pos_follow - 100.0).abs() < 1.0,
            "移除 cinematic 後應追蹤目標 100，實際: {pos_follow}"
        );
    }

    /// 整合：override push + shake add 同時生效。
    #[test]
    fn override_加_shake_同時生效() {
        let mut app = build_test_app_with_fixed_update(1.0 / 60.0);
        app.add_plugins(CameraPlugin);
        app.update();

        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                CameraShake::default(),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        app.world_mut()
            .resource_mut::<Events<CameraOverrideRequest>>()
            .send(CameraOverrideRequest {
                camera: Some(cam),
                op: CameraOverrideOp::Push(PushOverrideParams {
                    target_position: Vec2::new(500.0, 0.0),
                    target_zoom: None,
                    speed: 5.0,
                    priority: 50,
                    duration: None,
                }),
            });
        app.world_mut()
            .resource_mut::<Events<CameraShakeRequest>>()
            .send(CameraShakeRequest {
                camera: Some(cam),
                op: CameraShakeOp::Add(ShakeParams {
                    trauma: 1.0,
                    max_strength: 10.0,
                    decay_rate: 0.0,
                    ..ShakeParams::default()
                }),
            });

        for _ in 0..30 {
            app.update();
        }

        let t = app.world().get::<Transform>(cam).unwrap();
        assert!(
            t.translation.x > 100.0,
            "應朝 override target(500) lerp，實際: {}",
            t.translation.x
        );
    }

    /// 整合：plugin 註冊後 Events 與 Resources 皆存在。
    #[test]
    fn plugin_註冊新_events_與_resources() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(CameraPlugin);
        app.update();

        assert!(
            app.world()
                .get_resource::<Events<CameraOverrideRequest>>()
                .is_some(),
            "CameraOverrideRequest event 應已註冊"
        );
        assert!(
            app.world()
                .get_resource::<Events<CameraShakeRequest>>()
                .is_some(),
            "CameraShakeRequest event 應已註冊"
        );
        assert!(
            app.world().get_resource::<ShakeSeedCounter>().is_some(),
            "ShakeSeedCounter resource 應已存在"
        );
    }

    /// 整合：override + shake + bounds(clamp_shake=true) → shake 被 clamp。
    #[test]
    fn override_加_shake_加_bounds_clamp_shake_true_受限() {
        let mut app = build_test_app_with_fixed_update(1.0 / 60.0);
        app.add_plugins(CameraPlugin);
        app.update();

        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                CameraShake::default(),
                CameraBounds {
                    min: Vec2::new(-1.0, -1.0),
                    max: Vec2::new(1.0, 1.0),
                    clamp_shake: true,
                },
                Projection::Orthographic(bevy::render::camera::OrthographicProjection {
                    scaling_mode: bevy::render::camera::ScalingMode::FixedVertical {
                        viewport_height: 0.5,
                    },
                    ..bevy::render::camera::OrthographicProjection::default_2d()
                }),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        app.world_mut()
            .resource_mut::<Events<CameraShakeRequest>>()
            .send(CameraShakeRequest {
                camera: Some(cam),
                op: CameraShakeOp::Add(ShakeParams {
                    trauma: 1.0,
                    max_strength: 100.0,
                    decay_rate: 0.0,
                    ..ShakeParams::default()
                }),
            });

        for _ in 0..10 {
            app.update();
            let t = app.world().get::<Transform>(cam).unwrap();
            assert!(
                t.translation.x.abs() <= 0.57,
                "clamp_shake=true 時 x 應 ≤ 0.556，實際: {}",
                t.translation.x
            );
            assert!(
                t.translation.y.abs() <= 0.76,
                "clamp_shake=true 時 y 應 ≤ 0.75，實際: {}",
                t.translation.y
            );
        }
    }

    /// 整合：follow 基底 + override 覆寫 — 驗證系統鏈順序（follow 寫入 → override 覆蓋）。
    #[test]
    fn follow_與_override_鏈順序_override_覆蓋() {
        let mut app = build_test_app_with_fixed_update(1.0 / 60.0);
        app.add_plugins(CameraPlugin);
        app.update();

        app.world_mut()
            .spawn((CameraTarget, Transform::from_xyz(0.0, 0.0, 0.0)));

        let cam = app
            .world_mut()
            .spawn((
                CameraFollow::default(),
                PreviousTargetPosition::default(),
                CameraOverrideStack::default(),
                Transform::from_xyz(-500.0, 0.0, 0.0),
            ))
            .id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::new(1000.0, 0.0),
                target_zoom: None,
                speed: 5.0,
                priority: 100,
                duration: None,
            });
        }

        for _ in 0..60 {
            app.update();
        }

        let t = app.world().get::<Transform>(cam).unwrap();
        assert!(
            t.translation.x > 0.0,
            "override 目標 +1000，follow 目標 0，最終 x 應為正，實際: {}",
            t.translation.x
        );
    }
}
