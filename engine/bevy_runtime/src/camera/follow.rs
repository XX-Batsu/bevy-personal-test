//! 攝影機追蹤與 look-ahead 系統。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md` §6.1, §6.2, §6.2.1

use bevy::prelude::*;

use super::components::{
    CameraCinematic, CameraFollow, CameraLookAhead, CameraTarget, PreviousTargetPosition,
};
use super::cursor::CursorWorldPosition;

/// 計算 frame-rate-independent 衰減因子。
/// `dt` 已由呼叫者 cap 至 0.1。
fn decay_factor(speed: f32, dt: f32) -> f32 {
    1.0 - (-speed * dt).exp()
}

/// Update 系統：平滑追蹤 `CameraTarget` entity。
/// Cinematic 模式時跳過（`Without<CameraCinematic>`）。
#[allow(clippy::type_complexity)]
pub fn camera_follow_system(
    time: Res<Time>,
    target_query: Query<&Transform, With<CameraTarget>>,
    mut camera_query: Query<
        (&CameraFollow, &mut Transform),
        (Without<CameraCinematic>, Without<CameraTarget>),
    >,
) {
    let mut iter = target_query.iter();
    let Some(target_transform) = iter.next() else {
        bevy::utils::warn_once!("找不到 CameraTarget entity");
        return;
    };
    if iter.next().is_some() {
        bevy::utils::warn_once!("偵測到多個 CameraTarget entity，取第一個");
    }
    let target_pos = target_transform.translation.truncate();

    let dt = time.delta_secs().min(0.1);

    for (follow, mut cam_transform) in camera_query.iter_mut() {
        let factor = decay_factor(follow.speed, dt);
        let current = cam_transform.translation.truncate();
        let new_pos = current.lerp(target_pos, factor);
        cam_transform.translation.x = new_pos.x;
        cam_transform.translation.y = new_pos.y;
        tracing::debug!("攝影機追蹤：位置 ({:.1}, {:.1})", new_pos.x, new_pos.y);
    }
}

/// Update 系統：疊加 look-ahead 偏移。
/// 在 `camera_follow_system` 之後執行。
#[allow(clippy::type_complexity)]
pub fn camera_look_ahead_system(
    cursor_world: Res<CursorWorldPosition>,
    target_query: Query<&Transform, With<CameraTarget>>,
    mut camera_query: Query<
        (&CameraLookAhead, &PreviousTargetPosition, &mut Transform),
        (
            With<CameraFollow>,
            Without<CameraCinematic>,
            Without<CameraTarget>,
        ),
    >,
) {
    let Some(target_transform) = target_query.iter().next() else {
        return;
    };
    let target_pos = target_transform.translation.truncate();

    for (look_ahead, prev, mut cam_transform) in camera_query.iter_mut() {
        let velocity = target_pos - prev.position.unwrap_or(target_pos);
        let mouse = cursor_world.position.unwrap_or(target_pos) - target_pos;
        let offset = velocity * look_ahead.velocity_weight + mouse * look_ahead.mouse_weight;
        let offset = if offset.length() > look_ahead.max_offset {
            offset.normalize_or_zero() * look_ahead.max_offset
        } else {
            offset
        };
        cam_transform.translation.x += offset.x;
        cam_transform.translation.y += offset.y;
    }
}

