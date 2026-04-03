use crate::message::{DisconnectReason, NetMessage};
use serde::{Deserialize, Serialize};

/// 連線狀態
///
/// `Disconnected` 為終態（terminal state），一旦進入無法再轉移。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    /// 初始狀態：TCP/WebSocket 連線已建立，尚未收到握手訊息
    Connecting,
    /// 握手進行中：已收到 ClientHello，等待 KeyConfirm
    Handshaking,
    /// 握手完成：加密通道已建立，等待加入房間
    Ready,
    /// 遊戲進行中：已加入房間，可收發遊戲訊息
    Playing,
    /// 重連中：遊戲中斷線，等待重新握手
    Reconnecting,
    /// 終態：連線已關閉（攜帶斷線原因）
    Disconnected { reason: DisconnectReason },
}

/// 觸發狀態轉移的事件
#[derive(Debug, Clone)]
pub enum StateEvent {
    /// 收到 ClientHello（或重連時的 ClientHello）
    HelloReceived,
    /// 金鑰交換完成（收到有效 KeyConfirm）
    KeyConfirmed,
    /// 成功加入遊戲房間
    JoinedRoom,
    /// 離開房間（但保持連線）
    LeftRoom,
    /// 網路中斷（TCP 連線斷開）
    ConnectionLost,
    /// 明確斷線（主動或被動均可）
    Disconnected(DisconnectReason),
}

/// 非法狀態轉移錯誤
#[derive(Debug)]
pub struct InvalidTransition {
    /// 發生錯誤時的當前狀態
    pub from: State,
    /// 觸發錯誤的事件描述（Debug 格式字串）
    pub event: String,
}

impl std::fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "非法狀態轉移：從 {:?} 遇到事件 {}",
            self.from, self.event
        )
    }
}

impl std::error::Error for InvalidTransition {}

/// 連線狀態機
///
/// 管理單一客戶端連線的生命週期狀態。
/// server/gateway 每個 session 持有一個 ConnectionState 實例。
pub struct ConnectionState {
    state: State,
}

impl ConnectionState {
    /// 建立新的連線狀態機，初始狀態為 Connecting
    pub fn new() -> Self {
        Self {
            state: State::Connecting,
        }
    }

    /// 取得當前狀態（唯讀）
    pub fn current(&self) -> &State {
        &self.state
    }

    /// 執行狀態轉移
    ///
    /// # 檢查順序
    /// 1. 終態檢查：`Disconnected` 拒絕所有事件
    /// 2. 全域 `Disconnected` 轉移：任何非終態 + `StateEvent::Disconnected(reason)` → 進入終態
    /// 3. 一般轉移：精確匹配 `(當前狀態, 事件)` 對
    ///
    /// # Errors
    /// 若事件在當前狀態下不合法，返回 `InvalidTransition`，狀態不改變
    pub fn transition(&mut self, event: StateEvent) -> Result<&State, InvalidTransition> {
        // Step 1: 終態檢查——Disconnected 拒絕所有事件
        if matches!(self.state, State::Disconnected { .. }) {
            return Err(InvalidTransition {
                from: self.state.clone(),
                event: format!("{:?}", event),
            });
        }

        // Step 2: 全域 Disconnected 轉移——任何非終態均可接受
        if let StateEvent::Disconnected(reason) = &event {
            self.state = State::Disconnected {
                reason: reason.clone(),
            };
            return Ok(&self.state);
        }

        // Step 3: 一般轉移——精確匹配 (當前狀態, 事件)
        let next = match (&self.state, &event) {
            (State::Connecting, StateEvent::HelloReceived) => State::Handshaking,
            (State::Handshaking, StateEvent::KeyConfirmed) => State::Ready,
            (State::Ready, StateEvent::JoinedRoom) => State::Playing,
            (State::Playing, StateEvent::LeftRoom) => State::Ready,
            (State::Playing, StateEvent::ConnectionLost) => State::Reconnecting,
            (State::Reconnecting, StateEvent::HelloReceived) => State::Handshaking,
            _ => {
                // clone 僅在錯誤路徑執行，避免 happy path 的額外開銷
                return Err(InvalidTransition {
                    from: self.state.clone(),
                    event: format!("{:?}", event),
                });
            }
        };
        self.state = next;
        Ok(&self.state)
    }

