//! Netcode 子系統錯誤型別

use std::fmt;

/// Netcode 子系統錯誤型別
///
/// 覆蓋 client 端 prediction/rollback/reconnection 的所有錯誤情境。
/// Workspace 內部型別——不加 #[non_exhaustive]，確保 match 窮舉檢查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetcodeError {
    /// 要求的 tick 快照不在環形緩衝區內
    SnapshotNotFound(u64),
    /// InputBuffer 中存在空缺（from tick 到 to tick 之間缺失）
    InputBufferGap { from: u64, to: u64 },
    /// Resync 過程中 hash 不符（FullSyncPacket 驗證失敗）
    ResyncHashMismatch,
    /// Rollback 深度超過上限（最大 8 幀）
    RollbackDepthExceeded(u64),
}

impl fmt::Display for NetcodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetcodeError::SnapshotNotFound(tick) => {
                write!(f, "快照不存在：tick {}", tick)
            }
            NetcodeError::InputBufferGap { from, to } => {
                write!(f, "輸入緩衝區空缺：tick {} 至 {}", from, to)
            }
            NetcodeError::ResyncHashMismatch => {
                write!(f, "重新同步 hash 不符")
            }
            NetcodeError::RollbackDepthExceeded(depth) => {
                write!(f, "回溯深度超限：{} 幀（上限 8）", depth)
            }
        }
    }
}

impl std::error::Error for NetcodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_not_found_display() {
        let err = NetcodeError::SnapshotNotFound(42);
        let msg = format!("{}", err);
        assert!(msg.contains("42"));
        assert!(msg.contains("快照不存在"));
    }

    #[test]
    fn test_snapshot_not_found_display_zero() {
        let err = NetcodeError::SnapshotNotFound(0);
        let msg = format!("{}", err);
        assert!(msg.contains("0"));
    }

    #[test]
    fn test_input_buffer_gap_display() {
        let err = NetcodeError::InputBufferGap { from: 10, to: 15 };
        let msg = format!("{}", err);
        assert!(msg.contains("10"));
        assert!(msg.contains("15"));
    }

    #[test]
    fn test_resync_mismatch_display() {
        let err = NetcodeError::ResyncHashMismatch;
        let msg = format!("{}", err);
        assert!(msg.contains("重新同步"));
    }

    #[test]
    fn test_rollback_depth_exceeded_display() {
        let err = NetcodeError::RollbackDepthExceeded(9);
        let msg = format!("{}", err);
        assert!(msg.contains("9"));
        assert!(msg.contains("上限 8"));
    }

    #[test]
    fn test_netcode_error_eq_same_variant() {
        assert_eq!(
            NetcodeError::SnapshotNotFound(42),
            NetcodeError::SnapshotNotFound(42)
        );
    }

    #[test]
    fn test_netcode_error_ne_different_value() {
        assert_ne!(
            NetcodeError::RollbackDepthExceeded(8),
            NetcodeError::RollbackDepthExceeded(9)
        );
    }

    #[test]
    fn test_netcode_error_ne_different_variant() {
        assert_ne!(
            NetcodeError::SnapshotNotFound(1),
            NetcodeError::ResyncHashMismatch
        );
    }

    #[test]
    fn test_netcode_error_is_std_error() {
        let err = NetcodeError::SnapshotNotFound(1);
        let _: &dyn std::error::Error = &err;
    }
}
