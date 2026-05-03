//! flush_camera_ops system：將 BridgeState.camera_op_queue 的 CameraBridgeOp
//! 轉為 Phase A 的 CameraOverrideRequest / CameraShakeRequest events。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.8.5 / §6.3.2

use bevy::math::Vec2;
use bevy::prelude::*;
use bridge_types::CameraBridgeOp;

use super::camera_registry::CameraRegistry;
use super::events::{CameraOverrideOp, CameraOverrideRequest, CameraShakeOp, CameraShakeRequest};
use super::override_stack::PushOverrideParams;
use super::shake::ShakeParams;

use vm_bevy_bridge::SharedBridgeState;

/// Sentinel resolve：`zoom <= 0.0` → `None`（含 0 退化值，避免 zoom 設為 0）。
fn resolve_zoom(zoom: f32) -> Option<f32> {
    if zoom <= 0.0 {
        None
    } else {
        Some(zoom)
    }
}

/// Sentinel resolve：`duration < 0.0` → `None`（無期限）；`= 0` → `Some(0.0)`（立即過期）。
fn resolve_duration(duration: f32) -> Option<f32> {
    if duration < 0.0 {
        None
    } else {
        Some(duration)
    }
}

/// Sentinel resolve：`(0.0, 0.0)` → `None`（omnidirectional）；其餘 → `Some(Vec2)`。
fn resolve_direction(dir_x: f32, dir_y: f32) -> Option<Vec2> {
    if dir_x == 0.0 && dir_y == 0.0 {
        None
    } else {
        Some(Vec2::new(dir_x, dir_y))
    }
}

/// `FixedUpdate.FlushBridgeEvents` 系統：drain `BridgeState.camera_op_queue`，
/// resolve handle，發出 Phase A 的 `CameraOverrideRequest` / `CameraShakeRequest`。
///
/// 處理規則：
/// - `handle == 0` → broadcast（event.camera = None）
/// - `handle != 0` 且未註冊 → `warn_once!`，丟棄該 op
/// - sentinel 套用：zoom <= 0 → None；duration < 0 → None；(0,0) → None
pub fn flush_camera_ops(
    bridge_state: Res<SharedBridgeState>,
    registry: Res<CameraRegistry>,
    mut override_writer: EventWriter<CameraOverrideRequest>,
    mut shake_writer: EventWriter<CameraShakeRequest>,
) {
    let ops = {
        let mut state = bridge_state.0.lock().unwrap();
        state.camera_op_queue.drain()
    };

    for op in ops {
        let handle = match &op {
            CameraBridgeOp::Shake { handle, .. }
            | CameraBridgeOp::PushOverride { handle, .. }
            | CameraBridgeOp::PopOverride { handle }
            | CameraBridgeOp::ClearOverrides { handle }
            | CameraBridgeOp::ClearShakes { handle } => *handle,
        };

        let target: Option<Entity> = if handle == 0 {
            None
        } else {
            match registry.resolve(handle) {
                Some(e) => Some(e),
                None => {
                    bevy::utils::warn_once!("CameraHandle({}) 未註冊，丟棄該操作 {:?}", handle, op);
                    continue;
                }
            }
        };

        match op {
            CameraBridgeOp::Shake {
                handle: _,
                trauma,
                dir_x,
                dir_y,
                max_strength,
                decay_rate,
                direction_bias,
                perpendicular_damping,
            } => {
                shake_writer.send(CameraShakeRequest {
                    camera: target,
                    op: CameraShakeOp::Add(ShakeParams {
                        trauma,
                        decay_rate,
                        direction: resolve_direction(dir_x, dir_y),
                        max_strength,
                        direction_bias,
                        perpendicular_damping,
                        seed: None,
                    }),
                });
            }
            CameraBridgeOp::PushOverride {
                handle: _,
                x,
                y,
                zoom,
                speed,
                priority,
                duration,
            } => {
                override_writer.send(CameraOverrideRequest {
                    camera: target,
                    op: CameraOverrideOp::Push(PushOverrideParams {
                        target_position: Vec2::new(x, y),
                        target_zoom: resolve_zoom(zoom),
                        speed,
                        priority,
                        duration: resolve_duration(duration),
                    }),
                });
            }
            CameraBridgeOp::PopOverride { .. } => {
                override_writer.send(CameraOverrideRequest {
                    camera: target,
                    op: CameraOverrideOp::PopTop,
                });
            }
            CameraBridgeOp::ClearOverrides { .. } => {
                override_writer.send(CameraOverrideRequest {
                    camera: target,
                    op: CameraOverrideOp::Clear,
                });
            }
            CameraBridgeOp::ClearShakes { .. } => {
                shake_writer.send(CameraShakeRequest {
                    camera: target,
                    op: CameraShakeOp::Clear,
                });
            }
        }
    }
}

#[cfg(test)]
mod sentinel_tests {
    use super::*;
    use bevy::math::Vec2;

