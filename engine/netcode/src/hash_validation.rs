//! State hash 驗證（每 4 tick 上報 server）

use bridge_types::Blake3Hash;

/// 連續 mismatch 超過此閾值視為可疑
pub const SUSPICIOUS_THRESHOLD: u64 = 3;

/// 每 N tick 向 server 上報一次 hash
pub const HASH_REPORT_INTERVAL: u64 = 4;

/// Hash 驗證結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashValidationResult {
    /// Hash 一致
    Match,
    /// Hash 不符（尚未達到可疑閾值）
    Mismatch,
    /// 連續 mismatch 達到或超過 SUSPICIOUS_THRESHOLD(3)
    Suspicious,
}

/// State hash 驗證器
///
/// 追蹤 mismatch 次數與連續 mismatch 計數。
/// 連續 mismatch 達到 SUSPICIOUS_THRESHOLD 時回傳 Suspicious。
/// 重置連續計數由呼叫端驅動（caller-driven reset）。
pub struct HashValidator {
    /// 總 mismatch 次數（單調遞增，不重置）
    mismatch_count: u64,
    /// 連續 mismatch 次數（match 或手動 reset 時歸零）
    consecutive_mismatches: u64,
}

impl HashValidator {
    pub fn new() -> Self {
        Self {
            mismatch_count: 0,
            consecutive_mismatches: 0,
        }
    }

    /// 判斷此 tick 是否應上報 hash
    /// tick % HASH_REPORT_INTERVAL == 0 為 true（含 tick=0）
    pub fn should_report(tick: u64) -> bool {
        tick.is_multiple_of(HASH_REPORT_INTERVAL)
    }

    /// 驗證 local_hash 與 server_hash 是否一致
    ///
    /// Match 時自動重置 consecutive_mismatches。
    pub fn validate(
        &mut self,
        local_hash: Blake3Hash,
        server_hash: Blake3Hash,
    ) -> HashValidationResult {
        if local_hash == server_hash {
            self.consecutive_mismatches = 0;
            HashValidationResult::Match
        } else {
            self.mismatch_count += 1;
            self.consecutive_mismatches += 1;
            if self.consecutive_mismatches >= SUSPICIOUS_THRESHOLD {
                HashValidationResult::Suspicious
            } else {
                HashValidationResult::Mismatch
            }
        }
    }

    /// 手動重置連續 mismatch 計數
    pub fn reset_consecutive(&mut self) {
        self.consecutive_mismatches = 0;
    }

    /// 總 mismatch 次數
    pub fn mismatch_count(&self) -> u64 {
        self.mismatch_count
    }

    /// 連續 mismatch 次數
    pub fn consecutive_mismatches(&self) -> u64 {
        self.consecutive_mismatches
    }
}

impl Default for HashValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_a() -> Blake3Hash {
        [0xAA; 32]
    }

    fn hash_b() -> Blake3Hash {
        [0xBB; 32]
    }

    // === should_report 測試 ===

    #[test]
    fn should_report_tick_zero() {
        assert!(HashValidator::should_report(0));
    }

    #[test]
    fn should_report_tick_4() {
        assert!(HashValidator::should_report(4));
    }

    #[test]
    fn should_report_tick_8() {
        assert!(HashValidator::should_report(8));
    }

    #[test]
    fn should_not_report_tick_1() {
        assert!(!HashValidator::should_report(1));
    }

    #[test]
    fn should_not_report_tick_3() {
        assert!(!HashValidator::should_report(3));
    }

    #[test]
    fn should_report_tick_100() {
        assert!(HashValidator::should_report(100));
    }

    // === validate 測試 ===

    #[test]
    fn validate_match() {
        let mut hv = HashValidator::new();
        assert_eq!(hv.validate(hash_a(), hash_a()), HashValidationResult::Match);
        assert_eq!(hv.mismatch_count(), 0);
    }

    #[test]
    fn validate_mismatch() {
        let mut hv = HashValidator::new();
        assert_eq!(
            hv.validate(hash_a(), hash_b()),
            HashValidationResult::Mismatch
        );
        assert_eq!(hv.mismatch_count(), 1);
        assert_eq!(hv.consecutive_mismatches(), 1);
    }

    #[test]
    fn validate_suspicious_after_three() {
        let mut hv = HashValidator::new();
        assert_eq!(
            hv.validate(hash_a(), hash_b()),
            HashValidationResult::Mismatch
        );
        assert_eq!(
            hv.validate(hash_a(), hash_b()),
            HashValidationResult::Mismatch
        );
        assert_eq!(
            hv.validate(hash_a(), hash_b()),
            HashValidationResult::Suspicious
        );
        assert_eq!(hv.consecutive_mismatches(), 3);
    }

    #[test]
    fn validate_match_resets_consecutive() {
        let mut hv = HashValidator::new();
        hv.validate(hash_a(), hash_b()); // mismatch 1
        hv.validate(hash_a(), hash_b()); // mismatch 2
        hv.validate(hash_a(), hash_a()); // match → reset consecutive
        assert_eq!(hv.consecutive_mismatches(), 0);
        assert_eq!(hv.mismatch_count(), 2); // total not reset
    }

    #[test]
    fn validate_mismatch_count_monotonic() {
        let mut hv = HashValidator::new();
        hv.validate(hash_a(), hash_b());
        hv.validate(hash_a(), hash_a()); // match resets consecutive
        hv.validate(hash_a(), hash_b());
        assert_eq!(hv.mismatch_count(), 2);
        assert_eq!(hv.consecutive_mismatches(), 1);
    }

    #[test]
    fn validate_suspicious_stays_suspicious() {
        let mut hv = HashValidator::new();
        for _ in 0..5 {
            hv.validate(hash_a(), hash_b());
        }
        assert_eq!(
            hv.validate(hash_a(), hash_b()),
            HashValidationResult::Suspicious
        );
        assert_eq!(hv.consecutive_mismatches(), 6);
    }

    #[test]
    fn reset_consecutive_manual() {
        let mut hv = HashValidator::new();
        hv.validate(hash_a(), hash_b());
        hv.validate(hash_a(), hash_b());
        hv.reset_consecutive();
        assert_eq!(hv.consecutive_mismatches(), 0);
        assert_eq!(hv.mismatch_count(), 2); // total preserved
    }

    #[test]
    fn new_validator_is_clean() {
        let hv = HashValidator::new();
        assert_eq!(hv.mismatch_count(), 0);
        assert_eq!(hv.consecutive_mismatches(), 0);
    }

    #[test]
    fn default_same_as_new() {
        let hv = HashValidator::default();
        assert_eq!(hv.mismatch_count(), 0);
        assert_eq!(hv.consecutive_mismatches(), 0);
    }

    #[test]
    fn mismatch_after_reset_starts_from_one() {
        let mut hv = HashValidator::new();
        hv.validate(hash_a(), hash_b());
        hv.validate(hash_a(), hash_b());
        hv.reset_consecutive();
        assert_eq!(
            hv.validate(hash_a(), hash_b()),
            HashValidationResult::Mismatch
        );
        assert_eq!(hv.consecutive_mismatches(), 1);
    }
}