/// Update 系統：更新 `PreviousTargetPosition` 為本幀的目標位置。
/// 排在 `camera_look_ahead_system` 之後，確保 look-ahead 讀到上一幀的值。
pub fn update_previous_target_system(
    target_query: Query<&Transform, With<CameraTarget>>,
    mut camera_query: Query<&mut PreviousTargetPosition>,
) {
    let Some(target_transform) = target_query.iter().next() else {
        return;
    };
    let target_pos = target_transform.translation.truncate();

    for mut prev in camera_query.iter_mut() {
        prev.position = Some(target_pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn build_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<CursorWorldPosition>();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            1.0 / 60.0,
        )));
        app
    }

    fn spawn_target(app: &mut App, pos: Vec2) -> Entity {
        app.world_mut()
            .spawn((CameraTarget, Transform::from_xyz(pos.x, pos.y, 0.0)))
            .id()
    }

    fn spawn_camera_with_follow(app: &mut App, pos: Vec2, speed: f32) -> Entity {
        app.world_mut()
            .spawn((
                CameraFollow { speed },
                PreviousTargetPosition::default(),
                Transform::from_xyz(pos.x, pos.y, 0.0),
            ))
            .id()
    }

    // ── follow 收斂 ────────────────────────────────────────

    #[test]
    fn follow_60幀後趨近目標() {
        let mut app = build_test_app();
        app.add_systems(Update, camera_follow_system);
        spawn_target(&mut app, Vec2::new(100.0, 100.0));
        let cam = spawn_camera_with_follow(&mut app, Vec2::ZERO, 8.0);

        for _ in 0..60 {
            app.update();
        }

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        let dist = cam_pos.distance(Vec2::new(100.0, 100.0));
        assert!(dist < 0.1, "60 幀後攝影機應趨近目標，實際距離: {dist}");
    }

    #[test]
    fn follow_speed_很大值_一幀近乎瞬移() {
        let mut app = build_test_app();
        app.add_systems(Update, camera_follow_system);
        spawn_target(&mut app, Vec2::new(100.0, 0.0));
        let cam = spawn_camera_with_follow(&mut app, Vec2::ZERO, 1000.0);

        app.update(); // Frame 0：初始化時間（delta=0）
        app.update(); // Frame 1：ManualDuration 生效，觸發移動

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        let dist = cam_pos.distance(Vec2::new(100.0, 0.0));
        assert!(dist < 0.01, "speed=1000 應近乎瞬移，實際距離: {dist}");
    }

    #[test]
    fn follow_speed_零_不移動() {
        let mut app = build_test_app();
        spawn_target(&mut app, Vec2::new(100.0, 0.0));
        let cam = spawn_camera_with_follow(&mut app, Vec2::ZERO, 0.0);

        app.world_mut()
            .run_system_once(camera_follow_system)
            .unwrap();

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            cam_pos.abs_diff_eq(Vec2::ZERO, 1e-5),
            "speed=0 攝影機不應移動，實際: {cam_pos:?}"
        );
    }

    #[test]
    fn follow_無_camera_target_不_panic() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_follow(&mut app, Vec2::ZERO, 8.0);

        app.world_mut()
            .run_system_once(camera_follow_system)
            .unwrap();

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            cam_pos.abs_diff_eq(Vec2::ZERO, 1e-5),
            "無目標時攝影機不應移動"
        );
    }

    // ── frame-rate independence ───────────────────────────

    #[test]
    fn follow_frame_rate_independence() {
        let target = Vec2::new(100.0, 0.0);
        let speed = 8.0;

        let mut pos_30 = Vec2::ZERO;
        for _ in 0..30 {
            let dt: f32 = 1.0 / 30.0;
            let factor = 1.0 - (-speed * dt).exp();
            pos_30 = pos_30.lerp(target, factor);
        }

        let mut pos_120 = Vec2::ZERO;
        for _ in 0..120 {
            let dt: f32 = 1.0 / 120.0;
            let factor = 1.0 - (-speed * dt).exp();
            pos_120 = pos_120.lerp(target, factor);
        }

        let diff = (pos_30.x - pos_120.x).abs();
        assert!(
            diff < 0.01,
            "30fps 和 120fps 一秒後位置差異應 < 0.01，實際: {diff}"
        );
    }

    // ── dt cap ──────────────────────────────────────────────

    #[test]
    fn decay_factor_dt_cap_不超過預期值() {
        let speed = 8.0;
        let dt_capped = 0.1;
        let factor = decay_factor(speed, dt_capped);
        assert!(
            factor < 0.56,
            "dt=0.1（cap）時 factor 應 < 0.56，實際: {factor}"
        );
    }

    // ── look-ahead 偏移方向 ─────────────────────────────────

    #[test]
    fn look_ahead_滑鼠在目標右方_攝影機偏右() {
        let mut app = build_test_app();
        let target_pos = Vec2::new(50.0, 50.0);
        spawn_target(&mut app, target_pos);

        app.insert_resource(CursorWorldPosition {
            position: Some(Vec2::new(200.0, 50.0)),
        });

        let cam = app
            .world_mut()
            .spawn((
                CameraFollow::default(),
                CameraLookAhead::default(),
                PreviousTargetPosition {
                    position: Some(target_pos),
                },
                Transform::from_xyz(target_pos.x, target_pos.y, 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(camera_look_ahead_system)
            .unwrap();

        let cam_x = app.world().get::<Transform>(cam).unwrap().translation.x;
        assert!(
            cam_x > target_pos.x,
            "滑鼠在右方，攝影機 x 應偏右。cam_x={cam_x}, target_x={}",
            target_pos.x
        );
    }

    #[test]
    fn look_ahead_不超過_max_offset() {
        let mut app = build_test_app();
        let target_pos = Vec2::ZERO;
        spawn_target(&mut app, target_pos);

        app.insert_resource(CursorWorldPosition {
            position: Some(Vec2::new(99999.0, 99999.0)),
        });

        let cam = app
            .world_mut()
            .spawn((
                CameraFollow::default(),
                CameraLookAhead {
                    max_offset: 120.0,
                    ..CameraLookAhead::default()
                },
                PreviousTargetPosition {
                    position: Some(target_pos),
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(camera_look_ahead_system)
            .unwrap();

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            cam_pos.length() <= 120.0 + 0.01,
            "偏移不應超過 max_offset=120，實際: {}",
            cam_pos.length()
        );
    }

    #[test]
    fn look_ahead_cursor_none_不_panic_無偏移() {
        let mut app = build_test_app();
        let target_pos = Vec2::ZERO;
        spawn_target(&mut app, target_pos);

        app.insert_resource(CursorWorldPosition { position: None });

        let cam = app
            .world_mut()
            .spawn((
                CameraFollow::default(),
                CameraLookAhead::default(),
                PreviousTargetPosition {
                    position: Some(target_pos),
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(camera_look_ahead_system)
            .unwrap();

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            cam_pos.abs_diff_eq(Vec2::ZERO, 1e-5),
            "cursor None 且目標靜止時不應有偏移，實際: {cam_pos:?}"
        );
    }

    #[test]
    fn look_ahead_previous_none_velocity_為零() {
        let mut app = build_test_app();
        let target_pos = Vec2::new(50.0, 50.0);
        spawn_target(&mut app, target_pos);

        app.insert_resource(CursorWorldPosition {
            position: Some(target_pos),
        });

        let cam = app
            .world_mut()
            .spawn((
                CameraFollow::default(),
                CameraLookAhead::default(),
                PreviousTargetPosition { position: None },
                Transform::from_xyz(target_pos.x, target_pos.y, 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(camera_look_ahead_system)
            .unwrap();

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            cam_pos.abs_diff_eq(target_pos, 1e-5),
            "首幀 velocity=0 + mouse 在目標上 → 無偏移，實際: {cam_pos:?}"
        );
    }

    #[test]
    fn look_ahead_無_camera_follow_不執行() {
        let mut app = build_test_app();
        let target_pos = Vec2::new(50.0, 50.0);
        spawn_target(&mut app, target_pos);

        app.insert_resource(CursorWorldPosition {
            position: Some(Vec2::new(200.0, 50.0)),
        });

        let cam = app
            .world_mut()
            .spawn((
                CameraLookAhead::default(),
                PreviousTargetPosition {
                    position: Some(target_pos),
                },
                Transform::from_xyz(target_pos.x, target_pos.y, 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(camera_look_ahead_system)
            .unwrap();

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        assert!(
            cam_pos.abs_diff_eq(target_pos, 1e-5),
            "無 CameraFollow 時 look-ahead 不應執行，實際: {cam_pos:?}"
        );
    }

    // ── update_previous_target ──────────────────────────────

    #[test]
    fn update_previous_target_記錄本幀位置() {
        let mut app = build_test_app();
        let target_pos = Vec2::new(42.0, 99.0);
        spawn_target(&mut app, target_pos);

        let cam = app
            .world_mut()
            .spawn(PreviousTargetPosition::default())
            .id();

        app.world_mut()
            .run_system_once(update_previous_target_system)
            .unwrap();

        let prev = app.world().get::<PreviousTargetPosition>(cam).unwrap();
        assert_eq!(prev.position, Some(target_pos));
    }

    #[test]
    fn update_previous_target_無目標時不更新() {
        let mut app = build_test_app();

        let cam = app
            .world_mut()
            .spawn(PreviousTargetPosition::default())
            .id();

        app.world_mut()
            .run_system_once(update_previous_target_system)
            .unwrap();

        let prev = app.world().get::<PreviousTargetPosition>(cam).unwrap();
        assert!(prev.position.is_none());
    }
}
