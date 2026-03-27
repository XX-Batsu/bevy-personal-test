//! flush_bridge_events system — 將 BridgeEventQueue 中的事件轉換為 Bevy ECS Commands。
//!
//! # 設計依據
//! - docs/design/architecture/04-vm-bridge/README.md
//! - docs/design/script-engine/03-bridge-api/bridge-events.md
//!
//! # 簡化策略（Phase 9 基礎實作）
//!
//! - Entity 操作（Spawn/Despawn/SetTransform/SetRotation/SetScale/SetVisibility）：完整實作
//! - VFX（PlayVfx）：spawn placeholder entity
//! - StopVfx/StopSound/PlaySound/PlaySoundAt：靜默忽略（Phase 10 完善）
//! - UI 事件：tracing::debug log（Phase 10 實作 UI 系統）
//! - Animation：entity 存在時 insert marker component；否則 warn
//! - Network（SendPrediction/RequestState）：tracing::debug log（Phase 11 實作）

use bevy::prelude::*;
use bridge_types::BridgeEvent;
use deterministic::Clock;

use crate::event_queue::{BridgeDiagnostics, BridgeEntityMap, BridgeEventQueue, GameEntityTypeId};
use crate::mirror_sync::SharedBridgeState;

// ── Resource 定義 ────────────────────────────────────────────────────────

/// 時間來源 Resource — 包裝 `dyn Clock` trait object
///
/// 採用 `Box<dyn Clock>` 允許測試注入 MockClock，
/// 正式環境使用 NativeClock（native）或 WasmClock（WASM）。
#[derive(Resource)]
pub struct ClockResource(pub Box<dyn Clock>);

// ── Component 定義 ───────────────────────────────────────────────────────

/// 遊戲實體 Bundle — SpawnEntity 時插入的基本元件組
#[derive(Bundle)]
pub struct GameEntityBundle {
    pub transform: Transform,
    pub global_transform: GlobalTransform,
    pub visibility: Visibility,
    pub type_id: GameEntityTypeId,
}

/// 動畫播放請求 marker component
///
/// Phase 9 簡化實作：僅記錄 anim_id，Phase 10 替換為完整動畫系統。
#[derive(Component, Debug, Clone)]
pub struct PlayAnimationRequest {
    pub anim_id: i64,
    pub looping: bool,
}

/// 動畫混合請求 marker component
#[derive(Component, Debug, Clone)]
pub struct BlendAnimationRequest {
    pub anim_a: i64,
    pub anim_b: i64,
    pub weight: f64,
}

/// VFX placeholder marker component
#[derive(Component, Debug)]
pub struct VfxPlaceholder {
    pub vfx_id: i64,
}

// ── flush system ─────────────────────────────────────────────────────────

/// 將 BridgeEventQueue 中的事件轉換為 Bevy ECS Commands。
///
/// 排程位置：Last schedule（在所有遊戲邏輯之後執行）。
/// 計時使用 ClockResource（基礎設施計時，非遊戲邏輯）。
pub fn flush_bridge_events(
    mut commands: Commands,
    mut bridge_queue: ResMut<BridgeEventQueue>,
    mut entity_map: ResMut<BridgeEntityMap>,
    mut diagnostics: ResMut<BridgeDiagnostics>,
    clock: Res<ClockResource>,
    bridge_state: Res<SharedBridgeState>,
) {
    let start = clock.0.now_micros();
    let dropped_before = bridge_queue.dropped_count();

    let events: Vec<BridgeEvent> = bridge_queue.drain().collect();
    let event_count = events.len();

    for event in events {
        process_bridge_event(&event, &mut commands, &mut entity_map, &bridge_state);
    }

    let end = clock.0.now_micros();
    let elapsed_micros = end.saturating_sub(start);
    let duration = std::time::Duration::from_micros(elapsed_micros);

    // 更新診斷資料
    diagnostics.flush_duration = duration;
    diagnostics.events_processed = event_count;
    diagnostics.events_dropped = dropped_before;
    diagnostics.total_events_dropped += dropped_before as u64;

    // flush 耗時 > 500µs 時警告
    if elapsed_micros > 500 {
        tracing::warn!(
            elapsed_us = elapsed_micros,
            events = event_count,
            "flush_bridge_events 耗時超過 500µs"
        );
    }

    if event_count > 0 {
        tracing::debug!(
            events = event_count,
            dropped = dropped_before,
            elapsed_us = elapsed_micros,
            "flush_bridge_events 完成"
        );
    }
}

