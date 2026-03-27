//! Fallback components — ScriptDisabled → Bevy Component wrapper + fallback systems。
//!
//! 當腳本被自動停用時（Phase 8 ScriptManager 觸發），
//! 此模組提供 4 個 fallback system 確保遊戲繼續運作：
//!
//! 1. [`fallback_animation_system`]：使用 [`AnimationDefault`] 播放預設動畫
//! 2. [`fallback_ui_system`]：隱藏停用腳本控制的 entity
//! 3. [`fallback_event_system`]：清除停用 entity 的待處理事件
//! 4. [`fallback_movement_system`]：使用 [`ServerPosition`] 覆寫 Transform
//!
//! Component 為 vm_runtime 純 Rust struct 的 Bevy wrapper（newtype pattern）。

use bevy::prelude::*;
use std::collections::BTreeSet;

use bridge_types::EntityId;
use vm_runtime::{
    AnimationDefault as AnimationDefaultData, DisableReason, ScriptDisabled as ScriptDisabledData,
    ServerPosition as ServerPositionData,
};

use crate::event_queue::{BridgeEntityMap, BridgeEventQueue};
use crate::flush::PlayAnimationRequest;

// ── Component Wrappers ──────────────────────────────────────────────────

/// Bevy Component：標記腳本已停用的 entity
#[derive(Component, Clone, Debug)]
pub struct ScriptDisabled(pub ScriptDisabledData);

impl ScriptDisabled {
    pub fn new(script_id: impl Into<String>, reason: DisableReason, tick: u64) -> Self {
        Self(ScriptDisabledData::new(script_id, reason, tick))
    }

    pub fn script_id(&self) -> &str {
        &self.0.script_id
    }

    pub fn reason(&self) -> &DisableReason {
        &self.0.reason
    }

    pub fn disabled_at_tick(&self) -> u64 {
        self.0.disabled_at_tick
    }
}

/// Bevy Component：腳本停用時的預設動畫參數
#[derive(Component, Clone, Debug)]
pub struct AnimationDefault(pub AnimationDefaultData);

/// Bevy Component：伺服器權威位置
#[derive(Component, Clone, Debug)]
pub struct ServerPosition(pub ServerPositionData);

// ── Fallback Systems ────────────────────────────────────────────────────

/// fallback_animation_system：ScriptDisabled + AnimationDefault → 設定動畫
///
/// 當 entity 同時持有 ScriptDisabled 與 AnimationDefault 時：
/// - animation_id = Some(id) → 插入 PlayAnimationRequest（循環播放）
/// - animation_id = None → 移除 PlayAnimationRequest（停止播放）
pub fn fallback_animation_system(
    query: Query<(Entity, &ScriptDisabled, &AnimationDefault)>,
    mut commands: Commands,
) {
    for (entity, disabled, anim_default) in &query {
        match anim_default.0.animation_id {
            Some(anim_id) => {
                tracing::debug!(
                    script_id = %disabled.script_id(),
                    anim_id = anim_id,
                    "腳本停用，套用預設動畫"
                );
                commands.entity(entity).insert(PlayAnimationRequest {
                    anim_id: anim_id as i64,
                    looping: true,
                });
            }
            None => {
                tracing::debug!(
                    script_id = %disabled.script_id(),
                    "腳本停用，無預設動畫，移除動畫播放"
                );
                commands.entity(entity).remove::<PlayAnimationRequest>();
            }
        }
    }
}

/// fallback_ui_system：ScriptDisabled → Visibility::Hidden
///
/// 將所有標記 ScriptDisabled 的 entity 設為隱藏，
/// 避免無腳本控制的 entity 繼續顯示異常畫面。
pub fn fallback_ui_system(query: Query<(Entity, &ScriptDisabled)>, mut commands: Commands) {
    for (entity, disabled) in &query {
        tracing::debug!(
            script_id = %disabled.script_id(),
            "腳本停用，隱藏 entity"
        );
        commands.entity(entity).insert(Visibility::Hidden);
    }
}

