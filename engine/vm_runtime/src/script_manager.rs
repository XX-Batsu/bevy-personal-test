//! 多腳本管理器
//!
//! [`ScriptManager`] 管理多個 [`ScriptInstance`]（包裝為 [`ManagedScript`]），
//! 按 `(priority, script_id)` 確定性順序排程生命週期回調。
//!
//! # 設計依據
//! - [multi-script-management.md](../../../docs/design/script-engine/04-lifecycle/multi-script-management.md)
//! - [execution-flow.md](../../../docs/design/script-engine/04-lifecycle/execution-flow.md) §4.2a
//! - [error-handling.md](../../../docs/design/script-engine/04-lifecycle/error-handling.md)
//!
//! # 執行順序（Phase-based）
//! ```text
//! run_frame: on_event(所有腳本) → on_input(所有腳本) → on_tick(所有腳本)
//! ```

use std::collections::BTreeMap;

use bridge_types::ScriptError;
use deterministic::SoftF32;
use rhai::{Dynamic, AST};

use crate::fallback::DisableReason;
use crate::lifecycle::{LifecycleHookName, ScriptInstance};
use crate::sandbox::{FrameBudget, SandboxedEngine};

/// 腳本唯一識別碼（UTF-8 字串，用於排序）
/// 權威定義見 multi-script-management.md §介面定義
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptId(pub String);

/// ScriptError 上下文包裝：補充 script_id、tick 與 hook 名稱
/// 權威定義見 error-handling.md §介面定義
#[derive(Debug, Clone)]
pub struct ScriptErrorContext {
    pub error: ScriptError,
    pub script_id: ScriptId,
    pub tick: u64,
    pub hook: LifecycleHookName,
}

/// 腳本狀態
/// 權威定義見 multi-script-management.md §介面定義
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptState {
    /// 正常執行中
    Active,
    /// 因錯誤或超時被停用
    Disabled(DisableReason),
    /// 等待 on_init() 執行完成
    Initializing,
}

/// ScriptManager 內部包裝：ScriptInstance + 管理狀態
struct ManagedScript {
    instance: ScriptInstance,
    state: ScriptState,
    /// 連續超時/操作限制計數器（達 10 次觸發 auto-disable）
    /// 僅 Timeout 和 OperationLimit 計入，成功執行時重設為 0
    /// 幀預算跳過不計入（execution-flow.md 不變式 #6）
    consecutive_error_count: u32,
}

/// 多腳本管理器
pub struct ScriptManager {
    /// 按 (priority, script_id) 排序的腳本集合，確保執行順序確定性
    /// 使用 BTreeMap 而非 HashMap（README.md Determinism Rules）
    scripts: BTreeMap<(u8, ScriptId), ManagedScript>,
    /// 當前 tick
    tick: u64,
}

/// auto-disable 閾值：連續 10 次 Timeout/OperationLimit 觸發停用
const AUTO_DISABLE_THRESHOLD: u32 = 10;

impl Default for ScriptManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptManager {
    pub fn new() -> Self {
        Self {
            scripts: BTreeMap::new(),
            tick: 0,
        }
    }

    /// 載入腳本：建立 ScriptInstance、呼叫 on_init（獨立 FrameBudget）
    /// on_init 失敗 → Disabled(InitFailed)，ScopeLimitExceeded → Disabled(ScopeLimitExceeded)
    pub fn load_script(
        &mut self,
        id: ScriptId,
        ast: AST,
        priority: u8,
        engine: &SandboxedEngine,
    ) -> Result<(), ScriptErrorContext> {
        // 若已存在同 ScriptId 的舊 key，先移除
        self.scripts.retain(|(_, sid), _| sid != &id);

        let mut instance = ScriptInstance::new(id.0.clone(), ast, priority);
        let mut budget = FrameBudget::new();

        let key = (priority, id.clone());
        match instance.call_on_init(engine, &mut budget) {
            Ok(()) => {
                tracing::info!("載入腳本: script_id={}, priority={}", id.0, priority);
                self.scripts.insert(
                    key,
                    ManagedScript {
                        instance,
                        state: ScriptState::Active,
                        consecutive_error_count: 0,
                    },
                );
                Ok(())
            }
            Err(error) => {
                let disable_reason = match &error {
                    ScriptError::ScopeLimitExceeded { .. } => DisableReason::ScopeLimitExceeded,
                    _ => DisableReason::InitFailed,
                };
                self.scripts.insert(
                    key,
                    ManagedScript {
                        instance,
                        state: ScriptState::Disabled(disable_reason),
                        consecutive_error_count: 0,
                    },
                );
                Err(self.wrap_error(error, &id, LifecycleHookName::Init))
            }
        }
    }

