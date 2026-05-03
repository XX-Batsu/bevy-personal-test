//! 攝影機覆寫堆疊（Camera Override Stack） — priority-sorted Vec。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.1
//!
//! # 責任
//! 提供 per-camera 的覆寫機制：push 多個 override、按 priority 排序、
//! 可選 duration 自動移除、外部可依 id 主動移除。
//!
//! # Priority 使用指引（non-hardcoded，僅供參考）
//!
//! Priority 是任意 `i32`，遊戲層定義自己的階梯。典型參考範圍：
//! - `0..10`    背景 / 自動控制器（auto camera）
//! - `10..100`  暫時性 override（target swap、tracking adjustment）
//! - `100..1000` 腳本觸發的 override（scene transition、focus event）
//! - `1000+`    強制鎖定 override（cutscene、相容層）
//!
//! # Speed 使用指引（餵給 decay_factor(speed, dt) = 1 - exp(-speed * dt)）
//! 經驗對照（1 秒內收斂至 target 的百分比）：
//! - `speed=1.0` → ~63%
//! - `speed=3.0` → ~95%
//! - `speed=5.0` → ~99.3%
//! - `speed=8.0` → ~99.97%

use super::components::{decay_factor, CameraZoom};
use super::events::{CameraOverrideOp, CameraOverrideRequest};
use bevy::prelude::*;

/// 唯一識別一個 Override。push 時由 `CameraOverrideStack.next_id` 單調遞增產生。
pub type OverrideId = u64;

/// 單個 override 條目。
#[derive(Debug, Clone)]
pub struct Override {
    pub id: OverrideId,
    pub target_position: Vec2,
    pub target_zoom: Option<f32>,
    pub speed: f32,
    pub priority: i32,
    pub remaining: Option<f32>,
}

/// `push` 方法的參數（不含 `id`，由 stack 分配）。
#[derive(Debug, Clone)]
pub struct PushOverrideParams {
    pub target_position: Vec2,
    pub target_zoom: Option<f32>,
    pub speed: f32,
    pub priority: i32,
    pub duration: Option<f32>,
}

/// Per-camera override stack。
/// `entries` 按 `priority` 遞增排序；同 priority 新 entry 插入該群組尾端（後者上）。
/// `top()` 永遠回傳 `entries.last()`。
#[derive(Component, Default, Debug)]
pub struct CameraOverrideStack {
    entries: Vec<Override>,
    next_id: u64,
}

impl CameraOverrideStack {
    /// Push 一個新 override，回傳分配的 `OverrideId`。
    /// `duration < 0.0` 視為 `Some(0.0)` 並發出 `warn_once!`（下一幀 tick 立即移除）。
    pub fn push(&mut self, params: PushOverrideParams) -> OverrideId {
        let id = self.next_id;
        self.next_id += 1;

        let remaining = params.duration.map(|d| {
            if d < 0.0 {
                bevy::utils::warn_once!(
                    "CameraOverrideStack::push 收到 negative duration，視為 0.0"
                );
                0.0
            } else {
                d
            }
        });

        let entry = Override {
            id,
            target_position: params.target_position,
            target_zoom: params.target_zoom,
            speed: params.speed,
            priority: params.priority,
            remaining,
        };

        // 插入點：最後一個 priority <= new.priority 的 entry 之後（同 priority 後者上）
        let insert_at = self
            .entries
            .iter()
            .rposition(|e| e.priority <= entry.priority)
            .map_or(0, |i| i + 1);
        self.entries.insert(insert_at, entry);

        if self.entries.len() > 64 {
            bevy::utils::warn_once!("CameraOverrideStack 累積超過 64 個 entries（可能有洩漏）");
        }

        id
    }

    /// 依 id 移除；不存在則回傳 `None` 並發出 `warn_once!`。
    pub fn remove_by_id(&mut self, id: OverrideId) -> Option<Override> {
        match self.entries.iter().position(|e| e.id == id) {
            Some(idx) => Some(self.entries.remove(idx)),
            None => {
                bevy::utils::warn_once!("CameraOverrideStack::remove_by_id 找不到 id={}", id);
                None
            }
        }
    }

