//! Room 管理器
//!
//! 對齊上游 docs/design/architecture/18-non-functional/overview.md §RoomState 定義

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// 玩家 ID 型別別名（與 ConnectionId 同為 u64，Phase 13 確認映射關係）
pub type PlayerId = u64;

/// 房間 ID 型別別名
pub type RoomId = u64;

/// 房間狀態
///
/// 對齊上游 18-non-functional/overview.md 定義。
/// 純 Rust 伺服器端型別，不跨 FFI、不參與 state hash，無需 #[repr(C)]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomState {
    /// 等待玩家加入
    Lobby,
    /// 遊戲進行中
    InGame,
    /// 遊戲已結束
    Finished,
}

/// 房間資訊
#[derive(Debug, Clone)]
pub struct Room {
    pub room_id: RoomId,
    pub players: Vec<PlayerId>,
    pub state: RoomState,
    pub max_players: usize,
    /// Unix timestamp（毫秒），由 SystemTime::now() 取得
    pub created_at: u64,
}

/// 房間操作錯誤
#[derive(Debug, thiserror::Error)]
pub enum RoomError {
    #[error("房間已滿")]
    RoomFull,

    #[error("找不到房間")]
    RoomNotFound,

    #[error("玩家不在房間中")]
    PlayerNotInRoom,

    #[error("非法狀態轉換：目前狀態 {from:?}，預期 {expected}")]
    InvalidStateTransition {
        from: RoomState,
        expected: &'static str,
    },
}

/// Room 管理器
pub struct RoomManager {
    rooms: HashMap<RoomId, Room>,
    /// 從 1 開始遞增
    next_id: u64,
}

