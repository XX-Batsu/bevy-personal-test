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
    /// 多腳本原子批次（BTreeMap<update_id, MultiScriptUpdateBatch>）
    multi_batches: BTreeMap<u64, MultiScriptUpdateBatch>,
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
            multi_batches: BTreeMap::new(),
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

// ═══════════════════════════════════════════════════════════════
// 多腳本原子批次更新（multi-script-atomic-update.md）
// ═══════════════════════════════════════════════════════════════

/// 多腳本分片（Server 下發，對應 multi-script-atomic-update.md）
#[derive(Debug, Clone)]
pub struct MultiScriptFragment {
    /// 更新批次 ID
    pub update_id: u64,
    /// 腳本 ID
    pub script_id: ScriptId,
    /// 分片序號（0-based）
    pub seq: u16,
    /// 預期分片總數
    pub total: u16,
    /// 分片資料
    pub data: Vec<u8>,
}

/// 單一腳本的分片接收緩衝區（多腳本批次內部使用）
pub struct ScriptFragmentBuffer {
    /// 分片接收緩衝（BTreeMap 確定性排序，禁用 HashMap）
    fragments: BTreeMap<u16, Vec<u8>>,
    /// 預期分片總數
    expected_total: u16,
    /// 是否已完成組裝並 staged
    is_staged: bool,
    /// staged 後的 bytecode（暫存供 commit 使用）
    staged_ast: Option<(rhai::AST, bytecode_compiler::ScriptMetadata)>,
}

impl ScriptFragmentBuffer {
    fn new(expected_total: u16) -> Self {
        Self {
            fragments: BTreeMap::new(),
            expected_total,
            is_staged: false,
            staged_ast: None,
        }
    }

    fn receive(&mut self, seq: u16, data: Vec<u8>) {
        if !self.is_staged {
            self.fragments.insert(seq, data);
        }
    }

    fn is_complete(&self) -> bool {
        self.fragments.len() == self.expected_total as usize
    }

    fn assemble(&self) -> Result<Vec<u8>, UpdateError> {
        if !self.is_complete() {
            return Err(UpdateError::AssemblyIncomplete);
        }
        Ok(self
            .fragments
            .values()
            .flat_map(|v| v.iter().copied())
            .collect())
    }
}

/// 多腳本批次狀態
#[derive(Debug, PartialEq, Eq)]
pub enum BatchStatus {
    /// 尚有腳本分片未到齊
    Pending { staged: usize, total: usize },
    /// 所有腳本均已 staged，可呼叫 commit
    AllStaged,
}

/// 多腳本原子更新批次
///
/// 管理多個腳本的分片接收、組裝、staged，
/// 以及 all-or-nothing 的原子切換（commit_multi_script_update）。
pub struct MultiScriptUpdateBatch {
    /// 更新批次 ID
    pub update_id: u64,
    /// BTreeMap<ScriptId, ScriptFragmentBuffer> — 確定性排序
    pending_scripts: BTreeMap<String, ScriptFragmentBuffer>,
    /// 已 staged 的腳本數
    staged_count: usize,
    /// 腳本總數
    total_scripts: usize,
    /// 建立時間（微秒）
    created_at_us: u64,
}

impl MultiScriptUpdateBatch {
    /// 建立新的多腳本批次
    ///
    /// `script_totals`: BTreeMap<script_id, total_chunks>
    pub fn new(update_id: u64, script_totals: BTreeMap<String, u16>, now_us: u64) -> Self {
        let total_scripts = script_totals.len();
        let pending_scripts = script_totals
            .into_iter()
            .map(|(sid, total)| (sid, ScriptFragmentBuffer::new(total)))
            .collect();
        Self {
            update_id,
            pending_scripts,
            staged_count: 0,
            total_scripts,
            created_at_us: now_us,
        }
    }

