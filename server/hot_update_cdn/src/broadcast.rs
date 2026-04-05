//! OTA 多 client 廣播管理器與版本登錄表

use crate::ack::{AckStatus, OtaAck};
use crate::chunk::UpdateChunk;
use bridge_types::replay::Blake3Hash;
use key_exchange::session::ConnectionId;
use std::collections::HashMap;

/// Client ACK 狀態（模組私有）
#[derive(Debug, Clone)]
struct ClientAckState {
    status: AckStatus,
    version_hash: Option<Blake3Hash>,
}

/// 廣播 session（模組私有）
struct BroadcastSession {
    /// 保留 chunks 以便 client 重連時重送（session 完成後由 `remove_session` 清理）
    #[allow(dead_code)]
    chunks: Vec<UpdateChunk>,
    client_states: HashMap<ConnectionId, ClientAckState>,
}

/// 廣播進度快照
#[derive(Debug, Clone)]
pub struct BroadcastStatus {
    pub update_id: u64,
    pub total_clients: usize,
    /// 已回應的 client 數量（Ok + Rollback 皆計入）
    /// 語義為「已收到回應」而非「成功更新」——Rollback 也是有效回應。
    /// `acked_clients + unknown_clients.len() == total_clients` 恆成立。
    pub acked_clients: usize,
    pub unknown_clients: Vec<ConnectionId>,
    /// 回報 Rollback 的 client 列表，用於 Server 決策後續處理
    pub rollback_clients: Vec<ConnectionId>,
}

/// OTA 多 client 廣播管理器
///
/// 使用 `HashMap`（server crate 不受 engine 確定性規則限制）。
/// timeout 由外層 caller 管理（tokio::time::timeout），本結構僅管 session 狀態。
pub struct BroadcastManager {
    sessions: HashMap<u64, BroadcastSession>,
}

impl BroadcastManager {
    pub fn new() -> Self {
        BroadcastManager {
            sessions: HashMap::new(),
        }
    }

    /// 註冊廣播 session：記錄 update_id、chunks、client 列表。
    /// 所有 client 初始狀態為 AckStatus::Unknown。
    ///
    /// 超時由外層 caller 管理（tokio::time::timeout），本方法僅負責初始化 session 狀態。
    pub fn start_broadcast(
        &mut self,
        update_id: u64,
        chunks: Vec<UpdateChunk>,
        client_ids: Vec<ConnectionId>,
    ) {
        let client_states = client_ids
            .iter()
            .map(|&id| {
                (
                    id,
                    ClientAckState {
                        status: AckStatus::Unknown,
                        version_hash: None,
                    },
                )
            })
            .collect();

        let session = BroadcastSession {
            chunks,
            client_states,
        };
        self.sessions.insert(update_id, session);
        tracing::info!(
            "廣播 update_id={} 給 {} 個 client",
            update_id,
            client_ids.len()
        );
    }

    /// 處理收到的 client ACK，更新 session 內部狀態。
    /// 若 update_id 不存在則忽略（log warning）。
    pub fn on_ack(&mut self, update_id: u64, client_id: ConnectionId, ack: OtaAck) {
        // debug_assert 防止 Unknown 作為 Client 回報值
        debug_assert_ne!(
            ack.status,
            AckStatus::Unknown,
            "Unknown 為 Server 端標記，不應出現在 client ACK 中"
        );

        if let Some(session) = self.sessions.get_mut(&update_id) {
            if let Some(state) = session.client_states.get_mut(&client_id) {
                if ack.status == AckStatus::Ok {
                    state.version_hash = Some(ack.new_version_hash);
                }
                state.status = ack.status;
            }
        } else {
            tracing::warn!(
                "收到不存在的 update_id={} 的 ACK（client_id={}），忽略",
                update_id,
                client_id
            );
        }
    }

    /// 所有 client 的 status 均非 Unknown 時回傳 true
    pub fn is_complete(&self, update_id: u64) -> bool {
        if let Some(session) = self.sessions.get(&update_id) {
            session
                .client_states
                .values()
                .all(|s| s.status != AckStatus::Unknown)
        } else {
            false
        }
    }