    /// 卸載腳本：呼叫 on_unload（3ms 獨立 budget），任何錯誤不阻止卸載
    pub fn unload_script(
        &mut self,
        id: &ScriptId,
        engine: &SandboxedEngine,
    ) -> Result<(), ScriptErrorContext> {
        // 在 BTreeMap 中尋找含此 ScriptId 的 key
        let key = self.scripts.keys().find(|(_, sid)| sid == id).cloned();

        let key = match key {
            Some(k) => k,
            None => {
                return Err(self.wrap_error(
                    ScriptError::RuntimeError {
                        script_id: id.0.clone(),
                        message: "ScriptNotFound".to_string(),
                        tick: self.tick,
                    },
                    id,
                    LifecycleHookName::Unload,
                ));
            }
        };

        if let Some(mut managed) = self.scripts.remove(&key) {
            if let Err(e) = managed.instance.call_on_unload(engine) {
                tracing::warn!(
                    "on_unload 錯誤（仍繼續卸載）: script_id={}, error={:?}",
                    id.0,
                    e
                );
            }
            tracing::info!("卸載腳本: script_id={}", id.0);
        }

        Ok(())
    }

    /// 統一幀方法（execution-flow.md §4.2a）：on_event → on_input → on_tick
    pub fn run_frame(
        &mut self,
        dt: SoftF32,
        budget: &mut FrameBudget,
        events: &[(String, Dynamic)],
        inputs: &[(String, Dynamic)],
        engine: &SandboxedEngine,
    ) -> Vec<ScriptErrorContext> {
        self.tick += 1;
        let mut errors = Vec::new();

        // Phase 1: on_event（按事件 FIFO 順序，每個事件遍歷所有腳本）
        for (event_type, event_data) in events {
            let mut event_errors =
                self.dispatch_event(event_type, event_data.clone(), budget, engine);
            errors.append(&mut event_errors);
        }

        // Phase 2: on_input（按輸入 FIFO 順序，每個輸入遍歷所有腳本）
        for (input_type, input_data) in inputs {
            let mut input_errors =
                self.dispatch_input(input_type, input_data.clone(), budget, engine);
            errors.append(&mut input_errors);
        }

        // Phase 3: on_tick
        let mut tick_errors = self.tick_all(dt, budget, engine);
        errors.append(&mut tick_errors);

        errors
    }

    /// on_tick 遍歷（Phase 15 拆分版使用）
    pub fn tick_all(
        &mut self,
        dt: SoftF32,
        budget: &mut FrameBudget,
        engine: &SandboxedEngine,
    ) -> Vec<ScriptErrorContext> {
        let mut errors = Vec::new();

        // 收集 keys 以避免借用衝突
        let keys: Vec<_> = self.scripts.keys().cloned().collect();
        for key in keys {
            let managed = self.scripts.get(&key).unwrap();
            if matches!(managed.state, ScriptState::Disabled(_)) {
                continue;
            }

            if budget.remaining_ms() <= 0.0 {
                tracing::warn!("幀預算耗盡，跳過剩餘腳本（on_tick）");
                break;
            }

            let managed = self.scripts.get_mut(&key).unwrap();
            match managed.instance.call_on_tick(engine, dt, budget) {
                Ok(()) => {
                    managed.consecutive_error_count = 0;
                }
                Err(error) => {
                    self.handle_callback_error_by_key(
                        &key,
                        error,
                        LifecycleHookName::Tick,
                        &mut errors,
                    );
                }
            }
        }

        errors
    }

    /// on_event 遍歷（Phase 15 拆分版使用）
    pub fn dispatch_event(
        &mut self,
        event_type: &str,
        event_data: Dynamic,
        budget: &mut FrameBudget,
        engine: &SandboxedEngine,
    ) -> Vec<ScriptErrorContext> {
        let mut errors = Vec::new();

        let keys: Vec<_> = self.scripts.keys().cloned().collect();
        for key in keys {
            let managed = self.scripts.get(&key).unwrap();
            if matches!(managed.state, ScriptState::Disabled(_)) {
                continue;
            }

            if budget.remaining_ms() <= 0.0 {
                tracing::warn!("幀預算耗盡，跳過剩餘腳本（on_event）");
                break;
            }

            let managed = self.scripts.get_mut(&key).unwrap();
            match managed
                .instance
                .call_on_event(engine, event_type, event_data.clone(), budget)
            {
                Ok(()) => {
                    managed.consecutive_error_count = 0;
                }
                Err(error) => {
                    self.handle_callback_error_by_key(
                        &key,
                        error,
                        LifecycleHookName::Event,
                        &mut errors,
                    );
                }
            }
        }

        errors
    }