    #[test]
    fn resolve_zoom_negative_or_zero_returns_none() {
        assert_eq!(resolve_zoom(-1.0), None);
        assert_eq!(resolve_zoom(-0.001), None);
        assert_eq!(resolve_zoom(0.0), None);
    }

    #[test]
    fn resolve_zoom_positive_returns_some() {
        assert_eq!(resolve_zoom(0.5), Some(0.5));
        assert_eq!(resolve_zoom(720.0), Some(720.0));
    }

    #[test]
    fn resolve_duration_negative_returns_none() {
        assert_eq!(resolve_duration(-1.0), None);
        assert_eq!(resolve_duration(-0.001), None);
    }

    #[test]
    fn resolve_duration_zero_returns_some_zero() {
        assert_eq!(resolve_duration(0.0), Some(0.0));
    }

    #[test]
    fn resolve_duration_positive_returns_some() {
        assert_eq!(resolve_duration(2.0), Some(2.0));
    }

    #[test]
    fn resolve_direction_zero_vector_returns_none() {
        assert_eq!(resolve_direction(0.0, 0.0), None);
    }

    #[test]
    fn resolve_direction_nonzero_returns_some() {
        assert_eq!(resolve_direction(1.0, 0.0), Some(Vec2::new(1.0, 0.0)));
        assert_eq!(resolve_direction(0.0, 1.0), Some(Vec2::new(0.0, 1.0)));
        assert_eq!(resolve_direction(0.5, -0.5), Some(Vec2::new(0.5, -0.5)));
    }
}

