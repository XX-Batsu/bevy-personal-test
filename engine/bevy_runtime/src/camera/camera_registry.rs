//! 攝影機 handle registry：BridgeCamera marker + CameraRegistry resource +
//! 註冊/解除註冊系統。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.6 / §3.8.1

use bevy::prelude::*;
use std::collections::BTreeMap;

/// Camera entity 上掛的 marker，由 `bridge_camera_register_system` 偵測。
///
/// `handle = None` → 自動分配（從 1 開始遞增）
/// `handle = Some(n)` 且 `n != 0` → 使用指定 handle；衝突時 warn 不註冊
/// `handle = Some(0)` → warn 視為 None 自動分配（graceful fallback）
#[derive(Component, Default, Debug, Clone)]
pub struct BridgeCamera {
    pub handle: Option<u32>,
}

/// VM-facing camera handle registry。
///
/// 維護 `handle ↔ entity` 雙向對照（unregister 不需 O(N)）。
/// `next_id` 從 1 開始（0 保留為 broadcast sentinel），故 `Default` 自定義。
#[derive(Resource, Debug)]
pub struct CameraRegistry {
    handle_to_entity: BTreeMap<u32, Entity>,
    entity_to_handle: BTreeMap<Entity, u32>,
    next_id: u32,
}

impl Default for CameraRegistry {
    fn default() -> Self {
        Self {
            handle_to_entity: BTreeMap::new(),
            entity_to_handle: BTreeMap::new(),
            next_id: 1,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    HandleAlreadyTaken(u32),
}

impl CameraRegistry {
    pub fn len(&self) -> usize {
        self.handle_to_entity.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handle_to_entity.is_empty()
    }

    /// 解析 handle 為 entity；`handle = 0` 一律回 `None`（broadcast 由呼叫端處理）。
    pub fn resolve(&self, handle: u32) -> Option<Entity> {
        if handle == 0 {
            return None;
        }
        self.handle_to_entity.get(&handle).copied()
    }

    /// 註冊 entity。`requested = None` 自動分配遞增 id；`Some(0)` graceful fallback
    /// 至自動分配；`Some(n != 0)` 使用指定 id；衝突回 Err。
    ///
    /// **Idempotent（S7）**：若 entity 已註冊，回現有 handle，不變更 next_id。
    /// 純邏輯層不 warn `Some(0)`（warn 在 system 層處理；M14）。
    ///
    /// 注意：若 entity 已註冊，`requested` 參數會被靜默忽略（S7 idempotent 優先於
    /// `requested`）。呼叫端若需更換 handle，須先 `unregister(entity)` 再 `register`。
    pub fn register(
        &mut self,
        entity: Entity,
        requested: Option<u32>,
    ) -> Result<u32, RegisterError> {
        // S7：idempotent — entity 已註冊則回現有 handle
        if let Some(&existing) = self.entity_to_handle.get(&entity) {
            return Ok(existing);
        }

        let handle = match requested {
            None | Some(0) => {
                let h = self.next_id;
                self.next_id += 1;
                h
            }
            Some(n) => {
                if self.handle_to_entity.contains_key(&n) {
                    return Err(RegisterError::HandleAlreadyTaken(n));
                }
                if n >= self.next_id {
                    self.next_id = n + 1;
                }
                n
            }
        };
        self.handle_to_entity.insert(handle, entity);
        self.entity_to_handle.insert(entity, handle);
        Ok(handle)
    }

    pub fn unregister(&mut self, entity: Entity) -> Option<u32> {
        let handle = self.entity_to_handle.remove(&entity)?;
        self.handle_to_entity.remove(&handle);
        Some(handle)
    }
}

/// `PreUpdate` 系統：偵測新增的 `BridgeCamera`，向 `CameraRegistry` 註冊。
///
/// 衝突時 `warn_once!` 並略過該 entity。
/// `BridgeCamera::handle = Some(0)` 在此處 warn_once（純邏輯層不 warn；M14）。
pub fn bridge_camera_register_system(
    query: Query<(Entity, &BridgeCamera), Added<BridgeCamera>>,
    mut registry: ResMut<CameraRegistry>,
) {
    for (entity, marker) in query.iter() {
        if matches!(marker.handle, Some(0)) {
            bevy::utils::warn_once!(
                "BridgeCamera::handle = Some(0) 為 broadcast sentinel 保留值，已視為自動分配"
            );
        }

        match registry.register(entity, marker.handle) {
            Ok(handle) => {
                tracing::debug!(handle, ?entity, "BridgeCamera 已註冊");
            }
            Err(RegisterError::HandleAlreadyTaken(h)) => {
                bevy::utils::warn_once!(
                    "BridgeCamera handle={} 已被佔用，entity {:?} 不註冊",
                    h,
                    entity
                );
            }
        }
    }
}

/// `PreUpdate` 系統：偵測移除的 `BridgeCamera`（含 entity despawn），
/// 從 `CameraRegistry` 移除對應 entry。
pub fn bridge_camera_unregister_system(
    mut removed: RemovedComponents<BridgeCamera>,
    mut registry: ResMut<CameraRegistry>,
) {
    for entity in removed.read() {
        if let Some(handle) = registry.unregister(entity) {
            tracing::debug!(handle, ?entity, "BridgeCamera 已解除註冊");
        }
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use bevy::prelude::Entity;

    fn entity(id: u32) -> Entity {
        Entity::from_raw(id)
    }

    #[test]
    fn registry_default_next_id_is_one() {
        let mut r = CameraRegistry::default();
        assert_eq!(r.len(), 0);
        // next_id 從 1 開始：第一個 None 註冊應拿到 handle=1
        assert_eq!(
            r.register(Entity::from_raw(99), None).unwrap(),
            1,
            "next_id 從 1 開始"
        );
    }

    #[test]
    fn register_auto_allocates_incrementing_id() {
        let mut r = CameraRegistry::default();
        let h1 = r.register(entity(10), None).unwrap();
        let h2 = r.register(entity(11), None).unwrap();
        let h3 = r.register(entity(12), None).unwrap();
        assert_eq!(h1, 1);
        assert_eq!(h2, 2);
        assert_eq!(h3, 3);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn register_with_specified_id_unclaimed() {
        let mut r = CameraRegistry::default();
        let h = r.register(entity(10), Some(5)).unwrap();
        assert_eq!(h, 5);
        assert_eq!(r.resolve(5), Some(entity(10)));
        let h_next = r.register(entity(11), None).unwrap();
        assert_eq!(h_next, 6);
    }

    #[test]
    fn register_handle_some_zero_treated_as_auto() {
        // graceful fallback：純邏輯層不 warn
        let mut r = CameraRegistry::default();
        let h = r.register(entity(10), Some(0)).unwrap();
        assert_eq!(h, 1);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn register_duplicate_same_entity_idempotent() {
        // S7：重複 register 同 entity 應回現有 handle，不產生 orphan
        let mut r = CameraRegistry::default();
        let e = entity(10);
        let h1 = r.register(e, None).unwrap();
        let h2 = r.register(e, None).unwrap();
        let h3 = r.register(e, Some(99)).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1, h3);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn register_conflict_returns_handle_already_taken() {
        let mut r = CameraRegistry::default();
        r.register(entity(10), Some(5)).unwrap();
        let err = r.register(entity(11), Some(5)).unwrap_err();
        assert!(matches!(err, RegisterError::HandleAlreadyTaken(5)));
        assert_eq!(r.len(), 1);
        assert_eq!(r.resolve(5), Some(entity(10)));
    }

    #[test]
    fn resolve_unregistered_handle_returns_none() {
        let r = CameraRegistry::default();
        assert_eq!(r.resolve(0), None);
        assert_eq!(r.resolve(999), None);
    }

    #[test]
    fn resolve_registered_handle_returns_entity() {
        let mut r = CameraRegistry::default();
        let e = entity(10);
        let h = r.register(e, None).unwrap();
        assert_eq!(r.resolve(h), Some(e));
    }

    #[test]
    fn unregister_registered_entity_returns_handle_and_removes() {
        let mut r = CameraRegistry::default();
        let e = entity(10);
        let h = r.register(e, None).unwrap();
        assert_eq!(r.unregister(e), Some(h));
        assert_eq!(r.resolve(h), None);
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn unregister_nonexistent_returns_none() {
        let mut r = CameraRegistry::default();
        assert_eq!(r.unregister(entity(99)), None);
    }
}

#[cfg(test)]
mod system_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn build_app() -> App {
        let mut app = App::new();
        app.init_resource::<CameraRegistry>();
        app
    }

    #[test]
    fn register_system_auto_allocates_incrementing() {
        let mut app = build_app();
        let e1 = app.world_mut().spawn(BridgeCamera::default()).id();
        let e2 = app.world_mut().spawn(BridgeCamera::default()).id();
        let e3 = app.world_mut().spawn(BridgeCamera::default()).id();

        app.world_mut()
            .run_system_once(bridge_camera_register_system)
            .unwrap();

        let r = app.world().resource::<CameraRegistry>();
        assert_eq!(r.resolve(1), Some(e1));
        assert_eq!(r.resolve(2), Some(e2));
        assert_eq!(r.resolve(3), Some(e3));
    }

    #[test]
    fn register_system_with_specified_handle() {
        let mut app = build_app();
        let e = app.world_mut().spawn(BridgeCamera { handle: Some(7) }).id();
        app.world_mut()
            .run_system_once(bridge_camera_register_system)
            .unwrap();
        let r = app.world().resource::<CameraRegistry>();
        assert_eq!(r.resolve(7), Some(e));
    }

    #[test]
    fn register_system_handle_zero_auto_allocates() {
        let mut app = build_app();
        let e = app.world_mut().spawn(BridgeCamera { handle: Some(0) }).id();
        app.world_mut()
            .run_system_once(bridge_camera_register_system)
            .unwrap();
        let r = app.world().resource::<CameraRegistry>();
        assert_eq!(r.resolve(1), Some(e));
    }

    #[test]
    fn register_system_conflict_does_not_register() {
        let mut app = build_app();
        let e1 = app.world_mut().spawn(BridgeCamera { handle: Some(5) }).id();
        let _e2 = app.world_mut().spawn(BridgeCamera { handle: Some(5) }).id();
        app.world_mut()
            .run_system_once(bridge_camera_register_system)
            .unwrap();
        let r = app.world().resource::<CameraRegistry>();
        assert_eq!(r.resolve(5), Some(e1));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn register_system_added_fires_once() {
        let mut app = build_app();
        app.world_mut().spawn(BridgeCamera::default());
        app.world_mut()
            .run_system_once(bridge_camera_register_system)
            .unwrap();
        app.world_mut()
            .run_system_once(bridge_camera_register_system)
            .unwrap();
        let r = app.world().resource::<CameraRegistry>();
        assert_eq!(r.len(), 1, "Added<T> 不會在第二次 run 重複觸發");
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;
    use crate::camera::CameraPlugin;
    use std::sync::{Arc, Mutex};
    use vm_bevy_bridge::SharedBridgeState;
    use vm_runtime::BridgeState;

    fn build_app_with_camera_plugin() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // S4：必須設 TimeUpdateStrategy::ManualDuration（FixedUpdate 穩定 fire）
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1.0 / 60.0),
        ));
        // GamePlugin 會 configure GameFixedSet chain；測試需先 configure
        vm_bevy_bridge::configure_game_fixed_set_for_tests(&mut app);
        app.insert_resource(SharedBridgeState(Arc::new(Mutex::new(BridgeState::new()))));
        app.add_plugins(CameraPlugin);
        app
    }

    #[test]
    fn plugin_registers_camera_registry_resource() {
        let app = build_app_with_camera_plugin();
        assert!(app.world().get_resource::<CameraRegistry>().is_some());
    }

    #[test]
    fn plugin_register_system_runs_on_pre_update() {
        let mut app = build_app_with_camera_plugin();
        let e = app.world_mut().spawn(BridgeCamera::default()).id();
        app.update();
        let r = app.world().resource::<CameraRegistry>();
        assert_eq!(r.resolve(1), Some(e), "PreUpdate 應自動註冊");
    }

    #[test]
    fn plugin_unregister_camera_despawn() {
        // M18：透過 plugin schedule 推進，驗證 RemovedComponents 在 PreUpdate 可正常讀取
        let mut app = build_app_with_camera_plugin();
        let e = app.world_mut().spawn(BridgeCamera::default()).id();
        app.update();
        assert_eq!(app.world().resource::<CameraRegistry>().resolve(1), Some(e));

        app.world_mut().entity_mut(e).despawn();
        app.update();

        assert_eq!(app.world().resource::<CameraRegistry>().resolve(1), None);
        assert_eq!(app.world().resource::<CameraRegistry>().len(), 0);
    }
}
