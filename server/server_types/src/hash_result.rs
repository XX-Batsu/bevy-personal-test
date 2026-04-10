//! Hash 驗證結果類型

use bridge_types::{Blake3Hash, EntityId};

/// Hash 驗證結果
#[derive(Debug, Clone, PartialEq)]
pub enum HashCheckResult {
    /// Hash 一致
    Match,
    /// Hash 不一致（單次 mismatch）
    Mismatch {
        tick: u64,
        client_hash: Blake3Hash,
        server_hash: Blake3Hash,
    },
    /// 連續 mismatch 超過閾值，升級為可疑 client
    SuspiciousUpgrade {
        client_id: EntityId,
        consecutive_mismatches: u32,
        client_hash: Blake3Hash,
        server_hash: Blake3Hash,
    },
}
