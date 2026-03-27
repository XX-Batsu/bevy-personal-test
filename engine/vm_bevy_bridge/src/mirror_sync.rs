//! EcsMirror 同步 — 從 VM 側 BridgeState 讀取狀態更新 EcsMirror。
//!
//! # 設計依據
//! - docs/design/architecture/04-vm-bridge/README.md
//! - docs/design/script-engine/03-bridge-api/ecs-mirror.md
//!
//! # 資料流
//! BridgeState.ecs_mirror（VM 側寫入）→ update_ecs_mirror system → EcsMirrorResource（Bevy 側讀取）
//!
//! # 過濾邏輯
//! 僅同步 BridgeEntityMap 中已註冊的 entity（已 spawn 且未 despawn）。
//! 不在映射表中的 VM entity 會被排除，避免同步已銷毀的實體資料。

use bevy::prelude::*;
use bridge_types::{EcsMirror, EntityId};
use deterministic::SoftF32;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use vm_runtime::BridgeState;

use crate::event_queue::BridgeEntityMap;
use crate::handle_sweep::SimulationClock;

/// 60Hz 固定步進 delta_time（bit pattern: 1.0/60.0 ≈ 0.016666668）
/// 硬編碼常數，不從 Time<Fixed> 動態取得，確保所有 client bit-exact 一致。
const DELTA_TIME_60HZ: SoftF32 = SoftF32(0x3C88_8889);

// ── Resource 定義 ────────────────────────────────────────────────────────

/// Bevy Resource：包裝 BridgeState（Arc<Mutex>）以注入 Bevy World
///
/// 使用 Arc<Mutex> 滿足 Bevy Resource 的 Send + Sync 要求。
/// 實際 WASM 為單執行緒環境，Mutex 不會產生競爭開銷。
/// 允許 VM 側與 Bevy 側共享可變狀態。
#[derive(Resource, Clone)]
pub struct SharedBridgeState(pub Arc<Mutex<BridgeState>>);

/// Bevy Resource：包裝 EcsMirror（bridge_types crate）
///
/// `bridge_types::EcsMirror` 沒有 derive Resource（bridge_types 無 bevy 依賴），
/// 用 newtype wrapper 注入 Bevy World。
#[derive(Resource)]
pub struct EcsMirrorResource(pub EcsMirror);

impl Default for EcsMirrorResource {
    fn default() -> Self {
        Self(EcsMirror {
            entities: BTreeMap::new(),
            local_player_id: EntityId(0),
            frame_number: 0,
            delta_time: SoftF32::ZERO,
        })
    }
}

// ── update_ecs_mirror system ─────────────────────────────────────────────

