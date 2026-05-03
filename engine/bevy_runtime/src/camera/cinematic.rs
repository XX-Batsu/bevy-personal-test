//! `CameraCinematic` 相容層 — 橋接舊 API 至 `CameraOverrideStack`。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.7
//!
//! # 責任
//! 掛上 `CameraCinematic` component 自動 push 一個無 duration 的 override；
//! 移除 `CameraCinematic` 自動 pop。追蹤透過 `CameraCinematicShadow` 保存 `OverrideId`。
//!
//! # 限制
//! 不追蹤 `CameraCinematic` 欄位變更。若要改值請 remove + re-insert，
//! 或直接使用 `CameraOverrideRequest`。

use bevy::prelude::*;

use super::components::{DEFAULT_CINEMATIC_SPEED, DEFAULT_VIEWPORT_HEIGHT};
use super::override_stack::{CameraOverrideStack, OverrideId, PushOverrideParams};

/// 向後相容的攝影機演出模式。掛上自動 push 一個 `priority = self.priority`、無 duration 的 override；
/// 移除時自動 pop。新程式碼建議直接使用 `CameraOverrideRequest::Push`。
///
/// `CameraOverrideStack` 為必要元件，Bevy 自動插入預設值。
#[derive(Component, Debug, Clone)]
#[require(CameraOverrideStack)]
pub struct CameraCinematic {
    pub target_position: Vec2,
    pub target_zoom: f32,
    pub speed: f32,
    pub priority: i32,
}

impl Default for CameraCinematic {
    fn default() -> Self {
        Self {
            target_position: Vec2::ZERO,
            target_zoom: DEFAULT_VIEWPORT_HEIGHT,
            speed: DEFAULT_CINEMATIC_SPEED,
            priority: 1000,
        }
    }
}

/// Internal shadow component — 記錄 cinematic 對應的 `OverrideId`，移除時用。
/// 使用者不應直接操作此 component。
#[derive(Component, Debug)]
pub struct CameraCinematicShadow {
    pub override_id: OverrideId,
}

/// Observer：`CameraCinematic` 被插入時 push 一個 override 並記錄 shadow。
///
/// Bevy 0.15 API：`Trigger::entity()` 回傳被觀察的 entity（0.16 起改名 `target()`，本專案版本仍用 `entity()`）。
pub fn on_cinematic_added(
    trigger: Trigger<OnAdd, CameraCinematic>,
    mut commands: Commands,
    mut query: Query<(&CameraCinematic, &mut CameraOverrideStack)>,
) {
    let target = trigger.entity();
    if let Ok((cin, mut stack)) = query.get_mut(target) {
        let id = stack.push(PushOverrideParams {
            target_position: cin.target_position,
            target_zoom: Some(cin.target_zoom),
            speed: cin.speed,
            priority: cin.priority,
            duration: None,
        });
        commands
            .entity(target)
            .insert(CameraCinematicShadow { override_id: id });
    } else {
        bevy::utils::warn_once!(
            "CameraCinematic 被加入 entity {target:?}，但缺少 CameraOverrideStack — \
             相容層無效。請同時掛 CameraOverrideStack。"
        );
    }
}

/// Observer：`CameraCinematic` 被移除時 pop 對應的 override。
pub fn on_cinematic_removed(
    trigger: Trigger<OnRemove, CameraCinematic>,
    mut commands: Commands,
    mut query: Query<(&CameraCinematicShadow, &mut CameraOverrideStack)>,
) {
    let target = trigger.entity();
    if let Ok((shadow, mut stack)) = query.get_mut(target) {
        stack.remove_by_id(shadow.override_id);
        commands.entity(target).remove::<CameraCinematicShadow>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            1.0 / 60.0,
        )));
        app.add_observer(on_cinematic_added);
        app.add_observer(on_cinematic_removed);
        app
    }

    #[test]
    fn 加入_cinematic_自動_push_override() {
        let mut app = build_app();
        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                CameraCinematic {
                    target_position: Vec2::new(100.0, 200.0),
                    target_zoom: 500.0,
                    speed: 3.0,
                    priority: 100,
                },
            ))
            .id();

        // observers fire on spawn; stack.push 是同步的（直接改 stack），
        // 但 commands.insert(Shadow) 要等 app.update() flush
        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        assert_eq!(stack.len(), 1);
        let top = stack.top().unwrap();
        assert_eq!(top.target_position, Vec2::new(100.0, 200.0));
        assert_eq!(top.target_zoom, Some(500.0));
        assert_eq!(top.speed, 3.0);
        assert_eq!(top.priority, 100);
        assert_eq!(top.remaining, None);
    }

    #[test]
    fn 加入_cinematic_插入_shadow() {
        let mut app = build_app();
        let cam = app
            .world_mut()
            .spawn((CameraOverrideStack::default(), CameraCinematic::default()))
            .id();

        // commands.insert(CameraCinematicShadow) 在 flush 後生效
        app.update();
        assert!(app.world().get::<CameraCinematicShadow>(cam).is_some());
    }

    #[test]
    fn 移除_cinematic_自動_pop() {
        let mut app = build_app();
        let cam = app
            .world_mut()
            .spawn((CameraOverrideStack::default(), CameraCinematic::default()))
            .id();

        // flush shadow insert
        app.update();
        assert_eq!(
            app.world().get::<CameraOverrideStack>(cam).unwrap().len(),
            1
        );

        app.world_mut().entity_mut(cam).remove::<CameraCinematic>();
        // flush on_cinematic_removed commands（remove shadow）
        app.update();

        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        assert!(stack.is_empty());
        assert!(app.world().get::<CameraCinematicShadow>(cam).is_none());
    }

    #[test]
    fn default_priority_為_1000() {
        let c = CameraCinematic::default();
        assert_eq!(c.priority, 1000);
    }

    #[test]
    fn 無_override_stack_時_warn_不_panic() {
        let mut app = build_app();
        let _ = app.world_mut().spawn(CameraCinematic::default()).id();
        // 不應 panic；warn_once 被觸發但無法直接驗證
    }
}
