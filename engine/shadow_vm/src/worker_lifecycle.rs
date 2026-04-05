//! Shadow Worker 崩潰恢復狀態機
//!
//! 追蹤 Web Worker 生命週期狀態，並以滑動視窗限制重啟頻率（最多 3 次 / 3600 幀）。

use crate::ShadowVmError;

/// 每滑動視窗最多允許重啟次數
pub const MAX_RESTARTS_PER_WINDOW: u32 = 3;
/// 滑動視窗長度（幀數）
pub const RESTART_WINDOW_FRAMES: u64 = 3600;

/// Shadow Worker 生命週期狀態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Alive,
    Crashed,
    Restarting,
    Dead,
}

/// Shadow Worker 生命週期追蹤器
pub struct WorkerLifecycle {
    state: WorkerState,
    restart_count: u32,
    last_restart_window_start: u64,
    worker_url: String,
}

impl WorkerLifecycle {
    pub fn new(worker_url: String) -> Self {
        Self {
            state: WorkerState::Alive,
            restart_count: 0,
            last_restart_window_start: 0,
            worker_url,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.state == WorkerState::Alive
    }

    pub fn state(&self) -> WorkerState {
        self.state
    }

    /// Worker 崩潰事件
    /// Dead 為吸收態，不允許狀態轉換
    pub fn on_error(&mut self, current_frame: u64) {
        if self.state == WorkerState::Dead {
            tracing::warn!(
                frame = current_frame,
                "Shadow Worker 已 Dead，忽略 on_error"
            );
            return;
        }
        tracing::error!(frame = current_frame, state = ?self.state, "Shadow Worker 崩潰");
        self.state = WorkerState::Crashed;
    }

    /// 啟動重啟（increment-then-check 語義）
    pub fn start_restart(&mut self, current_frame: u64) -> Result<(), ShadowVmError> {
        if self.state == WorkerState::Dead {
            return Err(ShadowVmError::TooManyRestarts);
        }

        // 檢查視窗
        if current_frame.saturating_sub(self.last_restart_window_start) >= RESTART_WINDOW_FRAMES {
            self.restart_count = 0;
            self.last_restart_window_start = current_frame;
        }

        // increment-then-check
        self.restart_count += 1;
        if self.restart_count >= MAX_RESTARTS_PER_WINDOW {
            self.state = WorkerState::Dead;
            tracing::error!(
                restart_count = self.restart_count,
                "Shadow Worker 重啟次數達到上限，放棄 anti-cheat 驗證"
            );
            return Err(ShadowVmError::TooManyRestarts);
        }

        self.state = WorkerState::Restarting;
        tracing::info!(
            restart_count = self.restart_count,
            worker_url = %self.worker_url,
            "Shadow Worker 重啟中"
        );
        Ok(())
    }

    /// 收到 ShadowInitAck，完成重啟
    pub fn on_init_ack(&mut self) {
        if self.state == WorkerState::Restarting {
            self.state = WorkerState::Alive;
            tracing::info!("Shadow Worker 重啟完成");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_starts_alive() {
        let lifecycle = WorkerLifecycle::new("worker.js".to_string());
        assert_eq!(lifecycle.state(), WorkerState::Alive);
        assert!(lifecycle.is_alive());
    }

    #[test]
    fn test_alive_to_crashed() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(1);
        assert_eq!(lifecycle.state(), WorkerState::Crashed);
        assert!(!lifecycle.is_alive());
    }

    #[test]
    fn test_restarting_to_crashed() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(1);
        lifecycle.start_restart(1).unwrap();
        assert_eq!(lifecycle.state(), WorkerState::Restarting);
        lifecycle.on_error(50);
        assert_eq!(lifecycle.state(), WorkerState::Crashed);
    }

    #[test]
    fn test_on_error_dead_stays_dead() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        // 強制達到 Dead（3 次崩潰）
        lifecycle.on_error(0);
        lifecycle.start_restart(0).unwrap();
        lifecycle.on_init_ack();
        lifecycle.on_error(100);
        lifecycle.start_restart(100).unwrap();
        lifecycle.on_init_ack();
        lifecycle.on_error(200);
        let _ = lifecycle.start_restart(200); // → Dead
        assert_eq!(lifecycle.state(), WorkerState::Dead);
        lifecycle.on_error(999);
        assert_eq!(lifecycle.state(), WorkerState::Dead);
    }

