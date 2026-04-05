use std::collections::{BTreeMap, BTreeSet};

/// OTA batch 超時幀數（5s @ 60fps = 300 幀）
pub const BATCH_TIMEOUT_FRAMES: u64 = 300;

/// OTA Batch 狀態（所有 scripts 齊備後觸發 swap）
pub enum AtomicBatchStatus {
    /// 還有 scripts 未到達，繼續等待。
    Pending,
    /// 所有 scripts 到齊，可以執行 atomic swap。
    /// Vec 內容按 script_id 字典序排列（BTreeMap 迭代保證）。
    Ready(Vec<(String, Vec<u8>)>),
}

/// Pending OTA Batch（等待所有 scripts 到達）
struct PendingBatch {
    expected_scripts: BTreeSet<String>,
    received: BTreeMap<String, Vec<u8>>,
    created_at_frame: u64,
}

/// Multi-script 原子 OTA 更新管理器
/// 確保同一批次的所有 scripts 同時替換（all-or-nothing）
pub struct AtomicUpdateManager {
    pending_batches: BTreeMap<u64, PendingBatch>,
}

impl AtomicUpdateManager {
    pub fn new() -> Self {
        Self {
            pending_batches: BTreeMap::new(),
        }
    }

    /// 註冊一批更新中預期的 script IDs
    pub fn register_batch(&mut self, update_id: u64, script_ids: Vec<String>, current_frame: u64) {
        if script_ids.is_empty() {
            tracing::warn!(update_id, "空 script_ids，不建立 batch");
            return;
        }
        if self.pending_batches.contains_key(&update_id) {
            tracing::warn!(update_id, "重複 update_id，覆寫前次 batch");
        }
        let batch = PendingBatch {
            expected_scripts: script_ids.into_iter().collect(),
            received: BTreeMap::new(),
            created_at_frame: current_frame,
        };
        self.pending_batches.insert(update_id, batch);
        tracing::info!(update_id, "OTA batch 已註冊");
    }

    /// 收到一個 script 的完整 bytecode
    pub fn on_script_ready(
        &mut self,
        update_id: u64,
        script_id: String,
        bytecode: Vec<u8>,
    ) -> AtomicBatchStatus {
        let batch = match self.pending_batches.get_mut(&update_id) {
            Some(b) => b,
            None => {
                tracing::warn!(update_id, script_id = %script_id, "收到未知 update_id 的 script");
                return AtomicBatchStatus::Pending;
            }
        };

        if !batch.expected_scripts.contains(&script_id) {
            tracing::warn!(update_id, script_id = %script_id, "收到意外的 script_id，忽略");
            return AtomicBatchStatus::Pending;
        }

        batch.received.insert(script_id.clone(), bytecode);
        tracing::debug!(
            update_id,
            script_id = %script_id,
            received = batch.received.len(),
            expected = batch.expected_scripts.len(),
            "OTA script 到達"
        );

        if batch
            .expected_scripts
            .iter()
            .all(|id| batch.received.contains_key(id))
        {
            let batch = self.pending_batches.remove(&update_id).unwrap();
            let scripts: Vec<(String, Vec<u8>)> = batch.received.into_iter().collect();
            tracing::info!(
                update_id,
                scripts_count = scripts.len(),
                "OTA batch 完整，準備 atomic swap"
            );
            AtomicBatchStatus::Ready(scripts)
        } else {
            AtomicBatchStatus::Pending
        }
    }

    /// 檢查逾時批次，返回逾時的 update_id 清單（已從 pending_batches 移除）
    pub fn check_timeouts(&mut self, current_frame: u64) -> Vec<u64> {
        let mut expired = Vec::new();
        self.pending_batches.retain(|&id, batch| {
            let age = current_frame.saturating_sub(batch.created_at_frame);
            if age >= BATCH_TIMEOUT_FRAMES {
                tracing::warn!(
                    update_id = id,
                    received = batch.received.len(),
                    expected = batch.expected_scripts.len(),
                    "OTA batch 逾時，丟棄整批更新"
                );
                expired.push(id);
                false
            } else {
                true
            }
        });
        expired
    }
}