    /// 移除指定 update_id 的廣播 session，回傳是否成功移除。
    ///
    /// 外層 caller 應在 `is_complete(update_id) == true` 後呼叫此方法清理已完成的 session，
    /// 避免長期運行的 server 累積過多已結束的 session 資料。
    pub fn remove_session(&mut self, update_id: u64) -> bool {
        let removed = self.sessions.remove(&update_id).is_some();
        if removed {
            tracing::info!("廣播 session update_id={} 已清理", update_id);
        }
        removed
    }

    /// 批次清理所有已完成的 session（所有 client 皆已回應）。
    ///
    /// 回傳被清理的 update_id 列表。適合在定時排程中呼叫以防止記憶體洩漏。
    pub fn cleanup_completed(&mut self) -> Vec<u64> {
        let completed_ids: Vec<u64> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .client_states
                    .values()
                    .all(|s| s.status != AckStatus::Unknown)
            })
            .map(|(&id, _)| id)
            .collect();

        for &id in &completed_ids {
            self.sessions.remove(&id);
        }

        if !completed_ids.is_empty() {
            tracing::info!(
                "批次清理 {} 個已完成的廣播 session：{:?}",
                completed_ids.len(),
                completed_ids
            );
        }

        completed_ids
    }

    /// 回傳指定 update_id 的廣播進度，不存在則 None
    pub fn get_status(&self, update_id: u64) -> Option<BroadcastStatus> {
        let session = self.sessions.get(&update_id)?;
        let total_clients = session.client_states.len();

        let mut acked_clients = 0usize;
        let mut unknown_clients = Vec::new();
        let mut rollback_clients = Vec::new();

        for (&conn_id, state) in &session.client_states {
            match state.status {
                AckStatus::Unknown => unknown_clients.push(conn_id),
                AckStatus::Ok => acked_clients += 1,
                AckStatus::Rollback => {
                    acked_clients += 1;
                    rollback_clients.push(conn_id);
                }
            }
        }

        // 排序以確保輸出穩定（雖然 server crate 允許 HashMap，但測試中比對 Vec 需要穩定順序）
        unknown_clients.sort();
        rollback_clients.sort();

        Some(BroadcastStatus {
            update_id,
            total_clients,
            acked_clients,
            unknown_clients,
            rollback_clients,
        })
    }
}

impl Default for BroadcastManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Client 版本登錄表
pub struct VersionRegistry {
    versions: HashMap<ConnectionId, Blake3Hash>,
}

impl VersionRegistry {
    pub fn new() -> Self {
        VersionRegistry {
            versions: HashMap::new(),
        }
    }

    pub fn update_version(&mut self, client_id: ConnectionId, version_hash: Blake3Hash) {
        self.versions.insert(client_id, version_hash);
    }

    pub fn get_version(&self, client_id: ConnectionId) -> Option<&Blake3Hash> {
        self.versions.get(&client_id)
    }
}

