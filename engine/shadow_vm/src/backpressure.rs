use crate::ShadowVmError;

/// Shadow VM 請求佇列的最大 pending 數
pub const MAX_PENDING_REQUESTS: usize = 4;

/// 追蹤 pending Shadow 驗證請求數，防止 Shadow Worker 被洪水攻擊。
/// 當 Shadow Worker 慢於主線程時作為反壓機制。
pub struct ShadowRequestQueue {
    pending_count: usize,
}

impl ShadowRequestQueue {
    pub fn new() -> Self {
        Self { pending_count: 0 }
    }

    /// 嘗試發送一個 Shadow 驗證請求。
    /// 成功：pending_count += 1，回傳 Ok(())
    /// 失敗：pending_count 已達 MAX_PENDING_REQUESTS，回傳 Err(ShadowVmError::QueueFull)
    pub fn try_send(&mut self) -> Result<(), ShadowVmError> {
        if self.pending_count >= MAX_PENDING_REQUESTS {
            tracing::warn!(
                pending = self.pending_count,
                "Shadow VM 請求佇列已滿，跳過本次驗證"
            );
            return Err(ShadowVmError::QueueFull);
        }
        self.pending_count += 1;
        tracing::debug!(pending = self.pending_count, "Shadow 驗證請求已加入佇列");
        Ok(())
    }

    /// Shadow Worker 完成一次驗證後呼叫，降低 pending 計數。
    /// 使用飽和減法：pending_count 為 0 時不 panic。
    pub fn on_response_received(&mut self) {
        self.pending_count = self.pending_count.saturating_sub(1);
        tracing::debug!(
            pending = self.pending_count,
            "Shadow 驗證回應收到，佇列降低"
        );
    }

    pub fn pending(&self) -> usize {
        self.pending_count
    }

    /// 強制重置佇列（Worker 崩潰重啟後使用）
    pub fn reset(&mut self) {
        tracing::info!(
            old_pending = self.pending_count,
            "Shadow 請求佇列重置（Worker 重啟）"
        );
        self.pending_count = 0;
    }
}

impl Default for ShadowRequestQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_initial_state() {
        let queue = ShadowRequestQueue::new();
        assert_eq!(queue.pending(), 0);
    }

    #[test]
    fn test_send_first_request() {
        let mut queue = ShadowRequestQueue::new();
        assert!(queue.try_send().is_ok());
        assert_eq!(queue.pending(), 1);
    }

    #[test]
    fn test_send_up_to_limit() {
        let mut queue = ShadowRequestQueue::new();
        for _ in 0..MAX_PENDING_REQUESTS {
            assert!(queue.try_send().is_ok());
        }
        assert_eq!(queue.pending(), MAX_PENDING_REQUESTS);
    }

    #[test]
    fn test_send_exceeds_limit() {
        let mut queue = ShadowRequestQueue::new();
        for _ in 0..MAX_PENDING_REQUESTS {
            queue.try_send().unwrap();
        }
        let result = queue.try_send();
        assert!(matches!(result, Err(ShadowVmError::QueueFull)));
        assert_eq!(queue.pending(), MAX_PENDING_REQUESTS);
    }

    #[test]
    fn test_response_decreases_pending() {
        let mut queue = ShadowRequestQueue::new();
        queue.try_send().unwrap();
        queue.try_send().unwrap();
        queue.try_send().unwrap();
        queue.on_response_received();
        assert_eq!(queue.pending(), 2);
    }

    #[test]
    fn test_response_then_accept_new() {
        let mut queue = ShadowRequestQueue::new();
        for _ in 0..MAX_PENDING_REQUESTS {
            queue.try_send().unwrap();
        }
        queue.on_response_received();
        assert!(queue.try_send().is_ok());
        assert_eq!(queue.pending(), MAX_PENDING_REQUESTS);
    }

    #[test]
    fn test_response_on_empty_queue_saturates() {
        let mut queue = ShadowRequestQueue::new();
        queue.on_response_received();
        assert_eq!(queue.pending(), 0);
    }

    #[test]
    fn test_reset_clears_pending() {
        let mut queue = ShadowRequestQueue::new();
        queue.try_send().unwrap();
        queue.try_send().unwrap();
        queue.try_send().unwrap();
        assert_eq!(queue.pending(), 3);
        queue.reset();
        assert_eq!(queue.pending(), 0);
        assert!(queue.try_send().is_ok());
        assert_eq!(queue.pending(), 1);
    }
}
