//! OTA A/B buffer 熱更新管理
//!
//! 提供 [`UpdateBuffer`] 分片接收緩衝區與 [`HotUpdateManager`] 多批次更新管理器，
//! 支援亂序分片接收、5 秒超時清除、bytecode 載入驗證及 A/B swap 語義。
//!
//! # 設計依據
//! - [ota-flow.md](../../../docs/design/script-engine/07-hot-update/ota-flow.md)
//! - [fragment-transport.md](../../../docs/design/script-engine/07-hot-update/fragment-transport.md)
//! - [ota-ack.md](../../../docs/design/script-engine/07-hot-update/ota-ack.md)
//!
//! # 與 design doc 差異
//! - design doc（ota-flow.md）為單批次管理；本實作為多批次（`BTreeMap<u64, UpdateBuffer>`）
//! - 計時注入使用 `Box<dyn Fn() -> u64>` closure，非 `Clock` trait object
//! - `UpdateError` 為 buffer 層級錯誤，上層整合時由 `From<UpdateError> for OtaFailureReason` 橋接

use std::collections::BTreeMap;

use crate::sandbox::SandboxedEngine;
use crate::script_manager::{ScriptId, ScriptManager};

/// OTA 分片大小（64 KB）
pub const OTA_CHUNK_SIZE: usize = 65536;

/// OTA 分片超時閾值（5 秒，微秒）
pub const OTA_REASSEMBLY_TIMEOUT_US: u64 = 5_000_000;

/// OTA 更新失敗類型（buffer 層級）
///
/// 涵蓋分片組裝、bytecode 載入驗證、腳本初始化等錯誤。
/// 上層 OtaFailureReason（design doc）為本 enum 的子集，
/// 整合時由 `From<UpdateError> for OtaFailureReason` 橋接。
#[derive(Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// Ed25519 簽章驗證失敗
    SignatureVerificationFailed,
    /// Bytecode 版本不匹配
    VersionMismatch,
    /// AES-256-GCM 解密失敗
    DecryptionFailed,
    /// 新腳本 on_init() 執行失敗
    InitFailed(String),
    /// 分片尚未完全到齊
    AssemblyIncomplete,
    /// 5 秒內未收齊所有分片 → 超時
    Timeout,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SignatureVerificationFailed => write!(f, "Ed25519 簽章驗證失敗"),
            Self::VersionMismatch => write!(f, "Bytecode 版本不匹配"),
            Self::DecryptionFailed => write!(f, "AES-256-GCM 解密失敗"),
            Self::InitFailed(msg) => write!(f, "on_init() 執行失敗：{}", msg),
            Self::AssemblyIncomplete => write!(f, "分片尚未完全到齊"),
            Self::Timeout => write!(f, "分片超時（5 秒）"),
        }
    }
}

impl std::error::Error for UpdateError {}

/// 單一更新批次的分片接收緩衝區
pub struct UpdateBuffer {
    /// 更新批次 ID
    pub update_id: u64,
    /// 預期分片總數
    pub total: u16,
    /// BTreeMap<seq, data>：確定性容器，允許亂序插入（禁用 HashMap）
    fragments: BTreeMap<u16, Vec<u8>>,
    /// 建立時間（微秒），用於 5 秒超時判斷
    created_at_us: u64,
}

impl UpdateBuffer {
    /// 建立新的分片接收緩衝區
    ///
    /// `now_us`：建立時間（微秒），用於後續 `is_expired()` 超時判斷
    pub fn new(update_id: u64, total: u16, now_us: u64) -> Self {
        Self {
            update_id,
            total,
            fragments: BTreeMap::new(),
            created_at_us: now_us,
        }
    }

    /// 接收一個分片（重複 seq 以覆蓋方式去重）
    pub fn receive_fragment(&mut self, seq: u16, data: Vec<u8>) {
        // 重複 seq 覆蓋去重（fragment-transport.md 決策表：後到達覆蓋）
        self.fragments.insert(seq, data);
    }

    /// 是否所有分片已到齊
    pub fn is_complete(&self) -> bool {
        self.fragments.len() == self.total as usize
    }

    /// 將所有分片按 seq 順序拼接為完整 bytecode
    ///
    /// 前提：`is_complete() == true`，否則回傳 `Err(AssemblyIncomplete)`
    pub fn assemble(&self) -> Result<Vec<u8>, UpdateError> {
        if !self.is_complete() {
            return Err(UpdateError::AssemblyIncomplete);
        }
        // BTreeMap 保證 seq 由小到大排序（確定性重組）
        Ok(self
            .fragments
            .values()
            .flat_map(|v| v.iter().copied())
            .collect())
    }

