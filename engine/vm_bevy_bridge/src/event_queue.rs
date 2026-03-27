//! BridgeEventQueue、BridgeEntityMap、BridgeDiagnostics 定義。

use bevy::prelude::*;
use std::collections::BTreeMap;

use bridge_types::{BridgeEvent, EntityId};
use vm_runtime::EventQueue;

/// 每幀事件佇列容量上限
const MAX_QUEUE_CAPACITY: usize = 4096;

/// VM → ECS 事件佇列（Bevy Resource）
///
/// 每幀由 `flush_bridge_events` system 消費。
/// 容量上限 4,096 events/frame，超出時丟棄並記錄 dropped 計數。
#[derive(Resource)]
pub struct BridgeEventQueue {
    events: Vec<BridgeEvent>,
    dropped: usize,
}

impl Default for BridgeEventQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl BridgeEventQueue {
    pub fn new() -> Self {
        Self {
            events: Vec::with_capacity(256),
            dropped: 0,
        }
    }

    /// 從 Phase 8 EventQueue 搬運事件（呼叫 drain() 取得 owned Vec）。
    pub fn transfer_from(&mut self, event_queue: &mut EventQueue) {
        for event in event_queue.drain() {
            self.push(event);
        }
    }

    /// 超過 4,096 時丟棄並遞增 dropped，log error。
    pub fn push(&mut self, event: BridgeEvent) {
        if self.events.len() >= MAX_QUEUE_CAPACITY {
            self.dropped += 1;
            tracing::error!(
                dropped = self.dropped,
                "BridgeEventQueue 溢出，event 已丟棄"
            );
            return;
        }
        self.events.push(event);
    }

    /// 取出所有事件，重設 dropped = 0。Capacity 保留。
    pub fn drain(&mut self) -> std::vec::Drain<'_, BridgeEvent> {
        self.dropped = 0;
        self.events.drain(..)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn dropped_count(&self) -> usize {
        self.dropped
    }

    /// 過濾保留符合條件的事件，移除的數量計入 dropped。
    ///
    /// 供 Task 11 fallback 系統使用，移除已停用腳本產生的事件。
    pub fn retain(&mut self, f: impl FnMut(&BridgeEvent) -> bool) {
        let before = self.events.len();
        self.events.retain(f);
        let removed = before - self.events.len();
        self.dropped += removed;
    }
}

/// VM EntityId ↔ Bevy Entity 雙向映射
///
/// 使用 BTreeMap 確保確定性迭代順序（README.md Determinism Rules）。
#[derive(Resource, Default)]
pub struct BridgeEntityMap {
    vm_to_bevy: BTreeMap<EntityId, Entity>,
    bevy_to_vm: BTreeMap<Entity, EntityId>,
}

impl BridgeEntityMap {
    pub fn insert(&mut self, vm_id: EntityId, bevy_entity: Entity) {
        self.vm_to_bevy.insert(vm_id, bevy_entity);
        self.bevy_to_vm.insert(bevy_entity, vm_id);
    }

    pub fn get_bevy(&self, vm_id: EntityId) -> Option<Entity> {
        self.vm_to_bevy.get(&vm_id).copied()
    }

    pub fn get_vm(&self, bevy_entity: Entity) -> Option<EntityId> {
        self.bevy_to_vm.get(&bevy_entity).copied()
    }

    pub fn remove_by_vm(&mut self, vm_id: EntityId) -> Option<Entity> {
        self.vm_to_bevy.remove(&vm_id).inspect(|e| {
            self.bevy_to_vm.remove(e);
        })
    }

    pub fn remove_by_bevy(&mut self, bevy_entity: Entity) -> Option<EntityId> {
        self.bevy_to_vm.remove(&bevy_entity).inspect(|id| {
            self.vm_to_bevy.remove(id);
        })
    }

    pub fn len(&self) -> usize {
        self.vm_to_bevy.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vm_to_bevy.is_empty()
    }
}

/// Bridge flush 效能診斷資料
#[derive(Resource, Default)]
pub struct BridgeDiagnostics {
    /// 本幀 flush 耗時
    pub flush_duration: std::time::Duration,
    /// 本幀處理的 event 總數
    pub events_processed: usize,
    /// 本幀 Queue 溢出丟棄數
    pub events_dropped: usize,
    /// 累計丟棄總數（跨幀）
    pub total_events_dropped: u64,
}

/// 遊戲實體類型 ID（由腳本指定）
#[derive(Component)]
pub struct GameEntityTypeId(pub i64);

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::BridgeEvent;

    #[test]
    fn test_queue_overflow_discards_excess() {
        let mut q = BridgeEventQueue::new();
        for _ in 0..5000 {
            q.push(BridgeEvent::SpawnEntity { type_id: 1 });
        }
        assert_eq!(q.len(), 4096);
        assert_eq!(q.dropped_count(), 904);
    }

    #[test]
    fn test_drain_resets_dropped_count() {
        let mut q = BridgeEventQueue::new();
        for _ in 0..5000 {
            q.push(BridgeEvent::SpawnEntity { type_id: 1 });
        }
        assert_eq!(q.dropped_count(), 904);
        let _: Vec<_> = q.drain().collect();
        assert_eq!(q.dropped_count(), 0);
        assert_eq!(q.len(), 0);
        q.push(BridgeEvent::SpawnEntity { type_id: 2 });
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn test_transfer_from_event_queue() {
        let mut eq = EventQueue::new();
        eq.push(BridgeEvent::SpawnEntity { type_id: 1 });
        eq.push(BridgeEvent::SpawnEntity { type_id: 2 });
        let mut bq = BridgeEventQueue::new();
        bq.transfer_from(&mut eq);
        assert_eq!(bq.len(), 2);
        assert!(eq.is_empty());
    }

    #[test]
    fn test_entity_map_bidirectional_sync() {
        let mut map = BridgeEntityMap::default();
        let vm_id = EntityId(42);
        // 使用 Bevy World 建立 Entity
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        map.insert(vm_id, entity);
        assert_eq!(map.get_bevy(vm_id), Some(entity));
        assert_eq!(map.get_vm(entity), Some(vm_id));
    }

    #[test]
    fn test_entity_map_remove_bidirectional() {
        let mut map = BridgeEntityMap::default();
        let vm_id = EntityId(42);
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        map.insert(vm_id, entity);
        let removed = map.remove_by_vm(vm_id);
        assert_eq!(removed, Some(entity));
        assert!(map.get_bevy(vm_id).is_none());
        assert!(map.get_vm(entity).is_none());
    }
}