/// fallback_event_system：清除 disabled entity 的待處理事件
///
/// 收集所有 ScriptDisabled entity 對應的 VM EntityId，
/// 從 BridgeEventQueue 中移除這些 entity 的待處理事件。
/// 未在 BridgeEntityMap 中的 entity 會被跳過（不影響佇列）。
pub fn fallback_event_system(
    query: Query<(Entity, &ScriptDisabled)>,
    entity_map: Res<BridgeEntityMap>,
    mut bridge_queue: ResMut<BridgeEventQueue>,
) {
    // 收集所有停用 entity 的 VM EntityId（BTreeSet 確保確定性）
    let disabled_vm_ids: BTreeSet<EntityId> = query
        .iter()
        .filter_map(|(bevy_entity, _)| entity_map.get_vm(bevy_entity))
        .collect();

    if disabled_vm_ids.is_empty() {
        return;
    }

    let before = bridge_queue.len();
    bridge_queue.retain(|event| {
        match event.entity_id() {
            Some(eid) => !disabled_vm_ids.contains(&eid),
            None => true, // 無 entity_id 的事件保留（UI/VFX/Audio 等）
        }
    });
    let removed = before - bridge_queue.len();

    if removed > 0 {
        tracing::debug!(
            removed = removed,
            disabled_count = disabled_vm_ids.len(),
            "清除停用腳本 entity 的待處理事件"
        );
    }
}

/// fallback_movement_system：ScriptDisabled + ServerPosition → 覆蓋 Transform
///
/// 將 entity 的 Transform 位置設為 Server 權威位置，
/// 確保停用腳本的 entity 不會停留在錯誤位置。
pub fn fallback_movement_system(
    query: Query<(Entity, &ScriptDisabled, &ServerPosition)>,
    mut commands: Commands,
) {
    for (entity, disabled, server_pos) in &query {
        let pos = &server_pos.0.pos;
        let native_pos = Vec3::new(pos.x.to_native(), pos.y.to_native(), pos.z.to_native());
        tracing::debug!(
            script_id = %disabled.script_id(),
            x = %native_pos.x,
            y = %native_pos.y,
            z = %native_pos.z,
            "腳本停用，使用 Server 權威位置"
        );
        commands
            .entity(entity)
            .insert(Transform::from_translation(native_pos));
    }
}