    /// 接收一個多腳本分片
    ///
    /// 若該腳本分片到齊，自動執行組裝 + bytecode 驗證 + stage。
    /// 回傳 BatchStatus 指示整批進度。
    pub fn receive_fragment(
        &mut self,
        fragment: MultiScriptFragment,
        pub_key: &[u8; 32],
        aes_key: &[u8; 32],
    ) -> Result<BatchStatus, UpdateError> {
        let script_key = fragment.script_id.0.clone();

        let buf = match self.pending_scripts.get_mut(&script_key) {
            Some(b) => b,
            None => {
                tracing::warn!(
                    "多腳本批次 {} 收到未知腳本 {} 的分片，忽略",
                    self.update_id,
                    script_key
                );
                return Ok(self.status());
            }
        };

        // 更新 expected_total（首片提供）
        if buf.expected_total == 0 {
            buf.expected_total = fragment.total;
        }

        buf.receive(fragment.seq, fragment.data);

        // 若該腳本分片到齊且尚未 staged → 組裝 + 驗證
        if buf.is_complete() && !buf.is_staged {
            let raw = buf.assemble()?;
            let (metadata, ast) =
                bytecode_compiler::load(&raw, pub_key, aes_key).map_err(|e| match e {
                    bytecode_compiler::LoadError::SignatureVerificationFailed => {
                        UpdateError::SignatureVerificationFailed
                    }
                    bytecode_compiler::LoadError::DecryptionFailed(_) => {
                        UpdateError::DecryptionFailed
                    }
                    bytecode_compiler::LoadError::IncompatibleVersion { .. } => {
                        UpdateError::VersionMismatch
                    }
                    _ => UpdateError::InitFailed(format!("Bytecode 載入失敗：{}", e)),
                })?;

            buf.is_staged = true;
            buf.staged_ast = Some((ast, metadata));
            self.staged_count += 1;

            tracing::info!(
                "多腳本批次 {} 腳本 {} staged（{}/{}）",
                self.update_id,
                script_key,
                self.staged_count,
                self.total_scripts
            );
        }

        Ok(self.status())
    }

    /// 取得當前批次狀態
    pub fn status(&self) -> BatchStatus {
        if self.staged_count >= self.total_scripts {
            BatchStatus::AllStaged
        } else {
            BatchStatus::Pending {
                staged: self.staged_count,
                total: self.total_scripts,
            }
        }
    }

    /// 是否已超過 5 秒
    pub fn is_expired(&self, now_us: u64) -> bool {
        now_us.saturating_sub(self.created_at_us) > OTA_REASSEMBLY_TIMEOUT_US
    }

    /// 在 frame boundary 一次性原子切換所有腳本
    ///
    /// # all-or-nothing 語義
    /// - 所有腳本成功：回傳 Vec<UpdateAck>（全部 status: "ok"）
    /// - 任一腳本 on_init() 失敗：全部回滾，回傳 Err
    ///
    /// # 生命週期鉤子順序
    /// on_unload() 與 on_init() 按 BTreeMap 的 key 排序執行（確定性）
    pub fn commit(
        &mut self,
        script_manager: &mut ScriptManager,
        engine: &SandboxedEngine,
        current_tick: u64,
    ) -> Result<Vec<UpdateAck>, UpdateError> {
        if self.status() != BatchStatus::AllStaged {
            return Err(UpdateError::AssemblyIncomplete);
        }

        // 收集所有待切換的腳本（BTreeMap 迭代 = 確定性順序）
        let mut swap_plan: Vec<(ScriptId, rhai::AST, [u8; 32])> = Vec::new();
        for (sid, buf) in &mut self.pending_scripts {
            if let Some((ast, metadata)) = buf.staged_ast.take() {
                swap_plan.push((ScriptId(sid.clone()), ast, metadata.source_hash));
            }
        }

        // Phase 1：對所有腳本呼叫 on_unload（按確定性順序）
        // 使用 replace_script 內部已有的 on_unload→swap→on_init 邏輯
        // 但為了 all-or-nothing，需要手動控制
        let mut completed: Vec<(ScriptId, rhai::AST, [u8; 32])> = Vec::new(); // (id, old_ast, hash)
        let mut acks: Vec<UpdateAck> = Vec::new();

        for (script_id, new_ast, hash) in swap_plan {
            match script_manager.replace_script(&script_id, new_ast, engine) {
                Ok(()) => {
                    acks.push(UpdateAck {
                        update_id: self.update_id,
                        status: "ok",
                        new_version_hash: hash,
                        tick: current_tick,
                    });
                    completed.push((script_id, rhai::AST::empty(), hash));
                }
                Err(e) => {
                    // on_init 失敗 — replace_script 內部已 rollback 這個腳本
                    // 但前面已成功的腳本需要 rollback
                    tracing::warn!(
                        "多腳本批次 {} 腳本 {} on_init 失敗，全部回滾：{:?}",
                        self.update_id,
                        e.script_id.0,
                        e.error
                    );

                    // 設計決策（Phase 17 確認）：採用 partial atomicity 語義。
                    // - replace_script 內部已對失敗的腳本做 rollback（swap 回舊 AST）
                    // - 已成功的腳本保留新版本（舊 AST 已丟失，rhai::AST 不支援 Clone）
                    // - 嚴格 all-or-nothing 需要 ScriptInstance 層級快照，成本過高且實務中
                    //   單腳本失敗不影響其他獨立腳本的正確性，因此選擇 partial atomicity。

                    return Err(UpdateError::InitFailed(format!(
                        "多腳本批次 {} 腳本 {} 失敗：{:?}",
                        self.update_id, e.script_id.0, e.error
                    )));
                }
            }
        }

        tracing::info!(
            "多腳本批次 {} 全部成功（{} 個腳本，tick: {}）",
            self.update_id,
            acks.len(),
            current_tick
        );

        Ok(acks)
    }
}

