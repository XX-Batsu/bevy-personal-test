//! server/key_exchange：ECDH 握手管理與 session key lifecycle
//!
//! 提供 server 端 ECDH 握手管理器與 session key 生命週期管理。
//!
//! ## 主要型別
//! - [`HandshakeManager`]：執行 ECDH 握手（純密碼學計算，同步函數）
//! - [`SessionRegistry`]：管理每個連線的 session key，斷線時 zeroize 清零
//! - [`wire_format`]：Wire format 編解碼（`[4 bytes LE length] | [bincode(NetMessage)]`）
//!
//! ## 設計原則
//! - `begin_handshake` 為同步函數（純密碼學計算無 I/O）
//! - 超時與重試職責在 Phase 13 gateway caller 層
//! - server crate 不受 engine 確定性規則（Determinism Rules）約束

pub mod error;
pub mod handshake;
pub mod session;
pub mod wire_format;

pub use error::KeyExchangeError;
pub use handshake::{HandshakeManager, ServerHelloPayload, SessionInfo, SessionKey};
pub use session::{ConnectionId, SessionRegistry};
pub use wire_format::{decode, encode, NetMessage};
