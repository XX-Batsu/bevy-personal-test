//! Server 端 hash 驗證

use std::collections::BTreeMap;

use bridge_types::{Blake3Hash, EntityId};

/// 連續 mismatch 超過此閾值視為可疑
const SUSPICIOUS_THRESHOLD: u64 = 3;

/// 每個 client 的 hash 驗證狀態
pub struct ClientHashState {
    pub consecutive_mismatches: u64,
    pub total_mismatches: u64,
    pub is_suspicious: bool,
}

impl ClientHashState {
    pub fn new() -> Self {
        Self {
            consecutive_mismatches: 0,
            total_mismatches: 0,
            is_suspicious: false,
        }
    }
}

impl Default for ClientHashState {
    fn default() -> Self {
        Self::new()
    }
}

/// Server 端 hash 驗證器
pub struct HashChecker {
    clients: BTreeMap<EntityId, ClientHashState>,
}

impl HashChecker {
    pub fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
        }
    }

    /// 驗證 client 回報的 hash
    pub fn check(
        &mut self,
        client_id: EntityId,
        client_hash: Blake3Hash,
        server_hash: Blake3Hash,
    ) -> bool {
        let state = self.clients.entry(client_id).or_default();

        if client_hash == server_hash {
            state.consecutive_mismatches = 0;
            true
        } else {
            state.consecutive_mismatches += 1;
            state.total_mismatches += 1;
            if state.consecutive_mismatches >= SUSPICIOUS_THRESHOLD {
                state.is_suspicious = true;
                tracing::warn!(
                    client = client_id.0,
                    consecutive = state.consecutive_mismatches,
                    "Client 標記為可疑"
                );
            }
            false
        }
    }

    /// 取得 client 狀態
    pub fn get_client_state(&self, client_id: &EntityId) -> Option<&ClientHashState> {
        self.clients.get(client_id)
    }

    /// 移除 client
    pub fn remove_client(&mut self, client_id: &EntityId) {
        self.clients.remove(client_id);
    }
}

impl Default for HashChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_matching_hash() {
        let mut hc = HashChecker::new();
        let hash = [0xAA; 32];
        assert!(hc.check(EntityId(1), hash, hash));
    }

    #[test]
    fn check_mismatching_hash() {
        let mut hc = HashChecker::new();
        assert!(!hc.check(EntityId(1), [0xAA; 32], [0xBB; 32]));
    }

    #[test]
    fn suspicious_after_three_mismatches() {
        let mut hc = HashChecker::new();
        for _ in 0..3 {
            hc.check(EntityId(1), [0xAA; 32], [0xBB; 32]);
        }
        let state = hc.get_client_state(&EntityId(1)).unwrap();
        assert!(state.is_suspicious);
        assert_eq!(state.consecutive_mismatches, 3);
    }

    #[test]
    fn match_resets_consecutive() {
        let mut hc = HashChecker::new();
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32]);
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32]);
        hc.check(EntityId(1), [0xAA; 32], [0xAA; 32]);
        let state = hc.get_client_state(&EntityId(1)).unwrap();
        assert_eq!(state.consecutive_mismatches, 0);
        assert_eq!(state.total_mismatches, 2);
    }

    #[test]
    fn multiple_clients_independent() {
        let mut hc = HashChecker::new();
        hc.check(EntityId(1), [0xAA; 32], [0xBB; 32]);
        hc.check(EntityId(2), [0xAA; 32], [0xAA; 32]);
        let s1 = hc.get_client_state(&EntityId(1)).unwrap();
        let s2 = hc.get_client_state(&EntityId(2)).unwrap();
        assert_eq!(s1.total_mismatches, 1);
        assert_eq!(s2.total_mismatches, 0);
    }

    #[test]
    fn remove_client() {
        let mut hc = HashChecker::new();
        hc.check(EntityId(1), [0xAA; 32], [0xAA; 32]);
        hc.remove_client(&EntityId(1));
        assert!(hc.get_client_state(&EntityId(1)).is_none());
    }
}