    /// 檢查當前狀態是否允許接收此類訊息（白名單模式）
    ///
    /// 白名單安全模式：新增 `NetMessage` variant 預設被拒絕，需明確加入允許清單（fail-closed）。
    pub fn is_message_allowed(&self, msg: &NetMessage) -> bool {
        match &self.state {
            State::Connecting => matches!(msg, NetMessage::ClientHello { .. }),
            State::Handshaking => matches!(msg, NetMessage::KeyConfirm { .. }),
            State::Ready => matches!(
                msg,
                NetMessage::Ping { .. } | NetMessage::Pong { .. } | NetMessage::Disconnect { .. }
            ),
            State::Playing => matches!(
                msg,
                NetMessage::GameInput { .. }
                    | NetMessage::HashCheck { .. }
                    | NetMessage::OtaAck { .. }
                    | NetMessage::Ping { .. }
                    | NetMessage::Pong { .. }
                    | NetMessage::Disconnect { .. }
            ),
            State::Reconnecting => matches!(msg, NetMessage::ClientHello { .. }),
            State::Disconnected { .. } => false,
        }
    }
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{AckStatus, DisconnectReason, NetMessage};

    /// 輔助函式：建立狀態機並推進至指定狀態
    fn make_state(events: &[StateEvent]) -> ConnectionState {
        let mut s = ConnectionState::new();
        for e in events {
            s.transition(e.clone()).unwrap();
        }
        s
    }
    fn to_handshaking() -> ConnectionState {
        make_state(&[StateEvent::HelloReceived])
    }
    fn to_ready() -> ConnectionState {
        make_state(&[StateEvent::HelloReceived, StateEvent::KeyConfirmed])
    }
    fn to_playing() -> ConnectionState {
        make_state(&[
            StateEvent::HelloReceived,
            StateEvent::KeyConfirmed,
            StateEvent::JoinedRoom,
        ])
    }
    fn to_reconnecting() -> ConnectionState {
        make_state(&[
            StateEvent::HelloReceived,
            StateEvent::KeyConfirmed,
            StateEvent::JoinedRoom,
            StateEvent::ConnectionLost,
        ])
    }

    // ===== 合法狀態轉移（7 條邊 + 3 條 Disconnected 全域）=====

    #[test]
    fn initial_state_is_connecting() {
        assert!(matches!(
            ConnectionState::new().current(),
            State::Connecting
        ));
    }
    #[test]
    fn connecting_to_handshaking() {
        let mut s = ConnectionState::new();
        assert!(s.transition(StateEvent::HelloReceived).is_ok());
        assert!(matches!(s.current(), State::Handshaking));
    }
    #[test]
    fn handshaking_to_ready() {
        let mut s = to_handshaking();
        assert!(s.transition(StateEvent::KeyConfirmed).is_ok());
        assert!(matches!(s.current(), State::Ready));
    }
    #[test]
    fn ready_to_playing() {
        let mut s = to_ready();
        assert!(s.transition(StateEvent::JoinedRoom).is_ok());
        assert!(matches!(s.current(), State::Playing));
    }
    #[test]
    fn playing_to_ready_via_left_room() {
        let mut s = to_playing();
        assert!(s.transition(StateEvent::LeftRoom).is_ok());
        assert!(matches!(s.current(), State::Ready));
    }
    #[test]
    fn playing_to_reconnecting() {
        let mut s = to_playing();
        assert!(s.transition(StateEvent::ConnectionLost).is_ok());
        assert!(matches!(s.current(), State::Reconnecting));
    }
    #[test]
    fn reconnecting_to_handshaking() {
        let mut s = to_reconnecting();
        assert!(s.transition(StateEvent::HelloReceived).is_ok());
        assert!(matches!(s.current(), State::Handshaking));
    }
    #[test]
    fn connecting_to_disconnected() {
        let mut s = ConnectionState::new();
        assert!(s
            .transition(StateEvent::Disconnected(DisconnectReason::ServerShutdown))
            .is_ok());
        assert!(matches!(s.current(), State::Disconnected { .. }));
    }
    #[test]
    fn playing_to_disconnected() {
        let mut s = to_playing();
        assert!(s
            .transition(StateEvent::Disconnected(DisconnectReason::CheatDetected))
            .is_ok());
        assert!(matches!(
            s.current(),
            State::Disconnected {
                reason: DisconnectReason::CheatDetected
            }
        ));
    }
    #[test]
    fn ready_to_disconnected() {
        let mut s = to_ready();
        assert!(s
            .transition(StateEvent::Disconnected(DisconnectReason::Timeout))
            .is_ok());
        assert!(matches!(
            s.current(),
            State::Disconnected {
                reason: DisconnectReason::Timeout
            }
        ));
    }
    #[test]
    fn disconnected_preserves_reason() {
        let s = make_state(&[StateEvent::Disconnected(DisconnectReason::CheatDetected)]);
        assert_eq!(
            s.current(),
            &State::Disconnected {
                reason: DisconnectReason::CheatDetected
            }
        );
    }

