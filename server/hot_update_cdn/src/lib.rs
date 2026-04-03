//! server/hot_update_cdn：OTA bytecode 廣播、ACK 管理、Room 管理
//!
//! ## 主要型別
//! - [`BroadcastManager`]：OTA 多 client 廣播管理器
//! - [`VersionRegistry`]：Client 版本登錄表
//! - [`RoomManager`]：Room 生命週期管理
//! - [`split_into_chunks`] / [`reassemble_chunks`]：OTA bytecode 分片與重組
//!
//! ## 設計原則
//! - server crate 不受 engine 確定性規則（Determinism Rules）約束
//! - timeout 由外層 caller 管理（tokio::time::timeout）

pub mod ack;
pub mod broadcast;
pub mod chunk;
pub mod error;
pub mod room;

pub use ack::{AckStatus, OtaAck};
pub use broadcast::{BroadcastManager, BroadcastStatus, VersionRegistry};
pub use chunk::{reassemble_chunks, split_into_chunks, UpdateChunk};
pub use error::HotUpdateError;
pub use room::{PlayerId, Room, RoomError, RoomId, RoomManager, RoomState};
