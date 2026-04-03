//! 網路協議共用型別 — client 與 server 共用的訊息定義、Transport trait、連線狀態機
//!
//! # 模組架構
//!
//! - [`message`]：所有網路訊息型別（`NetMessage`、`AckStatus`、`DisconnectReason`）
//! - [`codec`]：wire format 編解碼（`[4 bytes LE length] | [bincode payload]`）
//! - [`connection`]：連線狀態機（`ConnectionState`、`State`、`StateEvent`）
//! - [`transport`]：傳輸層抽象（`Transport` trait、`Channel`、`channel_for()`）
//!
//! # 使用範例
//!
//! ```rust
//! use protocol::{NetMessage, encode, decode};
//!
//! let msg = NetMessage::Ping { timestamp: 42 };
//! let frame = encode(&msg).unwrap();
//! let decoded = decode(&frame).unwrap();
//! assert_eq!(msg, decoded);
//! ```

pub mod codec;
pub mod connection;
pub mod message;
pub mod transport;

// Codec 公開 API
pub use codec::CodecError;
pub use codec::MAX_MESSAGE_SIZE;
pub use codec::{decode, encode};

// ConnectionState 公開 API
pub use connection::ConnectionState;
pub use connection::InvalidTransition;
pub use connection::State;
pub use connection::StateEvent;

// NetMessage 公開 API
pub use message::AckStatus;
pub use message::DisconnectReason;
pub use message::NetMessage;

// Transport 公開 API
pub use transport::channel_for;
pub use transport::Channel;
pub use transport::Transport;
pub use transport::TransportError;
pub use transport::TransportType;