impl Default for VersionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ack::{AckStatus, OtaAck};

    fn make_ack(update_id: u64, status: AckStatus) -> OtaAck {
        OtaAck {
            update_id,
            status,
            new_version_hash: [0u8; 32],
            tick: 100,
        }
    }

    fn empty_chunks() -> Vec<UpdateChunk> {
        vec![]
    }

    /// 3 clients 全部 ACK Ok → is_complete == true
    #[test]
    fn test_broadcast_3_clients_all_ack_ok() {
        let mut manager = BroadcastManager::new();
        manager.start_broadcast(1, empty_chunks(), vec![1, 2, 3]);

        manager.on_ack(1, 1, make_ack(1, AckStatus::Ok));
        manager.on_ack(1, 2, make_ack(1, AckStatus::Ok));
        manager.on_ack(1, 3, make_ack(1, AckStatus::Ok));

        assert!(manager.is_complete(1), "所有 client ACK 後應 is_complete");
    }

    /// 全部 ACK 後 status 一致性
    #[test]
    fn test_all_ack_marks_complete_status() {
        let mut manager = BroadcastManager::new();
        manager.start_broadcast(1, empty_chunks(), vec![1, 2, 3]);

        manager.on_ack(1, 1, make_ack(1, AckStatus::Ok));
        manager.on_ack(1, 2, make_ack(1, AckStatus::Ok));
        manager.on_ack(1, 3, make_ack(1, AckStatus::Ok));

        let status = manager.get_status(1).unwrap();
        assert_eq!(status.acked_clients, 3, "acked_clients 應為 3");
        assert!(status.unknown_clients.is_empty(), "unknown_clients 應為空");
    }

    /// start_broadcast 後立即查詢：全部 Unknown
    #[test]
    fn test_initial_state_all_unknown() {
        let mut manager = BroadcastManager::new();
        manager.start_broadcast(1, empty_chunks(), vec![1, 2, 3]);

        assert!(!manager.is_complete(1), "初始狀態應 is_complete == false");
        let status = manager.get_status(1).unwrap();
        assert_eq!(
            status.unknown_clients.len(),
            3,
            "初始狀態 unknown_clients 應為 3"
        );
        assert_eq!(status.acked_clients, 0, "初始狀態 acked_clients 應為 0");
    }

    /// 部分 ACK：未回應的標記 Unknown
    #[test]
    fn test_partial_ack_only_missing_marked_unknown() {
        let mut manager = BroadcastManager::new();
        manager.start_broadcast(1, empty_chunks(), vec![1, 2, 3]);

        manager.on_ack(1, 1, make_ack(1, AckStatus::Ok));
        manager.on_ack(1, 2, make_ack(1, AckStatus::Ok));

        let status = manager.get_status(1).unwrap();
        assert_eq!(
            status.unknown_clients,
            vec![3],
            "client 3 應在 unknown_clients"
        );
        assert_eq!(status.acked_clients, 2, "acked_clients 應為 2");
        assert!(!manager.is_complete(1), "部分 ACK 不應 is_complete");
    }

    /// Rollback ACK 追蹤
    #[test]
    fn test_rollback_ack_tracked_in_rollback_clients() {
        let mut manager = BroadcastManager::new();
        manager.start_broadcast(1, empty_chunks(), vec![1]);

        manager.on_ack(1, 1, make_ack(1, AckStatus::Rollback));

        let status = manager.get_status(1).unwrap();
        assert!(
            status.rollback_clients.contains(&1),
            "Rollback client 應在 rollback_clients"
        );
        assert_eq!(status.acked_clients, 1, "Rollback 也算 acked");
    }

    /// 不存在的 update_id → 忽略不 panic
    #[test]
    fn test_on_ack_unknown_update_id_ignored() {
        let mut manager = BroadcastManager::new();
        // 不呼叫 start_broadcast，直接 on_ack
        manager.on_ack(999, 1, make_ack(999, AckStatus::Ok));
        assert!(
            manager.get_status(999).is_none(),
            "不存在的 update_id get_status 應為 None"
        );
    }

    /// VersionRegistry: update → get → 覆蓋 → get 未註冊
    #[test]
    fn test_version_registry_update_and_get() {
        let mut registry = VersionRegistry::new();
        let hash_a = [0xAAu8; 32];
        let hash_b = [0xBBu8; 32];

        registry.update_version(1, hash_a);
        assert_eq!(
            registry.get_version(1),
            Some(&hash_a),
            "get_version 應回傳 hash_a"
        );

        registry.update_version(1, hash_b);
        assert_eq!(
            registry.get_version(1),
            Some(&hash_b),
            "覆蓋後應回傳 hash_b"
        );

        assert_eq!(registry.get_version(2), None, "未註冊的 client 應回傳 None");
    }

    /// 空 client 列表 → is_complete 應 true
    #[test]
    fn test_empty_client_list_broadcast() {
        let mut manager = BroadcastManager::new();
        manager.start_broadcast(1, empty_chunks(), vec![]);

        assert!(manager.is_complete(1), "空 client 列表應立即 is_complete");
        let status = manager.get_status(1).unwrap();
        assert_eq!(status.total_clients, 0, "total_clients 應為 0");
    }
}
