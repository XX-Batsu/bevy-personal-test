//! Server 端 hash 驗證

use std::collections::{BTreeMap, BTreeSet};

use bridge_types::{Blake3Hash, EntityId};
use server_types::HashCheckResult;

/// Server 端 hash 驗證器
///
/// ## 語意規則
/// - `check()` 在 Match 時**不**重置 consecutive_mismatches
/// - 呼叫端需主動呼叫 `reset_client()` 於重連完成或硬同步後
/// - 理由：作弊者偶爾送對 hash 不應自動洗清嫌疑
/// - `SuspiciousUpgrade` 僅在首次達到閾值時觸發，後續超過閾值只回傳 `Mismatch`
pub struct HashChecker {
    /// 各 client 連續 mismatch 計數
    mismatch_counts: BTreeMap<EntityId, u32>,
    /// 已標記為可疑的 client
    suspicious_clients: BTreeSet<EntityId>,
    /// 累計驗證通過次數
    pass_count: u64,
    /// 累計驗證失敗次數
    fail_count: u64,
    /// 升級至可疑的連續 mismatch 閾值
    ///
    /// private：閾值應僅在建構時設定，執行期間不應修改
    /// （若將 3 改為 2，已累計 2 次的 client 不會立即升級，語義不一致）。
    suspicious_threshold: u32,
}

impl HashChecker {
    pub fn new() -> Self {
        Self::with_threshold(3)
    }

    /// `suspicious_threshold == 0` 語意：停用自動升級。
    /// 由於 mismatch 計數從 1 開始，永遠不等於 0，`SuspiciousUpgrade` 永遠不會觸發。
    /// 所有 mismatch 均回傳 `Mismatch`，`is_suspicious()` 永遠回傳 `false`。
    pub fn with_threshold(suspicious_threshold: u32) -> Self {
        Self {
            mismatch_counts: BTreeMap::new(),
            suspicious_clients: BTreeSet::new(),
            pass_count: 0,
            fail_count: 0,
            suspicious_threshold,
        }
    }

    /// 驗證 client 回報的 hash
    ///
    /// - `tick`：此次驗證對應的遊戲幀號，填入 `Mismatch` 結果中。
    /// - Match 時不重置 consecutive_mismatches；需主動呼叫 `reset_client()` 才會清除計數。
    /// - 首次達到 `suspicious_threshold` 回傳 `SuspiciousUpgrade`；之後超過閾值回傳 `Mismatch`，
    ///   不重複觸發升級事件，避免日誌洪泛。
    pub fn check(
        &mut self,
        client_id: EntityId,
        reported_hash: Blake3Hash,
        authoritative_hash: Blake3Hash,
        tick: u64,
    ) -> HashCheckResult {
        if reported_hash == authoritative_hash {
            self.pass_count += 1;
            HashCheckResult::Match
        } else {
            self.fail_count += 1;
            let count = self.mismatch_counts.entry(client_id).or_insert(0);
            *count += 1;
            let consecutive = *count;

            if consecutive == self.suspicious_threshold {
                // 首次達到閾值：升級並通知
                self.suspicious_clients.insert(client_id);
                tracing::warn!(client = client_id.0, consecutive, "Client 標記為可疑");
                HashCheckResult::SuspiciousUpgrade {
                    client_id,
                    consecutive_mismatches: consecutive,
                    client_hash: reported_hash,
                    server_hash: authoritative_hash,
                }
            } else {
                // 低於閾值，或已升級後繼續 mismatch（不重複觸發升級事件）
                HashCheckResult::Mismatch {
                    tick,
                    client_hash: reported_hash,
                    server_hash: authoritative_hash,
                }
            }
        }
    }

    /// 重連完成或硬同步後呼叫，清除 mismatch 計數與可疑標記
    pub fn reset_client(&mut self, client_id: EntityId) {
        self.mismatch_counts.remove(&client_id);
        self.suspicious_clients.remove(&client_id);
    }

    /// 查詢 client 是否已被標記為可疑
    pub fn is_suspicious(&self, client_id: EntityId) -> bool {
        self.suspicious_clients.contains(&client_id)
    }

    /// 累計驗證通過次數
    pub fn pass_count(&self) -> u64 {
        self.pass_count
    }

    /// 累計驗證失敗次數
    pub fn fail_count(&self) -> u64 {
        self.fail_count
    }
}