    /// 是否已超過 5 秒（`now_us - created_at_us > 5_000_000`）
    ///
    /// 使用 `saturating_sub` 防止時間回退導致下溢 panic
    pub fn is_expired(&self, now_us: u64) -> bool {
        now_us.saturating_sub(self.created_at_us) > OTA_REASSEMBLY_TIMEOUT_US
    }
}

/// OTA 更新確認（buffer 層級）
///
/// `try_apply()` 成功時回傳此結構。上層 Bevy system 負責將 `UpdateAck`
/// 轉換為 `OtaAck`（ota-ack.md）後透過 WebSocket 回報 Server。
#[derive(Debug)]
pub struct UpdateAck {
    /// 更新批次 ID
    pub update_id: u64,
    /// 更新狀態（"ok" 或 "rollback"）
    pub status: &'static str,
    /// 新 bytecode 的 blake3 hash
    pub new_version_hash: [u8; 32],
    /// 套用時的遊戲 tick
    pub tick: u64,
}

/// OTA 更新管理器
///
/// 多批次管理：每個 `update_id` 獨立維護分片接收緩衝區，
/// 由 `purge_timed_out()` 統一處理過期 buffer。
pub struct HotUpdateManager {
    /// BTreeMap<update_id, UpdateBuffer>，確定性容器（禁用 HashMap）
    pending: BTreeMap<u64, UpdateBuffer>,
    /// AES 解密 key（來自 session key 經 HKDF 衍生，由外部注入）
    aes_key: [u8; 32],
    /// Ed25519 公鑰（用於簽章驗證）
    pub_key: [u8; 32],
    /// 供 purge_timed_out / start_update 使用的 clock（抽象計時）
    /// 避免 std::time（WASM Constraints），上層以 Clock trait 實作注入
    now_us_fn: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl HotUpdateManager {
    /// 建立 OTA 更新管理器
    ///
    /// - `aes_key`: AES-256-GCM 解密金鑰
    /// - `pub_key`: Ed25519 公鑰（簽章驗證）
    /// - `now_us_fn`: 計時 closure（微秒），禁止使用 std::time（WASM Constraints）
    pub fn new(
        aes_key: [u8; 32],
        pub_key: [u8; 32],
        now_us_fn: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            pending: BTreeMap::new(),
            aes_key,
            pub_key,
            now_us_fn,
        }
    }

    /// 開始一個新的更新批次
    ///
    /// 同 `update_id` 重複呼叫會覆蓋先前的 buffer。
    pub fn start_update(&mut self, update_id: u64, total_chunks: u16) {
        let now_us = (self.now_us_fn)();
        self.pending.insert(
            update_id,
            UpdateBuffer::new(update_id, total_chunks, now_us),
        );
        tracing::info!(
            "OTA 更新 {} 開始接收，共 {} 個分片",
            update_id,
            total_chunks
        );
    }

    /// 接收一個分片
    ///
    /// 未經 `start_update` 的 `update_id` 會被靜默忽略（tracing::warn）
    pub fn receive_chunk(&mut self, update_id: u64, seq: u16, data: Vec<u8>) {
        if let Some(buf) = self.pending.get_mut(&update_id) {
            buf.receive_fragment(seq, data);
            tracing::debug!("OTA {} 收到分片 {}/{}", update_id, seq + 1, buf.total);
        } else {
            tracing::warn!("OTA 收到未知 update_id {} 的分片，忽略", update_id);
        }
    }