    #[test]
    fn test_start_restart_from_crashed() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(1);
        let result = lifecycle.start_restart(1);
        assert!(result.is_ok());
        assert_eq!(lifecycle.state(), WorkerState::Restarting);
    }

    #[test]
    fn test_init_ack_restores_alive() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(1);
        lifecycle.start_restart(1).unwrap();
        lifecycle.on_init_ack();
        assert_eq!(lifecycle.state(), WorkerState::Alive);
        assert!(lifecycle.is_alive());
    }

    #[test]
    fn test_init_ack_from_alive_noop() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        assert_eq!(lifecycle.state(), WorkerState::Alive);
        lifecycle.on_init_ack();
        assert_eq!(lifecycle.state(), WorkerState::Alive);
    }

    #[test]
    fn test_init_ack_from_crashed_noop() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(1);
        assert_eq!(lifecycle.state(), WorkerState::Crashed);
        lifecycle.on_init_ack();
        assert_eq!(lifecycle.state(), WorkerState::Crashed);
    }

    #[test]
    fn test_init_ack_from_dead_noop() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(0);
        lifecycle.start_restart(0).unwrap();
        lifecycle.on_init_ack();
        lifecycle.on_error(100);
        lifecycle.start_restart(100).unwrap();
        lifecycle.on_init_ack();
        lifecycle.on_error(200);
        let _ = lifecycle.start_restart(200); // → Dead
        assert_eq!(lifecycle.state(), WorkerState::Dead);
        lifecycle.on_init_ack();
        assert_eq!(lifecycle.state(), WorkerState::Dead);
    }

    #[test]
    fn test_full_restart_cycle() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(1);
        lifecycle.start_restart(1).unwrap();
        assert_eq!(lifecycle.state(), WorkerState::Restarting);
        lifecycle.on_init_ack();
        assert_eq!(lifecycle.state(), WorkerState::Alive);
        assert!(lifecycle.is_alive());
    }

    #[test]
    fn test_max_restarts_in_window() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        // 第 1 次重啟成功
        lifecycle.on_error(0);
        lifecycle.start_restart(0).unwrap();
        lifecycle.on_init_ack();
        // 第 2 次重啟成功
        lifecycle.on_error(100);
        lifecycle.start_restart(100).unwrap();
        lifecycle.on_init_ack();
        // 第 3 次 → restart_count 遞增後達到 MAX(3) → Dead
        lifecycle.on_error(200);
        let result = lifecycle.start_restart(200);
        assert!(matches!(result, Err(ShadowVmError::TooManyRestarts)));
        assert_eq!(lifecycle.state(), WorkerState::Dead);
    }

    #[test]
    fn test_restart_resets_after_window() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(0);
        lifecycle.start_restart(0).unwrap();
        lifecycle.on_init_ack();
        lifecycle.on_error(100);
        lifecycle.start_restart(100).unwrap();
        lifecycle.on_init_ack();
        // 超過視窗（3601 幀），重啟計數應重置
        lifecycle.on_error(3601);
        let result = lifecycle.start_restart(3601);
        assert!(result.is_ok());
        assert_eq!(lifecycle.state(), WorkerState::Restarting);
    }

    #[test]
    fn test_dead_state_cannot_restart() {
        let mut lifecycle = WorkerLifecycle::new("worker.js".to_string());
        lifecycle.on_error(0);
        lifecycle.start_restart(0).unwrap();
        lifecycle.on_init_ack();
        lifecycle.on_error(100);
        lifecycle.start_restart(100).unwrap();
        lifecycle.on_init_ack();
        lifecycle.on_error(200);
        let _ = lifecycle.start_restart(200); // → Dead
        assert_eq!(lifecycle.state(), WorkerState::Dead);
        lifecycle.on_error(300);
        let result = lifecycle.start_restart(300);
        assert!(matches!(result, Err(ShadowVmError::TooManyRestarts)));
        assert_eq!(lifecycle.state(), WorkerState::Dead);
    }
}
