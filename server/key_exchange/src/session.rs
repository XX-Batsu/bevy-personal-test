//! Session key lifecycle 管理
//!
//! `SessionRegistry` 管理每個連線的 session key，斷線時自動清零。

use std::collections::HashMap;
use std::time::Instant;
use zeroize::Zeroizing;

/// 連線 ID 型別別名
pub type ConnectionId = u64;

/// Session 登錄條目（模組私有）
struct SessionEntry {
    session_key: Zeroizing<[u8; 32]>,
    /// 建立時間，用於 Phase 14 key rotation 存活時長判斷
    #[allow(dead_code)]
    created_at: Instant,
}

/// Session key 登錄表
///
/// 使用 `HashMap`（server crate 不受 engine 確定性規則限制）。
/// `SessionEntry.session_key` 使用 `Zeroizing` 包裝，Drop 時自動清零。
pub struct SessionRegistry {
    sessions: HashMap<ConnectionId, SessionEntry>,
}

impl SessionRegistry {
    /// 建立空的 session 登錄表
    pub fn new() -> Self {
        SessionRegistry {
            sessions: HashMap::new(),
        }
    }

    /// 登錄新的 session key
    ///
    /// 若 `conn_id` 已存在，舊的 session key 在 HashMap 覆蓋時自動 Drop 並清零。
    pub fn register(&mut self, conn_id: ConnectionId, session_key: Zeroizing<[u8; 32]>) {
        tracing::info!("登錄 session conn_id={}", conn_id);
        self.sessions.insert(
            conn_id,
            SessionEntry {
                session_key,
                created_at: Instant::now(),
            },
        );
    }

    /// 取得 session key 的借用引用，不存在則回傳 `None`
    ///
    /// 回傳的 `&[u8; 32]` 為借用引用，不得 clone 至非 zeroize 容器。
    pub fn get(&self, conn_id: ConnectionId) -> Option<&[u8; 32]> {
        self.sessions.get(&conn_id).map(|entry| &*entry.session_key)
    }

    /// 移除 session key，`Zeroizing` 在 Drop 時自動清零
    ///
    /// 若 `conn_id` 不存在，靜默忽略（不 panic）。
    pub fn remove(&mut self, conn_id: ConnectionId) {
        tracing::info!("移除 session conn_id={}", conn_id);
        // 回傳值立即 drop，觸發 Zeroizing 清零
        let _ = self.sessions.remove(&conn_id);
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> Zeroizing<[u8; 32]> {
        Zeroizing::new([byte; 32])
    }

    #[test]
    fn test_register_then_get_happy_path() {
        let mut registry = SessionRegistry::new();
        registry.register(7, make_key(0xAB));
        assert_eq!(registry.get(7).unwrap(), &[0xAB; 32]);
    }

    #[test]
    fn test_session_key_zeroized_after_remove() {
        let mut registry = SessionRegistry::new();
        registry.register(1, make_key(0x55));
        registry.remove(1);
        assert!(registry.get(1).is_none());
    }

    #[test]
    fn test_concurrent_sessions_independent_keys() {
        let mut registry = SessionRegistry::new();
        registry.register(1, make_key(0x11));
        registry.register(2, make_key(0x22));
        assert_ne!(
            registry.get(1).unwrap(),
            registry.get(2).unwrap(),
            "兩連線的 session key 應不同"
        );
    }

    #[test]
    fn test_retrieve_nonexistent_connection() {
        let registry = SessionRegistry::new();
        assert!(registry.get(999).is_none(), "不存在的 conn_id 應回傳 None");
    }

    #[test]
    fn test_remove_nonexistent_connection() {
        let mut registry = SessionRegistry::new();
        // 對空 registry 呼叫 remove 不應 panic
        registry.remove(42);
        assert!(registry.get(42).is_none());
    }

    #[test]
    fn test_register_overwrite_existing() {
        let mut registry = SessionRegistry::new();
        registry.register(1, make_key(0xAA));
        registry.register(1, make_key(0xBB));
        assert_eq!(registry.get(1).unwrap(), &[0xBB; 32], "覆蓋後應回傳新值");
    }

    #[test]
    fn test_boundary_conn_id_u64_max() {
        let mut registry = SessionRegistry::new();
        registry.register(u64::MAX, make_key(0xFF));
        assert!(
            registry.get(u64::MAX).is_some(),
            "u64::MAX 應可作為合法 conn_id"
        );
    }

    #[test]
    fn test_many_connections_stress() {
        let mut registry = SessionRegistry::new();
        for i in 0u64..1000 {
            registry.register(i, Zeroizing::new([(i % 256) as u8; 32]));
        }
        for i in 0u64..1000 {
            assert_eq!(
                registry.get(i).unwrap(),
                &[(i % 256) as u8; 32],
                "conn_id={} 值應正確",
                i
            );
        }
        // 移除 500 個（偶數 conn_id）
        for i in (0u64..1000).step_by(2) {
            registry.remove(i);
        }
        // 偶數已移除
        for i in (0u64..1000).step_by(2) {
            assert!(registry.get(i).is_none(), "conn_id={} 應已被移除", i);
        }
        // 奇數仍存在
        for i in (1u64..1000).step_by(2) {
            assert!(registry.get(i).is_some(), "conn_id={} 應仍存在", i);
        }
    }

    #[test]
    fn test_remove_idempotent_multiple_times() {
        let mut registry = SessionRegistry::new();
        registry.register(5, make_key(0x55));
        registry.remove(5);
        registry.remove(5); // 再次移除不應 panic
        assert!(registry.get(5).is_none());
    }

    #[test]
    fn test_register_after_remove_new_entry() {
        let mut registry = SessionRegistry::new();
        registry.register(3, make_key(0xAA));
        registry.remove(3);
        registry.register(3, make_key(0xBB));
        assert_eq!(
            registry.get(3).unwrap(),
            &[0xBB; 32],
            "移除後重新登錄應回傳新值"
        );
    }
}
