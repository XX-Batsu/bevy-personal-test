use std::collections::HashMap;
use std::time::{Duration, Instant};

use protocol::connection::ConnectionState;
use tracing::info;

/// Session 唯一識別符（64 位元無號整數，從 1 單調遞增）
pub type SessionId = u64;

/// 房間識別符（語義對齊 room.rs 的 RoomId，均為 u64 type alias）
pub type RoomId = u64;

/// 單一連線 Session 的完整狀態
///
/// 注意：`created_at` 使用 `std::time::Instant`，此為 server-only 程式碼，
/// 不受「遊戲邏輯禁止使用 std::time」規則限制。
pub struct Session {
    /// 唯一識別符
    pub id: SessionId,
    /// 連線狀態機（定義於 engine/protocol）
    pub state: ConnectionState,
    /// Session 建立時間（用於 Phase 15 的逾時清理）
    pub created_at: Instant,
    /// 目前加入的房間（None 表示尚未加入房間）
    pub room_id: Option<RoomId>,
}

/// 管理所有活躍連線 Session 的容器
///
/// # 注意事項
/// - timeout 清理邏輯延後至 Phase 15 實作（`purge_expired()` 方法）
/// - 內部使用 `HashMap<SessionId, Session>`，非遊戲邏輯，無需使用 BTreeMap
/// - SessionId 採單調遞增，確保唯一性
pub struct SessionManager {
    sessions: HashMap<SessionId, Session>,
    next_id: SessionId,
    reconnect_timeout: Duration,
}

impl SessionManager {
    /// 建立新的 SessionManager
    pub fn new(reconnect_timeout: Duration) -> Self {
        Self {
            sessions: HashMap::new(),
            next_id: 1,
            reconnect_timeout,
        }
    }

    /// 建立並注冊一個新 Session，回傳其 SessionId
    /// SessionId 從 1 開始單調遞增（0 保留作為無效/哨兵值）
    pub fn register(&mut self) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;
        let session = Session {
            id,
            state: ConnectionState::new(),
            created_at: Instant::now(),
            room_id: None,
        };
        info!(session_id = id, "新 session 已注冊");
        self.sessions.insert(id, session);
        id
    }

    /// 以不可變引用取得 Session
    pub fn get(&self, id: SessionId) -> Option<&Session> {
        self.sessions.get(&id)
    }

    /// 以可變引用取得 Session（用於狀態轉換）
    pub fn get_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.sessions.get_mut(&id)
    }

    /// 移除 Session，回傳被移除的 Session（若不存在則回傳 None）
    pub fn remove(&mut self, id: SessionId) -> Option<Session> {
        let removed = self.sessions.remove(&id);
        if removed.is_some() {
            info!(session_id = id, "session 已移除");
        }
        removed
    }

    /// 回傳目前活躍 Session 數量
    pub fn active_count(&self) -> usize {
        self.sessions.len()
    }

    /// 回傳設定的重連逾時時間
    pub fn reconnect_timeout(&self) -> Duration {
        self.reconnect_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::connection::{State, StateEvent};

    #[test]
    fn register_and_get_session() {
        let mut manager = SessionManager::new(Duration::from_secs(30));
        let id = manager.register();
        let session = manager.get(id).expect("session 應存在");
        assert_eq!(session.id, id);
        // 初始狀態必須為 Connecting
        assert!(matches!(session.state.current(), State::Connecting));
        assert!(session.room_id.is_none());
    }

    #[test]
    fn remove_session() {
        let mut manager = SessionManager::new(Duration::from_secs(30));
        let id = manager.register();
        assert_eq!(manager.active_count(), 1);

        let removed = manager.remove(id);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, id);
        assert_eq!(manager.active_count(), 0);

        // 移除後 get 應回傳 None
        assert!(manager.get(id).is_none());
    }

    #[test]
    fn session_state_transitions() {
        let mut manager = SessionManager::new(Duration::from_secs(30));
        let id = manager.register();

        // 初始: Connecting
        assert!(matches!(
            manager.get(id).unwrap().state.current(),
            State::Connecting
        ));

        // HelloReceived → Handshaking
        {
            let session = manager.get_mut(id).unwrap();
            session
                .state
                .transition(StateEvent::HelloReceived)
                .expect("Connecting → Handshaking 應成功");
        }
        assert!(matches!(
            manager.get(id).unwrap().state.current(),
            State::Handshaking
        ));

        // KeyConfirmed → Ready
        {
            let session = manager.get_mut(id).unwrap();
            session
                .state
                .transition(StateEvent::KeyConfirmed)
                .expect("Handshaking → Ready 應成功");
        }
        assert!(matches!(
            manager.get(id).unwrap().state.current(),
            State::Ready
        ));
    }

    #[test]
    fn active_session_count() {
        let mut manager = SessionManager::new(Duration::from_secs(30));
        assert_eq!(manager.active_count(), 0);

        let id1 = manager.register();
        let id2 = manager.register();
        let id3 = manager.register();
        assert_eq!(manager.active_count(), 3);

        // id 應唯一
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);

        manager.remove(id2);
        assert_eq!(manager.active_count(), 2);
    }

    #[test]
    fn reconnect_timeout_preserved() {
        let timeout = Duration::from_secs(30);
        let manager = SessionManager::new(timeout);
        assert_eq!(manager.reconnect_timeout(), timeout);

        // 驗證不同逾時值亦精確傳遞
        let manager2 = SessionManager::new(Duration::from_millis(500));
        assert_eq!(manager2.reconnect_timeout(), Duration::from_millis(500));
    }

    #[test]
    fn next_id_monotonic() {
        let mut manager = SessionManager::new(Duration::from_secs(30));

        let id1 = manager.register();
        let id2 = manager.register();
        let id3 = manager.register();

        // SessionId 嚴格遞增
        assert!(id1 < id2);
        assert!(id2 < id3);

        // SessionId 從 1 開始（0 保留作為哨兵值）
        assert!(id1 >= 1);
    }

    #[test]
    fn remove_nonexistent_session() {
        let mut manager = SessionManager::new(Duration::from_secs(30));

        // 移除不存在的 id 應回傳 None 而不 panic
        assert!(manager.remove(999).is_none());
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn get_nonexistent_session() {
        let manager = SessionManager::new(Duration::from_secs(30));

        // 取得不存在的 id 應回傳 None 而不 panic
        assert!(manager.get(999).is_none());
    }
}
