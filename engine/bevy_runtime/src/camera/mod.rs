//! 遊戲攝影機系統 — ECS-heavy component-driven 架構。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md`
//!
//! # 架構
//! - 每個功能為獨立 Component，掛在攝影機 entity 上按需組合
//! - 7 個系統在 `Update` 以 `.chain()` 保證順序
//! - 所有計算使用 native `f32`（渲染層，不受 SoftF32 約束）

pub mod bounds;
pub mod cinematic;
pub mod components;
pub mod cursor;
pub mod follow;
pub mod zoom;

use bevy::prelude::*;

pub use bounds::camera_bounds_system;
pub use cinematic::camera_cinematic_system;
pub use components::{
    CameraBounds, CameraCinematic, CameraFollow, CameraLookAhead, CameraTarget, CameraZoom,
    PreviousTargetPosition, DEFAULT_CINEMATIC_SPEED, DEFAULT_FOLLOW_SPEED,
    DEFAULT_LOOK_AHEAD_MAX_OFFSET, DEFAULT_MOUSE_WEIGHT, DEFAULT_VELOCITY_WEIGHT,
    DEFAULT_VIEWPORT_HEIGHT, DEFAULT_ZOOM_MAX, DEFAULT_ZOOM_MIN, DEFAULT_ZOOM_SCROLL_SPEED,
    DEFAULT_ZOOM_SPEED,
};
pub use cursor::{cursor_position_system, viewport_to_world, CursorWorldPosition};
pub use follow::{camera_follow_system, camera_look_ahead_system, update_previous_target_system};
pub use zoom::camera_zoom_system;

/// 攝影機系統 Plugin。
///
/// 系統鏈（`Update`, `.chain()`）：
/// ```text
/// follow → look_ahead → update_previous_target → cinematic → zoom → bounds → cursor
/// ```
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CursorWorldPosition>().add_systems(
            Update,
            (
                camera_follow_system,
                camera_look_ahead_system,
                update_previous_target_system,
                camera_cinematic_system,
                camera_zoom_system,
                camera_bounds_system,
                cursor_position_system,
            )
                .chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_plugin_建構不_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<bevy::input::mouse::MouseWheel>();
        app.add_plugins(CameraPlugin);
        app.update();
    }

    #[test]
    fn camera_plugin_註冊_cursor_world_position_resource() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<bevy::input::mouse::MouseWheel>();
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
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<bevy::input::mouse::MouseWheel>();
        app.add_plugins(CameraPlugin);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1.0 / 60.0),
        ));
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
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<bevy::input::mouse::MouseWheel>();
        app.add_plugins(CameraPlugin);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1.0 / 60.0),
        ));
        app.update();

        let cam = app
            .world_mut()
            .spawn((
                CameraCinematic {
                    target_position: Vec2::new(9999.0, 9999.0),
                    target_zoom: 720.0,
                    speed: 100.0,
                },
                CameraBounds {
                    min: Vec2::new(-100.0, -100.0),
                    max: Vec2::new(100.0, 100.0),
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
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<bevy::input::mouse::MouseWheel>();
        app.add_plugins(CameraPlugin);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1.0 / 60.0),
        ));
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
}