    // ===== 非法轉移（錯誤路徑）=====

    #[test]
    fn invalid_transition_rejected() {
        let mut s = ConnectionState::new();
        assert!(s.transition(StateEvent::JoinedRoom).is_err());
        assert!(matches!(s.current(), State::Connecting));
    }
    #[test]
    fn invalid_connecting_key_confirmed() {
        let mut s = ConnectionState::new();
        assert!(s.transition(StateEvent::KeyConfirmed).is_err());
        assert!(matches!(s.current(), State::Connecting));
    }
    #[test]
    fn invalid_connecting_left_room() {
        let mut s = ConnectionState::new();
        assert!(s.transition(StateEvent::LeftRoom).is_err());
    }
    #[test]
    fn invalid_connecting_connection_lost() {
        let mut s = ConnectionState::new();
        assert!(s.transition(StateEvent::ConnectionLost).is_err());
    }
    #[test]
    fn invalid_handshaking_joined_room() {
        let mut s = to_handshaking();
        assert!(s.transition(StateEvent::JoinedRoom).is_err());
        assert!(matches!(s.current(), State::Handshaking));
    }
    #[test]
    fn invalid_ready_hello() {
        let mut s = to_ready();
        assert!(s.transition(StateEvent::HelloReceived).is_err());
        assert!(matches!(s.current(), State::Ready));
    }
    #[test]
    fn invalid_ready_connection_lost() {
        let mut s = to_ready();
        assert!(s.transition(StateEvent::ConnectionLost).is_err());
        assert!(matches!(s.current(), State::Ready));
    }
    #[test]
    fn invalid_reconnecting_left_room() {
        let mut s = to_reconnecting();
        assert!(s.transition(StateEvent::LeftRoom).is_err());
        assert!(matches!(s.current(), State::Reconnecting));
    }
    #[test]
    fn invalid_reconnecting_joined_room() {
        let mut s = to_reconnecting();
        assert!(s.transition(StateEvent::JoinedRoom).is_err());
        assert!(matches!(s.current(), State::Reconnecting));
    }
    #[test]
    fn invalid_reconnecting_key_confirmed() {
        let mut s = to_reconnecting();
        assert!(s.transition(StateEvent::KeyConfirmed).is_err());
        assert!(matches!(s.current(), State::Reconnecting));
    }

    // ===== 終態保護 =====

    #[test]
    fn disconnected_is_terminal() {
        // 迴圈測試所有非 Disconnected 的 StateEvent，確保終態拒絕一切事件
        let all_events = [
            StateEvent::HelloReceived,
            StateEvent::KeyConfirmed,
            StateEvent::JoinedRoom,
            StateEvent::LeftRoom,
            StateEvent::ConnectionLost,
        ];
        for event in &all_events {
            let mut s = make_state(&[StateEvent::Disconnected(DisconnectReason::Normal)]);
            assert!(
                s.transition(event.clone()).is_err(),
                "Disconnected 狀態應拒絕事件 {:?}",
                event
            );
        }
    }
    #[test]
    fn disconnected_rejects_disconnect_event() {
        let mut s = make_state(&[StateEvent::Disconnected(DisconnectReason::Normal)]);
        assert!(s
            .transition(StateEvent::Disconnected(DisconnectReason::Timeout))
            .is_err());
        assert_eq!(
            s.current(),
            &State::Disconnected {
                reason: DisconnectReason::Normal
            }
        );
    }

    // ===== is_message_allowed：每個狀態完整覆蓋 =====