    /// 移除最高 priority（即 entries.last()）。空 stack 回傳 `None`（不 warn）。
    pub fn pop_top(&mut self) -> Option<Override> {
        self.entries.pop()
    }

    /// 取最高 priority（`entries.last()`）。
    pub fn top(&self) -> Option<&Override> {
        self.entries.last()
    }

    /// 清空所有 entries。
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 當前是否無 entries。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 當前 entries 數量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 供 tick system 使用的 internal mut 存取。僅 crate 內可見。
    pub(crate) fn entries_mut(&mut self) -> &mut Vec<Override> {
        &mut self.entries
    }
}

/// `Update` 系統：對每個 camera 的 `CameraOverrideStack` 倒數 `remaining` 並移除到期 entries。
/// dt cap 至 0.1（與其他攝影機系統一致）。
pub fn override_stack_tick_system(time: Res<Time>, mut query: Query<&mut CameraOverrideStack>) {
    let dt = time.delta_secs().min(0.1);

    for mut stack in query.iter_mut() {
        let entries = stack.entries_mut();
        // 反向遍歷：index 從大到小，`remove(idx)` 不影響未檢查的前面 entries
        for i in (0..entries.len()).rev() {
            if let Some(remaining) = entries[i].remaining {
                let new_remaining = remaining - dt;
                if new_remaining <= 0.0 {
                    entries.remove(i);
                } else {
                    entries[i].remaining = Some(new_remaining);
                }
            }
        }
    }
}

/// `Update` 系統：若 stack 非空，用 `top()` 的 target 平滑 lerp 攝影機位置；
/// 若 top 指定 `target_zoom`，將 `CameraZoom.target` 設為該值（實際 lerp 交給 `camera_zoom_system`）。
///
/// empty stack 時不動（保留 follow/look-ahead 已寫入的結果）。
pub fn override_apply_system(
    time: Res<Time>,
    mut query: Query<(
        &CameraOverrideStack,
        &mut Transform,
        Option<&mut CameraZoom>,
    )>,
) {
    let dt = time.delta_secs().min(0.1);

    for (stack, mut transform, zoom_opt) in query.iter_mut() {
        let Some(top) = stack.top() else { continue };

        let factor = decay_factor(top.speed, dt);
        let current = transform.translation.truncate();
        let new_pos = current.lerp(top.target_position, factor);
        transform.translation.x = new_pos.x;
        transform.translation.y = new_pos.y;

        if let (Some(target_zoom), Some(mut zoom)) = (top.target_zoom, zoom_opt) {
            zoom.target = target_zoom;
        }

        tracing::debug!(
            "Override apply：位置 ({:.1}, {:.1})，factor {:.3}",
            transform.translation.x,
            transform.translation.y,
            factor
        );
    }
}

