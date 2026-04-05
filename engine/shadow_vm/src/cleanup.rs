// engine/shadow_vm/src/cleanup.rs

/// 追蹤 Shadow Worker 驗證完成次數，每達到 cleanup_interval 次觸發一次 Cleanup。
pub struct CleanupScheduler {
    validation_count: u64,
    cleanup_interval: u64,
}

impl CleanupScheduler {
    pub fn new() -> Self {
        Self::new_with_interval(60)
    }

    pub fn new_with_interval(interval: u64) -> Self {
        assert!(interval > 0, "cleanup_interval 不可為 0");
        Self {
            validation_count: 0,
            cleanup_interval: interval,
        }
    }

    /// 每次驗證完成後呼叫。若達到 cleanup_interval 次，重置計數並回傳 true（應發送 Cleanup）。
    pub fn on_validation_completed(&mut self) -> bool {
        self.validation_count += 1;
        if self.validation_count >= self.cleanup_interval {
            self.validation_count = 0;
            tracing::info!(
                interval = self.cleanup_interval,
                "Shadow Worker Cleanup 條件達成，發送 Cleanup 命令"
            );
            return true;
        }
        false
    }

    /// 回傳目前累積的驗證次數。
    pub fn validation_count(&self) -> u64 {
        self.validation_count
    }

    /// 回傳設定的 cleanup 間隔。
    pub fn cleanup_interval(&self) -> u64 {
        self.cleanup_interval
    }

    /// 重置計數器（Worker 重啟時呼叫）。
    pub fn reset(&mut self) {
        tracing::debug!(
            old_count = self.validation_count,
            "CleanupScheduler 重置（Worker 重啟）"
        );
        self.validation_count = 0;
    }
}

impl Default for CleanupScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_default_interval() {
        let scheduler = CleanupScheduler::new();
        assert_eq!(scheduler.validation_count(), 0);
        assert_eq!(scheduler.cleanup_interval(), 60);
    }

    #[test]
    fn test_new_with_custom_interval() {
        let scheduler = CleanupScheduler::new_with_interval(10);
        assert_eq!(scheduler.cleanup_interval(), 10);
        assert_eq!(scheduler.validation_count(), 0);
    }

    #[test]
    #[should_panic(expected = "cleanup_interval 不可為 0")]
    fn test_new_with_zero_interval_panics() {
        let _scheduler = CleanupScheduler::new_with_interval(0);
    }

    #[test]
    fn test_cleanup_triggered_at_60_validations() {
        let mut scheduler = CleanupScheduler::new();
        for i in 0..59 {
            let should_cleanup = scheduler.on_validation_completed();
            assert!(!should_cleanup, "第 {} 次不應觸發 cleanup", i + 1);
        }
        let should_cleanup = scheduler.on_validation_completed();
        assert!(should_cleanup, "第 60 次應觸發 cleanup");
    }

    #[test]
    fn test_cleanup_resets_counter() {
        let mut scheduler = CleanupScheduler::new();
        for _ in 0..60 {
            scheduler.on_validation_completed();
        }
        assert_eq!(scheduler.validation_count(), 0);
        for i in 0..59 {
            let trigger = scheduler.on_validation_completed();
            assert!(!trigger, "重置後第 {} 次不應觸發", i + 1);
        }
        let trigger = scheduler.on_validation_completed();
        assert!(trigger, "第二輪第 60 次應觸發");
    }

    #[test]
    fn test_cleanup_reset_on_worker_restart() {
        let mut scheduler = CleanupScheduler::new();
        for _ in 0..50 {
            scheduler.on_validation_completed();
        }
        scheduler.reset();
        assert_eq!(scheduler.validation_count(), 0);
        for _ in 0..59 {
            scheduler.on_validation_completed();
        }
        let trigger = scheduler.on_validation_completed();
        assert!(trigger);
    }

    #[test]
    fn test_validation_count_increments() {
        let mut scheduler = CleanupScheduler::new();
        scheduler.on_validation_completed();
        scheduler.on_validation_completed();
        assert_eq!(scheduler.validation_count(), 2);
    }

    #[test]
    fn test_custom_interval_triggers_correctly() {
        let mut scheduler = CleanupScheduler::new_with_interval(3);
        assert!(!scheduler.on_validation_completed()); // 1
        assert!(!scheduler.on_validation_completed()); // 2
        assert!(scheduler.on_validation_completed()); // 3 → 觸發
        assert_eq!(scheduler.validation_count(), 0); // 已自動重置
    }

    #[test]
    fn test_default_equals_new() {
        let a = CleanupScheduler::default();
        let b = CleanupScheduler::new();
        assert_eq!(a.validation_count(), b.validation_count());
        assert_eq!(a.cleanup_interval(), b.cleanup_interval());
    }
}