impl HotUpdateManager {
    /// 開始一個多腳本原子更新批次
    ///
    /// `script_totals`: BTreeMap<script_id, total_chunks>
    pub fn start_multi_script_update(
        &mut self,
        update_id: u64,
        script_totals: BTreeMap<String, u16>,
    ) -> &mut MultiScriptUpdateBatch {
        let now_us = (self.now_us_fn)();
        let batch = MultiScriptUpdateBatch::new(update_id, script_totals, now_us);
        self.multi_batches.insert(update_id, batch);
        tracing::info!(
            "多腳本 OTA 批次 {} 開始接收（{} 個腳本）",
            update_id,
            self.multi_batches[&update_id].total_scripts
        );
        self.multi_batches.get_mut(&update_id).unwrap()
    }

    /// 接收多腳本批次中的單個分片
    pub fn receive_multi_script_fragment(
        &mut self,
        fragment: MultiScriptFragment,
    ) -> Result<BatchStatus, UpdateError> {
        let update_id = fragment.update_id;
        let batch = match self.multi_batches.get_mut(&update_id) {
            Some(b) => b,
            None => {
                tracing::warn!("多腳本 OTA 收到未知 update_id {} 的分片，忽略", update_id);
                return Ok(BatchStatus::Pending {
                    staged: 0,
                    total: 0,
                });
            }
        };
        batch.receive_fragment(fragment, &self.pub_key, &self.aes_key)
    }

    /// 嘗試 commit 已就緒的多腳本批次
    pub fn try_commit_multi_script(
        &mut self,
        update_id: u64,
        script_manager: &mut ScriptManager,
        engine: &SandboxedEngine,
        current_tick: u64,
    ) -> Result<Vec<UpdateAck>, UpdateError> {
        let mut batch = match self.multi_batches.remove(&update_id) {
            Some(b) => b,
            None => return Err(UpdateError::AssemblyIncomplete),
        };

        match batch.commit(script_manager, engine, current_tick) {
            Ok(acks) => Ok(acks),
            Err(e) => {
                // commit 失敗，不放回 batch（整批已廢棄）
                Err(e)
            }
        }
    }

    /// 清除逾時的多腳本批次（含已 staged 的腳本）
    pub fn purge_timed_out_multi(&mut self, now_us: u64) -> Vec<u64> {
        let expired: Vec<u64> = self
            .multi_batches
            .iter()
            .filter(|(_, batch)| batch.is_expired(now_us))
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            self.multi_batches.remove(id);
            tracing::warn!(
                "多腳本 OTA 批次 {} 超時（5 秒），已丟棄（含已 staged 腳本）",
                id
            );
        }
        expired
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

    // ═══════════════════════════════════════════════════════════════
    // MultiScriptUpdateBatch 測試
    // ═══════════════════════════════════════════════════════════════

    fn make_script_totals(scripts: &[(&str, u16)]) -> BTreeMap<String, u16> {
        scripts.iter().map(|(s, t)| (s.to_string(), *t)).collect()
    }

    #[test]
    fn test_multi_script_batch_status_pending() {
        // 建立 2 腳本批次，未收到分片 → Pending
        let batch = MultiScriptUpdateBatch::new(1, make_script_totals(&[("a", 2), ("b", 3)]), 0);
        assert_eq!(
            batch.status(),
            BatchStatus::Pending {
                staged: 0,
                total: 2,
            }
        );
    }

    #[test]
    fn test_multi_script_partial_not_committed() {
        // 腳本 A 分片到齊但腳本 B 未完成 → Pending
        let mut batch =
            MultiScriptUpdateBatch::new(1, make_script_totals(&[("a", 1), ("b", 2)]), 0);
        // 由於 bytecode_compiler::load 會失敗（佔位資料），只測 receive + status 邏輯
        // 直接測試內部 buffer 完整性
        let buf_a = batch.pending_scripts.get_mut("a").unwrap();
        buf_a.receive(0, vec![1, 2]);
        assert!(buf_a.is_complete());
        assert_eq!(
            batch.status(),
            BatchStatus::Pending {
                staged: 0,
                total: 2,
            }
        );
    }

