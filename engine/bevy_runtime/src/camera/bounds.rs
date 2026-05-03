//! 攝影機邊界限制系統。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md` §6.5

use bevy::prelude::*;
use bevy::render::camera::ScalingMode;

use super::components::CameraBounds;

/// 目標輸出的固定畫面長寬比（架構規格 §18）。
/// bounds 系統使用此常數將 viewport_height 換算為 viewport_width。
pub(crate) const ASPECT_RATIO: f32 = 16.0 / 9.0;

/// Update 系統：clamp 攝影機位置在 `CameraBounds` 範圍內。
/// 無 `CameraBounds` component 的攝影機不受限制。
/// Cinematic 模式也受 bounds 限制（無 `Without<CameraCinematic>` filter）。
pub fn camera_bounds_system(mut query: Query<(&CameraBounds, &mut Transform, &Projection)>) {
    for (bounds, mut transform, projection) in query.iter_mut() {
        if bounds.min.x > bounds.max.x || bounds.min.y > bounds.max.y {
            bevy::utils::warn_once!(
                "CameraBounds min > max（min={:?}, max={:?}），跳過 clamp",
                bounds.min,
                bounds.max
            );
            continue;
        }

        let (half_w, half_h) = viewport_half_size(projection);
        clamp_to_bounds(&mut transform.translation, bounds, half_w, half_h);
    }
}

/// 將 transform.translation 在 (X, Y) 軸上 clamp 至 bounds 範圍內。
/// 給定 viewport 的半寬/半高，計算允許的中心點區間：
/// - 若 bounds 寬度 ≤ viewport 寬度（攝影機比地圖大）→ 置中
/// - 否則 clamp 至 [bounds.min + half, bounds.max - half]
///
/// 共用 helper：`camera_bounds_system` 與 `shake_apply_system`（clamp_shake=true）皆呼叫此函式，
/// 避免兩處 clamp 邏輯漂移（spec §4.3）。
///
/// 呼叫端須先確認 `bounds.min <= bounds.max`（無效範圍直接跳過）。
pub(crate) fn clamp_to_bounds(
    translation: &mut Vec3,
    bounds: &CameraBounds,
    half_w: f32,
    half_h: f32,
) {
    let bounds_width = bounds.max.x - bounds.min.x;
    if bounds_width <= half_w * 2.0 {
        translation.x = (bounds.min.x + bounds.max.x) / 2.0;
    } else {
        translation.x = translation
            .x
            .clamp(bounds.min.x + half_w, bounds.max.x - half_w);
    }
    let bounds_height = bounds.max.y - bounds.min.y;
    if bounds_height <= half_h * 2.0 {
        translation.y = (bounds.min.y + bounds.max.y) / 2.0;
    } else {
        translation.y = translation
            .y
            .clamp(bounds.min.y + half_h, bounds.max.y - half_h);
    }
}

