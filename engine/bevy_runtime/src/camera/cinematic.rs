//! Cinematic 演出模式系統。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md` §6.3

use bevy::prelude::*;

use super::components::{CameraCinematic, CameraZoom};

/// Update 系統：Cinematic 模式下控制攝影機位置與 zoom。
pub fn camera_cinematic_system(
    time: Res<Time>,
    mut query: Query<(&CameraCinematic, &mut Transform, Option<&mut CameraZoom>)>,
) {
    let dt = time.delta_secs().min(0.1);

    for (cinematic, mut transform, zoom) in query.iter_mut() {
        let factor = 1.0 - (-cinematic.speed * dt).exp();
        let current = transform.translation.truncate();
        let new_pos = current.lerp(cinematic.target_position, factor);
        transform.translation.x = new_pos.x;
        transform.translation.y = new_pos.y;

        tracing::debug!(
            "Cinematic 過渡：位置 ({:.1}, {:.1})，factor {:.3}",
            transform.translation.x,
            transform.translation.y,
            factor
        );

        if let Some(mut zoom) = zoom {
            zoom.target = cinematic.target_zoom;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::components::DEFAULT_CINEMATIC_SPEED;
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn build_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            1.0 / 60.0,
        )));
        app
    }

    #[test]
    fn cinematic_移動至目標位置() {
        let mut app = build_test_app();
        app.add_systems(Update, camera_cinematic_system);

        let cam = app
            .world_mut()
            .spawn((
                CameraCinematic {
                    target_position: Vec2::new(500.0, 300.0),
                    target_zoom: 720.0,
                    speed: DEFAULT_CINEMATIC_SPEED,
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        for _ in 0..120 {
            app.update();
        }

        let cam_pos = app
            .world()
            .get::<Transform>(cam)
            .unwrap()
            .translation
            .truncate();
        let dist = cam_pos.distance(Vec2::new(500.0, 300.0));
        assert!(dist < 0.1, "120 幀後應趨近目標，實際距離: {dist}");
    }

    #[test]
    fn cinematic_設定_zoom_target() {
        let mut app = build_test_app();

        app.world_mut().spawn((
            CameraCinematic {
                target_position: Vec2::ZERO,
                target_zoom: 500.0,
                speed: DEFAULT_CINEMATIC_SPEED,
            },
            CameraZoom::default(),
            Transform::default(),
        ));

        app.world_mut()
            .run_system_once(camera_cinematic_system)
            .unwrap();

        let zoom = app
            .world_mut()
            .query::<&CameraZoom>()
            .iter(app.world())
            .next()
            .unwrap();
        assert_eq!(zoom.target, 500.0, "cinematic 應設定 zoom.target");
    }

    #[test]
    fn cinematic_無_camera_zoom_不_panic() {
        let mut app = build_test_app();

        app.world_mut().spawn((
            CameraCinematic {
                target_position: Vec2::new(100.0, 0.0),
                target_zoom: 500.0,
                speed: DEFAULT_CINEMATIC_SPEED,
            },
            Transform::default(),
        ));

        app.world_mut()
            .run_system_once(camera_cinematic_system)
            .unwrap();
    }
}