/// `Update` 系統：消費 `CameraOverrideRequest` events，套用至對應 `CameraOverrideStack`。
/// `ev.camera = None` 時廣播至所有含 `CameraOverrideStack` 的 camera。
pub fn override_event_handler_system(
    mut events: EventReader<CameraOverrideRequest>,
    mut query: Query<(Entity, &mut CameraOverrideStack)>,
) {
    for ev in events.read() {
        let mut matched = false;
        for (entity, mut stack) in query.iter_mut() {
            if let Some(target) = ev.camera {
                if target != entity {
                    continue;
                }
            }
            matched = true;
            match ev.op.clone() {
                CameraOverrideOp::Push(params) => {
                    stack.push(params);
                }
                CameraOverrideOp::RemoveById(id) => {
                    stack.remove_by_id(id);
                }
                CameraOverrideOp::PopTop => {
                    stack.pop_top();
                }
                CameraOverrideOp::Clear => {
                    stack.clear();
                }
            }
        }
        if !matched {
            bevy::utils::warn_once!(
                "CameraOverrideRequest camera={:?} 找不到符合的 CameraOverrideStack",
                ev.camera
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(priority: i32) -> PushOverrideParams {
        PushOverrideParams {
            target_position: Vec2::ZERO,
            target_zoom: None,
            speed: 5.0,
            priority,
            duration: None,
        }
    }

    #[test]
    fn push_回傳單調遞增_id() {
        let mut stack = CameraOverrideStack::default();
        let id0 = stack.push(params(0));
        let id1 = stack.push(params(0));
        let id2 = stack.push(params(0));
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn push_後_top_取到最高_priority() {
        let mut stack = CameraOverrideStack::default();
        stack.push(params(10));
        stack.push(params(100));
        stack.push(params(50));
        assert_eq!(stack.top().map(|e| e.priority), Some(100));
    }

    #[test]
    fn 同_priority_後者上() {
        let mut stack = CameraOverrideStack::default();
        let _first = stack.push(PushOverrideParams {
            target_position: Vec2::new(1.0, 0.0),
            ..params(50)
        });
        let second = stack.push(PushOverrideParams {
            target_position: Vec2::new(2.0, 0.0),
            ..params(50)
        });
        assert_eq!(stack.top().map(|e| e.id), Some(second));
        assert_eq!(
            stack.top().map(|e| e.target_position),
            Some(Vec2::new(2.0, 0.0))
        );
    }

    #[test]
    fn pop_top_順序為高_priority_先出() {
        let mut stack = CameraOverrideStack::default();
        stack.push(params(10));
        stack.push(params(100));
        stack.push(params(50));
        assert_eq!(stack.pop_top().map(|e| e.priority), Some(100));
        assert_eq!(stack.pop_top().map(|e| e.priority), Some(50));
        assert_eq!(stack.pop_top().map(|e| e.priority), Some(10));
        assert_eq!(stack.pop_top().map(|e| e.priority), None);
    }

    #[test]
    fn remove_by_id_不存在回傳_none() {
        let mut stack = CameraOverrideStack::default();
        stack.push(params(10));
        assert!(stack.remove_by_id(9999).is_none());
    }

    #[test]
    fn remove_by_id_存在回傳_some_並移除() {
        let mut stack = CameraOverrideStack::default();
        let id = stack.push(params(10));
        stack.push(params(20));
        assert_eq!(stack.len(), 2);
        assert!(stack.remove_by_id(id).is_some());
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.top().map(|e| e.priority), Some(20));
    }

    #[test]
    fn clear_清空() {
        let mut stack = CameraOverrideStack::default();
        stack.push(params(10));
        stack.push(params(20));
        stack.clear();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
    }

    #[test]
    fn is_empty_初始為_true() {
        let stack = CameraOverrideStack::default();
        assert!(stack.is_empty());
    }

    #[test]
    fn negative_duration_視為零() {
        let mut stack = CameraOverrideStack::default();
        stack.push(PushOverrideParams {
            duration: Some(-1.0),
            ..params(10)
        });
        assert_eq!(stack.top().and_then(|e| e.remaining), Some(0.0));
    }

    #[test]
    fn duration_none_為無期限() {
        let mut stack = CameraOverrideStack::default();
        stack.push(PushOverrideParams {
            duration: None,
            ..params(10)
        });
        assert_eq!(stack.top().and_then(|e| e.remaining), None);
    }
}

#[cfg(test)]
mod system_tests {
    use super::super::components::CameraZoom;
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn build_app_with_time(secs: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            secs,
        )));
        // Frame 0：初始化時間（delta=0），讓後續每次 app.update() 均取得 ManualDuration delta
        app.update();
        app
    }

    #[test]
    fn tick_system_倒數_duration() {
        let mut app = build_app_with_time(0.1);
        let cam = app.world_mut().spawn(CameraOverrideStack::default()).id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 0,
                duration: Some(1.0),
            });
        }

        // Frame 1: time_system 套用 ManualDuration → delta=0.1
        app.update();
        // Frame 2: delta=0.1
        app.update();
        // 以 run_system_once 再次執行 tick，此時 Time.delta_secs()=0.1
        app.world_mut()
            .run_system_once(override_stack_tick_system)
            .unwrap();
        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        let remaining = stack.top().and_then(|e| e.remaining).unwrap();
        assert!(
            (remaining - 0.9).abs() < 0.01,
            "tick 0.1 後 remaining 應約 0.9，實際: {remaining}"
        );
    }

    #[test]
    fn tick_system_到期自動移除() {
        // dt cap=0.1；duration=0.05 < 0.1，單次 tick 即到期
        let mut app = build_app_with_time(0.5);
        let cam = app.world_mut().spawn(CameraOverrideStack::default()).id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 0,
                duration: Some(0.05),
            });
        }

        // Frame 1: delta=0.5，但 cap 至 0.1，0.05 - 0.1 ≤ 0 → 移除
        app.update();
        // Frame 2: 再次推進
        app.update();
        app.world_mut()
            .run_system_once(override_stack_tick_system)
            .unwrap();

        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        assert!(stack.is_empty(), "duration 0.05 + tick(cap 0.1) 後應被移除");
    }

    #[test]
    fn tick_system_duration_none_永不移除() {
        let mut app = build_app_with_time(1.0);
        let cam = app.world_mut().spawn(CameraOverrideStack::default()).id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 0,
                duration: None,
            });
        }

        // Frame 1: delta=1.0
        app.update();
        // Frame 2: delta=1.0
        app.update();
        for _ in 0..10 {
            app.world_mut()
                .run_system_once(override_stack_tick_system)
                .unwrap();
        }
        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        assert_eq!(stack.len(), 1, "duration=None 應永遠保留");
    }

    #[test]
    fn tick_system_dt_cap_0_1() {
        // delta=5s，但 tick 內部 cap 到 0.1 → 1.0 duration - 0.1 = 0.9 remaining
        let mut app = build_app_with_time(5.0);
        let cam = app.world_mut().spawn(CameraOverrideStack::default()).id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 0,
                duration: Some(1.0),
            });
        }

        // Frame 1: delta=5.0（ManualDuration），但 tick 內部 cap 至 0.1
        app.update();
        app.world_mut()
            .run_system_once(override_stack_tick_system)
            .unwrap();
        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        let remaining = stack.top().and_then(|e| e.remaining).unwrap();
        assert!(
            (remaining - 0.9).abs() < 0.01,
            "dt 應被 cap 至 0.1，remaining 應約 0.9，實際: {remaining}"
        );
    }

    #[test]
    fn apply_system_empty_stack_不動() {
        let mut app = build_app_with_time(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                Transform::from_xyz(100.0, 200.0, 0.0),
            ))
            .id();

        app.world_mut()
            .run_system_once(override_apply_system)
            .unwrap();

        let t = app.world().get::<Transform>(cam).unwrap();
        assert_eq!(t.translation.x, 100.0);
        assert_eq!(t.translation.y, 200.0);
    }

    #[test]
    fn apply_system_lerp_至_target() {
        let mut app = build_app_with_time(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::new(1000.0, 0.0),
                target_zoom: None,
                speed: 5.0,
                priority: 0,
                duration: None,
            });
        }

        app.update();
        app.world_mut()
            .run_system_once(override_apply_system)
            .unwrap();

        let t = app.world().get::<Transform>(cam).unwrap();
        assert!(t.translation.x > 0.0 && t.translation.x < 1000.0);
    }

    #[test]
    fn apply_system_多_entry_用最高_priority() {
        let mut app = build_app_with_time(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::new(-100.0, 0.0),
                target_zoom: None,
                speed: 5.0,
                priority: 10,
                duration: None,
            });
            stack.push(PushOverrideParams {
                target_position: Vec2::new(100.0, 0.0),
                target_zoom: None,
                speed: 5.0,
                priority: 100,
                duration: None,
            });
        }

        app.update();
        app.world_mut()
            .run_system_once(override_apply_system)
            .unwrap();

        let t = app.world().get::<Transform>(cam).unwrap();
        assert!(
            t.translation.x > 0.0,
            "高 priority 目標為 +100，x 應為正，實際: {}",
            t.translation.x
        );
    }

    #[test]
    fn apply_system_target_zoom_some_設定_zoom_target() {
        let mut app = build_app_with_time(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                CameraZoom::default(),
                Transform::default(),
            ))
            .id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: Some(500.0),
                speed: 5.0,
                priority: 0,
                duration: None,
            });
        }

        app.update();
        app.world_mut()
            .run_system_once(override_apply_system)
            .unwrap();

        let zoom = app.world().get::<CameraZoom>(cam).unwrap();
        assert_eq!(zoom.target, 500.0);
    }

    #[test]
    fn apply_system_target_zoom_none_不改變_zoom() {
        let mut app = build_app_with_time(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                CameraZoom::default(),
                Transform::default(),
            ))
            .id();

        let original_target = app.world().get::<CameraZoom>(cam).unwrap().target;

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 0,
                duration: None,
            });
        }

        app.update();
        app.world_mut()
            .run_system_once(override_apply_system)
            .unwrap();

        let zoom = app.world().get::<CameraZoom>(cam).unwrap();
        assert_eq!(zoom.target, original_target);
    }

    #[test]
    fn apply_system_speed_零_不移動() {
        let mut app = build_app_with_time(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraOverrideStack::default(),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::new(1000.0, 0.0),
                target_zoom: None,
                speed: 0.0,
                priority: 0,
                duration: None,
            });
        }

        app.update();
        app.world_mut()
            .run_system_once(override_apply_system)
            .unwrap();

        let t = app.world().get::<Transform>(cam).unwrap();
        assert!(
            t.translation.x.abs() < 0.01,
            "speed=0 不應移動，實際: {}",
            t.translation.x
        );
    }

    use super::super::events::{CameraOverrideOp, CameraOverrideRequest};

    #[test]
    fn event_handler_push_插入_stack() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<CameraOverrideRequest>();

        let cam = app.world_mut().spawn(CameraOverrideStack::default()).id();

        app.world_mut()
            .resource_mut::<Events<CameraOverrideRequest>>()
            .send(CameraOverrideRequest {
                camera: Some(cam),
                op: CameraOverrideOp::Push(PushOverrideParams {
                    target_position: Vec2::new(10.0, 0.0),
                    target_zoom: None,
                    speed: 5.0,
                    priority: 50,
                    duration: None,
                }),
            });

        app.world_mut()
            .run_system_once(override_event_handler_system)
            .unwrap();

        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.top().map(|e| e.priority), Some(50));
    }

    #[test]
    fn event_handler_camera_none_廣播至所有() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<CameraOverrideRequest>();

        let cam_a = app.world_mut().spawn(CameraOverrideStack::default()).id();
        let cam_b = app.world_mut().spawn(CameraOverrideStack::default()).id();

        app.world_mut()
            .resource_mut::<Events<CameraOverrideRequest>>()
            .send(CameraOverrideRequest {
                camera: None,
                op: CameraOverrideOp::Push(PushOverrideParams {
                    target_position: Vec2::ZERO,
                    target_zoom: None,
                    speed: 5.0,
                    priority: 10,
                    duration: None,
                }),
            });

        app.world_mut()
            .run_system_once(override_event_handler_system)
            .unwrap();

        assert_eq!(
            app.world().get::<CameraOverrideStack>(cam_a).unwrap().len(),
            1
        );
        assert_eq!(
            app.world().get::<CameraOverrideStack>(cam_b).unwrap().len(),
            1
        );
    }

    #[test]
    fn event_handler_pop_top_移除最高() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<CameraOverrideRequest>();

        let cam = app.world_mut().spawn(CameraOverrideStack::default()).id();
        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 10,
                duration: None,
            });
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 100,
                duration: None,
            });
        }

        app.world_mut()
            .resource_mut::<Events<CameraOverrideRequest>>()
            .send(CameraOverrideRequest {
                camera: Some(cam),
                op: CameraOverrideOp::PopTop,
            });

        app.world_mut()
            .run_system_once(override_event_handler_system)
            .unwrap();

        let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.top().map(|e| e.priority), Some(10));
    }

    #[test]
    fn event_handler_clear_清空() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_event::<CameraOverrideRequest>();

        let cam = app.world_mut().spawn(CameraOverrideStack::default()).id();
        {
            let mut stack = app.world_mut().get_mut::<CameraOverrideStack>(cam).unwrap();
            stack.push(PushOverrideParams {
                target_position: Vec2::ZERO,
                target_zoom: None,
                speed: 5.0,
                priority: 10,
                duration: None,
            });
        }

        app.world_mut()
            .resource_mut::<Events<CameraOverrideRequest>>()
            .send(CameraOverrideRequest {
                camera: Some(cam),
                op: CameraOverrideOp::Clear,
            });

        app.world_mut()
            .run_system_once(override_event_handler_system)
            .unwrap();

        assert!(app
            .world()
            .get::<CameraOverrideStack>(cam)
            .unwrap()
            .is_empty());
    }
}