impl Default for HashChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Match 語意 ──

    #[test]
    fn check_match_returns_match_result() {
        let mut hc = HashChecker::new();
        let hash = [0xAA; 32];
        assert_eq!(hc.check(EntityId(1), hash, hash, 0), HashCheckResult::Match);
    }

    #[test]
    fn check_match_increments_pass_count() {
        let mut hc = HashChecker::new();
        let hash = [0xAA; 32];
        hc.check(EntityId(1), hash, hash, 0);
        assert_eq!(hc.pass_count(), 1);
        assert_eq!(hc.fail_count(), 0);
    }

    // 核心語意：match 不重置 consecutive
    #[test]
    fn match_does_not_reset_consecutive_mismatches() {
        let mut hc = HashChecker::new();
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0); // mismatch 1
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0); // mismatch 2
        hc.check(EntityId(1), [0xAA; 32], [0xAA; 32], 0); // match — 不應重置
                                                          // 再 mismatch 一次應立刻升為 suspicious（累計 3 次）
        let result = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(matches!(result, HashCheckResult::SuspiciousUpgrade { .. }));
    }

    // ── Mismatch 語意 ──

    #[test]
    fn check_mismatch_returns_mismatch_result() {
        let mut hc = HashChecker::new();
        let result = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(matches!(result, HashCheckResult::Mismatch { .. }));
    }

    #[test]
    fn check_mismatch_increments_fail_count() {
        let mut hc = HashChecker::new();
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert_eq!(hc.fail_count(), 1);
        assert_eq!(hc.pass_count(), 0);
    }

    // Fix 3 驗收：tick 正確傳遞至 Mismatch 結果
    #[test]
    fn check_tick_propagated_to_mismatch() {
        let mut hc = HashChecker::new();
        let result = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 42);
        assert!(matches!(result, HashCheckResult::Mismatch { tick: 42, .. }));
    }

    // ── SuspiciousUpgrade 語意 ──

    #[test]
    fn suspicious_upgrade_after_threshold_mismatches() {
        let mut hc = HashChecker::with_threshold(3);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        let result = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(matches!(
            result,
            HashCheckResult::SuspiciousUpgrade {
                consecutive_mismatches: 3,
                ..
            }
        ));
        assert!(hc.is_suspicious(EntityId(1)));
    }

    #[test]
    fn suspicious_upgrade_contains_hash_info() {
        let mut hc = HashChecker::with_threshold(1);
        let client_hash = [0xAA; 32];
        let server_hash = [0xBB; 32];
        let result = hc.check(EntityId(1), client_hash, server_hash, 0);
        match result {
            HashCheckResult::SuspiciousUpgrade {
                client_hash: ch,
                server_hash: sh,
                ..
            } => {
                assert_eq!(ch, client_hash);
                assert_eq!(sh, server_hash);
            }
            _ => panic!("Expected SuspiciousUpgrade"),
        }
    }

    // Fix 5 驗收：SuspiciousUpgrade 只觸發一次，後續超過閾值回傳 Mismatch
    #[test]
    fn suspicious_upgrade_fires_only_once() {
        let mut hc = HashChecker::with_threshold(2);
        // 第 1 次：低於閾值，回傳 Mismatch
        let r1 = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(matches!(r1, HashCheckResult::Mismatch { .. }));
        // 第 2 次：首次達到閾值，回傳 SuspiciousUpgrade
        let r2 = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(matches!(r2, HashCheckResult::SuspiciousUpgrade { .. }));
        assert!(hc.is_suspicious(EntityId(1)));
        // 第 3 次：超過閾值，回傳 Mismatch（不重複觸發升級事件）
        let r3 = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(matches!(r3, HashCheckResult::Mismatch { .. }));
    }

    // ── reset_client 語意 ──

    #[test]
    fn reset_client_clears_consecutive_and_suspicious() {
        let mut hc = HashChecker::with_threshold(2);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0); // suspicious
        assert!(hc.is_suspicious(EntityId(1)));
        hc.reset_client(EntityId(1));
        assert!(!hc.is_suspicious(EntityId(1)));
        // 重置後需重新累積到 threshold 才再升級
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(!hc.is_suspicious(EntityId(1)));
    }

    #[test]
    fn reset_nonexistent_client_is_noop() {
        let mut hc = HashChecker::new();
        hc.reset_client(EntityId(99)); // 不 panic
    }

    // ── 多 client 獨立性 ──

    #[test]
    fn multiple_clients_independent() {
        let mut hc = HashChecker::new();
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        hc.check(EntityId(2), [0xAA; 32], [0xAA; 32], 0);
        assert_eq!(hc.fail_count(), 1);
        assert_eq!(hc.pass_count(), 1);
        assert!(!hc.is_suspicious(EntityId(2)));
    }

    // ── threshold 邊界 ──

    #[test]
    fn threshold_one_marks_suspicious_on_first_mismatch() {
        let mut hc = HashChecker::with_threshold(1);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        assert!(hc.is_suspicious(EntityId(1)));
    }

    // Fix 5 回歸：確認 SuspiciousUpgrade 在多次超過閾值後仍不重複觸發
    #[test]
    fn suspicious_upgrade_fires_only_once_across_many_mismatches() {
        let mut hc = HashChecker::with_threshold(2);
        // 第 1 次：低於閾值 → Mismatch
        assert!(matches!(
            hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 1),
            HashCheckResult::Mismatch { .. }
        ));
        // 第 2 次：首次達到閾值 → SuspiciousUpgrade
        assert!(matches!(
            hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 2),
            HashCheckResult::SuspiciousUpgrade { .. }
        ));
        // 第 3-7 次：超過閾值，全部應為 Mismatch，不得再次觸發 SuspiciousUpgrade
        for tick in 3..=7u64 {
            let result = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], tick);
            assert!(
                matches!(result, HashCheckResult::Mismatch { .. }),
                "第 {tick} 次應為 Mismatch，不應重複觸發 SuspiciousUpgrade"
            );
        }
    }

    // with_threshold(0) 語意驗證：停用升級功能
    #[test]
    fn threshold_zero_never_marks_suspicious() {
        let mut hc = HashChecker::with_threshold(0);
        for tick in 0..10u64 {
            let result = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], tick);
            assert!(
                matches!(result, HashCheckResult::Mismatch { .. }),
                "threshold=0 時 SuspiciousUpgrade 永遠不應觸發"
            );
        }
        assert!(!hc.is_suspicious(EntityId(1)));
    }

    // ── Default trait ──

    #[test]
    fn default_creates_checker_with_threshold_3() {
        // Default::default() 應等同於 new()（threshold=3）
        let mut hc = HashChecker::default();
        // 兩次 mismatch → 仍為 Mismatch（未達閾值）
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 1);
        assert!(!hc.is_suspicious(EntityId(1)));
        // 第三次 → SuspiciousUpgrade
        let r = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 2);
        assert!(matches!(r, HashCheckResult::SuspiciousUpgrade { .. }));
    }

    // ── pass_count / fail_count 多次累積 ──

    #[test]
    fn pass_and_fail_counts_accumulate_across_calls() {
        let mut hc = HashChecker::new();
        let hash = [0x11; 32];
        // 3 次 match，2 次 mismatch
        hc.check(EntityId(1), hash, hash, 0);
        hc.check(EntityId(2), hash, hash, 1);
        hc.check(EntityId(3), hash, hash, 2);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 3);
        hc.check(EntityId(2), [0xAA; 32], [0xBB; 32], 4);
        assert_eq!(hc.pass_count(), 3);
        assert_eq!(hc.fail_count(), 2);
    }

    // reset_client 不影響 pass_count / fail_count（計數器不隨重置清零）
    #[test]
    fn reset_client_does_not_affect_counters() {
        let mut hc = HashChecker::with_threshold(2);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0); // fail
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 1); // fail + suspicious
        assert_eq!(hc.fail_count(), 2);
        hc.reset_client(EntityId(1));
        // 重置後計數器保持不變
        assert_eq!(hc.fail_count(), 2);
        assert_eq!(hc.pass_count(), 0);
    }

    // ── SuspiciousUpgrade client_id 欄位正確 ──

    #[test]
    fn suspicious_upgrade_contains_correct_client_id() {
        let mut hc = HashChecker::with_threshold(1);
        let result = hc.check(EntityId(42), [0xAA; 32], [0xBB; 32], 0);
        match result {
            HashCheckResult::SuspiciousUpgrade { client_id, .. } => {
                assert_eq!(client_id, EntityId(42));
            }
            _ => panic!("Expected SuspiciousUpgrade"),
        }
    }

    // reset_client 後需重新累積到 threshold 才再升級（計數真正歸零）
    #[test]
    fn reset_client_resets_mismatch_count_to_zero() {
        let mut hc = HashChecker::with_threshold(2);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 0); // mismatch count = 1
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 1); // count = 2 → SuspiciousUpgrade
        assert!(hc.is_suspicious(EntityId(1)));
        hc.reset_client(EntityId(1));
        // 重置後：第一次 mismatch 應為 Mismatch（count 歸零，未達閾值）
        let r1 = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 2);
        assert!(
            matches!(r1, HashCheckResult::Mismatch { .. }),
            "重置後第一次 mismatch 不應直接升級（計數應從 0 重新累積）"
        );
        assert!(!hc.is_suspicious(EntityId(1)));
        // 再 mismatch 一次才達到閾值
        let r2 = hc.check(EntityId(1), [0xAA; 32], [0xBB; 32], 3);
        assert!(matches!(r2, HashCheckResult::SuspiciousUpgrade { .. }));
    }

    // ── is_suspicious 對未知 client 回傳 false ──

    #[test]
    fn is_suspicious_returns_false_for_unknown_client() {
        let hc = HashChecker::new();
        assert!(!hc.is_suspicious(EntityId(999)));
    }
}
