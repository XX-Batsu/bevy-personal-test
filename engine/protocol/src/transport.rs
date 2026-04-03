use async_trait::async_trait;

use crate::message::NetMessage;

/// 傳輸協議類型
///
/// 決定 server/gateway 使用哪種底層傳輸實作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    /// WebSocket（基於 tokio-tungstenite）
    WebSocket,
    /// WebTransport（基於 wtransport，HTTP/3 + QUIC）
    WebTransport,
}

/// 通道類型，決定訊息的傳輸可靠性語義
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// 可靠有序通道（TCP 語義 / WebTransport reliable stream）
    Reliable,
    /// 不可靠無序通道（UDP 語義 / WebTransport datagram）
    Unreliable,
}

/// 傳輸層錯誤
#[derive(Debug)]
pub enum TransportError {
    /// 連線已關閉
    ConnectionClosed,
    /// Codec 錯誤（序列化/反序列化失敗）
    Codec(crate::codec::CodecError),
    /// IO 錯誤（網路層錯誤，以字串描述）
    Io(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionClosed => write!(f, "連線已關閉"),
            Self::Codec(e) => write!(f, "編碼錯誤: {:?}", e),
            Self::Io(e) => write!(f, "IO 錯誤: {}", e),
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(e) => Some(e),
            _ => None,
        }
    }
}

/// 傳輸層抽象介面
///
/// client 和 server 各自實作此 trait：
/// - server/gateway：`WsTransport<S>`（Task 16）、`WtTransport`（Task 17）
/// - WASM client：未來實作（透過 wasm-bindgen WebSocket API）
///
/// # Thread Safety
/// 實作者必須是 `Send + Sync`：
/// - server/gateway：用於 tokio 多執行緒 runtime
/// - WASM client：Web API 為單執行緒，但 WASM 型別預設滿足 Send + Sync
#[async_trait]
pub trait Transport: Send + Sync {
    /// 透過可靠通道發送訊息（TCP 語義）
    ///
    /// 握手、快照、OTA 等重要訊息使用此方法
    async fn send_reliable(&self, msg: &NetMessage) -> Result<(), TransportError>;

    /// 透過不可靠通道發送訊息（UDP 語義）
    ///
    /// GameInput、HashCheck、Ping、Pong 使用此方法
    /// WebSocket 實作中降級為 send_reliable（WebSocket 本身為可靠通道）
    async fn send_unreliable(&self, msg: &NetMessage) -> Result<(), TransportError>;

    /// 接收下一個訊息（阻塞直到有訊息或連線關閉）
    ///
    /// 連線關閉時返回 `Err(TransportError::ConnectionClosed)`
    async fn recv(&self) -> Result<NetMessage, TransportError>;

    /// 關閉連線
    async fn close(&self) -> Result<(), TransportError>;

    /// 返回此連線使用的傳輸協議類型
    fn transport_type(&self) -> TransportType;
}

/// 根據訊息類型決定應走的通道
///
/// # 規則
/// - `GameInput`、`HashCheck`、`Ping`、`Pong` → `Unreliable`（低延遲優先）
/// - 其餘所有訊息 → `Reliable`（確保送達）
///
/// # 注意
/// 新增 `NetMessage` variant 時，請同步審查此函式：
/// 預設走 `Reliable`（wildcard），若該 variant 應走 `Unreliable` 需明確加入分支。
#[must_use]
pub fn channel_for(msg: &NetMessage) -> Channel {
    match msg {
        NetMessage::GameInput { .. } => Channel::Unreliable,
        NetMessage::HashCheck { .. } => Channel::Unreliable,
        NetMessage::Ping { .. } => Channel::Unreliable,
        NetMessage::Pong { .. } => Channel::Unreliable,
        _ => Channel::Reliable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{DisconnectReason, NetMessage};

    #[test]
    fn game_input_is_unreliable() {
        let msg = NetMessage::GameInput {
            tick: 0,
            inputs: vec![],
        };
        assert_eq!(channel_for(&msg), Channel::Unreliable);
    }

    #[test]
    fn hash_check_is_unreliable() {
        let msg = NetMessage::HashCheck {
            tick: 0,
            hash: [0; 32],
        };
        assert_eq!(channel_for(&msg), Channel::Unreliable);
    }

    #[test]
    fn ping_pong_are_unreliable() {
        assert_eq!(
            channel_for(&NetMessage::Ping { timestamp: 0 }),
            Channel::Unreliable
        );
        assert_eq!(
            channel_for(&NetMessage::Pong { timestamp: 0 }),
            Channel::Unreliable
        );
    }

    #[test]
    fn handshake_is_reliable() {
        let msg = NetMessage::ClientHello {
            public_key: [0; 32],
        };
        assert_eq!(channel_for(&msg), Channel::Reliable);
    }

    #[test]
    fn ota_is_reliable() {
        let msg = NetMessage::OtaChunk {
            update_id: 0u64,
            seq: 0u16,
            total: 0u16,
            data: vec![],
        };
        assert_eq!(channel_for(&msg), Channel::Reliable);
    }

    #[test]
    fn disconnect_is_reliable() {
        let msg = NetMessage::Disconnect {
            reason: DisconnectReason::Normal,
        };
        assert_eq!(channel_for(&msg), Channel::Reliable);
    }

    #[test]
    fn transport_error_display_messages() {
        let closed = TransportError::ConnectionClosed;
        assert_eq!(closed.to_string(), "連線已關閉");

        let io = TransportError::Io("timeout".into());
        assert!(io.to_string().contains("IO 錯誤"));

        // Codec variant 需要一個 CodecError 實例
        let codec = TransportError::Codec(crate::codec::CodecError::InsufficientData);
        assert!(codec.to_string().contains("編碼錯誤"));
    }

    #[test]
    fn transport_error_source_chain() {
        use std::error::Error;

        let closed = TransportError::ConnectionClosed;
        assert!(closed.source().is_none());

        let io = TransportError::Io("network down".into());
        assert!(io.source().is_none());

        let codec = TransportError::Codec(crate::codec::CodecError::InsufficientData);
        assert!(
            codec.source().is_some(),
            "Codec variant 應返回 Some 以支援錯誤鏈"
        );
    }
}
