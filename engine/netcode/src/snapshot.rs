//! Ring Buffer 快照緩衝區與玩家輸入緩衝區

use std::collections::BTreeMap;
use std::collections::VecDeque;

use bridge_types::{EcsMirror, PlayerInput};
use serde::{Deserialize, Serialize};

/// Ring Buffer 快照容量（= MAX_ROLLBACK_DEPTH(8) + margin(2)）
pub const SNAPSHOT_CAPACITY: usize = 10;

/// 環形快照緩衝區，儲存最近 N 幀的完整 state snapshot
///
/// 使用 VecDeque 實現 O(1) push_back + pop_front，
/// pre-allocated 容量，穩態寫入零分配。
pub struct SnapshotBuffer<T: Clone> {
    buffer: VecDeque<(u64, T)>,
    capacity: usize,
}

impl<T: Clone> SnapshotBuffer<T> {
    /// 建立容量為 SNAPSHOT_CAPACITY(10) 的空緩衝區
    pub fn new() -> Self {
        Self {
            buffer: VecDeque::with_capacity(SNAPSHOT_CAPACITY),
            capacity: SNAPSHOT_CAPACITY,
        }
    }

    /// 推入新快照；若已達容量上限，淘汰最舊的（pop_front）
    pub fn push(&mut self, tick: u64, snapshot: T) {
        if self.buffer.len() == self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back((tick, snapshot));
    }

    /// 以 tick 號查找快照；線性搜尋（容量 ≤10，O(10)）
    pub fn get(&self, tick: u64) -> Option<&T> {
        self.buffer.iter().find(|(t, _)| *t == tick).map(|(_, s)| s)
    }

    /// 清空所有快照
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// 目前儲存的快照數量
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// 是否為空
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

impl<T: Clone> Default for SnapshotBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// 單幀完整遊戲狀態快照（用於 Rollback 還原）
///
/// 僅儲存確定性狀態（EcsMirror + RNG state），state_hash 由外部
/// compute_state_hash()（Phase 3）按需計算。
///
/// ⚠ 欄位順序為系統契約：bincode 依定義順序序列化
/// （tick → ecs_mirror → rng_state），改變順序將破壞跨版本相容性。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameSnapshot {
    /// 幀序號（邏輯 tick，60Hz）
    pub tick: u64,
    /// ECS 實體狀態快照（VM 側只讀 Mirror）
    pub ecs_mirror: EcsMirror,
    /// PCG-XSH-RR 完整內部狀態：(state: u64, increment: u64) little-endian
    pub rng_state: [u8; 16],
}

/// 玩家輸入緩衝區，BTreeMap 確保 tick 鍵升序（確定性）
pub struct InputBuffer {
    inner: BTreeMap<u64, Vec<PlayerInput>>,
    window: u64,
}

impl InputBuffer {
    pub fn new() -> Self {
        Self {
            inner: BTreeMap::new(),
            window: SNAPSHOT_CAPACITY as u64,
        }
    }

    /// 插入指定 tick 的輸入
    pub fn insert(&mut self, tick: u64, inputs: Vec<PlayerInput>) {
        self.inner.insert(tick, inputs);
    }

    /// 查詢指定 tick 的輸入
    pub fn get(&self, tick: u64) -> Option<&Vec<PlayerInput>> {
        self.inner.get(&tick)
    }

    /// 保留最近 window 個 tick，丟棄舊的
    ///
    /// 語義：保留 tick > current_tick - window 的 entry
    /// 當 current_tick < window 時不丟棄任何 entry（避免 u64 underflow）
    pub fn retain_recent(&mut self, current_tick: u64) {
        if current_tick >= self.window {
            let cutoff = current_tick - self.window;
            self.inner.retain(|&t, _| t > cutoff);
        }
    }

