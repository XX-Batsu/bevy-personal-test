use std::collections::HashMap;

/// 房間唯一識別碼
pub type RoomId = u64;

/// 連線唯一識別碼
pub type ConnectionId = u64;

/// 房間狀態機
/// 合法轉移：Waiting → Running → Finished
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomState {
    /// 等待玩家加入（可加入、不可開始）
    Waiting,
    /// 遊戲進行中（不可加入新玩家）
    Running,
    /// 遊戲已結束（只讀，等待清理）
    Finished,
}

/// Room Manager 操作錯誤
#[derive(Debug, PartialEq, Eq)]
pub enum RoomError {
    /// 指定的房間不存在
    NotFound,
    /// 房間已達人數上限
    Full,
    /// 當前狀態不允許此操作
    InvalidState,
    /// 玩家不在房間中
    PlayerNotFound,
}

/// 單一房間資料
#[derive(Debug, Clone)]
pub struct Room {
    pub id: RoomId,
    pub state: RoomState,
    pub players: Vec<ConnectionId>,
    pub max_players: usize,
}

/// 房間管理器
/// server crate 不受 determinism 規則限制，可使用 HashMap
pub struct RoomManager {
    rooms: HashMap<RoomId, Room>,
    next_id: RoomId,
    /// Phase 13 骨架階段不使用；Phase 15 整合層補齊 backpressure 邏輯
    #[allow(dead_code)]
    max_rooms: usize,
}

impl RoomManager {
    /// 建立新的 RoomManager，max_rooms 限制可同時存在的房間數
    pub fn new(max_rooms: usize) -> Self {
        Self {
            rooms: HashMap::new(),
            next_id: 1,
            max_rooms,
        }
    }

    /// 建立新房間，回傳新 RoomId（從 1 開始遞增）
    /// Phase 13 骨架不檢查 max_rooms，backpressure 留至 Phase 15 整合層
    pub fn create_room(&mut self, max_players: usize) -> RoomId {
        let id = self.next_id;
        self.next_id += 1;
        let room = Room {
            id,
            state: RoomState::Waiting,
            players: Vec::new(),
            max_players,
        };
        self.rooms.insert(id, room);
        id
    }

    /// 取得房間的唯讀參考
    pub fn get_room(&self, room_id: RoomId) -> Option<&Room> {
        self.rooms.get(&room_id)
    }

    /// 玩家加入房間（僅允許 Waiting 狀態）
    /// 檢查順序：NotFound → InvalidState → Full
    pub fn join_room(&mut self, room_id: RoomId, conn_id: ConnectionId) -> Result<(), RoomError> {
        let room = self.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        if room.state != RoomState::Waiting {
            return Err(RoomError::InvalidState);
        }
        if room.players.len() >= room.max_players {
            return Err(RoomError::Full);
        }
        room.players.push(conn_id);
        Ok(())
    }

    /// 玩家離開房間（不限狀態）
    pub fn leave_room(&mut self, room_id: RoomId, conn_id: ConnectionId) -> Result<(), RoomError> {
        let room = self.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        if !room.players.contains(&conn_id) {
            return Err(RoomError::PlayerNotFound);
        }
        room.players.retain(|&id| id != conn_id);
        Ok(())
    }

    /// 將房間狀態從 Waiting 推進到 Running
    pub fn start_room(&mut self, room_id: RoomId) -> Result<(), RoomError> {
        let room = self.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        if room.state != RoomState::Waiting {
            return Err(RoomError::InvalidState);
        }
        room.state = RoomState::Running;
        Ok(())
    }

    /// 將房間狀態從 Running 推進到 Finished
    pub fn finish_room(&mut self, room_id: RoomId) -> Result<(), RoomError> {
        let room = self.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        if room.state != RoomState::Running {
            return Err(RoomError::InvalidState);
        }
        room.state = RoomState::Finished;
        Ok(())
    }

    /// 回傳所有房間 ID 的快照（順序不保證）
    pub fn list_rooms(&self) -> Vec<RoomId> {
        self.rooms.keys().copied().collect()
    }

    /// 移除房間（僅允許 Finished 狀態）
    pub fn remove_room(&mut self, room_id: RoomId) -> Result<(), RoomError> {
        let room = self.rooms.get(&room_id).ok_or(RoomError::NotFound)?;
        if room.state != RoomState::Finished {
            return Err(RoomError::InvalidState);
        }
        self.rooms.remove(&room_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_room() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        let room = mgr.get_room(id).expect("房間應存在");
        assert_eq!(room.state, RoomState::Waiting);
        assert_eq!(room.max_players, 4);
        assert!(room.players.is_empty());
    }

    #[test]
    fn join_and_leave_room() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        assert_eq!(mgr.join_room(id, 1), Ok(()));
        assert_eq!(mgr.get_room(id).unwrap().players.len(), 1);
        assert_eq!(mgr.leave_room(id, 1), Ok(()));
        assert_eq!(mgr.get_room(id).unwrap().players.len(), 0);
    }