impl Default for AtomicUpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_script_batch_ready() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["script_a".to_string()], 0);
        let status = mgr.on_script_ready(1, "script_a".to_string(), b"bytecode_a".to_vec());
        assert!(matches!(status, AtomicBatchStatus::Ready(_)));
        if let AtomicBatchStatus::Ready(scripts) = status {
            assert_eq!(scripts.len(), 1);
            assert_eq!(scripts[0].0, "script_a");
            assert_eq!(scripts[0].1, b"bytecode_a");
        }
    }

    #[test]
    fn test_two_scripts_partial_then_ready() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["s1".to_string(), "s2".to_string()], 0);
        let status1 = mgr.on_script_ready(1, "s1".to_string(), b"bc1".to_vec());
        assert!(matches!(status1, AtomicBatchStatus::Pending));
        let status2 = mgr.on_script_ready(1, "s2".to_string(), b"bc2".to_vec());
        assert!(matches!(status2, AtomicBatchStatus::Ready(_)));
    }

    #[test]
    fn test_batch_timeout_discards() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["s1".to_string(), "s2".to_string()], 0);
        mgr.on_script_ready(1, "s1".to_string(), b"bc1".to_vec());
        let expired = mgr.check_timeouts(BATCH_TIMEOUT_FRAMES);
        assert_eq!(expired, vec![1]);
    }

    #[test]
    fn test_batch_within_timeout_not_discarded() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["s1".to_string(), "s2".to_string()], 0);
        mgr.on_script_ready(1, "s1".to_string(), b"bc1".to_vec());
        let expired = mgr.check_timeouts(BATCH_TIMEOUT_FRAMES - 1);
        assert!(expired.is_empty());
    }

    #[test]
    fn test_independent_batches() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["a".to_string(), "b".to_string()], 0);
        mgr.register_batch(2, vec!["c".to_string()], 0);
        assert!(matches!(
            mgr.on_script_ready(1, "a".to_string(), vec![1]),
            AtomicBatchStatus::Pending
        ));
        assert!(matches!(
            mgr.on_script_ready(2, "c".to_string(), vec![2]),
            AtomicBatchStatus::Ready(_)
        ));
    }

    #[test]
    fn test_unknown_update_id_returns_pending() {
        let mut mgr = AtomicUpdateManager::new();
        let status = mgr.on_script_ready(999, "s".to_string(), vec![]);
        assert!(matches!(status, AtomicBatchStatus::Pending));
    }

    #[test]
    fn test_three_scripts_all_arrive() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(
            1,
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
            0,
        );
        assert!(matches!(
            mgr.on_script_ready(1, "x".to_string(), b"bx".to_vec()),
            AtomicBatchStatus::Pending
        ));
        assert!(matches!(
            mgr.on_script_ready(1, "y".to_string(), b"by".to_vec()),
            AtomicBatchStatus::Pending
        ));
        let status = mgr.on_script_ready(1, "z".to_string(), b"bz".to_vec());
        if let AtomicBatchStatus::Ready(scripts) = status {
            assert_eq!(scripts.len(), 3);
        } else {
            panic!("預期 Ready，得到 Pending");
        }
    }

    #[test]
    fn test_ready_batch_removed_from_pending() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["s1".to_string()], 0);
        let status = mgr.on_script_ready(1, "s1".to_string(), b"bc".to_vec());
        assert!(matches!(status, AtomicBatchStatus::Ready(_)));
        let expired = mgr.check_timeouts(BATCH_TIMEOUT_FRAMES + 100);
        assert!(expired.is_empty());
    }

    #[test]
    fn test_zero_scripts_batch_not_registered() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec![], 0);
        let status = mgr.on_script_ready(1, "s".to_string(), vec![]);
        assert!(matches!(status, AtomicBatchStatus::Pending));
    }

    #[test]
    fn test_duplicate_script_id_overwrites() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["a".to_string(), "b".to_string()], 0);
        mgr.on_script_ready(1, "a".to_string(), b"old_a".to_vec());
        mgr.on_script_ready(1, "a".to_string(), b"new_a".to_vec());
        let status = mgr.on_script_ready(1, "b".to_string(), b"bc_b".to_vec());
        if let AtomicBatchStatus::Ready(scripts) = status {
            let a_entry = scripts.iter().find(|(id, _)| id == "a").unwrap();
            assert_eq!(a_entry.1, b"new_a");
        } else {
            panic!("預期 Ready，得到 Pending");
        }
    }

    #[test]
    fn test_unexpected_script_id_ignored() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["a".to_string(), "b".to_string()], 0);
        assert!(matches!(
            mgr.on_script_ready(1, "c".to_string(), b"bc_c".to_vec()),
            AtomicBatchStatus::Pending
        ));
        mgr.on_script_ready(1, "a".to_string(), b"bc_a".to_vec());
        let status = mgr.on_script_ready(1, "b".to_string(), b"bc_b".to_vec());
        if let AtomicBatchStatus::Ready(scripts) = status {
            assert_eq!(scripts.len(), 2);
            assert!(scripts.iter().all(|(id, _)| id == "a" || id == "b"));
        } else {
            panic!("預期 Ready，得到 Pending");
        }
    }

    #[test]
    fn test_ready_vec_sorted_by_script_id() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(
            1,
            vec!["c".to_string(), "a".to_string(), "b".to_string()],
            0,
        );
        mgr.on_script_ready(1, "c".to_string(), b"bc_c".to_vec());
        mgr.on_script_ready(1, "a".to_string(), b"bc_a".to_vec());
        let status = mgr.on_script_ready(1, "b".to_string(), b"bc_b".to_vec());
        if let AtomicBatchStatus::Ready(scripts) = status {
            assert_eq!(scripts[0].0, "a");
            assert_eq!(scripts[1].0, "b");
            assert_eq!(scripts[2].0, "c");
        } else {
            panic!("預期 Ready，得到 Pending");
        }
    }

    #[test]
    fn test_timeout_batch_not_reported_twice() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["s1".to_string(), "s2".to_string()], 0);
        mgr.on_script_ready(1, "s1".to_string(), b"bc1".to_vec());
        assert_eq!(mgr.check_timeouts(BATCH_TIMEOUT_FRAMES), vec![1]);
        assert!(mgr.check_timeouts(BATCH_TIMEOUT_FRAMES + 100).is_empty());
    }

    #[test]
    fn test_multiple_batches_interleaved_arrival() {
        let mut mgr = AtomicUpdateManager::new();
        mgr.register_batch(1, vec!["a".to_string(), "b".to_string()], 0);
        mgr.register_batch(2, vec!["c".to_string(), "d".to_string()], 0);
        assert!(matches!(
            mgr.on_script_ready(1, "a".to_string(), b"a1".to_vec()),
            AtomicBatchStatus::Pending
        ));
        assert!(matches!(
            mgr.on_script_ready(2, "c".to_string(), b"c2".to_vec()),
            AtomicBatchStatus::Pending
        ));
        assert!(matches!(
            mgr.on_script_ready(1, "b".to_string(), b"b1".to_vec()),
            AtomicBatchStatus::Ready(_)
        ));
        assert!(matches!(
            mgr.on_script_ready(2, "d".to_string(), b"d2".to_vec()),
            AtomicBatchStatus::Ready(_)
        ));
    }
}
