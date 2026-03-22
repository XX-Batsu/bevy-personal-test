//! ScriptMetadata 定義與 bincode 序列化
//!
//! 腳本元資料（§5.2a）— 明文儲存於 header，bincode 序列化。
//! 位於加密區之外，因為 `priority` 和 `script_id`
//! 需要在解密前讀取（排序、版本管理）。

use bridge_types::Blake3Hash;
use serde::{Deserialize, Serialize};

/// 腳本元資料（§5.2a）— 明文儲存於 header，bincode 序列化
///
/// 位於加密區之外（明文），因為 `priority` 和 `script_id`
/// 需要在解密前讀取（排序、版本管理）。
/// Ed25519 簽章範圍覆蓋 metadata bytes + plaintext payload，防止篡改。
///
/// 不使用 #[repr(C)]：含 String（heap-allocated，非 FFI-safe），
/// 序列化穩定性由 bincode 保證，不依賴記憶體佈局。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptMetadata {
    /// 唯一識別碼，e.g. "skill_vfx"
    pub script_id: String,
    /// 執行優先序（0 = 最高，255 = 最低），見 04-lifecycle/multi-script-management.md §4.3
    pub priority: u8,
    /// .rhai 原始碼 blake3 hash，用於 OTA 差異比對
    pub source_hash: Blake3Hash,
    /// Build-time Unix timestamp (seconds)
    /// Note: 使用 std::time::SystemTime — bytecode compiler 是 build tool，
    /// 不受 WASM/Determinism Rules 限制（不在 game logic 路徑上）
    pub build_timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_metadata_bincode_round_trip() {
        let meta = ScriptMetadata {
            script_id: "test_script".to_string(),
            priority: 5,
            source_hash: [0xAB; 32],
            build_timestamp: 1709827200,
        };
        let bytes = bincode::serialize(&meta).unwrap();
        let decoded: ScriptMetadata = bincode::deserialize(&bytes).unwrap();
        assert_eq!(meta, decoded);
    }

    #[test]
    fn script_metadata_with_empty_id() {
        let meta = ScriptMetadata {
            script_id: String::new(),
            priority: 0,
            source_hash: [0; 32],
            build_timestamp: 0,
        };
        let bytes = bincode::serialize(&meta).unwrap();
        let decoded: ScriptMetadata = bincode::deserialize(&bytes).unwrap();
        assert_eq!(meta, decoded);
    }
}