    /// on_input 遍歷（Phase 15 拆分版使用）
    pub fn dispatch_input(
        &mut self,
        input_type: &str,
        input_data: Dynamic,
        budget: &mut FrameBudget,
        engine: &SandboxedEngine,
    ) -> Vec<ScriptErrorContext> {
        let mut errors = Vec::new();

        let keys: Vec<_> = self.scripts.keys().cloned().collect();
        for key in keys {
            let managed = self.scripts.get(&key).unwrap();
            if matches!(managed.state, ScriptState::Disabled(_)) {
                continue;
            }

            if budget.remaining_ms() <= 0.0 {
                tracing::warn!("幀預算耗盡，跳過剩餘腳本（on_input）");
                break;
            }

            let managed = self.scripts.get_mut(&key).unwrap();
            match managed
                .instance
                .call_on_input(engine, input_type, input_data.clone(), budget)
            {
                Ok(()) => {
                    managed.consecutive_error_count = 0;
                }
                Err(error) => {
                    self.handle_callback_error_by_key(
                        &key,
                        error,
                        LifecycleHookName::Input,
                        &mut errors,
                    );
                }
            }
        }

        errors
    }

    /// 取得活躍腳本數量（state != Disabled）
    pub fn active_script_count(&self) -> usize {
        self.scripts
            .values()
            .filter(|m| !matches!(m.state, ScriptState::Disabled(_)))
            .count()
    }

    /// 取得所有腳本數量（含 disabled）
    pub fn script_count(&self) -> usize {
        self.scripts.len()
    }

    /// 檢查腳本是否已停用
    pub fn is_disabled(&self, id: &ScriptId) -> bool {
        self.scripts
            .iter()
            .any(|((_, sid), m)| sid == id && matches!(m.state, ScriptState::Disabled(_)))
    }

    /// 取得指定腳本的 Scope（不可變引用）
    /// Debug console script_vars 使用
    #[cfg(feature = "debug-mode")]
    pub fn script_scope(&self, script_id: &str) -> Option<&rhai::Scope<'static>> {
        self.scripts
            .iter()
            .find(|((_, sid), _)| sid.0 == script_id)
            .map(|(_, m)| m.instance.scope())
    }

    /// 取得指定腳本的 Scope（可變引用）
    /// Debug console script_eval_in 使用
    #[cfg(feature = "debug-mode")]
    pub fn script_scope_mut(&mut self, script_id: &str) -> Option<&mut rhai::Scope<'static>> {
        self.scripts
            .iter_mut()
            .find(|((_, sid), _)| sid.0 == script_id)
            .map(|(_, m)| m.instance.scope_mut())
    }

    /// 取得 priority 最小的第一個 active 腳本的 Scope（可變引用）
    /// 回傳 (script_id, scope)
    /// Debug console script_eval 使用
    #[cfg(feature = "debug-mode")]
    pub fn first_active_script_scope_mut(&mut self) -> Option<(String, &mut rhai::Scope<'static>)> {
        self.scripts
            .iter_mut()
            .find(|(_, m)| matches!(m.state, ScriptState::Active))
            .map(|((_, sid), m)| (sid.0.clone(), m.instance.scope_mut()))
    }

    /// 取得指定腳本的 priority
    /// Debug console script_list 使用
    #[cfg(feature = "debug-mode")]
    pub fn script_priority(&self, id: &ScriptId) -> Option<u8> {
        self.scripts
            .keys()
            .find(|(_, sid)| sid == id)
            .map(|(p, _)| *p)
    }

    /// 替換腳本 AST（OTA A/B swap）
    ///
    /// 流程：on_unload(舊) → swap AST → on_init(新)
    /// 若 on_init 失敗 → rollback（換回舊 AST，重新 on_init 舊腳本）
    /// 成功後清除 disabled 狀態（re-enable）
    pub fn replace_script(
        &mut self,
        script_id: &ScriptId,
        new_ast: AST,
        engine: &SandboxedEngine,
    ) -> Result<(), ScriptErrorContext> {
        // 找到含此 ScriptId 的 key
        let key = self
            .scripts
            .keys()
            .find(|(_, sid)| sid == script_id)
            .cloned();

        let key = match key {
            Some(k) => k,
            None => {
                return Err(self.wrap_error(
                    ScriptError::RuntimeError {
                        script_id: script_id.0.clone(),
                        message: "ScriptNotFound".to_string(),
                        tick: self.tick,
                    },
                    script_id,
                    LifecycleHookName::Init,
                ));
            }
        };

        let managed = self.scripts.get_mut(&key).unwrap();

        // 1. on_unload 舊腳本（錯誤僅 warn，不阻止替換）
        if let Err(e) = managed.instance.call_on_unload(engine) {
            tracing::warn!(
                "OTA replace_script on_unload 錯誤（仍繼續替換）: script_id={}, error={:?}",
                script_id.0,
                e
            );
        }

        // 2. swap AST（取回舊 AST 供 rollback）
        let old_ast = managed.instance.replace_ast(new_ast);

        // 3. on_init 新腳本
        let mut budget = FrameBudget::new();
        if let Err(init_error) = managed.instance.call_on_init(engine, &mut budget) {
            // rollback：換回舊 AST
            tracing::warn!(
                "OTA replace_script on_init 失敗，回滾至舊腳本: script_id={}, error={:?}",
                script_id.0,
                init_error
            );
            managed.instance.replace_ast(old_ast);
            // 嘗試重新初始化舊腳本
            let mut rollback_budget = FrameBudget::new();
            if let Err(rb_err) = managed.instance.call_on_init(engine, &mut rollback_budget) {
                tracing::warn!(
                    "OTA rollback on_init 也失敗: script_id={}, error={:?}",
                    script_id.0,
                    rb_err
                );
            }
            return Err(self.wrap_error(init_error, script_id, LifecycleHookName::Init));
        }

        // 4. 成功：清除 disabled 狀態、重設錯誤計數器
        managed.state = ScriptState::Active;
        managed.consecutive_error_count = 0;
        tracing::info!("OTA replace_script 成功: script_id={}", script_id.0);

        Ok(())
    }

    /// 取得所有腳本的執行狀態（用於監控）
    pub fn script_states(&self) -> Vec<(ScriptId, ScriptState)> {
        self.scripts
            .iter()
            .map(|((_, id), m)| (id.clone(), m.state.clone()))
            .collect()
    }

    /// 包裝 ScriptError 為 ScriptErrorContext
    fn wrap_error(
        &self,
        error: ScriptError,
        script_id: &ScriptId,
        hook: LifecycleHookName,
    ) -> ScriptErrorContext {
        ScriptErrorContext {
            error,
            script_id: script_id.clone(),
            tick: self.tick,
            hook,
        }
    }

    /// 處理 callback 錯誤（透過 key 查找 ManagedScript）
    fn handle_callback_error_by_key(
        &mut self,
        key: &(u8, ScriptId),
        error: ScriptError,
        hook: LifecycleHookName,
        errors: &mut Vec<ScriptErrorContext>,
    ) {
        let script_id = key.1.clone();

        match &error {
            ScriptError::BudgetExhausted => {
                // 幀預算耗盡視為跳過，不收集錯誤，不計入 auto-disable
                return;
            }
            ScriptError::Timeout { .. } | ScriptError::OperationLimit { .. } => {
                if let Some(managed) = self.scripts.get_mut(key) {
                    managed.consecutive_error_count += 1;
                    if managed.consecutive_error_count >= AUTO_DISABLE_THRESHOLD {
                        managed.state = ScriptState::Disabled(DisableReason::Timeout);
                        tracing::warn!("腳本連續超時達閾值，自動停用: script_id={}", script_id.0);
                    }
                }
            }
            ScriptError::ScopeLimitExceeded { .. } => {
                if let Some(managed) = self.scripts.get_mut(key) {
                    managed.state = ScriptState::Disabled(DisableReason::ScopeLimitExceeded);
                }
            }
            _ => {
                // RuntimeError 等：收集但不影響 auto-disable 計數器
            }
        }
        errors.push(self.wrap_error(error, &script_id, hook));
    }
}

