//! Replay 系統錯誤型別

use std::fmt;

/// Replay 系統錯誤型別
///
/// 覆蓋 replay 錄製（save）與回放（load）的所有錯誤情境。
/// Workspace 內部型別——不加 #[non_exhaustive]，確保 match 窮舉檢查。
#[derive(Debug)]
pub enum ReplayError {
    /// 不支援的 replay 格式版本
    UnsupportedVersion(u16),
    /// bincode 序列化失敗（save 路徑）
    SerializationFailed(bincode::Error),
    /// bincode 反序列化失敗（load 路徑）
    DeserializationFailed(bincode::Error),
    /// gzip 壓縮或解壓縮失敗（I/O 錯誤）
    DecompressionFailed(std::io::Error),
    /// 空的 replay（無幀資料）
    EmptyReplay,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::UnsupportedVersion(v) => write!(f, "不支援的 replay 版本：{}", v),
            ReplayError::SerializationFailed(e) => write!(f, "序列化失敗：{}", e),
            ReplayError::DeserializationFailed(e) => write!(f, "反序列化失敗：{}", e),
            ReplayError::DecompressionFailed(e) => write!(f, "解壓縮失敗：{}", e),
            ReplayError::EmptyReplay => write!(f, "空的 replay（無幀資料）"),
        }
    }
}

impl std::error::Error for ReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReplayError::SerializationFailed(e) => Some(e),
            ReplayError::DeserializationFailed(e) => Some(e),
            ReplayError::DecompressionFailed(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn test_replay_unsupported_version_display() {
        let err = ReplayError::UnsupportedVersion(99);
        let msg = format!("{}", err);
        assert!(msg.contains("99"));
        assert!(msg.contains("不支援"));
    }

    #[test]
    fn test_replay_serialization_source() {
        // 產生一個 bincode::Error：嘗試反序列化不正確的資料
        let bad: Result<u64, _> = bincode::deserialize(&[0xFF]);
        let bincode_err = bad.unwrap_err();
        let err = ReplayError::SerializationFailed(bincode_err);
        assert!(err.source().is_some());
    }

    #[test]
    fn test_replay_deserialization_source() {
        let bad: Result<u64, _> = bincode::deserialize(&[0xFF]);
        let bincode_err = bad.unwrap_err();
        let err = ReplayError::DeserializationFailed(bincode_err);
        assert!(err.source().is_some());
    }

    #[test]
    fn test_replay_decompression_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "test");
        let err = ReplayError::DecompressionFailed(io_err);
        assert!(err.source().is_some());
    }

    #[test]
    fn test_replay_empty_replay_source() {
        let err = ReplayError::EmptyReplay;
        assert!(err.source().is_none());
    }

    #[test]
    fn test_replay_empty_replay_display() {
        let err = ReplayError::EmptyReplay;
        let msg = format!("{}", err);
        assert!(msg.contains("空的 replay"));
    }

    #[test]
    fn test_replay_unsupported_version_matches() {
        let err = ReplayError::UnsupportedVersion(0);
        assert!(matches!(err, ReplayError::UnsupportedVersion(0)));
    }

    #[test]
    fn test_replay_error_is_std_error() {
        let err = ReplayError::EmptyReplay;
        let _: &dyn std::error::Error = &err;
    }
}
