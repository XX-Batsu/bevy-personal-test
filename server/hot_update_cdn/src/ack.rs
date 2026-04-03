//! OTA ACK 狀態型別
//!
//! 對齊上游 docs/design/script-engine/07-hot-update/ota-ack.md §7.4

use bridge_types::replay::Blake3Hash;

/// Client 回報的 OTA 更新結果
///
/// 設計依據：上游 ota-ack.md §7.4 定義 Client→Server ACK 僅 Ok/Rollback 兩種狀態。
/// `Unknown` 為 Server 端超時標記（BroadcastManager 內部使用），
/// 不會出現在 Client 回報的 OtaAck.status 中。
#[derive(Debug, Clone, PartialEq)]
pub enum AckStatus {
    /// 切換成功，新腳本 on_init() 執行無誤
    Ok,
    /// 切換失敗並已回退至舊腳本
    Rollback,
    /// Server 端超時標記（5 秒未收到 ACK）——非 Client 回報值
    Unknown,
}

/// Client 回報的 OTA ACK（對齊設計文件 §7.4）
#[derive(Debug, Clone)]
pub struct OtaAck {
    pub update_id: u64,
    pub status: AckStatus,
    pub new_version_hash: Blake3Hash,
    pub tick: u64,
}
