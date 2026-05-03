//! 攝影機縮放系統。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md` §6.4

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::camera::ScalingMode;

use super::components::{decay_factor, CameraZoom};

/// Update 系統：處理滾輪輸入與 zoom lerp。
pub fn camera_zoom_system(
    time: Res<Time>,
    mut wheel_events: EventReader<MouseWheel>,
    mut query: Query<(&mut CameraZoom, &mut Projection)>,
) {
    let dt = time.delta_secs().min(0.1);

    // 累加滾輪 delta
    let scroll_delta: f32 = wheel_events.read().map(|e| e.y).sum();

    for (mut zoom, mut projection) in query.iter_mut() {
        // 滾輪調整 target
        if scroll_delta.abs() > f32::EPSILON {
            zoom.target -= scroll_delta * zoom.scroll_speed;
            tracing::debug!(
                "Zoom target 變更：{:.1}（滾輪 delta {:.2}）",
                zoom.target,
                scroll_delta
            );
        }

        // clamp target（防禦 min > max 的配置錯誤）
        let clamp_min = zoom.min.min(zoom.max);
        let clamp_max = zoom.min.max(zoom.max);
        zoom.target = zoom.target.clamp(clamp_min, clamp_max);

        // lerp current → target
        let factor = decay_factor(zoom.speed, dt);
        zoom.current += (zoom.target - zoom.current) * factor;

        // 更新 Projection
        if let Projection::Orthographic(ref mut ortho) = *projection {
            ortho.scaling_mode = ScalingMode::FixedVertical {
                viewport_height: zoom.current,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::components::{DEFAULT_VIEWPORT_HEIGHT, DEFAULT_ZOOM_MAX, DEFAULT_ZOOM_MIN};
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::render::camera::OrthographicProjection;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn build_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<MouseWheel>();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            1.0 / 60.0,
        )));
        app
    }

    fn spawn_camera_with_zoom(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                CameraZoom::default(),
                Projection::Orthographic(OrthographicProjection {
                    scaling_mode: ScalingMode::FixedVertical {
                        viewport_height: DEFAULT_VIEWPORT_HEIGHT,
                    },
                    ..OrthographicProjection::default_2d()
                }),
                Transform::default(),
            ))
            .id()
    }

    #[test]
    fn zoom_滾輪_改變_target() {
        let mut app = build_test_app();
        spawn_camera_with_zoom(&mut app);

        app.world_mut().send_event(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });

        app.world_mut().run_system_once(camera_zoom_system).unwrap();

        let zoom = app
            .world_mut()
            .query::<&CameraZoom>()
            .iter(app.world())
            .next()
            .unwrap();

        // scroll_speed=60, y=1 → target -= 60 → 720 - 60 = 660
        assert!(
            (zoom.target - 660.0).abs() < 0.01,
            "zoom.target 應為 660，實際: {}",
            zoom.target
        );
    }

    #[test]
    fn zoom_clamp_不超出_min_max() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_zoom(&mut app);

        app.world_mut().get_mut::<CameraZoom>(cam).unwrap().target = 9999.0;

        app.world_mut().run_system_once(camera_zoom_system).unwrap();

        let zoom = app.world().get::<CameraZoom>(cam).unwrap();
        assert_eq!(zoom.target, DEFAULT_ZOOM_MAX, "target 應被 clamp 至 max");

        app.world_mut().get_mut::<CameraZoom>(cam).unwrap().target = 1.0;

        app.world_mut().run_system_once(camera_zoom_system).unwrap();

        let zoom = app.world().get::<CameraZoom>(cam).unwrap();
        assert_eq!(zoom.target, DEFAULT_ZOOM_MIN, "target 應被 clamp 至 min");
    }

    #[test]
    fn zoom_程式控制_lerp_收斂() {
        let mut app = build_test_app();
        app.add_systems(Update, camera_zoom_system);
        let cam = spawn_camera_with_zoom(&mut app);

        app.world_mut().get_mut::<CameraZoom>(cam).unwrap().target = 500.0;

        for _ in 0..120 {
            app.update();
        }

        let zoom = app.world().get::<CameraZoom>(cam).unwrap();
        assert!(
            (zoom.current - 500.0).abs() < 0.1,
            "120 幀後 current 應趨近 target=500，實際: {}",
            zoom.current
        );
    }

    #[test]
    fn zoom_更新_projection_scaling_mode() {
        let mut app = build_test_app();
        app.add_systems(Update, camera_zoom_system);
        let cam = spawn_camera_with_zoom(&mut app);

        app.world_mut().get_mut::<CameraZoom>(cam).unwrap().target = 500.0;

        for _ in 0..120 {
            app.update();
        }

        let projection = app.world().get::<Projection>(cam).unwrap();
        if let Projection::Orthographic(ortho) = projection {
            match ortho.scaling_mode {
                ScalingMode::FixedVertical { viewport_height } => {
                    assert!(
                        (viewport_height - 500.0).abs() < 0.1,
                        "Projection 的 viewport_height 應趨近 500，實際: {viewport_height}"
                    );
                }
                other => panic!("應為 FixedVertical，實際: {other:?}"),
            }
        } else {
            panic!("應為 Orthographic projection");
        }
    }

    #[test]
    fn zoom_反向滾輪_zoom_out() {
        let mut app = build_test_app();
        spawn_camera_with_zoom(&mut app);

        // y=-1 → target += scroll_speed → 720 + 60 = 780
        app.world_mut().send_event(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -1.0,
            window: Entity::PLACEHOLDER,
        });

        app.world_mut().run_system_once(camera_zoom_system).unwrap();

        let zoom = app
            .world_mut()
            .query::<&CameraZoom>()
            .iter(app.world())
            .next()
            .unwrap();
        assert!(
            (zoom.target - 780.0).abs() < 0.01,
            "反向滾輪 zoom.target 應為 780，實際: {}",
            zoom.target
        );
    }

    #[test]
    fn zoom_無滾輪事件_target_不變() {
        let mut app = build_test_app();
        let cam = spawn_camera_with_zoom(&mut app);

        // 不傳送任何 MouseWheel 事件
        app.world_mut().run_system_once(camera_zoom_system).unwrap();

        let zoom = app.world().get::<CameraZoom>(cam).unwrap();
        assert_eq!(
            zoom.target, DEFAULT_VIEWPORT_HEIGHT,
            "無滾輪事件時 target 不應改變"
        );
    }

    #[test]
    fn zoom_perspective_projection_不_panic() {
        let mut app = build_test_app();

        app.world_mut().spawn((
            CameraZoom::default(),
            Projection::Perspective(bevy::render::camera::PerspectiveProjection::default()),
            Transform::default(),
        ));

        // 不 panic 即通過（Perspective 分支不更新 scaling_mode）
        app.world_mut().run_system_once(camera_zoom_system).unwrap();
    }

    #[test]
    fn zoom_min_大於_max_不_panic() {
        let mut app = build_test_app();

        let cam = app
            .world_mut()
            .spawn((
                CameraZoom {
                    min: 1000.0, // min > max（配置錯誤）
                    max: 200.0,
                    ..CameraZoom::default()
                },
                Projection::Orthographic(OrthographicProjection {
                    scaling_mode: ScalingMode::FixedVertical {
                        viewport_height: DEFAULT_VIEWPORT_HEIGHT,
                    },
                    ..OrthographicProjection::default_2d()
                }),
                Transform::default(),
            ))
            .id();

        // 不 panic 即通過
        app.world_mut().run_system_once(camera_zoom_system).unwrap();

        let zoom = app.world().get::<CameraZoom>(cam).unwrap();
        // target 應被 clamp 在 [200, 1000] 範圍（系統自動修正 min/max 順序）
        assert!(
            zoom.target >= 200.0 && zoom.target <= 1000.0,
            "min > max 時 target 應被 clamp 在修正後範圍內，實際: {}",
            zoom.target
        );
    }
}
