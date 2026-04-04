//! Shadow VM 通訊協議型別
//!
//! 定義主線程與 Shadow Worker 之間的跨邊界型別。
//! 所有型別使用 `serde::Serialize`/`serde::Deserialize`（bincode 1.x 透過 serde 序列化）。
//! 不標記 `#[repr(C)]`：含 Vec/String/enum 等非 FFI-safe 欄位，佈局由 bincode 保證。
//!
//! 權威定義見 architecture/02-concurrency/postmessage-communication.md

use serde::{Deserialize, Serialize};

use crate::PlayerInput;

/// 單幀 snapshot，主線程傳給 Shadow Worker。
/// 不標記 #[repr(C)]：含 Vec<PlayerInput> 非 FFI-safe，僅依賴 bincode 序列化。
/// 權威定義見 postmessage-communication.md。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadowFrame {
    /// 遊戲 tick 編號
    pub tick: u64,
    /// 本幀所有玩家輸入
    pub inputs: Vec<PlayerInput>,
    /// PCG state（u64 LE）+ increment（u64 LE），共 16 bytes
    pub rng_state: [u8; 16],
    /// blake3 hash，由主 VM 計算後附帶送出
    pub ecs_mirror_hash: [u8; 32],
}

/// 主線程 → Shadow Worker 驗證請求（最近 4 幀）。
/// 不標記 #[repr(C)]：含 Vec<ShadowFrame> 非 FFI-safe。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadowRequest {
    pub frames: Vec<ShadowFrame>,
}

/// Shadow Worker → 主線程 驗證結果。
/// 不標記 #[repr(C)]：含 Vec<u64> 與 enum（ShadowStatus）非 FFI-safe。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadowResponse {
    pub status: ShadowStatus,
    /// 已檢查完畢的 tick 清單
    pub checked_ticks: Vec<u64>,
}

/// 驗證結果狀態
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ShadowStatus {
    /// 所有幀 hash 比對一致
    AllMatch,
    /// 某幀出現不匹配
    Mismatch {
        tick: u64,
        expected_hash: [u8; 32],
        actual_hash: [u8; 32],
    },
    /// Worker 內部錯誤（不視為作弊）
    Error(String),
}

/// 主線程 → Shadow Worker 初始化訊息。
/// Worker 端須以 blake3 驗證 bytecode 完整性：
/// `assert_eq!(blake3::hash(&bytecode).as_bytes(), &bytecode_hash)`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadowInit {
    pub bytecode: Vec<u8>,
    /// blake3 hash of bytecode，Worker 端用於完整性驗證
    pub bytecode_hash: [u8; 32],
}

/// Shadow Worker → 主線程 初始化確認
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadowInitAck;