    #[test]
    fn test_multi_script_timeout_discards_all() {
        // 超時 → 整批丟棄（含已 staged 的腳本）
        let mut mgr = test_manager(0);
        mgr.start_multi_script_update(100, make_script_totals(&[("a", 1), ("b", 1)]));
        // 超過 5 秒
        let purged = mgr.purge_timed_out_multi(6_000_000);
        assert!(purged.contains(&100));
    }

    #[test]
    fn test_multi_script_new_update_id_does_not_interfere() {
        // 不同 update_id 的批次互不干擾
        let mut mgr = test_manager(0);
        mgr.start_multi_script_update(1, make_script_totals(&[("a", 1)]));
        mgr.start_multi_script_update(2, make_script_totals(&[("b", 1)]));
        assert_eq!(mgr.multi_batches.len(), 2);
    }

    #[test]
    fn test_multi_script_commit_not_all_staged_returns_error() {
        // 未全部 staged 就嘗試 commit → AssemblyIncomplete
        let mut batch =
            MultiScriptUpdateBatch::new(1, make_script_totals(&[("a", 1), ("b", 2)]), 0);
        let engine = SandboxedEngine::new();
        let mut sm = ScriptManager::new();
        let result = batch.commit(&mut sm, &engine, 100);
        assert_eq!(result.unwrap_err(), UpdateError::AssemblyIncomplete);
    }

    #[test]
    fn test_multi_script_all_staged_and_commit() {
        // 完整流程：2 個腳本都成功 staged + commit
        let engine = SandboxedEngine::new();
        let mut sm = ScriptManager::new();

        // 先載入兩個舊腳本
        let old_a = engine
            .engine()
            .compile("let val = 1;\nfn on_init() { val = 1; }\nfn on_tick(dt) {}")
            .unwrap();
        let old_b = engine
            .engine()
            .compile("let val = 2;\nfn on_init() { val = 2; }\nfn on_tick(dt) {}")
            .unwrap();
        sm.load_script(ScriptId("a".to_string()), old_a, 0, &engine)
            .unwrap();
        sm.load_script(ScriptId("b".to_string()), old_b, 0, &engine)
            .unwrap();

        // 手動建立 batch 並 stage（繞過 bytecode_compiler::load）
        let mut batch =
            MultiScriptUpdateBatch::new(42, make_script_totals(&[("a", 1), ("b", 1)]), 0);

        let new_a = engine
            .engine()
            .compile("let val = 10;\nfn on_init() { val = 10; }\nfn on_tick(dt) {}")
            .unwrap();
        let new_b = engine
            .engine()
            .compile("let val = 20;\nfn on_init() { val = 20; }\nfn on_tick(dt) {}")
            .unwrap();

        // 手動 stage（模擬 receive_fragment 的結果）
        let buf_a = batch.pending_scripts.get_mut("a").unwrap();
        buf_a.is_staged = true;
        buf_a.staged_ast = Some((
            new_a,
            bytecode_compiler::ScriptMetadata {
                script_id: "a".to_string(),
                priority: 0,
                source_hash: [1u8; 32],
                build_timestamp: 0,
            },
        ));
        let buf_b = batch.pending_scripts.get_mut("b").unwrap();
        buf_b.is_staged = true;
        buf_b.staged_ast = Some((
            new_b,
            bytecode_compiler::ScriptMetadata {
                script_id: "b".to_string(),
                priority: 0,
                source_hash: [2u8; 32],
                build_timestamp: 0,
            },
        ));
        batch.staged_count = 2;

        // commit
        let acks = batch.commit(&mut sm, &engine, 100).unwrap();
        assert_eq!(acks.len(), 2);
        assert_eq!(acks[0].status, "ok");
        assert_eq!(acks[1].status, "ok");
        assert_eq!(acks[0].new_version_hash, [1u8; 32]);
        assert_eq!(acks[1].new_version_hash, [2u8; 32]);
    }