/// 從 VM 側 BridgeState.ecs_mirror 同步至 Bevy 側 EcsMirrorResource。
///
/// 排程位置：Last schedule（在所有遊戲邏輯之後執行）。
///
/// # 同步邏輯
/// 1. 遞增 SimulationClock frame
/// 2. 從 BridgeState.ecs_mirror.entities 讀取全部 entity
/// 3. 以 BridgeEntityMap 為過濾條件，僅保留已註冊的 entity
/// 4. 寫入 EcsMirrorResource（frame_number、delta_time、entities）
pub fn update_ecs_mirror(
    bridge_state: Res<SharedBridgeState>,
    mut mirror_res: ResMut<EcsMirrorResource>,
    entity_map: Res<BridgeEntityMap>,
    mut sim_clock: ResMut<SimulationClock>,
) {
    // 1. 遞增模擬時鐘
    sim_clock.frame += 1;

    // 2. delta_time 硬編碼 60Hz（bit pattern: 1.0/60.0 ≈ 0.016666668）
    let delta_time = DELTA_TIME_60HZ;

    // 3. 從 BridgeState.ecs_mirror.entities 讀取，以 entity_map 為過濾條件
    let state = bridge_state.0.lock().unwrap();
    let mut new_entities = BTreeMap::new();

    for (eid, mirrored) in &state.ecs_mirror.entities {
        if entity_map.get_bevy(*eid).is_some() {
            new_entities.insert(*eid, mirrored.clone());
        }
    }

    // 4. 寫入 EcsMirror Resource
    let mirror = &mut mirror_res.0;
    mirror.entities = new_entities;
    mirror.frame_number = sim_clock.frame;
    mirror.delta_time = delta_time;

    tracing::debug!(
        frame = sim_clock.frame,
        entities = mirror.entities.len(),
        "EcsMirror 同步完成"
    );
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, EntityState, MirroredEntity};
    use deterministic::SoftVec3;

    /// 建構 mirror 同步測試用 Bevy App
    fn build_mirror_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let bridge_state = Arc::new(Mutex::new(BridgeState::new()));
        app.insert_resource(SharedBridgeState(bridge_state));
        app.init_resource::<EcsMirrorResource>();
        app.init_resource::<BridgeEntityMap>();
        app.init_resource::<SimulationClock>();
        app.add_systems(Update, update_ecs_mirror);
        app
    }

    /// 輔助函式：建構 MirroredEntity
    fn make_entity(x: f32, y: f32, z: f32) -> MirroredEntity {
        MirroredEntity {
            position: SoftVec3::new(
                SoftF32::from_f32(x),
                SoftF32::from_f32(y),
                SoftF32::from_f32(z),
            ),
            rotation: SoftVec3::new(SoftF32::ZERO, SoftF32::ZERO, SoftF32::ZERO),
            scale: SoftVec3::new(SoftF32::ONE, SoftF32::ONE, SoftF32::ONE),
            hp: 100,
            max_hp: 100,
            state: EntityState::IDLE,
            animation_id: None,
            custom: BTreeMap::new(),
        }
    }

    /// 輔助函式：在 BridgeEntityMap 中註冊 VM entity
    fn register_entity(app: &mut App, vm_id: EntityId) {
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<BridgeEntityMap>()
            .insert(vm_id, bevy_entity);
    }

    /// 輔助函式：在 BridgeState.ecs_mirror 中插入 entity
    fn insert_vm_entity(app: &App, vm_id: EntityId, entity: MirroredEntity) {
        let shared = app.world().resource::<SharedBridgeState>();
        shared
            .0
            .lock()
            .unwrap()
            .ecs_mirror
            .entities
            .insert(vm_id, entity);
    }

    /// 輔助函式：從 BridgeState.ecs_mirror 中移除 entity
    fn remove_vm_entity(app: &App, vm_id: EntityId) {
        let shared = app.world().resource::<SharedBridgeState>();
        shared.0.lock().unwrap().ecs_mirror.entities.remove(&vm_id);
    }

    // ── 1. 位置同步 bit-exact ────────────────────────────────────────

    #[test]
    fn test_bridge_state_position_synced_to_mirror() {
        let mut app = build_mirror_test_app();
        let vm_id = EntityId(1);

        // 在 BridgeEntityMap 註冊
        register_entity(&mut app, vm_id);

        // 在 BridgeState.ecs_mirror 插入 entity
        let entity = make_entity(10.0, 20.0, 30.0);
        insert_vm_entity(&app, vm_id, entity);

        app.update();

        // 驗證 EcsMirrorResource 中 entity 的位置（bit-exact）
        let mirror = app.world().resource::<EcsMirrorResource>();
        let synced = mirror.0.entities.get(&vm_id).expect("應存在同步的 entity");
        assert_eq!(synced.position.x, SoftF32::from_f32(10.0));
        assert_eq!(synced.position.y, SoftF32::from_f32(20.0));
        assert_eq!(synced.position.z, SoftF32::from_f32(30.0));
    }

    // ── 2. 銷毀 entity 後從 mirror 移除 ─────────────────────────────

    #[test]
    fn test_despawned_entity_removed_from_mirror() {
        let mut app = build_mirror_test_app();
        let vm_id = EntityId(1);

        register_entity(&mut app, vm_id);
        insert_vm_entity(&app, vm_id, make_entity(1.0, 2.0, 3.0));

        app.update();

        // 確認已同步
        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.entities.len(), 1);

        // 從 BridgeEntityMap 移除（模擬 despawn）
        app.world_mut()
            .resource_mut::<BridgeEntityMap>()
            .remove_by_vm(vm_id);

        app.update();

        // mirror 應不再包含該 entity
        let mirror = app.world().resource::<EcsMirrorResource>();
        assert!(mirror.0.entities.is_empty());
    }

    // ── 3. 多 entity 全部同步 ────────────────────────────────────────

    #[test]
    fn test_multiple_entities_all_reflected() {
        let mut app = build_mirror_test_app();

        for i in 1..=5 {
            let vm_id = EntityId(i);
            register_entity(&mut app, vm_id);
            insert_vm_entity(
                &app,
                vm_id,
                make_entity(i as f32, i as f32 * 2.0, i as f32 * 3.0),
            );
        }

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.entities.len(), 5);

        for i in 1..=5u64 {
            let synced = mirror
                .0
                .entities
                .get(&EntityId(i))
                .expect("每個 entity 都應同步");
            assert_eq!(synced.position.x, SoftF32::from_f32(i as f32));
        }
    }

    // ── 4. frame_number 每次同步遞增 ────────────────────────────────

    #[test]
    fn test_frame_number_increments_each_sync() {
        let mut app = build_mirror_test_app();

        app.update();
        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.frame_number, 1);

        app.update();
        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.frame_number, 2);

        app.update();
        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.frame_number, 3);
    }

    // ── 5. delta_time 硬編碼常數 ─────────────────────────────────────

    #[test]
    fn test_delta_time_hardcoded_constant() {
        let mut app = build_mirror_test_app();

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        let expected = SoftF32::from_f32(1.0 / 60.0);
        assert_eq!(
            mirror.0.delta_time, expected,
            "delta_time 應為 1.0/60.0 的 bit-exact SoftF32"
        );
    }

    // ── 6. 同幀 despawn + spawn 不同 entity ──────────────────────────

    #[test]
    fn test_same_frame_despawn_then_spawn_different_entity() {
        let mut app = build_mirror_test_app();

        let vm_id_old = EntityId(1);
        let vm_id_new = EntityId(2);

        register_entity(&mut app, vm_id_old);
        insert_vm_entity(&app, vm_id_old, make_entity(1.0, 0.0, 0.0));

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.entities.len(), 1);
        assert!(mirror.0.entities.contains_key(&vm_id_old));

        // 移除舊 entity 的映射，新增新 entity 的映射
        app.world_mut()
            .resource_mut::<BridgeEntityMap>()
            .remove_by_vm(vm_id_old);
        register_entity(&mut app, vm_id_new);

        // 更新 VM 側 mirror
        remove_vm_entity(&app, vm_id_old);
        insert_vm_entity(&app, vm_id_new, make_entity(99.0, 0.0, 0.0));

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.entities.len(), 1);
        assert!(!mirror.0.entities.contains_key(&vm_id_old));
        let synced = mirror.0.entities.get(&vm_id_new).expect("新 entity 應存在");
        assert_eq!(synced.position.x, SoftF32::from_f32(99.0));
    }

    // ── 7. 未在 bridge map 中的 entity 被排除 ────────────────────────

    #[test]
    fn test_entity_not_in_bridge_map_excluded() {
        let mut app = build_mirror_test_app();

        let registered_id = EntityId(1);
        let unregistered_id = EntityId(2);

        // 只註冊 entity 1
        register_entity(&mut app, registered_id);

        // 但 VM 側有兩個 entity
        insert_vm_entity(&app, registered_id, make_entity(1.0, 0.0, 0.0));
        insert_vm_entity(&app, unregistered_id, make_entity(2.0, 0.0, 0.0));

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.entities.len(), 1);
        assert!(mirror.0.entities.contains_key(&registered_id));
        assert!(!mirror.0.entities.contains_key(&unregistered_id));
    }

    // ── 8. 空 BridgeState 時 mirror 清空 ─────────────────────────────

    #[test]
    fn test_mirror_cleared_on_empty_bridge_state() {
        let mut app = build_mirror_test_app();

        let vm_id = EntityId(1);
        register_entity(&mut app, vm_id);
        insert_vm_entity(&app, vm_id, make_entity(5.0, 5.0, 5.0));

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        assert_eq!(mirror.0.entities.len(), 1);

        // 清空 VM 側 ecs_mirror
        {
            let shared = app.world().resource::<SharedBridgeState>();
            shared.0.lock().unwrap().ecs_mirror.entities.clear();
        }

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        assert!(
            mirror.0.entities.is_empty(),
            "VM 側清空後，Bevy 側 mirror 也應為空"
        );
    }

    // ── 9. custom 欄位同步 ───────────────────────────────────────────

    #[test]
    fn test_mirror_state_custom_fields_synced() {
        let mut app = build_mirror_test_app();

        let vm_id = EntityId(1);
        register_entity(&mut app, vm_id);

        let mut entity = make_entity(0.0, 0.0, 0.0);
        entity
            .custom
            .insert("mana".to_string(), DeterministicValue::Int(50));
        entity.custom.insert(
            "speed".to_string(),
            DeterministicValue::Float(SoftF32::from_f32(5.5)),
        );
        entity
            .custom
            .insert("alive".to_string(), DeterministicValue::Bool(true));
        insert_vm_entity(&app, vm_id, entity);

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        let synced = mirror.0.entities.get(&vm_id).expect("應存在同步的 entity");
        assert_eq!(synced.custom.len(), 3);
        assert_eq!(
            synced.custom.get("mana"),
            Some(&DeterministicValue::Int(50))
        );
        assert_eq!(
            synced.custom.get("speed"),
            Some(&DeterministicValue::Float(SoftF32::from_f32(5.5)))
        );
        assert_eq!(
            synced.custom.get("alive"),
            Some(&DeterministicValue::Bool(true))
        );
    }

    // ── 10. 多幀狀態更新 ─────────────────────────────────────────────

    #[test]
    fn test_multi_frame_state_update() {
        let mut app = build_mirror_test_app();

        let vm_id = EntityId(1);
        register_entity(&mut app, vm_id);

        // 第 1 幀：hp=100, position=(0,0,0)
        let mut entity = make_entity(0.0, 0.0, 0.0);
        entity.hp = 100;
        insert_vm_entity(&app, vm_id, entity);

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        let synced = mirror.0.entities.get(&vm_id).unwrap();
        assert_eq!(synced.hp, 100);
        assert_eq!(synced.position.x, SoftF32::from_f32(0.0));

        // 第 2 幀：hp=80, position=(10,0,0), state=MOVING
        {
            let shared = app.world().resource::<SharedBridgeState>();
            let mut state = shared.0.lock().unwrap();
            let e = state.ecs_mirror.entities.get_mut(&vm_id).unwrap();
            e.hp = 80;
            e.position.x = SoftF32::from_f32(10.0);
            e.state = EntityState::MOVING;
        }

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        let synced = mirror.0.entities.get(&vm_id).unwrap();
        assert_eq!(synced.hp, 80);
        assert_eq!(synced.position.x, SoftF32::from_f32(10.0));
        assert_eq!(synced.state, EntityState::MOVING);
        assert_eq!(mirror.0.frame_number, 2);

        // 第 3 幀：hp=0, state=DEAD
        {
            let shared = app.world().resource::<SharedBridgeState>();
            let mut state = shared.0.lock().unwrap();
            let e = state.ecs_mirror.entities.get_mut(&vm_id).unwrap();
            e.hp = 0;
            e.state = EntityState::DEAD;
        }

        app.update();

        let mirror = app.world().resource::<EcsMirrorResource>();
        let synced = mirror.0.entities.get(&vm_id).unwrap();
        assert_eq!(synced.hp, 0);
        assert_eq!(synced.state, EntityState::DEAD);
        assert_eq!(mirror.0.frame_number, 3);
    }
}