    #[test]
    fn is_message_allowed_connecting() {
        let s = ConnectionState::new();
        assert!(s.is_message_allowed(&NetMessage::ClientHello {
            public_key: [0; 32]
        }));
        assert!(!s.is_message_allowed(&NetMessage::GameInput {
            tick: 0,
            inputs: vec![]
        }));
        assert!(!s.is_message_allowed(&NetMessage::Ping { timestamp: 0 }));
        assert!(!s.is_message_allowed(&NetMessage::KeyConfirm {
            encrypted_ack: vec![],
            nonce: [0; 12]
        }));
    }
    #[test]
    fn is_message_allowed_handshaking() {
        let s = to_handshaking();
        assert!(s.is_message_allowed(&NetMessage::KeyConfirm {
            encrypted_ack: vec![1, 2, 3],
            nonce: [0; 12]
        }));
        assert!(!s.is_message_allowed(&NetMessage::ClientHello {
            public_key: [0; 32]
        }));
        assert!(!s.is_message_allowed(&NetMessage::GameInput {
            tick: 0,
            inputs: vec![]
        }));
        assert!(!s.is_message_allowed(&NetMessage::Ping { timestamp: 0 }));
    }
    #[test]
    fn is_message_allowed_ready() {
        let s = to_ready();
        assert!(s.is_message_allowed(&NetMessage::Ping { timestamp: 0 }));
        assert!(s.is_message_allowed(&NetMessage::Pong { timestamp: 0 }));
        assert!(s.is_message_allowed(&NetMessage::Disconnect {
            reason: DisconnectReason::Normal
        }));
        assert!(!s.is_message_allowed(&NetMessage::GameInput {
            tick: 0,
            inputs: vec![]
        }));
        assert!(!s.is_message_allowed(&NetMessage::ClientHello {
            public_key: [0; 32]
        }));
    }
    #[test]
    fn is_message_allowed_playing() {
        let s = to_playing();
        // 允許（對齊 task-07：GameInput / HashCheck / OtaAck / Ping / Pong / Disconnect）
        assert!(s.is_message_allowed(&NetMessage::GameInput {
            tick: 0,
            inputs: vec![]
        }));
        assert!(s.is_message_allowed(&NetMessage::HashCheck {
            tick: 0,
            hash: [0; 32]
        }));
        assert!(s.is_message_allowed(&NetMessage::OtaAck {
            update_id: 1,
            status: AckStatus::Ok,
            version_hash: [0; 32],
            tick: 100,
        }));
        assert!(s.is_message_allowed(&NetMessage::Ping { timestamp: 0 }));
        assert!(s.is_message_allowed(&NetMessage::Pong { timestamp: 0 }));
        assert!(s.is_message_allowed(&NetMessage::Disconnect {
            reason: DisconnectReason::Normal
        }));
        // 拒絕
        assert!(!s.is_message_allowed(&NetMessage::ClientHello {
            public_key: [0; 32]
        }));
        assert!(!s.is_message_allowed(&NetMessage::KeyConfirm {
            encrypted_ack: vec![],
            nonce: [0; 12]
        }));
    }
    #[test]
    fn is_message_allowed_reconnecting() {
        let s = to_reconnecting();
        assert!(s.is_message_allowed(&NetMessage::ClientHello {
            public_key: [0; 32]
        }));
        assert!(!s.is_message_allowed(&NetMessage::GameInput {
            tick: 0,
            inputs: vec![]
        }));
        assert!(!s.is_message_allowed(&NetMessage::Ping { timestamp: 0 }));
        assert!(!s.is_message_allowed(&NetMessage::KeyConfirm {
            encrypted_ack: vec![],
            nonce: [0; 12]
        }));
    }
    #[test]
    fn is_message_allowed_disconnected() {
        let s = make_state(&[StateEvent::Disconnected(DisconnectReason::Normal)]);
        assert!(!s.is_message_allowed(&NetMessage::ClientHello {
            public_key: [0; 32]
        }));
        assert!(!s.is_message_allowed(&NetMessage::GameInput {
            tick: 0,
            inputs: vec![]
        }));
        assert!(!s.is_message_allowed(&NetMessage::Ping { timestamp: 0 }));
        assert!(!s.is_message_allowed(&NetMessage::Disconnect {
            reason: DisconnectReason::Normal
        }));
    }
}
