//! Client-Server 版本綁定模組。
//!
//! 提供 [`ClientVersion`]（版本資訊）與 [`VersionChecker`]（相容性檢查）。
//! 版本比對為字串精確比較（pkg_version + build_hash 雙重比對）。
//!
//! ## 版本嵌入
//! `BUILD_VERSION` 為 `Cargo.toml` 版本號，`BUILD_HASH` 為 git commit hash。
//! 由 `build.rs`（vergen）在 compile time 注入。

use serde::{Deserialize, Serialize};

/// Cargo.toml 版本號（compile time 嵌入）
pub const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Git commit hash（compile time 嵌入）
///
/// 使用 `VERGEN_GIT_SHA` 環境變數（由 vergen build script 注入）。
/// 若未設定（如無 build.rs），fallback 為 `"unknown"`。
pub const BUILD_HASH: &str = match option_env!("VERGEN_GIT_SHA") {
    Some(hash) => hash,
    None => "unknown",
};

/// Client WASM 版本資訊
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientVersion {
    /// Cargo.toml 版本號
    pub pkg_version: &'static str,
    /// git commit hash（由 vergen 注入）
    pub build_hash: &'static str,
}

/// 版本相容性檢查結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionResult {
    /// 版本相符，可繼續
    Compatible,
    /// 版本不匹配，需強制重載頁面
    IncompatibleReload,
}

/// 版本檢查錯誤
#[derive(Debug, Clone)]
pub enum VersionError {
    /// 網路通訊失敗
    NetworkError(String),
    /// 反序列化/格式錯誤
    ProtocolError(String),
}

/// 版本管理器（純本地同步比較，不涉及網路）
pub struct VersionChecker {
    pub local: ClientVersion,
}

impl VersionChecker {
    /// 建立版本管理器（使用 compile time 嵌入的版本資訊）
    pub fn from_build_info() -> Self {
        Self {
            local: ClientVersion {
                pkg_version: BUILD_VERSION,
                build_hash: BUILD_HASH,
            },
        }
    }

    /// 取得本地版本資訊
    pub fn local_version(&self) -> &ClientVersion {
        &self.local
    }

    /// 與 Server 版本比對，回傳相容性結果
    ///
    /// 同時比對 `pkg_version` 與 `build_hash`（字串精確比對）。
    /// 任一不符即回傳 `IncompatibleReload`（overview.md 不變式 #2）。
    ///
    /// # 版本格式
    /// 任一參數為空字串 → `Err(VersionError::ProtocolError)`。
    ///
    /// # Errors
    /// - `VersionError::ProtocolError` — 版本字串為空
    pub fn check_compatibility(
        &self,
        server_version: &str,
        server_build_hash: &str,
    ) -> Result<VersionResult, VersionError> {
        if server_version.is_empty() || server_build_hash.is_empty() {
            return Err(VersionError::ProtocolError(
                "Server 版本資訊為空".to_string(),
            ));
        }
        if self.local.pkg_version == server_version && self.local.build_hash == server_build_hash {
            Ok(VersionResult::Compatible)
        } else {
            Ok(VersionResult::IncompatibleReload)
        }
    }
}

/// 版本交換訊息結構
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionCheckRequest {
    pub client_version: String,
    pub build_hash: String,
}

/// Server 回應的版本相容性結果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionCheckResponse {
    pub server_version: String,
    pub server_build_hash: String,
    pub compatible: bool,
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_version_embedded() {
        assert!(!BUILD_VERSION.is_empty());
        assert!(!BUILD_HASH.is_empty());
    }

    #[test]
    fn test_same_version_compatible() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker.check_compatibility("1.0.0", "abc123").unwrap();
        assert_eq!(result, VersionResult::Compatible);
    }

    #[test]
    fn test_different_version_incompatible() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker.check_compatibility("1.1.0", "abc123").unwrap();
        assert_eq!(result, VersionResult::IncompatibleReload);
    }

    #[test]
    fn test_version_in_handshake_payload() {
        let req = VersionCheckRequest {
            client_version: "1.0.0".to_string(),
            build_hash: "abc123".to_string(),
        };
        assert_eq!(req.client_version, "1.0.0");
        assert_eq!(req.build_hash, "abc123");
    }

    #[test]
    fn test_empty_version_protocol_error() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker.check_compatibility("", "");
        assert!(matches!(result, Err(VersionError::ProtocolError(_))));
    }

    #[test]
    fn test_malformed_version_incompatible() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker
            .check_compatibility("abc.def.ghi", "abc123")
            .unwrap();
        assert_eq!(result, VersionResult::IncompatibleReload);
    }

    #[test]
    fn test_version_check_request_serialization_roundtrip() {
        let req = VersionCheckRequest {
            client_version: "1.0.0".to_string(),
            build_hash: "abc123def456".to_string(),
        };
        let encoded = bincode::serialize(&req).unwrap();
        let decoded: VersionCheckRequest = bincode::deserialize(&encoded).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn test_version_check_response_serialization_roundtrip() {
        let resp = VersionCheckResponse {
            server_version: "1.0.0".to_string(),
            server_build_hash: "abc123def456".to_string(),
            compatible: true,
        };
        let encoded = bincode::serialize(&resp).unwrap();
        let decoded: VersionCheckResponse = bincode::deserialize(&encoded).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn test_major_version_difference_incompatible() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker.check_compatibility("2.0.0", "abc123").unwrap();
        assert_eq!(result, VersionResult::IncompatibleReload);
    }

    #[test]
    fn test_patch_version_difference_incompatible() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker.check_compatibility("1.0.1", "abc123").unwrap();
        assert_eq!(result, VersionResult::IncompatibleReload);
    }

    #[test]
    fn test_same_version_different_hash() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker.check_compatibility("1.0.0", "def456").unwrap();
        assert_eq!(result, VersionResult::IncompatibleReload);
    }

    #[test]
    fn test_different_version_same_hash() {
        let checker = VersionChecker {
            local: ClientVersion {
                pkg_version: "1.0.0",
                build_hash: "abc123",
            },
        };
        let result = checker.check_compatibility("2.0.0", "abc123").unwrap();
        assert_eq!(result, VersionResult::IncompatibleReload);
    }

    #[test]
    fn test_from_build_info() {
        let checker = VersionChecker::from_build_info();
        assert_eq!(checker.local.pkg_version, BUILD_VERSION);
        assert_eq!(checker.local.build_hash, BUILD_HASH);
    }
}
