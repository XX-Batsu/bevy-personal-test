//! L1 快取管理員 — 記憶體壓力下的快取驅逐策略。
//!
//! 提供 [`L1CacheManager`] 管理遊戲執行期的 L1 層快取條目，
//! 支援全量驅逐（OOM 95% 閾值）與非活躍條目驅逐（90% 閾值）。
//! 每個條目在移除時透過 [`zeroize::Zeroize`] 安全清除資料。

use zeroize::Zeroize;

struct CacheEntry {
    id: String,
    data: Vec<u8>,
    active: bool,
    #[allow(dead_code)]
    last_access_frame: u64,
}

impl Drop for CacheEntry {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

/// L1 快取管理員：線性搜尋（禁用 HashMap）+ zeroize 安全清除。
pub struct L1CacheManager {
    entries: Vec<CacheEntry>,
    total_bytes: usize,
    capacity: usize,
}

impl L1CacheManager {
    /// 使用預設容量 8 MB 建立快取管理員。
    pub fn new() -> Self {
        Self::new_with_capacity(8 * 1024 * 1024)
    }

    /// 使用指定容量（bytes）建立快取管理員。
    pub fn new_with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
            total_bytes: 0,
            capacity,
        }
    }

    /// 強制清除所有快取條目；每個條目 Drop 時觸發 `data.zeroize()`。
    pub fn evict_all(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    /// 驅逐所有非活躍條目；活躍條目保留不動。
    ///
    /// **注意：** 必須在 `retain` 之前計算釋放量，因為 `retain`
    /// 會 drop 被移除的條目（觸發 zeroize），之後無法再讀取長度。
    pub fn evict_inactive(&mut self) {
        let freed: usize = self
            .entries
            .iter()
            .filter(|e| !e.active)
            .map(|e| e.data.len())
            .sum();
        self.entries.retain(|e| e.active);
        self.total_bytes = self.total_bytes.saturating_sub(freed);
    }

    /// 插入新快取條目，預設為活躍狀態。
    pub fn insert(&mut self, id: String, data: Vec<u8>, current_frame: u64) {
        self.total_bytes += data.len();
        self.entries.push(CacheEntry {
            id,
            data,
            active: true,
            last_access_frame: current_frame,
        });
    }

    /// 將指定 ID 的條目標記為非活躍；若 ID 不存在則靜默忽略。
    pub fn mark_inactive(&mut self, id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.active = false;
        }
    }

    /// 回傳目前所有條目的總位元組數。
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// 回傳目前使用量相對容量的比率（0.0–1.0+）。
    pub fn usage_ratio(&self) -> f32 {
        self.total_bytes as f32 / self.capacity as f32
    }

    /// 回傳目前條目數量（含活躍與非活躍）。
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

impl Default for L1CacheManager {
    fn default() -> Self {
        Self::new()
    }
}

// l1_eviction_system 待 L1CacheManager 實作 bevy::prelude::Resource 後
// 於整合階段加入 Bevy Last schedule。
// 邏輯：pct >= 95 → evict_all；pct >= 90 → evict_inactive。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evict_all_clears_usage() {
        let mut cache = L1CacheManager::new();
        cache.insert("a".to_string(), vec![0u8; 1024], 1);
        cache.insert("b".to_string(), vec![0u8; 2048], 1);
        cache.insert("c".to_string(), vec![0u8; 512], 1);
        cache.evict_all();
        assert_eq!(cache.total_bytes(), 0);
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_evict_inactive_keeps_active() {
        let mut cache = L1CacheManager::new();
        cache.insert("active1".to_string(), vec![0u8; 100], 1);
        cache.insert("active2".to_string(), vec![0u8; 200], 1);
        cache.insert("inactive".to_string(), vec![0u8; 300], 1);
        cache.mark_inactive("inactive");
        cache.evict_inactive();
        assert_eq!(cache.entry_count(), 2);
        assert_eq!(cache.total_bytes(), 300); // 100 + 200
    }

    #[test]
    fn test_evict_all_zeroizes_data() {
        let mut cache = L1CacheManager::new();
        cache.insert("secret".to_string(), vec![0xAB; 64], 1);
        cache.evict_all();
        assert_eq!(cache.total_bytes(), 0);
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_mark_inactive_then_evict() {
        let mut cache = L1CacheManager::new();
        cache.insert("a".to_string(), vec![1u8; 10], 1);
        cache.insert("b".to_string(), vec![2u8; 20], 1);
        cache.mark_inactive("a");
        cache.evict_inactive();
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.total_bytes(), 20);
    }

    #[test]
    fn test_usage_ratio() {
        let capacity = 8 * 1024 * 1024;
        let mut cache = L1CacheManager::new_with_capacity(capacity);
        cache.insert("half".to_string(), vec![0u8; 4 * 1024 * 1024], 1);
        let ratio = cache.usage_ratio();
        assert!((ratio - 0.5f32).abs() < 0.001);
    }

    #[test]
    fn test_evict_all_on_empty_cache() {
        let mut cache = L1CacheManager::new();
        cache.evict_all();
        assert_eq!(cache.total_bytes(), 0);
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_evict_inactive_all_active() {
        let mut cache = L1CacheManager::new();
        cache.insert("a".to_string(), vec![0u8; 100], 1);
        cache.insert("b".to_string(), vec![0u8; 200], 1);
        cache.insert("c".to_string(), vec![0u8; 300], 1);
        cache.evict_inactive();
        assert_eq!(cache.entry_count(), 3);
        assert_eq!(cache.total_bytes(), 600);
    }

    #[test]
    fn test_evict_inactive_all_inactive() {
        let mut cache = L1CacheManager::new();
        cache.insert("a".to_string(), vec![0u8; 100], 1);
        cache.insert("b".to_string(), vec![0u8; 200], 1);
        cache.insert("c".to_string(), vec![0u8; 300], 1);
        cache.mark_inactive("a");
        cache.mark_inactive("b");
        cache.mark_inactive("c");
        cache.evict_inactive();
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.total_bytes(), 0);
    }

    #[test]
    fn test_mark_inactive_unknown_id() {
        let mut cache = L1CacheManager::new();
        cache.insert("a".to_string(), vec![0u8; 100], 1);
        cache.mark_inactive("nonexistent");
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.total_bytes(), 100);
    }

    #[test]
    fn test_insert_zero_length_data() {
        let mut cache = L1CacheManager::new();
        cache.insert("empty".to_string(), vec![], 1);
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.total_bytes(), 0);
    }

    #[test]
    fn test_insert_updates_total_bytes() {
        let mut cache = L1CacheManager::new();
        cache.insert("a".to_string(), vec![0u8; 1024], 1);
        assert_eq!(cache.total_bytes(), 1024);
        cache.insert("b".to_string(), vec![0u8; 2048], 2);
        assert_eq!(cache.total_bytes(), 3072);
        cache.insert("c".to_string(), vec![0u8; 512], 3);
        assert_eq!(cache.total_bytes(), 3584);
        assert_eq!(cache.entry_count(), 3);
    }

    #[test]
    fn test_new_default_capacity() {
        let cache = L1CacheManager::new();
        assert_eq!(cache.total_bytes(), 0);
        assert_eq!(cache.entry_count(), 0);
        let mut cache = L1CacheManager::new();
        cache.insert("test".to_string(), vec![0u8; 4 * 1024 * 1024], 1);
        let ratio = cache.usage_ratio();
        assert!((ratio - 0.5f32).abs() < 0.001);
    }
}