/// 從 Projection 取得 viewport 半寬/半高。
pub(crate) fn viewport_half_size(projection: &Projection) -> (f32, f32) {
    if let Projection::Orthographic(ortho) = projection {
        let half_h = match ortho.scaling_mode {
            ScalingMode::FixedVertical { viewport_height } => viewport_height / 2.0,
            _ => ortho.area.height() / 2.0,
        };
        // 本遊戲固定輸出 16:9（架構規格 §18），FixedVertical 模式寬度 = viewport_height × ASPECT_RATIO。
        // 若需支援非 16:9（如編輯器、分割螢幕），改為 ortho.area.width() / 2.0（渲染後才有值）。
        let half_w = half_h * ASPECT_RATIO;
        (half_w, half_h)
    } else {
        (0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::render::camera::OrthographicProjection;

    fn build_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app
    }

    fn spawn_camera_with_bounds(
        app: &mut App,
        cam_pos: Vec2,
        bounds_min: Vec2,
        bounds_max: Vec2,
        viewport_height: f32,
    ) -> Entity {
        app.world_mut()
            .spawn((
                CameraBounds {
                    min: bounds_min,
                    max: bounds_max,
                    clamp_shake: false,
                },
                Projection::Orthographic(OrthographicProjection {
                    scaling_mode: ScalingMode::FixedVertical { viewport_height },
                    ..OrthographicProjection::default_2d()
                }),
                Transform::from_xyz(cam_pos.x, cam_pos.y, 0.0),
            ))
            .id()
    }

    #[test]
    fn bounds_clamp_攝影機超出邊界() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_bounds(
            &mut app,
            Vec2::new(9999.0, 9999.0),
            Vec2::new(-500.0, -500.0),
            Vec2::new(500.0, 500.0),
            720.0,
        );

        app.world_mut()
            .run_system_once(camera_bounds_system)
            .unwrap();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();

        // half_h = 360, half_w = 640
        // bounds 寬 1000, viewport 寬 1280 → X 軸置中 = 0.0
        // bounds 高 1000, viewport 高 720 → clamp Y 至 500-360=140
        assert!(
            pos.x.abs() < 0.01,
            "X 軸 viewport>bounds 應置中，實際: {}",
            pos.x
        );
        assert!(
            (pos.y - 140.0).abs() < 0.01,
            "Y 軸應被 clamp 至 max-half_h=140（精確值），實際: {}",
            pos.y
        );
    }

    #[test]
    fn bounds_viewport_大於_bounds_置中() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_bounds(
            &mut app,
            Vec2::new(100.0, 100.0),
            Vec2::new(-50.0, -50.0),
            Vec2::new(50.0, 50.0),
            720.0,
        );

        app.world_mut()
            .run_system_once(camera_bounds_system)
            .unwrap();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            pos.abs_diff_eq(Vec2::ZERO, 0.01),
            "viewport > bounds 時應置中，實際: {pos:?}"
        );
    }

    #[test]
    fn bounds_min_等於_max_置中() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_bounds(
            &mut app,
            Vec2::new(100.0, 100.0),
            Vec2::new(50.0, 50.0),
            Vec2::new(50.0, 50.0),
            720.0,
        );

        app.world_mut()
            .run_system_once(camera_bounds_system)
            .unwrap();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            pos.abs_diff_eq(Vec2::new(50.0, 50.0), 0.01),
            "min==max 時應置中在該點，實際: {pos:?}"
        );
    }

    #[test]
    fn bounds_攝影機在邊界內_不移動() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_bounds(
            &mut app,
            Vec2::new(0.0, 0.0),
            Vec2::new(-5000.0, -5000.0),
            Vec2::new(5000.0, 5000.0),
            720.0,
        );

        app.world_mut()
            .run_system_once(camera_bounds_system)
            .unwrap();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            pos.abs_diff_eq(Vec2::ZERO, 0.01),
            "在大邊界內攝影機不應移動，實際: {pos:?}"
        );
    }

    #[test]
    fn bounds_clamp_攝影機超出負方向邊界() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_bounds(
            &mut app,
            Vec2::new(-9999.0, -9999.0),
            Vec2::new(-500.0, -500.0),
            Vec2::new(500.0, 500.0),
            720.0,
        );

        app.world_mut()
            .run_system_once(camera_bounds_system)
            .unwrap();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();

        // half_h = 360, half_w = 640
        // bounds 寬 1000, viewport 寬 1280 → X 軸置中 = 0.0
        // bounds 高 1000, viewport 高 720 → clamp Y 至 -500+360=-140
        assert!(
            pos.x.abs() < 0.01,
            "X 軸 viewport>bounds 應置中，實際: {}",
            pos.x
        );
        assert!(
            (pos.y - (-140.0)).abs() < 0.01,
            "Y 軸應被 clamp 至 min+half_h=-140，實際: {}",
            pos.y
        );
    }

    #[test]
    fn bounds_無_camera_bounds_不受限制() {
        let mut app = build_test_app();

        // 不掛 CameraBounds — 只有 Projection + Transform
        let cam = app
            .world_mut()
            .spawn((
                Projection::Orthographic(OrthographicProjection {
                    scaling_mode: ScalingMode::FixedVertical {
                        viewport_height: 720.0,
                    },
                    ..OrthographicProjection::default_2d()
                }),
                Transform::from_xyz(99999.0, 99999.0, 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(camera_bounds_system)
            .unwrap();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            (pos.x - 99999.0).abs() < 0.01 && (pos.y - 99999.0).abs() < 0.01,
            "無 CameraBounds 時位置不應被修改，實際: {pos:?}"
        );
    }

    #[test]
    fn bounds_min_大於_max_不_panic_跳過_clamp() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_bounds(
            &mut app,
            Vec2::new(50.0, 50.0),
            Vec2::new(100.0, 100.0), // min > max（配置錯誤）
            Vec2::new(-100.0, -100.0),
            720.0,
        );

        app.world_mut()
            .run_system_once(camera_bounds_system)
            .unwrap();

        let pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        // min > max 時跳過 clamp，位置不應改變
        assert!(
            pos.abs_diff_eq(Vec2::new(50.0, 50.0), 0.01),
            "min > max 時應跳過 clamp，位置不應改變，實際: {pos:?}"
        );
    }
}
