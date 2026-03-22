//! state_hash — blake3 state hashing for deterministic validation
//!
//! 此 crate 提供基於 blake3 的遊戲狀態雜湊計算，
//! 用於 client-server 確定性驗證。
//!
//! 不依賴 serde — state_hash 不做序列化，只做 hash 計算。

mod hasher;

pub use bridge_types::Blake3Hash;
pub use hasher::{compute_state_hash, hash_entity};