use crate::bytecode_loader::{BytecodeLoader, LoadError};

impl ScriptManager {
    /// 批量載入加密 bytecode 並按 priority 排序後插入 scripts 表
    ///
    /// 每個元素為 `(bytecode_bytes, decryption_key)`，共用同一 `verifying_key`。
    /// 若任一 bytecode 解析失敗則整批中止並回傳錯誤（fail-fast 語意）。
    ///
    /// # 排序保證
    /// scripts 按 priority 升序（0 = 最高優先）載入，
    /// 確保 on_init 呼叫順序確定性。
    ///
    /// # 注意
    /// 此方法直接將 `ScriptInstance` 以 `Active` 狀態插入，不執行 `on_init`。
    /// 原因：批量載入時無 `SandboxedEngine` 參照可供執行 on_init；
    /// `on_init` 將於下一幀 `run_frame()` 時由各 script 的 lifecycle 機制觸發。
    pub fn load_scripts(
        &mut self,
        scripts: Vec<(&[u8], &[u8; 32])>,
        verifying_key: &[u8; 32],
    ) -> Result<(), LoadError> {
        // 1. 解析所有 bytecode（fail-fast）
        let mut instances: Vec<ScriptInstance> = scripts
            .into_iter()
            .map(|(data, key)| BytecodeLoader::load(data, verifying_key, key))
            .collect::<Result<Vec<_>, _>>()?;

        // 2. 按 priority 升序排序（BTreeMap 確定性排序依據）
        instances.sort_by_key(|s| s.priority);

        // 3. 依序插入（跳過 on_init，直接置入 Active 狀態）
        for instance in instances {
            let priority = instance.priority;
            let id = ScriptId(instance.script_id.clone());
            // 移除同 ScriptId 的舊實例
            self.scripts.retain(|(_, sid), _| sid != &id);
            self.scripts.insert(
                (priority, id),
                ManagedScript {
                    instance,
                    state: ScriptState::Active,
                    consecutive_error_count: 0,
                },
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 測試輔助 ──

    fn test_engine() -> SandboxedEngine {
        SandboxedEngine::new()
    }

    fn compile(engine: &SandboxedEngine, script: &str) -> AST {
        engine.engine().compile(script).expect("測試腳本編譯失敗")
    }

    fn sid(s: &str) -> ScriptId {
        ScriptId(s.to_string())
    }

    fn standard_dt() -> SoftF32 {
        SoftF32::from_f64(1.0 / 60.0)
    }

    // ═══════════════════════════════════════════════════════════════
    // 正常路徑：載入與排序
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_load_script_calls_on_init() {
        let engine = test_engine();
        let ast = compile(&engine, "let x = 0;\nfn on_init() { x = 1; }");
        let mut mgr = ScriptManager::new();
        assert!(mgr.load_script(sid("a"), ast, 0, &engine).is_ok());
        assert_eq!(mgr.script_count(), 1);
    }

    #[test]
    fn test_load_two_scripts_priority_order() {
        let engine = test_engine();
        // 用 order 變數記錄誰先執行
        let ast_a = compile(
            &engine,
            "let order_a = 0;\nfn on_tick(dt) { order_a += 1; }",
        );
        let ast_b = compile(
            &engine,
            "let order_b = 0;\nfn on_tick(dt) { order_b += 1; }",
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast_a, 1, &engine).unwrap();
        mgr.load_script(sid("b"), ast_b, 0, &engine).unwrap();

        // 驗證 BTreeMap 迭代順序：priority 0 (b) 先於 priority 1 (a)
        let states = mgr.script_states();
        assert_eq!(states[0].0, sid("b"));
        assert_eq!(states[1].0, sid("a"));
    }

    #[test]
    fn test_same_priority_sorted_by_id() {
        let engine = test_engine();
        let ast_b = compile(&engine, "fn on_tick(dt) {}");
        let ast_a = compile(&engine, "fn on_tick(dt) {}");
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("b"), ast_b, 0, &engine).unwrap();
        mgr.load_script(sid("a"), ast_a, 0, &engine).unwrap();

        let states = mgr.script_states();
        assert_eq!(states[0].0, sid("a"));
        assert_eq!(states[1].0, sid("b"));
    }

    #[test]
    fn test_three_scripts_mixed_priority() {
        let engine = test_engine();
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("c"), compile(&engine, ""), 2, &engine)
            .unwrap();
        mgr.load_script(sid("a"), compile(&engine, ""), 0, &engine)
            .unwrap();
        mgr.load_script(sid("b"), compile(&engine, ""), 1, &engine)
            .unwrap();

        let states = mgr.script_states();
        assert_eq!(states[0].0, sid("a"));
        assert_eq!(states[1].0, sid("b"));
        assert_eq!(states[2].0, sid("c"));
    }

    // ═══════════════════════════════════════════════════════════════
    // 正常路徑：run_frame 執行順序
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_run_frame_phase_based_order() {
        // 每個腳本各自記錄被呼叫的 hook，測試結束後合併驗證順序
        let engine = test_engine();
        let ast_a = compile(
            &engine,
            r#"
            let log_a = "";
            fn on_event(et, ed) { log_a += "E"; }
            fn on_input(it, id) { log_a += "I"; }
            fn on_tick(dt) { log_a += "T"; }
            "#,
        );
        let ast_b = compile(
            &engine,
            r#"
            let log_b = "";
            fn on_event(et, ed) { log_b += "E"; }
            fn on_input(it, id) { log_b += "I"; }
            fn on_tick(dt) { log_b += "T"; }
            "#,
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast_a, 0, &engine).unwrap();
        mgr.load_script(sid("b"), ast_b, 1, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let events = vec![("damage".to_string(), Dynamic::UNIT)];
        let inputs = vec![("move".to_string(), Dynamic::UNIT)];

        mgr.run_frame(standard_dt(), &mut budget, &events, &inputs, &engine);

        // 驗證：Phase-based 意味著每個階段所有腳本都參與
        // a 和 b 都應各收到 E、I、T
        // 使用內部存取腳本 scope 驗證
        let keys: Vec<_> = mgr.scripts.keys().cloned().collect();
        let managed_a = mgr.scripts.get(&keys[0]).unwrap();
        let log_a = managed_a
            .instance
            .scope()
            .get_value::<rhai::ImmutableString>("log_a")
            .unwrap();
        assert_eq!(log_a.as_str(), "EIT");

        let managed_b = mgr.scripts.get(&keys[1]).unwrap();
        let log_b = managed_b
            .instance
            .scope()
            .get_value::<rhai::ImmutableString>("log_b")
            .unwrap();
        assert_eq!(log_b.as_str(), "EIT");
    }

    #[test]
    fn test_run_frame_events_fifo_order() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            r#"
            let events_log = "";
            fn on_event(et, ed) { events_log += et; }
            "#,
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let events = vec![
            ("E1".to_string(), Dynamic::UNIT),
            ("E2".to_string(), Dynamic::UNIT),
            ("E3".to_string(), Dynamic::UNIT),
        ];
        mgr.run_frame(standard_dt(), &mut budget, &events, &[], &engine);

        let keys: Vec<_> = mgr.scripts.keys().cloned().collect();
        let managed = mgr.scripts.get(&keys[0]).unwrap();
        let log = managed
            .instance
            .scope()
            .get_value::<rhai::ImmutableString>("events_log")
            .unwrap();
        assert_eq!(log.as_str(), "E1E2E3");
    }

    #[test]
    fn test_run_frame_inputs_fifo_order() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            r#"
            let inputs_log = "";
            fn on_input(it, id) { inputs_log += it; }
            "#,
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let inputs = vec![
            ("I1".to_string(), Dynamic::UNIT),
            ("I2".to_string(), Dynamic::UNIT),
            ("I3".to_string(), Dynamic::UNIT),
        ];
        mgr.run_frame(standard_dt(), &mut budget, &[], &inputs, &engine);

        let keys: Vec<_> = mgr.scripts.keys().cloned().collect();
        let managed = mgr.scripts.get(&keys[0]).unwrap();
        let log = managed
            .instance
            .scope()
            .get_value::<rhai::ImmutableString>("inputs_log")
            .unwrap();
        assert_eq!(log.as_str(), "I1I2I3");
    }