// ── 測試 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::BridgeEvent;
    use deterministic::{SoftF32, SoftVec3};

    fn build_fallback_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<BridgeEventQueue>();
        app.init_resource::<BridgeEntityMap>();
        app.add_systems(
            Update,
            (
                fallback_animation_system,
                fallback_ui_system,
                fallback_event_system,
                fallback_movement_system,
            ),
        );
        app
    }

    fn soft_vec3(x: f32, y: f32, z: f32) -> SoftVec3 {
        SoftVec3::new(
            SoftF32::from_f32(x),
            SoftF32::from_f32(y),
            SoftF32::from_f32(z),
        )
    }

    // ── 1. ScriptDisabled wrapper 建構測試 ─────────────────────────────

    #[test]
    fn test_script_disabled_wrapper_new() {
        let comp = ScriptDisabled::new("test.rhai", DisableReason::Timeout, 100);
        assert_eq!(comp.script_id(), "test.rhai");
        assert_eq!(comp.reason(), &DisableReason::Timeout);
        assert_eq!(comp.disabled_at_tick(), 100);
    }

    // ── 2. DisableReason 全 variant 測試 ──────────────────────────────

    #[test]
    fn test_script_disabled_all_reasons() {
        let reasons = [
            DisableReason::Timeout,
            DisableReason::OperationLimit,
            DisableReason::ScopeLimitExceeded,
            DisableReason::InitFailed,
        ];
        for reason in &reasons {
            let comp = ScriptDisabled::new("x.rhai", reason.clone(), 0);
            assert_eq!(comp.reason(), reason);
        }
    }

    // ── 3. AnimationDefault wrapper Some/None 測試 ────────────────────

    #[test]
    fn test_animation_default_wrapper_some() {
        let comp = AnimationDefault(AnimationDefaultData {
            animation_id: Some(42),
        });
        assert_eq!(comp.0.animation_id, Some(42));
    }

    #[test]
    fn test_animation_default_wrapper_none() {
        let comp = AnimationDefault(AnimationDefaultData { animation_id: None });
        assert_eq!(comp.0.animation_id, None);
    }

    // ── 4. fallback_animation 有預設動畫 → 插入 PlayAnimationRequest ──

    #[test]
    fn test_fallback_animation_applied_when_script_disabled() {
        let mut app = build_fallback_test_app();
        let entity = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("anim.rhai", DisableReason::Timeout, 10),
                AnimationDefault(AnimationDefaultData {
                    animation_id: Some(5),
                }),
            ))
            .id();

        app.update();

        let world = app.world();
        let req = world.get::<PlayAnimationRequest>(entity);
        assert!(req.is_some(), "應插入 PlayAnimationRequest");
        let req = req.unwrap();
        assert_eq!(req.anim_id, 5);
        assert!(req.looping);
    }

    // ── 5. fallback_animation None → 移除 PlayAnimationRequest ────────

    #[test]
    fn test_fallback_animation_none_stops_playback() {
        let mut app = build_fallback_test_app();
        let entity = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("anim.rhai", DisableReason::Timeout, 10),
                AnimationDefault(AnimationDefaultData { animation_id: None }),
                PlayAnimationRequest {
                    anim_id: 99,
                    looping: false,
                },
            ))
            .id();

        app.update();

        let world = app.world();
        assert!(
            world.get::<PlayAnimationRequest>(entity).is_none(),
            "animation_id=None 時應移除 PlayAnimationRequest"
        );
    }

    // ── 6. fallback_ui 隱藏停用 entity ──────────────────────────────────

    #[test]
    fn test_fallback_ui_hides_when_script_disabled() {
        let mut app = build_fallback_test_app();
        let entity = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("ui.rhai", DisableReason::OperationLimit, 20),
                Visibility::Visible,
            ))
            .id();

        app.update();

        let world = app.world();
        let vis = world.get::<Visibility>(entity);
        assert!(vis.is_some());
        assert_eq!(*vis.unwrap(), Visibility::Hidden);
    }

    // ── 7. fallback_event 移除停用 entity 的待處理事件 ────────────────

    #[test]
    fn test_fallback_event_discards_pending_events() {
        let mut app = build_fallback_test_app();

        // 建立 entity 並註冊映射
        let entity = app
            .world_mut()
            .spawn(ScriptDisabled::new("evt.rhai", DisableReason::Timeout, 30))
            .id();
        let vm_id = EntityId(100);
        app.world_mut()
            .resource_mut::<BridgeEntityMap>()
            .insert(vm_id, entity);

        // 推入關聯事件與無關事件
        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SetTransform {
                eid: vm_id,
                pos: soft_vec3(1.0, 2.0, 3.0),
            });
            queue.push(BridgeEvent::SpawnEntity { type_id: 99 }); // 無 entity_id，保留
            queue.push(BridgeEvent::PlayAnimation {
                eid: vm_id,
                anim_id: 7,
            });
        }

        app.update();

        let queue = app.world().resource::<BridgeEventQueue>();
        // 只剩 SpawnEntity（無 entity_id 的事件保留）
        assert_eq!(queue.len(), 1);
    }

    // ── 8. fallback_event 無停用 entity → 佇列不變 ────────────────────

    #[test]
    fn test_fallback_event_no_disabled_no_change() {
        let mut app = build_fallback_test_app();

        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SpawnEntity { type_id: 1 });
            queue.push(BridgeEvent::SetTransform {
                eid: EntityId(50),
                pos: soft_vec3(0.0, 0.0, 0.0),
            });
        }

        app.update();

        let queue = app.world().resource::<BridgeEventQueue>();
        assert_eq!(queue.len(), 2, "無停用 entity 時佇列應不變");
    }

    // ── 9. fallback_event entity 不在 map 中 → 佇列不變 ──────────────

    #[test]
    fn test_fallback_event_entity_not_in_map() {
        let mut app = build_fallback_test_app();

        // 建立停用 entity 但不註冊到 BridgeEntityMap
        app.world_mut().spawn(ScriptDisabled::new(
            "orphan.rhai",
            DisableReason::InitFailed,
            0,
        ));

        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SetTransform {
                eid: EntityId(999),
                pos: soft_vec3(1.0, 1.0, 1.0),
            });
        }

        app.update();

        let queue = app.world().resource::<BridgeEventQueue>();
        assert_eq!(queue.len(), 1, "entity 不在 map 中，佇列事件應保留");
    }

    // ── 10. fallback_movement 覆寫 Transform ───────────────────────────

    #[test]
    fn test_fallback_movement_uses_server_position() {
        let mut app = build_fallback_test_app();
        let entity = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("move.rhai", DisableReason::Timeout, 40),
                ServerPosition(ServerPositionData {
                    pos: soft_vec3(10.0, 20.0, 30.0),
                }),
                Transform::default(),
            ))
            .id();

        app.update();

        let world = app.world();
        let transform = world.get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation.x, 10.0);
        assert_eq!(transform.translation.y, 20.0);
        assert_eq!(transform.translation.z, 30.0);
    }

    // ── 11. 無 ScriptDisabled → fallback 不作用 ────────────────────────

    #[test]
    fn test_fallback_not_applied_when_script_enabled() {
        let mut app = build_fallback_test_app();

        // 只有 AnimationDefault + ServerPosition，無 ScriptDisabled
        let entity = app
            .world_mut()
            .spawn((
                AnimationDefault(AnimationDefaultData {
                    animation_id: Some(1),
                }),
                ServerPosition(ServerPositionData {
                    pos: soft_vec3(99.0, 99.0, 99.0),
                }),
                Transform::default(),
                Visibility::Visible,
            ))
            .id();

        app.update();

        let world = app.world();
        // 無 ScriptDisabled → 不應插入 PlayAnimationRequest
        assert!(
            world.get::<PlayAnimationRequest>(entity).is_none(),
            "無 ScriptDisabled 時不應觸發 fallback_animation"
        );
        // Visibility 應維持 Visible
        assert_eq!(
            *world.get::<Visibility>(entity).unwrap(),
            Visibility::Visible
        );
        // Transform 應維持原始值
        let t = world.get::<Transform>(entity).unwrap();
        assert_eq!(t.translation, Vec3::ZERO);
    }

    // ── 12. despawn 後 fallback 不 panic ─────────────────────────────

    #[test]
    fn test_fallback_despawned_entity_no_panic() {
        let mut app = build_fallback_test_app();

        // Spawn entity 帶 ScriptDisabled
        let entity = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("despawn.rhai", DisableReason::Timeout, 50),
                AnimationDefault(AnimationDefaultData {
                    animation_id: Some(1),
                }),
                ServerPosition(ServerPositionData {
                    pos: soft_vec3(1.0, 2.0, 3.0),
                }),
                Transform::default(),
                Visibility::Visible,
            ))
            .id();

        // Despawn 該 entity
        app.world_mut().despawn(entity);

        // 4 個 fallback systems 不應 panic（Bevy Query 自動排除已 despawn 的 entity）
        app.update();
    }

    // ── 13. ScriptDisabled 移除後 fallback 不再介入 ──────────────────

    #[test]
    fn test_fallback_disabled_then_reenabled() {
        let mut app = build_fallback_test_app();

        // Spawn entity 帶 ScriptDisabled + AnimationDefault + ServerPosition + Visibility::Visible
        let entity = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("toggle.rhai", DisableReason::Timeout, 60),
                AnimationDefault(AnimationDefaultData {
                    animation_id: Some(3),
                }),
                ServerPosition(ServerPositionData {
                    pos: soft_vec3(10.0, 20.0, 30.0),
                }),
                Transform::default(),
                Visibility::Visible,
            ))
            .id();

        // 第一次 update：fallback 應介入
        app.update();

        // 驗證 Visibility 被設為 Hidden
        let vis = app.world().get::<Visibility>(entity).unwrap();
        assert_eq!(*vis, Visibility::Hidden);

        // 移除 ScriptDisabled component（模擬腳本重新啟用）
        app.world_mut()
            .entity_mut(entity)
            .remove::<ScriptDisabled>();

        // 手動把 Transform 改成特定值
        let custom_transform = Transform::from_translation(Vec3::new(99.0, 88.0, 77.0));
        app.world_mut().entity_mut(entity).insert(custom_transform);

        // 第二次 update：fallback 不再介入
        app.update();

        // 確認 Transform 不變（fallback_movement_system 沒有覆蓋回 ServerPosition）
        let transform = app.world().get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation.x, 99.0);
        assert_eq!(transform.translation.y, 88.0);
        assert_eq!(transform.translation.z, 77.0);
    }

    // ── 14（原 12）. 多個停用 entity 同時處理 ─────────────────────────

    #[test]
    fn test_fallback_multiple_disabled_entities() {
        let mut app = build_fallback_test_app();

        let vm_id_a = EntityId(10);
        let vm_id_b = EntityId(20);

        let entity_a = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("a.rhai", DisableReason::Timeout, 100),
                AnimationDefault(AnimationDefaultData {
                    animation_id: Some(1),
                }),
                ServerPosition(ServerPositionData {
                    pos: soft_vec3(1.0, 2.0, 3.0),
                }),
                Transform::default(),
                Visibility::Visible,
            ))
            .id();

        let entity_b = app
            .world_mut()
            .spawn((
                ScriptDisabled::new("b.rhai", DisableReason::OperationLimit, 200),
                AnimationDefault(AnimationDefaultData { animation_id: None }),
                ServerPosition(ServerPositionData {
                    pos: soft_vec3(4.0, 5.0, 6.0),
                }),
                Transform::default(),
                Visibility::Visible,
                PlayAnimationRequest {
                    anim_id: 77,
                    looping: true,
                },
            ))
            .id();

        // 註冊映射
        {
            let mut map = app.world_mut().resource_mut::<BridgeEntityMap>();
            map.insert(vm_id_a, entity_a);
            map.insert(vm_id_b, entity_b);
        }

        // 推入兩個 entity 的事件 + 一個無關事件
        {
            let mut queue = app.world_mut().resource_mut::<BridgeEventQueue>();
            queue.push(BridgeEvent::SetTransform {
                eid: vm_id_a,
                pos: soft_vec3(0.0, 0.0, 0.0),
            });
            queue.push(BridgeEvent::PlayAnimation {
                eid: vm_id_b,
                anim_id: 3,
            });
            queue.push(BridgeEvent::ShowToast {
                msg: "保留".to_string(),
                duration_ms: 1000,
            });
        }

        app.update();

        let world = app.world();

        // entity_a: animation_id=Some(1) → PlayAnimationRequest
        let req_a = world.get::<PlayAnimationRequest>(entity_a);
        assert!(req_a.is_some());
        assert_eq!(req_a.unwrap().anim_id, 1);

        // entity_b: animation_id=None → 移除 PlayAnimationRequest
        assert!(world.get::<PlayAnimationRequest>(entity_b).is_none());

        // 兩者都隱藏
        assert_eq!(
            *world.get::<Visibility>(entity_a).unwrap(),
            Visibility::Hidden
        );
        assert_eq!(
            *world.get::<Visibility>(entity_b).unwrap(),
            Visibility::Hidden
        );

        // entity_a Transform → (1, 2, 3)
        let t_a = world.get::<Transform>(entity_a).unwrap();
        assert_eq!(t_a.translation.x, 1.0);
        assert_eq!(t_a.translation.y, 2.0);
        assert_eq!(t_a.translation.z, 3.0);

        // entity_b Transform → (4, 5, 6)
        let t_b = world.get::<Transform>(entity_b).unwrap();
        assert_eq!(t_b.translation.x, 4.0);
        assert_eq!(t_b.translation.y, 5.0);
        assert_eq!(t_b.translation.z, 6.0);

        // 佇列只剩 ShowToast（無 entity_id 的事件保留）
        let queue = world.resource::<BridgeEventQueue>();
        assert_eq!(queue.len(), 1);
    }
}