impl RoomManager {
    pub fn new() -> Self {
        RoomManager {
            rooms: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn create_room(&mut self, max_players: usize) -> RoomId {
        let room_id = self.next_id;
        self.next_id += 1;

        // saturating_duration_since 避免時鐘倒轉 panic
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let room = Room {
            room_id,
            players: Vec::new(),
            state: RoomState::Lobby,
            max_players,
            created_at,
        };
        self.rooms.insert(room_id, room);
        tracing::info!("建立房間 room_id={} max_players={}", room_id, max_players);
        room_id
    }

    pub fn join_room(&mut self, room_id: RoomId, player_id: PlayerId) -> Result<(), RoomError> {
        let room = self
            .rooms
            .get_mut(&room_id)
            .ok_or(RoomError::RoomNotFound)?;
        if room.players.len() >= room.max_players {
            return Err(RoomError::RoomFull);
        }
        room.players.push(player_id);
        tracing::info!("玩家 {} 加入房間 {}", player_id, room_id);
        Ok(())
    }

    pub fn leave_room(&mut self, room_id: RoomId, player_id: PlayerId) -> Result<(), RoomError> {
        let room = self
            .rooms
            .get_mut(&room_id)
            .ok_or(RoomError::RoomNotFound)?;
        let pos = room
            .players
            .iter()
            .position(|&p| p == player_id)
            .ok_or(RoomError::PlayerNotInRoom)?;
        room.players.remove(pos);
        tracing::info!("玩家 {} 離開房間 {}", player_id, room_id);
        Ok(())
    }

    pub fn list_rooms(&self) -> Vec<&Room> {
        self.rooms.values().collect()
    }

    pub fn start_room(&mut self, room_id: RoomId) -> Result<(), RoomError> {
        let room = self
            .rooms
            .get_mut(&room_id)
            .ok_or(RoomError::RoomNotFound)?;
        if room.state != RoomState::Lobby {
            return Err(RoomError::InvalidStateTransition {
                from: room.state.clone(),
                expected: "Lobby",
            });
        }
        room.state = RoomState::InGame;
        tracing::info!("房間 {} 開始遊戲", room_id);
        Ok(())
    }

    pub fn finish_room(&mut self, room_id: RoomId) -> Result<(), RoomError> {
        let room = self
            .rooms
            .get_mut(&room_id)
            .ok_or(RoomError::RoomNotFound)?;
        if room.state != RoomState::InGame {
            return Err(RoomError::InvalidStateTransition {
                from: room.state.clone(),
                expected: "InGame",
            });
        }
        room.state = RoomState::Finished;
        tracing::info!("房間 {} 遊戲結束", room_id);
        Ok(())
    }

    pub fn remove_room(&mut self, room_id: RoomId) -> Result<(), RoomError> {
        self.rooms.remove(&room_id).ok_or(RoomError::RoomNotFound)?;
        tracing::info!("移除房間 {}", room_id);
        Ok(())
    }
}

impl Default for RoomManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// create_room → state == Lobby
    #[test]
    fn test_create_room_lobby_state() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);
        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert_eq!(room.state, RoomState::Lobby, "初始狀態應為 Lobby");
        assert_eq!(room.max_players, 4, "max_players 應為 4");
    }

    /// join_room → player 加入
    #[test]
    fn test_join_room_player_added() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);
        manager.join_room(room_id, 1).unwrap();

        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert!(room.players.contains(&1), "player 1 應在房間中");
    }

    /// leave_room → player 移除
    #[test]
    fn test_leave_room_player_removed() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);
        manager.join_room(room_id, 1).unwrap();
        manager.leave_room(room_id, 1).unwrap();

        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert!(!room.players.contains(&1), "player 1 應已離開房間");
    }

    /// 房間滿員 → join 拒絕
    #[test]
    fn test_room_full_reject_join() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(2);
        manager.join_room(room_id, 1).unwrap();
        manager.join_room(room_id, 2).unwrap();

        let result = manager.join_room(room_id, 3);
        assert!(
            matches!(result, Err(RoomError::RoomFull)),
            "第 3 個玩家加入應回傳 RoomFull"
        );
    }

    /// list_rooms 回傳所有房間
    #[test]
    fn test_list_rooms_returns_all() {
        let mut manager = RoomManager::new();
        manager.create_room(4);
        manager.create_room(4);
        manager.create_room(4);

        assert_eq!(manager.list_rooms().len(), 3, "應有 3 個房間");
    }

    /// 完整 lifecycle：Lobby → InGame → Finished
    #[test]
    fn test_room_lifecycle_lobby_ingame_finished() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);

        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert_eq!(room.state, RoomState::Lobby);
        drop(rooms);

        manager.start_room(room_id).unwrap();
        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert_eq!(room.state, RoomState::InGame);
        drop(rooms);

        manager.finish_room(room_id).unwrap();
        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert_eq!(room.state, RoomState::Finished);
    }

    /// Lobby 狀態呼叫 finish_room → InvalidStateTransition
    #[test]
    fn test_finish_room_in_lobby_state_error() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);

        let result = manager.finish_room(room_id);
        assert!(
            matches!(
                result,
                Err(RoomError::InvalidStateTransition {
                    from: RoomState::Lobby,
                    expected: "InGame"
                })
            ),
            "Lobby 狀態呼叫 finish_room 應回傳 InvalidStateTransition"
        );
    }

    /// remove_room 不存在 → RoomNotFound
    #[test]
    fn test_remove_nonexistent_room_error() {
        let mut manager = RoomManager::new();
        let result = manager.remove_room(999);
        assert!(
            matches!(result, Err(RoomError::RoomNotFound)),
            "移除不存在的房間應回傳 RoomNotFound"
        );
    }

    /// leave_room player 不在房間 → PlayerNotInRoom
    #[test]
    fn test_leave_room_player_not_in_room() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);

        let result = manager.leave_room(room_id, 99);
        assert!(
            matches!(result, Err(RoomError::PlayerNotInRoom)),
            "離開不在房間的 player 應回傳 PlayerNotInRoom"
        );
    }

    /// room_id 從 1 開始遞增
    #[test]
    fn test_room_id_increments() {
        let mut manager = RoomManager::new();
        let id1 = manager.create_room(4);
        let id2 = manager.create_room(4);
        let id3 = manager.create_room(4);

        assert_eq!(id1, 1, "第一個 room_id 應為 1");
        assert_eq!(id2, 2, "第二個 room_id 應為 2");
        assert_eq!(id3, 3, "第三個 room_id 應為 3");
    }

    /// join → leave → join 一致性
    #[test]
    fn test_join_leave_join_consistency() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);

        manager.join_room(room_id, 1).unwrap();
        manager.leave_room(room_id, 1).unwrap();
        manager.join_room(room_id, 1).unwrap();

        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert_eq!(
            room.players,
            vec![1],
            "循環 join/leave/join 後 players 應為 [1]"
        );
    }

    /// Finished 狀態呼叫 start_room → InvalidStateTransition
    #[test]
    fn test_start_room_in_finished_state_error() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);

        manager.start_room(room_id).unwrap();
        manager.finish_room(room_id).unwrap();

        let result = manager.start_room(room_id);
        assert!(
            matches!(
                result,
                Err(RoomError::InvalidStateTransition {
                    from: RoomState::Finished,
                    expected: "Lobby"
                })
            ),
            "Finished 狀態呼叫 start_room 應回傳 InvalidStateTransition"
        );
    }

    /// created_at > 0
    #[test]
    fn test_created_at_valid_timestamp() {
        let mut manager = RoomManager::new();
        let room_id = manager.create_room(4);

        let rooms = manager.list_rooms();
        let room = rooms.iter().find(|r| r.room_id == room_id).unwrap();
        assert!(room.created_at > 0, "created_at 應為正整數 Unix timestamp");
    }

    /// RoomState Debug 輸出
    #[test]
    fn test_room_state_debug_output() {
        assert_eq!(format!("{:?}", RoomState::Lobby), "Lobby");
        assert_eq!(format!("{:?}", RoomState::InGame), "InGame");
        assert_eq!(format!("{:?}", RoomState::Finished), "Finished");
    }
}