    /// 嘗試在 frame boundary 應用更新
    ///
    /// # 呼叫時機約束
    /// **僅在 Bevy `Last` schedule 呼叫。** 禁止在 `FixedUpdate` 內部呼叫，
    /// 違反此規則會導致同一 tick 前半使用 v1、後半使用 v2，破壞幀確定性。
    /// 對齊 ota-flow.md 不變式 #1。
    ///
    /// # 流程
    /// 1. 找出完整的 buffer（BTreeMap 迭代順序確定性：取最小 `update_id`）
    /// 2. 組裝分片為完整 bytecode
    /// 3. 呼叫 `bytecode_compiler::load()` 驗證 + 載入
    /// 4. 呼叫 `ScriptManager::replace_script()` 進行 A/B swap
    /// 5. 回傳 `UpdateAck`
    pub fn try_apply(
        &mut self,
        script_manager: &mut ScriptManager,
        engine: &SandboxedEngine,
        current_tick: u64,
    ) -> Result<UpdateAck, UpdateError> {
        // 找出完整的 buffer（BTreeMap 迭代順序確定性：取最小 update_id）
        let update_id = self
            .pending
            .iter()
            .find(|(_, buf)| buf.is_complete())
            .map(|(id, _)| *id)
            .ok_or(UpdateError::AssemblyIncomplete)?;

        let buf = self.pending.remove(&update_id).unwrap();

        // 1. 組裝分片
        let raw_bytecode = buf.assemble()?;

        // 2. load（驗證順序為上游契約：版本檢查 → AES-GCM 解密 → Ed25519 驗章 → AST 反序列化）
        let (metadata, ast) = bytecode_compiler::load(&raw_bytecode, &self.pub_key, &self.aes_key)
            .map_err(|e| match e {
                bytecode_compiler::LoadError::SignatureVerificationFailed => {
                    UpdateError::SignatureVerificationFailed
                }
                bytecode_compiler::LoadError::DecryptionFailed(_) => UpdateError::DecryptionFailed,
                bytecode_compiler::LoadError::IncompatibleVersion { .. } => {
                    UpdateError::VersionMismatch
                }
                bytecode_compiler::LoadError::Format(_) => UpdateError::DecryptionFailed,
                bytecode_compiler::LoadError::SourceDecodeFailed => UpdateError::DecryptionFailed,
                bytecode_compiler::LoadError::RhaiCompileFailed(_) => {
                    UpdateError::InitFailed("Rhai 編譯失敗".to_string())
                }
                bytecode_compiler::LoadError::DebugBuildBytecode => UpdateError::VersionMismatch,
            })?;

        // 3. 讀取 metadata.source_hash 作為 new_version_hash（避免重算 blake3）
        let new_version_hash = metadata.source_hash;
        let script_id = ScriptId(metadata.script_id.clone());

        // 4. 呼叫 replace_script（內部處理 on_unload → swap → on_init → rollback）
        script_manager
            .replace_script(&script_id, ast, engine)
            .map_err(|e| UpdateError::InitFailed(format!("{:?}", e.error)))?;

        tracing::info!(
            "OTA 更新 {} 成功套用腳本 {}（tick: {}）",
            update_id,
            metadata.script_id,
            current_tick
        );

        Ok(UpdateAck {
            update_id,
            status: "ok",
            new_version_hash,
            tick: current_tick,
        })
    }