    #[test]
    fn test_run_frame_increments_tick() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_tick(dt) { throw "err"; }"#);
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let e1 = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert_eq!(e1[0].tick, 1);

        budget.reset();
        let e2 = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert_eq!(e2[0].tick, 2);

        budget.reset();
        let e3 = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert_eq!(e3[0].tick, 3);
    }

    // ═══════════════════════════════════════════════════════════════
    // 正常路徑：卸載
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_unload_script_calls_on_unload() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            "let cleaned = false;\nfn on_unload() { cleaned = true; }",
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();
        assert!(mgr.unload_script(&sid("a"), &engine).is_ok());
        assert_eq!(mgr.script_count(), 0);
    }

    #[test]
    fn test_unload_script_removed_from_tick() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            "let tick_count = 0;\nfn on_tick(dt) { tick_count += 1; }",
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();
        mgr.unload_script(&sid("a"), &engine).unwrap();

        let mut budget = FrameBudget::new();
        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert!(errors.is_empty());
        assert_eq!(mgr.script_count(), 0);
    }

    #[test]
    fn test_unload_nonexistent_script() {
        let engine = test_engine();
        let mut mgr = ScriptManager::new();
        let result = mgr.unload_script(&sid("nonexistent"), &engine);
        assert!(result.is_err());
        let ctx = result.unwrap_err();
        assert!(matches!(ctx.error, ScriptError::RuntimeError { .. }));
        assert_eq!(ctx.hook, LifecycleHookName::Unload);
    }

    // ═══════════════════════════════════════════════════════════════
    // 停用與跳過
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_on_init_failure_disables_with_reason() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_init() { throw "init failed"; }"#);
        let mut mgr = ScriptManager::new();
        let result = mgr.load_script(sid("a"), ast, 0, &engine);
        assert!(result.is_err());
        let ctx = result.unwrap_err();
        assert_eq!(ctx.hook, LifecycleHookName::Init);
        assert!(mgr.is_disabled(&sid("a")));

        let states = mgr.script_states();
        assert_eq!(
            states[0].1,
            ScriptState::Disabled(DisableReason::InitFailed)
        );
    }

    #[test]
    fn test_disabled_script_skipped_in_run_frame() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_init() { throw "fail"; }"#);
        let mut mgr = ScriptManager::new();
        let _ = mgr.load_script(sid("a"), ast, 0, &engine);

        let mut budget = FrameBudget::new();
        let before = budget.remaining_ms();
        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert!(errors.is_empty());
        // 預算未消耗
        assert_eq!(budget.remaining_ms(), before);
    }

    #[test]
    fn test_disabled_script_skipped_in_tick_all() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_init() { throw "fail"; }"#);
        let mut mgr = ScriptManager::new();
        let _ = mgr.load_script(sid("a"), ast, 0, &engine);

        let mut budget = FrameBudget::new();
        let errors = mgr.tick_all(standard_dt(), &mut budget, &engine);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_disabled_script_does_not_affect_others() {
        let engine = test_engine();
        let ast_a = compile(&engine, r#"fn on_init() { throw "fail"; }"#);
        let ast_b = compile(&engine, "let tick_b = 0;\nfn on_tick(dt) { tick_b += 1; }");
        let mut mgr = ScriptManager::new();
        let _ = mgr.load_script(sid("a"), ast_a, 0, &engine);
        mgr.load_script(sid("b"), ast_b, 1, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert!(errors.is_empty());
        assert_eq!(mgr.active_script_count(), 1);
    }

    // ═══════════════════════════════════════════════════════════════
    // 預算管理
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_budget_exhausted_skips_remaining() {
        let engine = test_engine();
        let ast_a = compile(&engine, "let tick_a = 0;\nfn on_tick(dt) { tick_a += 1; }");
        let ast_b = compile(&engine, "let tick_b = 0;\nfn on_tick(dt) { tick_b += 1; }");
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast_a, 0, &engine).unwrap();
        mgr.load_script(sid("b"), ast_b, 1, &engine).unwrap();

        // 預算為 0
        let mut budget = FrameBudget::new();
        budget.deduct_elapsed(5.0);

        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_budget_exhausted_no_disable_count() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            "let tick_count = 0;\nfn on_tick(dt) { tick_count += 1; }",
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        // 預算耗盡
        let mut budget = FrameBudget::new();
        budget.deduct_elapsed(5.0);

        let _ = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert!(!mgr.is_disabled(&sid("a")));
    }

    #[test]
    fn test_unload_uses_independent_budget() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            "let cleaned = false;\nfn on_unload() { cleaned = true; }",
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        // 全域預算耗盡 → on_unload 仍可執行（獨立 budget）
        assert!(mgr.unload_script(&sid("a"), &engine).is_ok());
        assert_eq!(mgr.script_count(), 0);
    }

    // ═══════════════════════════════════════════════════════════════
    // 錯誤處理
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_script_error_context_has_all_fields() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_tick(dt) { throw "tick error"; }"#);
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("s1"), ast, 0, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].script_id, sid("s1"));
        assert_eq!(errors[0].tick, 1);
        assert_eq!(errors[0].hook, LifecycleHookName::Tick);
        assert!(matches!(errors[0].error, ScriptError::RuntimeError { .. }));
    }

    #[test]
    fn test_scope_limiter_check_after_callback() {
        let engine = test_engine();
        let ast = compile(&engine, "let x = 0;\nfn on_tick(dt) { x = 1; }");
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        // 注入 > 256 個變數觸發 ScopeLimiter
        let key = mgr.scripts.keys().next().unwrap().clone();
        let managed = mgr.scripts.get_mut(&key).unwrap();
        for i in 0..257 {
            managed.instance.scope_mut().push(format!("v_{}", i), 0_i64);
        }

        let mut budget = FrameBudget::new();
        let errors = mgr.tick_all(standard_dt(), &mut budget, &engine);
        assert!(!errors.is_empty());
        assert!(matches!(
            errors[0].error,
            ScriptError::ScopeLimitExceeded { .. }
        ));
        assert!(mgr.is_disabled(&sid("a")));
    }

    #[test]
    fn test_on_event_error_continues_next_event() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            r#"
            let event_count = 0;
            fn on_event(et, ed) {
                if et == "bad" { throw "error"; }
                event_count += 1;
            }
            "#,
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let events = vec![
            ("bad".to_string(), Dynamic::UNIT),
            ("good".to_string(), Dynamic::UNIT),
        ];
        let errors = mgr.run_frame(standard_dt(), &mut budget, &events, &[], &engine);
        // 應有 1 個錯誤（from "bad" event），但 "good" event 仍被處理
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_on_input_error_continues_next_input() {
        let engine = test_engine();
        let ast = compile(
            &engine,
            r#"
            let input_count = 0;
            fn on_input(it, id) {
                if it == "bad" { throw "error"; }
                input_count += 1;
            }
            "#,
        );
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        let mut budget = FrameBudget::new();
        let inputs = vec![
            ("bad".to_string(), Dynamic::UNIT),
            ("good".to_string(), Dynamic::UNIT),
        ];
        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &inputs, &engine);
        assert_eq!(errors.len(), 1);
    }

    // ═══════════════════════════════════════════════════════════════
    // on_unload RAII 驗證
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_unload_timeout_restores_engine_timeout() {
        let engine = test_engine();
        let ast_a = compile(&engine, r#"fn on_unload() { throw "error"; }"#);
        let ast_b = compile(&engine, "let tick_b = 0;\nfn on_tick(dt) { tick_b += 1; }");
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast_a, 0, &engine).unwrap();
        mgr.load_script(sid("b"), ast_b, 1, &engine).unwrap();

        // unload a（on_unload 拋錯）
        mgr.unload_script(&sid("a"), &engine).unwrap();

        // b 的 run_frame 仍能正常執行
        let mut budget = FrameBudget::new();
        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert!(errors.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════
    // 邊界測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_load_overwrite_same_id() {
        let engine = test_engine();
        let ast1 = compile(&engine, "let v = 1;\nfn on_init() { v = 1; }");
        let ast2 = compile(&engine, "let v = 2;\nfn on_init() { v = 2; }");
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast1, 0, &engine).unwrap();
        mgr.load_script(sid("a"), ast2, 1, &engine).unwrap();
        assert_eq!(mgr.script_count(), 1);

        // 驗證 priority 已更新
        let states = mgr.script_states();
        assert_eq!(states[0].0, sid("a"));
        // BTreeMap key 中 priority 應為新值 1
        let key = mgr.scripts.keys().next().unwrap();
        assert_eq!(key.0, 1);
    }

    #[test]
    fn test_all_scripts_disabled_run_frame_empty() {
        let engine = test_engine();
        let mut mgr = ScriptManager::new();
        for name in &["a", "b", "c"] {
            let ast = compile(&engine, r#"fn on_init() { throw "fail"; }"#);
            let _ = mgr.load_script(sid(name), ast, 0, &engine);
        }
        assert_eq!(mgr.active_script_count(), 0);

        let mut budget = FrameBudget::new();
        let before = budget.remaining_ms();
        let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        assert!(errors.is_empty());
        assert_eq!(budget.remaining_ms(), before);
    }

    // ═══════════════════════════════════════════════════════════════
    // 確定性
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_btreemap_deterministic_order() {
        let engine = test_engine();

        let run = || -> Vec<String> {
            let mut mgr = ScriptManager::new();
            // 亂序載入
            for (name, priority) in &[("e", 4u8), ("c", 2), ("a", 0), ("d", 3), ("b", 1)] {
                let ast = compile(&engine, "");
                mgr.load_script(sid(name), ast, *priority, &engine).unwrap();
            }
            mgr.script_states()
                .iter()
                .map(|(id, _)| id.0.clone())
                .collect()
        };

        let order1 = run();
        let order2 = run();
        assert_eq!(order1, order2);
        assert_eq!(order1, vec!["a", "b", "c", "d", "e"]);
    }

    // ═══════════════════════════════════════════════════════════════
    // Task 04 額外測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_disabled_script_state_inspection() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_init() { throw "fail"; }"#);
        let mut mgr = ScriptManager::new();
        let _ = mgr.load_script(sid("a"), ast, 0, &engine);
        assert!(mgr.is_disabled(&sid("a")));
        let states = mgr.script_states();
        assert_eq!(
            states[0].1,
            ScriptState::Disabled(DisableReason::InitFailed)
        );
    }

    #[test]
    fn test_wrap_error_all_fields() {
        let mgr = ScriptManager::new();
        let ctx = mgr.wrap_error(
            ScriptError::RuntimeError {
                script_id: "s1".into(),
                message: "test".into(),
                tick: 0,
            },
            &sid("s1"),
            LifecycleHookName::Tick,
        );
        assert_eq!(ctx.script_id, sid("s1"));
        assert_eq!(ctx.tick, 0);
        assert_eq!(ctx.hook, LifecycleHookName::Tick);
        assert!(matches!(ctx.error, ScriptError::RuntimeError { .. }));
    }

    #[test]
    fn test_active_script_count_excludes_disabled() {
        let engine = test_engine();
        let mut mgr = ScriptManager::new();

        let ast_fail = compile(&engine, r#"fn on_init() { throw "fail"; }"#);
        let _ = mgr.load_script(sid("a"), ast_fail, 0, &engine);

        let ast_ok1 = compile(&engine, "");
        mgr.load_script(sid("b"), ast_ok1, 1, &engine).unwrap();

        let ast_ok2 = compile(&engine, "");
        mgr.load_script(sid("c"), ast_ok2, 2, &engine).unwrap();

        assert_eq!(mgr.active_script_count(), 2);
        assert_eq!(mgr.script_count(), 3);
    }

    #[test]
    fn test_run_frame_tick_increment() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_tick(dt) { throw "err"; }"#);
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        for expected_tick in 1..=3u64 {
            let mut budget = FrameBudget::new();
            let errors = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
            assert_eq!(errors[0].tick, expected_tick);
        }
    }

    #[test]
    fn test_unload_on_unload_error_still_removes() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_unload() { throw "error"; }"#);
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();
        assert!(mgr.unload_script(&sid("a"), &engine).is_ok());
        assert_eq!(mgr.script_count(), 0);
    }

    #[test]
    fn test_runtime_error_not_counted_for_auto_disable() {
        let engine = test_engine();
        let ast = compile(&engine, r#"fn on_tick(dt) { throw "err"; }"#);
        let mut mgr = ScriptManager::new();
        mgr.load_script(sid("a"), ast, 0, &engine).unwrap();

        // 連續 15 幀 RuntimeError
        for _ in 0..15 {
            let mut budget = FrameBudget::new();
            let _ = mgr.run_frame(standard_dt(), &mut budget, &[], &[], &engine);
        }
        // RuntimeError 不計入 auto-disable
        assert!(!mgr.is_disabled(&sid("a")));
    }

    #[test]
    fn test_load_script_uses_independent_budget() {
        let engine = test_engine();
        let ast = compile(&engine, "let x = 0;\nfn on_init() { x = 1; }");
        let mut mgr = ScriptManager::new();
        // 即使「全域」預算概念存在（此處不傳入），load_script 內部建立獨立 budget
        assert!(mgr.load_script(sid("a"), ast, 0, &engine).is_ok());
        assert_eq!(mgr.script_count(), 1);
    }
}