/// 處理單一 BridgeEvent，轉換為對應的 Bevy ECS 操作
fn process_bridge_event(
    event: &BridgeEvent,
    commands: &mut Commands,
    entity_map: &mut BridgeEntityMap,
    bridge_state: &SharedBridgeState,
) {
    match event {
        // ─── Entity 操作 ───────────────────────────────────────
        BridgeEvent::SpawnEntity { type_id } => {
            let vm_id = bridge_state.0.lock().unwrap().entity_id_allocator.next();
            let bevy_entity = commands
                .spawn(GameEntityBundle {
                    transform: Transform::default(),
                    global_transform: GlobalTransform::default(),
                    visibility: Visibility::default(),
                    type_id: GameEntityTypeId(*type_id),
                })
                .id();
            entity_map.insert(vm_id, bevy_entity);
            tracing::debug!(vm_id = vm_id.0, type_id, "已生成實體");
        }

        BridgeEvent::DespawnEntity { eid } => {
            if let Some(bevy_entity) = entity_map.remove_by_vm(*eid) {
                commands.entity(bevy_entity).despawn_recursive();
                tracing::debug!(vm_id = eid.0, "已移除實體");
            } else {
                tracing::warn!(vm_id = eid.0, "Despawn 失敗：找不到 VM Entity 映射");
            }
        }

        BridgeEvent::SetTransform { eid, pos } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                let translation =
                    Vec3::new(pos.x.to_native(), pos.y.to_native(), pos.z.to_native());
                commands.entity(bevy_entity).insert(Transform {
                    translation,
                    ..Default::default()
                });
            } else {
                tracing::warn!(vm_id = eid.0, "SetTransform 失敗：找不到 VM Entity 映射");
            }
        }

        BridgeEvent::SetRotation { eid, euler } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                let rotation = Quat::from_euler(
                    EulerRot::XYZ,
                    euler.x.to_native(),
                    euler.y.to_native(),
                    euler.z.to_native(),
                );
                commands.entity(bevy_entity).insert(Transform {
                    rotation,
                    ..Default::default()
                });
            } else {
                tracing::warn!(vm_id = eid.0, "SetRotation 失敗：找不到 VM Entity 映射");
            }
        }

        BridgeEvent::SetScale { eid, scale } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                let scale_vec = Vec3::new(
                    scale.x.to_native(),
                    scale.y.to_native(),
                    scale.z.to_native(),
                );
                commands.entity(bevy_entity).insert(Transform {
                    scale: scale_vec,
                    ..Default::default()
                });
            } else {
                tracing::warn!(vm_id = eid.0, "SetScale 失敗：找不到 VM Entity 映射");
            }
        }

        BridgeEvent::SetVisibility { eid, visible } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                let vis = if *visible {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
                commands.entity(bevy_entity).insert(vis);
            } else {
                tracing::warn!(vm_id = eid.0, "SetVisibility 失敗：找不到 VM Entity 映射");
            }
        }

        // ─── VFX / Audio ──────────────────────────────────────
        BridgeEvent::PlayVfx { vfx_id, pos } => {
            // Phase 9 placeholder：spawn 空 entity 帶 Transform + VfxPlaceholder
            let translation = Vec3::new(pos.x.to_native(), pos.y.to_native(), pos.z.to_native());
            commands.spawn((
                Transform::from_translation(translation),
                GlobalTransform::default(),
                Visibility::default(),
                VfxPlaceholder { vfx_id: *vfx_id },
            ));
            tracing::debug!(vfx_id, "PlayVfx placeholder 已建立");
        }

        BridgeEvent::StopVfx { handle } => {
            // Phase 10 完善：依 handle 找到 VFX entity 並移除
            tracing::debug!(handle = handle.0, "StopVfx 已忽略（Phase 10 實作）");
        }

        BridgeEvent::PlaySound { sound_id } => {
            tracing::debug!(sound_id, "PlaySound 已忽略（Phase 10 實作）");
        }

        BridgeEvent::PlaySoundAt { sound_id, pos: _ } => {
            tracing::debug!(sound_id, "PlaySoundAt 已忽略（Phase 10 實作）");
        }

        BridgeEvent::StopSound { handle } => {
            tracing::debug!(handle = handle.0, "StopSound 已忽略（Phase 10 實作）");
        }

        // ─── UI ───────────────────────────────────────────────
        BridgeEvent::ShowDialog { dialog_id } => {
            tracing::debug!(dialog_id, "ShowDialog（Phase 10 實作 UI 系統）");
        }

        BridgeEvent::HideDialog { dialog_id } => {
            tracing::debug!(dialog_id, "HideDialog（Phase 10 實作 UI 系統）");
        }

        BridgeEvent::UpdateHud { key, value } => {
            tracing::debug!(key, ?value, "UpdateHud（Phase 10 實作 UI 系統）");
        }

        BridgeEvent::SetHealthBar { eid, current, max } => {
            tracing::debug!(
                vm_id = eid.0,
                current,
                max,
                "SetHealthBar（Phase 10 實作 UI 系統）"
            );
        }

        BridgeEvent::ShowDamageNumber { x, y, value } => {
            tracing::debug!(x, y, value, "ShowDamageNumber（Phase 10 實作 UI 系統）");
        }

        BridgeEvent::ShowToast { msg, duration_ms } => {
            tracing::debug!(msg, duration_ms, "ShowToast（Phase 10 實作 UI 系統）");
        }

        // ─── Animation ────────────────────────────────────────
        BridgeEvent::PlayAnimation { eid, anim_id } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                commands.entity(bevy_entity).insert(PlayAnimationRequest {
                    anim_id: *anim_id,
                    looping: true,
                });
            } else {
                tracing::warn!(
                    vm_id = eid.0,
                    anim_id,
                    "PlayAnimation 失敗：找不到 VM Entity 映射"
                );
            }
        }

        BridgeEvent::PlayAnimationOnce { eid, anim_id } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                commands.entity(bevy_entity).insert(PlayAnimationRequest {
                    anim_id: *anim_id,
                    looping: false,
                });
            } else {
                tracing::warn!(
                    vm_id = eid.0,
                    anim_id,
                    "PlayAnimationOnce 失敗：找不到 VM Entity 映射"
                );
            }
        }

        BridgeEvent::StopAnimation { eid } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                commands
                    .entity(bevy_entity)
                    .remove::<PlayAnimationRequest>();
                commands
                    .entity(bevy_entity)
                    .remove::<BlendAnimationRequest>();
            } else {
                tracing::warn!(vm_id = eid.0, "StopAnimation 失敗：找不到 VM Entity 映射");
            }
        }

        BridgeEvent::BlendAnimation {
            eid,
            anim_a,
            anim_b,
            weight,
        } => {
            if let Some(bevy_entity) = entity_map.get_bevy(*eid) {
                commands.entity(bevy_entity).insert(BlendAnimationRequest {
                    anim_a: *anim_a,
                    anim_b: *anim_b,
                    weight: *weight,
                });
            } else {
                tracing::warn!(vm_id = eid.0, "BlendAnimation 失敗：找不到 VM Entity 映射");
            }
        }

        // ─── Network ──────────────────────────────────────────
        BridgeEvent::SendPrediction { input_type, data } => {
            tracing::debug!(input_type, ?data, "SendPrediction（Phase 11 實作 Netcode）");
        }

        BridgeEvent::RequestState { key } => {
            tracing::debug!(key, "RequestState（Phase 11 實作 Netcode）");
        }
    }
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::EntityId;
    use deterministic::{NativeClock, SoftF32, SoftVec3};
    use std::sync::{Arc, Mutex};

    /// 建構 flush 測試用 Bevy App
    fn build_flush_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<BridgeEventQueue>();
        app.init_resource::<BridgeEntityMap>();
        app.init_resource::<BridgeDiagnostics>();
        app.insert_resource(ClockResource(Box::new(NativeClock::new())));
        let bridge_state = SharedBridgeState(Arc::new(Mutex::new(vm_runtime::BridgeState::new())));
        app.insert_resource(bridge_state);
        app.add_systems(Update, flush_bridge_events);
        app
    }

    /// 輔助函式：建構 SoftVec3
    fn soft_vec3(x: f32, y: f32, z: f32) -> SoftVec3 {
        SoftVec3 {
            x: SoftF32::from_f32(x),
            y: SoftF32::from_f32(y),
            z: SoftF32::from_f32(z),
        }
    }

    #[test]
    fn test_spawn_entity_creates_bevy_entity() {
        let mut app = build_flush_test_app();

        // 推入 SpawnEntity 事件
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SpawnEntity { type_id: 42 });

        app.update();

        // 驗證 entity map 有映射
        let entity_map = app.world().resource::<BridgeEntityMap>();
        assert_eq!(entity_map.len(), 1);
        let bevy_entity = entity_map
            .get_bevy(EntityId(1))
            .expect("應存在 VM EntityId(1) 映射");

        // 驗證 Bevy entity 有 GameEntityTypeId component
        let type_id = app
            .world()
            .entity(bevy_entity)
            .get::<GameEntityTypeId>()
            .expect("應有 GameEntityTypeId component");
        assert_eq!(type_id.0, 42);

        // 驗證有 Transform component
        assert!(app.world().entity(bevy_entity).get::<Transform>().is_some());
    }

    #[test]
    fn test_despawn_entity_removes_entity() {
        let mut app = build_flush_test_app();

        // 先 spawn
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SpawnEntity { type_id: 1 });
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        assert_eq!(entity_map.len(), 1);
        let bevy_entity = entity_map.get_bevy(EntityId(1)).unwrap();

        // 再 despawn
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::DespawnEntity { eid: EntityId(1) });
        app.update();

        // 映射應已移除
        let entity_map = app.world().resource::<BridgeEntityMap>();
        assert_eq!(entity_map.len(), 0);

        // Bevy entity 應已不存在
        assert!(app.world().get_entity(bevy_entity).is_err());
    }

    #[test]
    fn test_set_transform_updates_transform() {
        let mut app = build_flush_test_app();

        // Spawn 後設定 transform
        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SpawnEntity { type_id: 1 });
        }
        app.update();

        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SetTransform {
                eid: EntityId(1),
                pos: soft_vec3(10.0, 20.0, 30.0),
            });
        }
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        let bevy_entity = entity_map.get_bevy(EntityId(1)).unwrap();
        let transform = app.world().entity(bevy_entity).get::<Transform>().unwrap();

        // SoftF32 → f32 精度驗證
        let tolerance = 0.001_f32;
        assert!((transform.translation.x - 10.0).abs() < tolerance);
        assert!((transform.translation.y - 20.0).abs() < tolerance);
        assert!((transform.translation.z - 30.0).abs() < tolerance);
    }

    #[test]
    fn test_set_visibility_updates_visibility() {
        let mut app = build_flush_test_app();

        // Spawn
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SpawnEntity { type_id: 1 });
        app.update();

        // 設定 hidden
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SetVisibility {
                eid: EntityId(1),
                visible: false,
            });
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        let bevy_entity = entity_map.get_bevy(EntityId(1)).unwrap();
        let vis = app.world().entity(bevy_entity).get::<Visibility>().unwrap();
        assert_eq!(*vis, Visibility::Hidden);

        // 設定 visible
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SetVisibility {
                eid: EntityId(1),
                visible: true,
            });
        app.update();

        let vis = app.world().entity(bevy_entity).get::<Visibility>().unwrap();
        assert_eq!(*vis, Visibility::Visible);
    }

    #[test]
    fn test_empty_queue_flush_no_error() {
        let mut app = build_flush_test_app();
        // 不推入任何事件，直接 update
        app.update();

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 0);
        assert_eq!(diag.events_dropped, 0);
    }

    #[test]
    fn test_unknown_entity_id_skipped() {
        let mut app = build_flush_test_app();

        // 推入針對不存在 entity 的操作
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SetTransform {
                eid: EntityId(999),
                pos: soft_vec3(1.0, 2.0, 3.0),
            });
        app.update();

        // 不應 panic，診斷應記錄已處理 1 個事件
        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 1);
    }

    #[test]
    fn test_multi_event_causal_order() {
        let mut app = build_flush_test_app();

        // 同一幀 spawn + set transform + set visibility
        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SpawnEntity { type_id: 5 });
            // 注意：SpawnEntity 分配 EntityId(1)，後續事件使用 EntityId(1)
            queue.push(BridgeEvent::SetTransform {
                eid: EntityId(1),
                pos: soft_vec3(100.0, 200.0, 300.0),
            });
            queue.push(BridgeEvent::SetVisibility {
                eid: EntityId(1),
                visible: false,
            });
        }
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        let bevy_entity = entity_map.get_bevy(EntityId(1)).unwrap();
        let transform = app.world().entity(bevy_entity).get::<Transform>().unwrap();
        let vis = app.world().entity(bevy_entity).get::<Visibility>().unwrap();

        let tolerance = 0.001_f32;
        assert!((transform.translation.x - 100.0).abs() < tolerance);
        assert_eq!(*vis, Visibility::Hidden);

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 3);
    }

    #[test]
    fn test_diagnostics_events_processed_count() {
        let mut app = build_flush_test_app();

        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SpawnEntity { type_id: 1 });
            queue.push(BridgeEvent::SpawnEntity { type_id: 2 });
            queue.push(BridgeEvent::SpawnEntity { type_id: 3 });
        }
        app.update();

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 3);
        assert_eq!(diag.events_dropped, 0);
    }

    #[test]
    fn test_flush_continues_after_entity_not_found() {
        let mut app = build_flush_test_app();

        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            // 第一個事件指向不存在的 entity
            queue.push(BridgeEvent::DespawnEntity { eid: EntityId(999) });
            // 第二個事件是合法的 spawn
            queue.push(BridgeEvent::SpawnEntity { type_id: 7 });
        }
        app.update();

        // 第二個事件應成功處理
        let entity_map = app.world().resource::<BridgeEntityMap>();
        assert_eq!(entity_map.len(), 1);
        assert!(entity_map.get_bevy(EntityId(1)).is_some());

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 2);
    }

    #[test]
    fn test_play_animation_inserts_marker() {
        let mut app = build_flush_test_app();

        // Spawn entity
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SpawnEntity { type_id: 1 });
        app.update();

        // PlayAnimation
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::PlayAnimation {
                eid: EntityId(1),
                anim_id: 42,
            });
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        let bevy_entity = entity_map.get_bevy(EntityId(1)).unwrap();
        let anim = app
            .world()
            .entity(bevy_entity)
            .get::<PlayAnimationRequest>()
            .expect("應有 PlayAnimationRequest component");
        assert_eq!(anim.anim_id, 42);
        assert!(anim.looping);
    }

    #[test]
    fn test_stop_animation_removes_marker() {
        let mut app = build_flush_test_app();

        // Spawn + PlayAnimation
        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SpawnEntity { type_id: 1 });
        }
        app.update();
        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::PlayAnimation {
                eid: EntityId(1),
                anim_id: 10,
            });
        }
        app.update();

        // StopAnimation
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::StopAnimation { eid: EntityId(1) });
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        let bevy_entity = entity_map.get_bevy(EntityId(1)).unwrap();
        assert!(app
            .world()
            .entity(bevy_entity)
            .get::<PlayAnimationRequest>()
            .is_none());
    }

    #[test]
    fn test_play_vfx_spawns_placeholder() {
        let mut app = build_flush_test_app();

        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::PlayVfx {
                vfx_id: 99,
                pos: soft_vec3(5.0, 10.0, 15.0),
            });
        app.update();

        // 應有一個帶 VfxPlaceholder 的 entity
        let mut query = app.world_mut().query::<(&VfxPlaceholder, &Transform)>();
        let results: Vec<_> = query.iter(app.world()).collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.vfx_id, 99);
        let tolerance = 0.001_f32;
        assert!((results[0].1.translation.x - 5.0).abs() < tolerance);
    }

    #[test]
    fn test_despawn_nonexistent_no_panic() {
        let mut app = build_flush_test_app();

        // Despawn 不存在的 entity 不應 panic
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::DespawnEntity {
                eid: EntityId(12345),
            });
        app.update();

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 1);
    }

    #[test]
    fn test_retain_removes_events_and_counts_dropped() {
        let mut queue = BridgeEventQueue::new();
        queue.push(BridgeEvent::SpawnEntity { type_id: 1 });
        queue.push(BridgeEvent::SpawnEntity { type_id: 2 });
        queue.push(BridgeEvent::SpawnEntity { type_id: 3 });

        // 只保留 type_id == 2
        queue.retain(|e| matches!(e, BridgeEvent::SpawnEntity { type_id: 2 }));

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dropped_count(), 2);
    }

    #[test]
    fn test_multiple_spawns_sequential_ids() {
        let mut app = build_flush_test_app();

        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SpawnEntity { type_id: 1 });
            queue.push(BridgeEvent::SpawnEntity { type_id: 2 });
            queue.push(BridgeEvent::SpawnEntity { type_id: 3 });
        }
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        assert_eq!(entity_map.len(), 3);
        // EntityIdAllocator 從 1 開始遞增
        assert!(entity_map.get_bevy(EntityId(1)).is_some());
        assert!(entity_map.get_bevy(EntityId(2)).is_some());
        assert!(entity_map.get_bevy(EntityId(3)).is_some());
    }

    // ── M-3: 精度與靜默忽略測試 ────────────────────────────────────────

    #[test]
    fn test_set_transform_softf32_bit_exact() {
        let mut app = build_flush_test_app();

        // Spawn entity
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SpawnEntity { type_id: 1 });
        app.update();

        // SetTransform 使用 SoftF32::from_f32(0.1) 構造的 SoftVec3
        let pos = SoftVec3 {
            x: SoftF32::from_f32(0.1),
            y: SoftF32::from_f32(0.2),
            z: SoftF32::from_f32(0.3),
        };
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::SetTransform {
                eid: EntityId(1),
                pos,
            });
        app.update();

        let entity_map = app.world().resource::<BridgeEntityMap>();
        let bevy_entity = entity_map.get_bevy(EntityId(1)).unwrap();
        let transform = app.world().entity(bevy_entity).get::<Transform>().unwrap();

        // bit-exact 驗證：SoftF32→f32 是純 bit reinterpret，無精度損失
        assert_eq!(transform.translation.x.to_bits(), 0.1f32.to_bits());
        assert_eq!(transform.translation.y.to_bits(), 0.2f32.to_bits());
        assert_eq!(transform.translation.z.to_bits(), 0.3f32.to_bits());
    }

    #[test]
    fn test_diagnostics_events_dropped_overflow() {
        let mut app = build_flush_test_app();

        // push 4100 個 SpawnEntity 事件（容量上限 4096，溢出 4 個）
        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            for i in 0..4100 {
                queue.push(BridgeEvent::SpawnEntity { type_id: i as i64 });
            }
        }

        app.update();

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_dropped, 4, "應有 4 個事件因溢出被丟棄");
        assert_eq!(diag.total_events_dropped, 4, "累計丟棄總數應為 4");
    }

    #[test]
    fn test_stop_vfx_invalid_handle_silent() {
        use bridge_types::EffectHandle;

        let mut app = build_flush_test_app();

        // push StopVfx with invalid handle — 不應 panic（fire-and-forget）
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::StopVfx {
                handle: EffectHandle(9999),
            });

        app.update();

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 1);
    }

    #[test]
    fn test_stop_sound_invalid_handle_silent() {
        use bridge_types::SoundHandle;

        let mut app = build_flush_test_app();

        // push StopSound with invalid handle — 不應 panic（fire-and-forget）
        app.world_mut()
            .resource_mut::<BridgeEventQueue>()
            .push(BridgeEvent::StopSound {
                handle: SoundHandle(9999),
            });

        app.update();

        let diag = app.world().resource::<BridgeDiagnostics>();
        assert_eq!(diag.events_processed, 1);
    }
}