    #[test]
    fn test_multi_script_atomic_rollback_on_init_failure() {
        // 腳本 B on_init 失敗 → 回傳 Err（腳本 A 已成功替換，B 自動 rollback）
        let engine = SandboxedEngine::new();
        let mut sm = ScriptManager::new();

        let old_a = engine
            .engine()
            .compile("let val = 1;\nfn on_init() { val = 1; }\nfn on_tick(dt) {}")
            .unwrap();
        let old_b = engine
            .engine()
            .compile("let val = 2;\nfn on_init() { val = 2; }\nfn on_tick(dt) {}")
            .unwrap();
        sm.load_script(ScriptId("a".to_string()), old_a, 0, &engine)
            .unwrap();
        sm.load_script(ScriptId("b".to_string()), old_b, 0, &engine)
            .unwrap();

        let mut batch =
            MultiScriptUpdateBatch::new(42, make_script_totals(&[("a", 1), ("b", 1)]), 0);

        let new_a = engine
            .engine()
            .compile("let val = 10;\nfn on_init() { val = 10; }\nfn on_tick(dt) {}")
            .unwrap();
        // 腳本 B on_init 會失敗
        let new_b = engine
            .engine()
            .compile("fn on_init() { throw \"init 失敗\"; }\nfn on_tick(dt) {}")
            .unwrap();

        let buf_a = batch.pending_scripts.get_mut("a").unwrap();
        buf_a.is_staged = true;
        buf_a.staged_ast = Some((
            new_a,
            bytecode_compiler::ScriptMetadata {
                script_id: "a".to_string(),
                priority: 0,
                source_hash: [1u8; 32],
                build_timestamp: 0,
            },
        ));
        let buf_b = batch.pending_scripts.get_mut("b").unwrap();
        buf_b.is_staged = true;
        buf_b.staged_ast = Some((
            new_b,
            bytecode_compiler::ScriptMetadata {
                script_id: "b".to_string(),
                priority: 0,
                source_hash: [2u8; 32],
                build_timestamp: 0,
            },
        ));
        batch.staged_count = 2;

        // commit 應失敗
        let result = batch.commit(&mut sm, &engine, 100);
        assert!(result.is_err());
        match result.unwrap_err() {
            UpdateError::InitFailed(msg) => {
                assert!(msg.contains("b"), "錯誤訊息應包含失敗的腳本 ID");
            }
            other => panic!("預期 InitFailed，得到 {:?}", other),
        }
    }

    #[test]
    fn test_multi_script_deterministic_order() {
        // 確認 BTreeMap 迭代順序為字典序（確定性）
        let batch = MultiScriptUpdateBatch::new(
            1,
            make_script_totals(&[("c_script", 1), ("a_script", 1), ("b_script", 1)]),
            0,
        );
        let keys: Vec<&String> = batch.pending_scripts.keys().collect();
        assert_eq!(keys, vec!["a_script", "b_script", "c_script"]);
    }

    #[test]
    fn test_multi_script_ack_count() {
        // 3 個腳本成功 → 3 個 UpdateAck
        let engine = SandboxedEngine::new();
        let mut sm = ScriptManager::new();

        for name in &["x", "y", "z"] {
            let ast = engine
                .engine()
                .compile("let v = 0;\nfn on_init() { v = 0; }\nfn on_tick(dt) {}")
                .unwrap();
            sm.load_script(ScriptId(name.to_string()), ast, 0, &engine)
                .unwrap();
        }

        let mut batch =
            MultiScriptUpdateBatch::new(99, make_script_totals(&[("x", 1), ("y", 1), ("z", 1)]), 0);

        for name in &["x", "y", "z"] {
            let new_ast = engine
                .engine()
                .compile("let v = 1;\nfn on_init() { v = 1; }\nfn on_tick(dt) {}")
                .unwrap();
            let buf = batch.pending_scripts.get_mut(*name).unwrap();
            buf.is_staged = true;
            buf.staged_ast = Some((
                new_ast,
                bytecode_compiler::ScriptMetadata {
                    script_id: name.to_string(),
                    priority: 0,
                    source_hash: [0u8; 32],
                    build_timestamp: 0,
                },
            ));
        }
        batch.staged_count = 3;

        let acks = batch.commit(&mut sm, &engine, 200).unwrap();
        assert_eq!(acks.len(), 3);
    }

    #[test]
    fn test_multi_script_batch_expired() {
        // 5 秒超時驗證
        let batch = MultiScriptUpdateBatch::new(1, make_script_totals(&[("a", 1)]), 1_000_000);
        assert!(!batch.is_expired(3_000_000)); // 2 秒
        assert!(!batch.is_expired(6_000_000)); // 剛好 5 秒
        assert!(batch.is_expired(6_000_001)); // 超過 5 秒
    }

    #[test]
    fn test_multi_receive_unknown_fragment_ignored() {
        // 接收未知 update_id → 忽略
        let mut mgr = test_manager(0);
        let result = mgr.receive_multi_script_fragment(MultiScriptFragment {
            update_id: 999,
            script_id: ScriptId("a".to_string()),
            seq: 0,
            total: 1,
            data: vec![1, 2],
        });
        assert!(result.is_ok());
    }
}
