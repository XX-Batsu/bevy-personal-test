//! 攝影機縮放系統。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md` §6.4

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::camera::ScalingMode;

use super::components::CameraZoom;

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

        // clamp target
        zoom.target = zoom.target.clamp(zoom.min, zoom.max);

        // lerp current → target
        let factor = 1.0 - (-zoom.speed * dt).exp();
        zoom.current = zoom.current + (zoom.target - zoom.current) * factor;

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
}