    /// 目前緩衝的 tick 數量
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 是否為空
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, EntityId};

    // === SnapshotBuffer 測試 ===

    #[test]
    fn snapshot_buffer_new_is_empty() {
        let buf: SnapshotBuffer<u32> = SnapshotBuffer::new();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn snapshot_buffer_push_and_get() {
        let mut buf = SnapshotBuffer::new();
        buf.push(1, 100u32);
        buf.push(2, 200u32);
        assert_eq!(buf.get(1), Some(&100));
        assert_eq!(buf.get(2), Some(&200));
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn snapshot_buffer_evicts_oldest_on_overflow() {
        let mut buf = SnapshotBuffer::new();
        for i in 1..=11u64 {
            buf.push(i, i as u32);
        }
        assert_eq!(buf.len(), 10);
        assert_eq!(buf.get(1), None);
        assert_eq!(buf.get(2), Some(&2));
        assert_eq!(buf.get(11), Some(&11));
    }

    #[test]
    fn snapshot_buffer_get_nonexistent_returns_none() {
        let buf: SnapshotBuffer<u32> = SnapshotBuffer::new();
        assert_eq!(buf.get(99), None);
    }

    #[test]
    fn snapshot_buffer_clear() {
        let mut buf = SnapshotBuffer::new();
        for i in 1..=5u64 {
            buf.push(i, i as u32);
        }
        buf.clear();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
        for i in 1..=5u64 {
            assert_eq!(buf.get(i), None);
        }
    }

    #[test]
    fn snapshot_buffer_deterministic_order() {
        let mut buf_a = SnapshotBuffer::new();
        let mut buf_b = SnapshotBuffer::new();
        for i in 1..=15u64 {
            buf_a.push(i, i as u32 * 10);
            buf_b.push(i, i as u32 * 10);
        }
        for tick in 1..=15u64 {
            assert_eq!(buf_a.get(tick), buf_b.get(tick));
        }
        assert_eq!(buf_a.len(), buf_b.len());
    }

    #[test]
    fn snapshot_buffer_default_is_empty() {
        let buf: SnapshotBuffer<u32> = SnapshotBuffer::default();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    // === InputBuffer 測試 ===

    #[test]
    fn input_buffer_new_is_empty() {
        let buf = InputBuffer::new();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn input_buffer_insert_and_get() {
        let mut buf = InputBuffer::new();
        let inputs = vec![PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: DeterministicValue::Unit,
            tick: 5,
        }];
        buf.insert(5, inputs.clone());
        assert_eq!(buf.get(5), Some(&inputs));
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn input_buffer_retain_recent_evicts_old() {
        let mut buf = InputBuffer::new();
        for i in 1..=11u64 {
            buf.insert(i, vec![]);
        }
        buf.retain_recent(11);
        assert_eq!(buf.get(1), None);
        assert!(buf.get(2).is_some());
        assert!(buf.get(11).is_some());
    }

    #[test]
    fn input_buffer_retain_recent_small_tick_no_eviction() {
        let mut buf = InputBuffer::new();
        for i in 1..=5u64 {
            buf.insert(i, vec![]);
        }
        buf.retain_recent(5);
        assert_eq!(buf.len(), 5);
    }

    #[test]
    fn input_buffer_get_nonexistent_returns_none() {
        let buf = InputBuffer::new();
        assert_eq!(buf.get(0), None);
        assert_eq!(buf.get(u64::MAX), None);
    }

    #[test]
    fn input_buffer_deterministic_btreemap_order() {
        let mut buf = InputBuffer::new();
        buf.insert(5, vec![]);
        buf.insert(1, vec![]);
        buf.insert(3, vec![]);
        assert!(buf.get(1).is_some());
        assert!(buf.get(3).is_some());
        assert!(buf.get(5).is_some());
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn game_snapshot_field_sizes() {
        use std::mem::size_of;
        assert_eq!(size_of::<u64>(), 8);
        assert_eq!(size_of::<[u8; 16]>(), 16);
    }
}