    #[test]
    fn room_full_rejects_join() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(1);
        assert_eq!(mgr.join_room(id, 1), Ok(()));
        assert_eq!(mgr.join_room(id, 2), Err(RoomError::Full));
    }

    #[test]
    fn join_nonexistent_room_fails() {
        let mut mgr = RoomManager::new(10);
        assert_eq!(mgr.join_room(999, 1), Err(RoomError::NotFound));
    }

    #[test]
    fn join_running_room_fails() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        mgr.start_room(id).unwrap();
        assert_eq!(mgr.join_room(id, 1), Err(RoomError::InvalidState));
    }

    #[test]
    fn room_lifecycle_waiting_running_finished() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        assert_eq!(mgr.get_room(id).unwrap().state, RoomState::Waiting);
        mgr.start_room(id).unwrap();
        assert_eq!(mgr.get_room(id).unwrap().state, RoomState::Running);
        mgr.finish_room(id).unwrap();
        assert_eq!(mgr.get_room(id).unwrap().state, RoomState::Finished);
    }

    #[test]
    fn list_rooms() {
        let mut mgr = RoomManager::new(10);
        let id1 = mgr.create_room(4);
        let id2 = mgr.create_room(4);
        let id3 = mgr.create_room(4);
        let list = mgr.list_rooms();
        assert_eq!(list.len(), 3);
        assert!(list.contains(&id1));
        assert!(list.contains(&id2));
        assert!(list.contains(&id3));
    }

    #[test]
    fn list_rooms_empty() {
        let mgr = RoomManager::new(10);
        let list = mgr.list_rooms();
        assert!(list.is_empty());
    }

    #[test]
    fn remove_room() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        mgr.start_room(id).unwrap();
        mgr.finish_room(id).unwrap();
        assert_eq!(mgr.remove_room(id), Ok(()));
        assert!(mgr.get_room(id).is_none());
    }

    #[test]
    fn cannot_remove_running_room() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        mgr.start_room(id).unwrap();
        assert_eq!(mgr.remove_room(id), Err(RoomError::InvalidState));
    }

    #[test]
    fn cannot_remove_waiting_room() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        assert_eq!(mgr.remove_room(id), Err(RoomError::InvalidState));
    }

    #[test]
    fn remove_nonexistent_room_fails() {
        let mut mgr = RoomManager::new(10);
        assert_eq!(mgr.remove_room(999), Err(RoomError::NotFound));
    }

    #[test]
    fn leave_nonexistent_room_fails() {
        let mut mgr = RoomManager::new(10);
        assert_eq!(mgr.leave_room(999, 1), Err(RoomError::NotFound));
    }

    #[test]
    fn leave_nonexistent_player_fails() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        assert_eq!(mgr.leave_room(id, 999), Err(RoomError::PlayerNotFound));
    }

    #[test]
    fn start_nonexistent_room_fails() {
        let mut mgr = RoomManager::new(10);
        assert_eq!(mgr.start_room(999), Err(RoomError::NotFound));
    }

    #[test]
    fn waiting_state_finish_room_fails() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        assert_eq!(mgr.finish_room(id), Err(RoomError::InvalidState));
    }

    #[test]
    fn finished_state_start_room_fails() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        mgr.start_room(id).unwrap();
        mgr.finish_room(id).unwrap();
        assert_eq!(mgr.start_room(id), Err(RoomError::InvalidState));
    }

    #[test]
    fn finish_nonexistent_room_fails() {
        let mut mgr = RoomManager::new(10);
        assert_eq!(mgr.finish_room(999), Err(RoomError::NotFound));
    }

    #[test]
    fn join_finished_room_fails() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        mgr.start_room(id).unwrap();
        mgr.finish_room(id).unwrap();
        assert_eq!(mgr.join_room(id, 1), Err(RoomError::InvalidState));
    }

    #[test]
    fn waiting_state_cannot_skip_to_finished() {
        let mut mgr = RoomManager::new(10);
        let id = mgr.create_room(4);
        assert_eq!(mgr.finish_room(id), Err(RoomError::InvalidState));
    }
}