#[cfg(test)]
mod system_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    // 同 crate：直接 use super::events
    use crate::camera::events::{CameraOverrideRequest, CameraShakeRequest};
    use std::sync::{Arc, Mutex};
    use vm_runtime::BridgeState;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_event::<CameraOverrideRequest>();
        app.add_event::<CameraShakeRequest>();
        app.init_resource::<CameraRegistry>();
        app.insert_resource(SharedBridgeState(Arc::new(Mutex::new(BridgeState::new()))));
        app
    }

    fn push_op(app: &mut App, op: CameraBridgeOp) {
        let bs = app.world().resource::<SharedBridgeState>().clone();
        bs.0.lock().unwrap().camera_op_queue.push(op);
    }

    fn collect_override_events(app: &mut App) -> Vec<CameraOverrideRequest> {
        let events = app
            .world_mut()
            .resource_mut::<Events<CameraOverrideRequest>>();
        let mut cursor = events.get_cursor();
        cursor.read(&events).cloned().collect()
    }

    fn collect_shake_events(app: &mut App) -> Vec<CameraShakeRequest> {
        let events = app.world_mut().resource_mut::<Events<CameraShakeRequest>>();
        let mut cursor = events.get_cursor();
        cursor.read(&events).cloned().collect()
    }

    #[test]
    fn shake_handle_zero_broadcasts() {
        let mut app = build_app();
        push_op(
            &mut app,
            CameraBridgeOp::Shake {
                handle: 0,
                trauma: 0.5,
                dir_x: 0.0,
                dir_y: 0.0,
                max_strength: 25.0,
                decay_rate: 2.0,
                direction_bias: 1.5,
                perpendicular_damping: 0.3,
            },
        );

        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        let evs = collect_shake_events(&mut app);
        assert_eq!(evs.len(), 1);
        assert!(
            evs[0].camera.is_none(),
            "handle=0 應為 broadcast (camera=None)"
        );
    }

    #[test]
    fn shake_handle_registered_translates_to_entity() {
        let mut app = build_app();
        let cam = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<CameraRegistry>()
            .register(cam, Some(1))
            .unwrap();

        push_op(
            &mut app,
            CameraBridgeOp::Shake {
                handle: 1,
                trauma: 0.8,
                dir_x: 1.0,
                dir_y: 0.0,
                max_strength: 25.0,
                decay_rate: 2.0,
                direction_bias: 1.5,
                perpendicular_damping: 0.3,
            },
        );

        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        let evs = collect_shake_events(&mut app);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].camera, Some(cam));
        let CameraShakeOp::Add(p) = &evs[0].op else {
            panic!("應為 Add op");
        };
        assert_eq!(p.trauma, 0.8);
        assert_eq!(p.direction, Some(Vec2::new(1.0, 0.0)));
    }

    #[test]
    fn shake_handle_unregistered_drops() {
        // S6 已知限制：本測試只驗證「無 event 產生」，不驗證 warn_once 是否被觸發。
        // 未來若 flush_camera_ops 改為「silent drop（不 warn）」，本測試仍會 pass。
        // 若需嚴格驗證 warn 觸發，加 `tracing-test` crate capture log 補強。
        let mut app = build_app();
        push_op(
            &mut app,
            CameraBridgeOp::Shake {
                handle: 999,
                trauma: 0.5,
                dir_x: 0.0,
                dir_y: 0.0,
                max_strength: 25.0,
                decay_rate: 2.0,
                direction_bias: 1.5,
                perpendicular_damping: 0.3,
            },
        );

        app.world_mut().run_system_once(flush_camera_ops).unwrap();
        assert_eq!(collect_shake_events(&mut app).len(), 0);
    }

    #[test]
    fn push_override_zoom_zero_returns_none() {
        let mut app = build_app();
        push_op(
            &mut app,
            CameraBridgeOp::PushOverride {
                handle: 0,
                x: 100.0,
                y: 200.0,
                zoom: 0.0, // sentinel
                speed: 5.0,
                priority: 50,
                duration: -1.0, // sentinel
            },
        );

        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        let evs = collect_override_events(&mut app);
        assert_eq!(evs.len(), 1);
        let CameraOverrideOp::Push(p) = &evs[0].op else {
            panic!("應為 Push op");
        };
        assert_eq!(p.target_zoom, None);
        assert!(p.duration.is_none());
        assert_eq!(p.target_position, Vec2::new(100.0, 200.0));
    }

    #[test]
    fn push_override_zoom_negative_returns_none() {
        let mut app = build_app();
        push_op(
            &mut app,
            CameraBridgeOp::PushOverride {
                handle: 0,
                x: 0.0,
                y: 0.0,
                zoom: -1.0,
                speed: 5.0,
                priority: 0,
                duration: 0.0,
            },
        );
        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        let evs = collect_override_events(&mut app);
        let CameraOverrideOp::Push(p) = &evs[0].op else {
            unreachable!()
        };
        assert_eq!(p.target_zoom, None);
        assert_eq!(p.duration, Some(0.0)); // 0 不是 None sentinel
    }

    #[test]
    fn push_override_zoom_positive_returns_some() {
        let mut app = build_app();
        push_op(
            &mut app,
            CameraBridgeOp::PushOverride {
                handle: 0,
                x: 0.0,
                y: 0.0,
                zoom: 720.0,
                speed: 5.0,
                priority: 0,
                duration: -1.0,
            },
        );
        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        let CameraOverrideOp::Push(p) = &collect_override_events(&mut app)[0].op else {
            unreachable!()
        };
        assert_eq!(p.target_zoom, Some(720.0));
    }

    #[test]
    fn pop_override_forwards() {
        let mut app = build_app();
        push_op(&mut app, CameraBridgeOp::PopOverride { handle: 0 });
        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        let evs = collect_override_events(&mut app);
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0].op, CameraOverrideOp::PopTop));
    }

    #[test]
    fn clear_overrides_forwards() {
        let mut app = build_app();
        push_op(&mut app, CameraBridgeOp::ClearOverrides { handle: 0 });
        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        assert!(matches!(
            collect_override_events(&mut app)[0].op,
            CameraOverrideOp::Clear
        ));
    }

    #[test]
    fn clear_shakes_forwards() {
        let mut app = build_app();
        push_op(&mut app, CameraBridgeOp::ClearShakes { handle: 0 });
        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        assert!(matches!(
            collect_shake_events(&mut app)[0].op,
            CameraShakeOp::Clear
        ));
    }

    #[test]
    fn multiple_ops_fifo_order() {
        let mut app = build_app();
        push_op(&mut app, CameraBridgeOp::PopOverride { handle: 0 });
        push_op(&mut app, CameraBridgeOp::ClearShakes { handle: 0 });
        push_op(&mut app, CameraBridgeOp::ClearOverrides { handle: 0 });

        app.world_mut().run_system_once(flush_camera_ops).unwrap();

        let ovr_evs = collect_override_events(&mut app);
        assert_eq!(ovr_evs.len(), 2);
        assert!(matches!(ovr_evs[0].op, CameraOverrideOp::PopTop));
        assert!(matches!(ovr_evs[1].op, CameraOverrideOp::Clear));
        let shk_evs = collect_shake_events(&mut app);
        assert_eq!(shk_evs.len(), 1);
        assert!(matches!(shk_evs[0].op, CameraShakeOp::Clear));
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;
    use crate::camera::camera_registry::BridgeCamera;
    use crate::camera::CameraPlugin;
    use std::sync::{Arc, Mutex};
    use vm_runtime::BridgeState;

    #[test]
    fn flush_camera_ops_runs_on_fixed_update() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1.0 / 60.0),
        ));
        vm_bevy_bridge::configure_game_fixed_set_for_tests(&mut app);
        let bs = SharedBridgeState(Arc::new(Mutex::new(BridgeState::new())));
        app.insert_resource(bs.clone());
        app.add_plugins(CameraPlugin);

        let cam = app.world_mut().spawn(BridgeCamera::default()).id();

        // 推進一幀讓 PreUpdate register fire
        app.update();

        // push op 後再推一幀讓 FixedUpdate flush
        bs.0.lock()
            .unwrap()
            .camera_op_queue
            .push(CameraBridgeOp::ClearShakes { handle: 1 });

        app.update();

        let events = app.world_mut().resource_mut::<Events<CameraShakeRequest>>();
        let mut cursor = events.get_cursor();
        let evs: Vec<_> = cursor.read(&events).cloned().collect();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].camera, Some(cam));
        assert!(matches!(evs[0].op, CameraShakeOp::Clear));
    }
}