    /// 清除逾時（超過 5 秒）的未完成 buffer
    ///
    /// 回傳被清除的 `update_id` 清單
    pub fn purge_timed_out(&mut self, now_us: u64) -> Vec<u64> {
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, buf)| buf.is_expired(now_us))
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            self.pending.remove(id);
            tracing::warn!("OTA 更新 {} 分片超時（5 秒），已丟棄", id);
        }
        expired
    }

    /// 取得目前 pending 的更新數量（監控用）
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// 測試用 helper：建立固定時間的 HotUpdateManager
    /// `now_us_fn` 回傳固定值，用於超時測試的可控計時
    fn test_manager(fixed_now_us: u64) -> HotUpdateManager {
        HotUpdateManager::new(
            [0u8; 32], // aes_key（測試用佔位）
            [0u8; 32], // pub_key（測試用佔位）
            Box::new(move || fixed_now_us),
        )
    }

    /// 測試用 helper：建立可遞增時間的 HotUpdateManager
    #[allow(dead_code)]
    fn test_manager_with_clock(clock: Arc<AtomicU64>) -> HotUpdateManager {
        HotUpdateManager::new(
            [0u8; 32],
            [0u8; 32],
            Box::new(move || clock.load(Ordering::Relaxed)),
        )
    }

    // ═══════════════════════════════════════════════════════════════
    // UpdateBuffer 測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_buffer_stores_fragments() {
        // 分片正確儲存
        let mut buf = UpdateBuffer::new(1, 3, 0);
        buf.receive_fragment(0, vec![1, 2, 3]);
        buf.receive_fragment(1, vec![4, 5, 6]);
        assert!(!buf.is_complete()); // 只有 2/3 片
        buf.receive_fragment(2, vec![7, 8, 9]);
        assert!(buf.is_complete());
    }

    #[test]
    fn test_buffer_reassemble_in_order() {
        // 亂序到達 → assemble() 按 seq 自然排序（BTreeMap<u16, Vec<u8>> 自動以 key 升序迭代）
        let mut buf = UpdateBuffer::new(1, 3, 0);
        buf.receive_fragment(2, vec![7, 8]);
        buf.receive_fragment(0, vec![1, 2]);
        buf.receive_fragment(1, vec![4, 5]);
        let assembled = buf.assemble().unwrap();
        assert_eq!(assembled, vec![1, 2, 4, 5, 7, 8]);
    }

    #[test]
    fn test_buffer_assemble_incomplete_returns_error() {
        // 未完成時嘗試 assemble → AssemblyIncomplete
        let mut buf = UpdateBuffer::new(1, 3, 0);
        buf.receive_fragment(0, vec![1, 2]);
        assert_eq!(buf.assemble(), Err(UpdateError::AssemblyIncomplete));
    }

    #[test]
    fn test_buffer_duplicate_fragment_dedup_by_overwrite() {
        // 重複分片以覆蓋方式處理（後到的覆蓋先到的）
        let mut buf = UpdateBuffer::new(1, 2, 0);
        buf.receive_fragment(0, vec![1, 2, 3]);
        buf.receive_fragment(0, vec![9, 9, 9]); // 覆蓋
        buf.receive_fragment(1, vec![4, 5]);
        let assembled = buf.assemble().unwrap();
        assert_eq!(&assembled[..3], &[9, 9, 9]);
    }

    #[test]
    fn test_buffer_is_expired() {
        // 建立於 t=1_000_000us，5 秒後（t=6_000_001us）應過期
        let buf = UpdateBuffer::new(1, 3, 1_000_000);
        assert!(!buf.is_expired(3_000_000)); // 2 秒後：未過期
        assert!(!buf.is_expired(6_000_000)); // 剛好 5 秒：未過期（> 非 >=）
        assert!(buf.is_expired(6_000_001)); // 超過 5 秒：過期
    }

    #[test]
    fn test_is_expired_saturating_sub_no_panic() {
        // 時間回退（now_us < created_at_us）不應 panic
        let buf = UpdateBuffer::new(1, 3, 100);
        assert!(!buf.is_expired(50)); // saturating_sub → 0，不過期
    }

    // ═══════════════════════════════════════════════════════════════
    // HotUpdateManager 測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_manager_start_and_receive() {
        let mut mgr = test_manager(0);
        mgr.start_update(42, 2);
        mgr.receive_chunk(42, 0, vec![1, 2]);
        mgr.receive_chunk(42, 1, vec![3, 4]);
        // 驗證 buffer 存在且完整（通過 pending_count 側試）
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn test_purge_timed_out_removes_old_buffers() {
        // start_update 呼叫 now_us_fn() 取得建立時間 = 0us
        let mut mgr = test_manager(0);
        mgr.start_update(100, 3);
        // now = 6_000_000us（6 秒）→ 超過 5 秒閾值
        let purged = mgr.purge_timed_out(6_000_000);
        assert!(purged.contains(&100));
        assert_eq!(mgr.pending_count(), 0);
    }

    #[test]
    fn test_purge_keeps_recent_buffers() {
        let mut mgr = test_manager(0);
        mgr.start_update(200, 3);
        // 3 秒內不應被清除
        let purged = mgr.purge_timed_out(3_000_000);
        assert!(!purged.contains(&200));
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn test_purge_boundary_exact_5s_not_expired() {
        // 剛好 5 秒（5_000_000us）不應過期（> 非 >=）
        let mut mgr = test_manager(0);
        mgr.start_update(300, 3);
        let purged = mgr.purge_timed_out(5_000_000);
        assert!(!purged.contains(&300));
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn test_receive_chunk_unknown_update_id_ignored() {
        // 未經 start_update 的 update_id → 忽略，不 panic
        let mut mgr = test_manager(0);
        mgr.receive_chunk(999, 0, vec![1, 2]);
        assert_eq!(mgr.pending_count(), 0);
    }

    #[test]
    fn test_multiple_concurrent_updates() {
        // 多批次並行接收
        let mut mgr = test_manager(0);
        mgr.start_update(1, 2);
        mgr.start_update(2, 3);
        mgr.receive_chunk(1, 0, vec![1]);
        mgr.receive_chunk(2, 0, vec![2]);
        mgr.receive_chunk(1, 1, vec![3]);
        assert_eq!(mgr.pending_count(), 2);
    }

    #[test]
    fn test_start_update_overwrites_same_id() {
        // 同 update_id 重複 start_update 會覆蓋
        let mut mgr = test_manager(0);
        mgr.start_update(1, 2);
        mgr.receive_chunk(1, 0, vec![1, 2]);
        // 重新 start_update 覆蓋
        mgr.start_update(1, 3);
        assert_eq!(mgr.pending_count(), 1);
    }

    // ═══════════════════════════════════════════════════════════════
    // OTA A/B swap 語義測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_swap_calls_on_unload_then_on_init() {
        // 驗證 replace_script 內部：on_unload 先於 on_init
        let engine = SandboxedEngine::new();
        let old_ast = engine
            .engine()
            .compile(
                "let state = \"old\";\nfn on_init() { state = \"old_init\"; }\nfn on_unload() { state = \"unloaded\"; }\nfn on_tick(dt) {}",
            )
            .unwrap();
        let new_ast = engine
            .engine()
            .compile(
                "let state = \"new\";\nfn on_init() { state = \"new_init\"; }\nfn on_tick(dt) {}",
            )
            .unwrap();

        let mut mgr = ScriptManager::new();
        let sid = ScriptId("test".to_string());
        mgr.load_script(sid.clone(), old_ast, 0, &engine).unwrap();

        // replace 應成功
        let result = mgr.replace_script(&sid, new_ast, &engine);
        assert!(result.is_ok());
    }

    #[test]
    fn test_swap_failure_on_init_rolls_back() {
        // on_init 失敗 → replace_script 回傳 Err，舊腳本仍在
        let engine = SandboxedEngine::new();
        let old_ast = engine
            .engine()
            .compile("let val = 1;\nfn on_init() { val = 1; }\nfn on_tick(dt) {}")
            .unwrap();
        // 新腳本 on_init 會拋錯
        let new_ast = engine
            .engine()
            .compile("fn on_init() { throw \"init 失敗\"; }\nfn on_tick(dt) {}")
            .unwrap();

        let mut mgr = ScriptManager::new();
        let sid = ScriptId("test".to_string());
        mgr.load_script(sid.clone(), old_ast, 0, &engine).unwrap();

        // replace 應失敗
        let result = mgr.replace_script(&sid, new_ast, &engine);
        assert!(result.is_err());

        // 舊腳本仍在且 active
        assert_eq!(mgr.script_count(), 1);
        assert!(!mgr.is_disabled(&sid));
    }

    // ═══════════════════════════════════════════════════════════════
    // 型別完整性 + 結構驗證
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_timeout_error_variant_exists() {
        // 確認 Timeout 錯誤型別存在且可建構
        let _e: UpdateError = UpdateError::Timeout;
    }

    #[test]
    fn test_assembly_incomplete_error_variant_exists() {
        // 確認 AssemblyIncomplete 錯誤型別存在且可建構
        let _e: UpdateError = UpdateError::AssemblyIncomplete;
    }

    #[test]
    fn test_update_ack_contains_correct_fields() {
        // UpdateAck 結構正確
        let ack = UpdateAck {
            update_id: 42,
            status: "ok",
            new_version_hash: [0u8; 32],
            tick: 100,
        };
        assert_eq!(ack.update_id, 42);
        assert_eq!(ack.status, "ok");
        assert_eq!(ack.tick, 100);
        assert_eq!(ack.new_version_hash, [0u8; 32]);
    }

    #[test]
    fn test_update_error_display() {
        // 確認 Display impl 正確
        assert_eq!(
            format!("{}", UpdateError::SignatureVerificationFailed),
            "Ed25519 簽章驗證失敗"
        );
        assert_eq!(
            format!("{}", UpdateError::AssemblyIncomplete),
            "分片尚未完全到齊"
        );
        assert_eq!(
            format!("{}", UpdateError::InitFailed("test".to_string())),
            "on_init() 執行失敗：test"
        );
    }

    #[test]
    fn test_constants() {
        // 驗證常數值正確
        assert_eq!(OTA_CHUNK_SIZE, 65536);
        assert_eq!(OTA_REASSEMBLY_TIMEOUT_US, 5_000_000);
    }

    #[test]
    fn test_try_apply_no_complete_buffer() {
        // 沒有完整 buffer 時 try_apply 回傳 AssemblyIncomplete
        let mut mgr = test_manager(0);
        mgr.start_update(1, 3);
        mgr.receive_chunk(1, 0, vec![1, 2]);
        // 只有 1/3 片

        let engine = SandboxedEngine::new();
        let mut sm = ScriptManager::new();
        let result = mgr.try_apply(&mut sm, &engine, 100);
        assert_eq!(result.unwrap_err(), UpdateError::AssemblyIncomplete);
    }

    #[test]
    fn test_purge_multiple_expired() {
        // 多個過期 buffer 同時被清除
        let mut mgr = test_manager(0);
        mgr.start_update(1, 2);
        mgr.start_update(2, 3);
        mgr.start_update(3, 1);
        let purged = mgr.purge_timed_out(6_000_000);
        assert_eq!(purged.len(), 3);
        assert_eq!(mgr.pending_count(), 0);
    }
}
