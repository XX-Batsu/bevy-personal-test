//! 腳本生命週期管理
//!
//! 定義 [`ScriptInstance`] — 單一腳本實例，持有 AST、Scope 與元資料。
//! 提供生命週期回調（on_init/on_event/on_input/on_tick/on_unload）呼叫介面。
//!
//! # 設計依據
//! - [04-lifecycle/README.md](../../../docs/design/script-engine/04-lifecycle/README.md)
//! - [04-lifecycle/error-handling.md](../../../docs/design/script-engine/04-lifecycle/error-handling.md)
//!
//! # 生命週期流程
//! ```text
//! on_init → (on_event → on_input → on_tick) × N → on_unload
//! ```

use bridge_types::ScriptError;
use deterministic::SoftF32;
use rhai::{Dynamic, Scope, AST};

use crate::sandbox::{FrameBudget, SandboxedEngine};
use crate::scope_limiter::ScopeLimiter;

/// 生命週期 hook 名稱（用於錯誤上下文與日誌）
///
/// 權威定義見 error-handling.md §介面定義
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleHookName {
    Init,
    Tick,
    Event,
    Input,
    Unload,
}

/// 單一腳本實例，持有 AST、Scope 與元資料
///
/// Scope 跨 callback 持久化（on_init 設置的變數在 on_tick 中可見）。
/// 所有 callback 執行後呼叫 `ScopeLimiter::check_scope_sizes()` 驗證大小。
pub struct ScriptInstance {
    /// 腳本唯一識別碼
    pub script_id: String,
    /// 編譯後的 Rhai AST
    ast: AST,
    /// 持久化 Scope（跨 callback 保留變數）
    scope: Scope<'static>,
    /// 執行優先順序（0 = 最高，255 = 最低）
    pub priority: u8,
    /// 是否已執行過 AST 頂層程式碼（初始化 Scope 變數）
    initialized: bool,
}

impl ScriptInstance {
    /// 建立新的 ScriptInstance（priority: 0=最高, 255=最低）
    pub fn new(script_id: String, ast: AST, priority: u8) -> Self {
        Self {
            script_id,
            ast,
            scope: Scope::new(),
            priority,
            initialized: false,
        }
    }

    /// 檢查 AST 中是否定義了指定函數名
    pub fn has_function(&self, name: &str) -> bool {
        self.ast.iter_functions().any(|f| f.name == name)
    }

    /// 取得 Scope 的不可變參考（供外部檢查）
    pub fn scope(&self) -> &Scope<'static> {
        &self.scope
    }

    /// 取得 Scope 的可變參考（供 ScopeLimiter 等外部元件操作）
    pub fn scope_mut(&mut self) -> &mut Scope<'static> {
        &mut self.scope
    }

    /// 替換 AST 並重置 Scope（OTA A/B swap 使用）
    ///
    /// 回傳舊 AST（供 rollback 使用）。替換後 Scope 清空、initialized 重置，
    /// 呼叫端需自行呼叫 call_on_init() 初始化新腳本。
    pub fn replace_ast(&mut self, new_ast: AST) -> AST {
        let old_ast = std::mem::replace(&mut self.ast, new_ast);
        self.scope = Scope::new();
        self.initialized = false;
        old_ast
    }

    /// 呼叫 on_init()。FnNotFound → Ok(())，執行後驗證 Scope 大小。
    pub fn call_on_init(
        &mut self,
        engine: &SandboxedEngine,
        budget: &mut FrameBudget,
    ) -> Result<(), ScriptError> {
        self.call_lifecycle_fn(engine, "on_init", (), Some(budget))
    }

    /// 呼叫 on_event(event_type, event_data)。執行後驗證 Scope 大小。
    pub fn call_on_event(
        &mut self,
        engine: &SandboxedEngine,
        event_type: &str,
        event_data: Dynamic,
        budget: &mut FrameBudget,
    ) -> Result<(), ScriptError> {
        self.call_lifecycle_fn(
            engine,
            "on_event",
            (event_type.to_string(), event_data),
            Some(budget),
        )
    }

    /// 呼叫 on_input(input_type, input_data)。執行後驗證 Scope 大小。
    pub fn call_on_input(
        &mut self,
        engine: &SandboxedEngine,
        input_type: &str,
        input_data: Dynamic,
        budget: &mut FrameBudget,
    ) -> Result<(), ScriptError> {
        self.call_lifecycle_fn(
            engine,
            "on_input",
            (input_type.to_string(), input_data),
            Some(budget),
        )
    }

    /// 呼叫 on_tick(dt)。dt: SoftF32 → f64 → Dynamic（Rhai 不原生支援 SoftF32）。
    /// 確定性由 Rust 側 SoftF32 保證，腳本計算結果不回寫 game state。
    pub fn call_on_tick(
        &mut self,
        engine: &SandboxedEngine,
        dt: SoftF32,
        budget: &mut FrameBudget,
    ) -> Result<(), ScriptError> {
        let dt_dynamic = Dynamic::from(dt.to_f64());
        self.call_lifecycle_fn(engine, "on_tick", (dt_dynamic,), Some(budget))
    }

    /// 呼叫 on_unload()。3ms timeout（含 1ms grace period），不受 FrameBudget 限制。
    /// 使用 RAII guard（TimeoutGuard）確保 panic-safety：timeout 必定恢復為 2ms。
    pub fn call_on_unload(&mut self, engine: &SandboxedEngine) -> Result<(), ScriptError> {
        let _guard = TimeoutGuard::new(engine, UNLOAD_TIMEOUT_MICROS);
        self.call_lifecycle_fn(engine, "on_unload", (), None)
    }

    /// 內部通用生命週期函數呼叫。流程：
    /// 1. has_function → 不存在即 Ok(())
    /// 2. budget 檢查 → ≤ 0ms 則 BudgetExhausted
    /// 3. call_fn_with_scope() → ErrorFunctionNotFound 映射為 Ok(())（雙重保護）
    /// 4. budget.deduct_elapsed()
    /// 5. ScopeLimiter::check_scope_sizes()
    ///
    /// tick 欄位填 0，由 ScriptManager 透過 ScriptErrorContext 補充。
    fn call_lifecycle_fn<A: rhai::FuncArgs>(
        &mut self,
        engine: &SandboxedEngine,
        fn_name: &str,
        args: A,
        budget: Option<&mut FrameBudget>,
    ) -> Result<(), ScriptError> {
        // 0. 首次呼叫時執行 AST 頂層程式碼，初始化 Scope 變數
        //    Rhai 的 call_fn 為 sandboxed 呼叫：函數內 `let` 宣告在返回後被 scope rewind 移除。
        //    因此頂層 `let x = 0;` 必須透過 eval_ast_with_scope 執行，函數才能修改 x。
        if !self.initialized {
            self.initialized = true;
            let _ = engine
                .eval_ast_with_scope(&mut self.scope, &self.ast)
                .map_err(|e| self.map_hook_error(*e))?;
        }

        // 1. 函數不存在 → 靜默跳過
        if !self.has_function(fn_name) {
            return Ok(());
        }

        // 2. FrameBudget 檢查：剩餘 ≤ 0ms 時不執行腳本
        if let Some(ref budget) = budget {
            if budget.remaining_ms() <= 0.0 {
                return Err(ScriptError::BudgetExhausted);
            }
        }

        // 3. 記錄開始時間、呼叫函數
        let start_micros = engine.now_micros();
        let result = match engine.call_fn_with_scope(&mut self.scope, &self.ast, fn_name, args) {
            Ok(_) => Ok(()),
            Err(e) => {
                // 雙重保護：ErrorFunctionNotFound 即使穿過 has_function 檢查，
                // 仍映射為 Ok(())（靜默跳過）
                if matches!(*e, rhai::EvalAltResult::ErrorFunctionNotFound(_, _)) {
                    Ok(())
                } else {
                    Err(self.map_hook_error(*e))
                }
            }
        };

        // 4. FrameBudget 扣減（無論成功或失敗都要扣減已消耗的時間）
        if let Some(budget) = budget {
            let elapsed_micros = engine.now_micros().saturating_sub(start_micros);
            let elapsed_ms = elapsed_micros as f64 / 1000.0;
            budget.deduct_elapsed(elapsed_ms);
        }

        // 5. 執行成功時檢查 Scope 大小限制
        if result.is_ok() {
            ScopeLimiter::check_scope_sizes(&self.scope).map_err(|e| {
                ScriptError::ScopeLimitExceeded {
                    script_id: self.script_id.clone(),
                    detail: e.to_string(),
                    tick: 0,
                }
            })?;
        }

        result
    }

    /// 將 Rhai EvalAltResult 映射為 ScriptError
    ///
    /// tick 欄位統一填 0，由 ScriptManager 層透過 ScriptErrorContext 補充。
    ///
    /// # 前置條件
    /// ErrorFunctionNotFound 已在 call_lifecycle_fn 中處理（映射為 Ok(())），
    /// 不會到達此方法。
    fn map_hook_error(&self, error: rhai::EvalAltResult) -> ScriptError {
        use rhai::EvalAltResult::*;
        match &error {
            ErrorTooManyOperations(_) => ScriptError::OperationLimit {
                script_id: self.script_id.clone(),
                ops: 50_000,
                tick: 0,
            },
            ErrorTerminated(value, _) => {
                let elapsed_micros = value.as_int().unwrap_or(0) as f64;
                ScriptError::Timeout {
                    script_id: self.script_id.clone(),
                    elapsed_ms: elapsed_micros / 1000.0,
                    tick: 0,
                }
            }
            _ => ScriptError::RuntimeError {
                script_id: self.script_id.clone(),
                message: error.to_string(),
                tick: 0,
            },
        }
    }
}

/// RAII guard：建構時 set timeout，Drop 時恢復 DEFAULT_TIMEOUT_MICROS。
struct TimeoutGuard<'a> {
    engine: &'a SandboxedEngine,
    restore_value: u64,
}

impl<'a> TimeoutGuard<'a> {
    fn new(engine: &'a SandboxedEngine, timeout_micros: u64) -> Self {
        engine.set_timeout_micros(timeout_micros);
        Self {
            engine,
            restore_value: DEFAULT_TIMEOUT_MICROS,
        }
    }
}

impl<'a> Drop for TimeoutGuard<'a> {
    fn drop(&mut self) {
        self.engine.set_timeout_micros(self.restore_value);
    }
}

/// on_unload 超時閾值：3ms = 3,000μs（含 1ms grace period）
const UNLOAD_TIMEOUT_MICROS: u64 = 3_000;

/// 預設超時閾值：2ms = 2,000μs
const DEFAULT_TIMEOUT_MICROS: u64 = 2_000;

#[cfg(test)]
mod tests {
    use super::*;
    use deterministic::SoftF32;

    // ── 測試輔助函數 ──

    fn test_engine() -> SandboxedEngine {
        SandboxedEngine::new()
    }

    fn compile_script(engine: &SandboxedEngine, script: &str) -> AST {
        engine.engine().compile(script).expect("測試腳本編譯失敗")
    }

    fn standard_dt() -> SoftF32 {
        SoftF32::from_f64(1.0 / 60.0)
    }

    fn test_budget() -> FrameBudget {
        FrameBudget::new()
    }

    // ═══════════════════════════════════════════════════════════════
    // 正常路徑
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_on_init_called_once() {
        let engine = test_engine();
        let ast = compile_script(&engine, "let x = 0;\nfn on_init() { x = 42; }");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        assert!(inst.call_on_init(&engine, &mut budget).is_ok());
        assert_eq!(inst.scope().get_value::<i64>("x"), Some(42));
    }

    #[test]
    fn test_on_tick_receives_dt() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            "let elapsed = 0.0;\nfn on_tick(dt) { elapsed = dt; }",
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let dt = SoftF32::from_f64(1.0 / 60.0);
        assert!(inst.call_on_tick(&engine, dt, &mut budget).is_ok());
        let elapsed = inst.scope().get_value::<f64>("elapsed").unwrap();
        assert!((elapsed - dt.to_f64()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_on_unload_called() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            "let cleaned = false;\nfn on_unload() { cleaned = true; }",
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        assert!(inst.call_on_unload(&engine).is_ok());
        assert_eq!(inst.scope().get_value::<bool>("cleaned"), Some(true));
    }

    #[test]
    fn test_full_lifecycle() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let phase = "";
            fn on_init() { phase = "init"; }
            fn on_tick(dt) { phase = "tick"; }
            fn on_unload() { phase = "unload"; }
            "#,
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let dt = standard_dt();

        assert!(inst.call_on_init(&engine, &mut budget).is_ok());
        assert_eq!(
            inst.scope()
                .get_value::<rhai::ImmutableString>("phase")
                .unwrap()
                .as_str(),
            "init"
        );

        for _ in 0..3 {
            assert!(inst.call_on_tick(&engine, dt, &mut budget).is_ok());
        }
        assert_eq!(
            inst.scope()
                .get_value::<rhai::ImmutableString>("phase")
                .unwrap()
                .as_str(),
            "tick"
        );

        assert!(inst.call_on_unload(&engine).is_ok());
        assert_eq!(
            inst.scope()
                .get_value::<rhai::ImmutableString>("phase")
                .unwrap()
                .as_str(),
            "unload"
        );
    }

    #[test]
    fn test_scope_persists_across_callbacks() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let counter = 0;
            fn on_init() { counter = 0; }
            fn on_tick(dt) { counter += 1; }
            "#,
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let dt = standard_dt();

        inst.call_on_init(&engine, &mut budget).unwrap();
        for _ in 0..3 {
            inst.call_on_tick(&engine, dt, &mut budget).unwrap();
        }
        assert_eq!(inst.scope().get_value::<i64>("counter"), Some(3));
    }

    #[test]
    fn test_on_event_dual_params() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let et = "";
            let ed = 0;
            fn on_event(event_type, event_data) {
                et = event_type;
                ed = event_data;
            }
            "#,
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        assert!(inst
            .call_on_event(&engine, "damage", Dynamic::from(42_i64), &mut budget)
            .is_ok());
        assert_eq!(
            inst.scope()
                .get_value::<rhai::ImmutableString>("et")
                .unwrap()
                .as_str(),
            "damage"
        );
        assert_eq!(inst.scope().get_value::<i64>("ed"), Some(42));
    }

    #[test]
    fn test_on_input_dual_params() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let it = "";
            let id = 0;
            fn on_input(input_type, input_data) {
                it = input_type;
                id = input_data;
            }
            "#,
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        assert!(inst
            .call_on_input(&engine, "move", Dynamic::from(1_i64), &mut budget)
            .is_ok());
        assert_eq!(
            inst.scope()
                .get_value::<rhai::ImmutableString>("it")
                .unwrap()
                .as_str(),
            "move"
        );
        assert_eq!(inst.scope().get_value::<i64>("id"), Some(1));
    }

    #[test]
    fn test_has_function_true() {
        let engine = test_engine();
        let ast = compile_script(&engine, "fn on_init() {}");
        let inst = ScriptInstance::new("test".into(), ast, 0);
        assert!(inst.has_function("on_init"));
    }

    #[test]
    fn test_has_function_false() {
        let engine = test_engine();
        let ast = compile_script(&engine, "fn other() {}");
        let inst = ScriptInstance::new("test".into(), ast, 0);
        assert!(!inst.has_function("on_init"));
    }

    #[test]
    fn test_callback_order_verification() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let order = "";
            fn on_init() { order = ""; }
            fn on_event(et, ed) { order += "E"; }
            fn on_input(it, id) { order += "I"; }
            fn on_tick(dt) { order += "T"; }
            "#,
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        inst.call_on_init(&engine, &mut budget).unwrap();
        inst.call_on_event(&engine, "e", Dynamic::UNIT, &mut budget)
            .unwrap();
        inst.call_on_input(&engine, "i", Dynamic::UNIT, &mut budget)
            .unwrap();
        inst.call_on_tick(&engine, standard_dt(), &mut budget)
            .unwrap();
        assert_eq!(
            inst.scope()
                .get_value::<rhai::ImmutableString>("order")
                .unwrap()
                .as_str(),
            "EIT"
        );
    }

    #[test]
    fn test_multi_script_priority_order() {
        let engine = test_engine();
        let ast1 = compile_script(&engine, "fn on_init() {}");
        let ast2 = compile_script(&engine, "fn on_init() {}");
        let inst1 = ScriptInstance::new("b_script".into(), ast1, 10);
        let inst2 = ScriptInstance::new("a_script".into(), ast2, 5);

        let mut scripts = vec![inst1, inst2];
        scripts.sort_by(|a, b| (a.priority, &a.script_id).cmp(&(b.priority, &b.script_id)));

        assert_eq!(scripts[0].priority, 5);
        assert_eq!(scripts[0].script_id, "a_script");
        assert_eq!(scripts[1].priority, 10);
        assert_eq!(scripts[1].script_id, "b_script");
    }

    // ═══════════════════════════════════════════════════════════════
    // 空輸入與缺失函式
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_missing_on_init_no_error() {
        let engine = test_engine();
        let ast = compile_script(&engine, "fn other() {}");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        assert!(inst.call_on_init(&engine, &mut budget).is_ok());
    }

    #[test]
    fn test_missing_all_hooks_no_error() {
        let engine = test_engine();
        let ast = compile_script(&engine, "let x = 1;");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        assert!(inst.call_on_init(&engine, &mut budget).is_ok());
        assert!(inst
            .call_on_event(&engine, "e", Dynamic::UNIT, &mut budget)
            .is_ok());
        assert!(inst
            .call_on_input(&engine, "i", Dynamic::UNIT, &mut budget)
            .is_ok());
        assert!(inst
            .call_on_tick(&engine, standard_dt(), &mut budget)
            .is_ok());
        assert!(inst.call_on_unload(&engine).is_ok());
    }

    #[test]
    fn test_on_event_empty_event_type() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            "let et = \"\";\nfn on_event(event_type, event_data) { et = event_type; }",
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        assert!(inst
            .call_on_event(&engine, "", Dynamic::UNIT, &mut budget)
            .is_ok());
        assert_eq!(
            inst.scope()
                .get_value::<rhai::ImmutableString>("et")
                .unwrap()
                .as_str(),
            ""
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // 邊界值
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_on_tick_dt_zero() {
        let engine = test_engine();
        let ast = compile_script(&engine, "let d = 0.0;\nfn on_tick(dt) { d = dt; }");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let dt = SoftF32::from_f64(0.0);
        assert!(inst.call_on_tick(&engine, dt, &mut budget).is_ok());
        let d = inst.scope().get_value::<f64>("d").unwrap();
        assert!((d - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_priority_min_max() {
        let engine = test_engine();
        let ast0 = compile_script(&engine, "fn on_init() {}");
        let ast255 = compile_script(&engine, "fn on_init() {}");
        let inst0 = ScriptInstance::new("min".into(), ast0, 0);
        let inst255 = ScriptInstance::new("max".into(), ast255, 255);
        assert_eq!(inst0.priority, 0);
        assert_eq!(inst255.priority, 255);
    }

    // ═══════════════════════════════════════════════════════════════
    // 錯誤路徑
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_on_init_error_propagates() {
        let engine = test_engine();
        let ast = compile_script(&engine, r#"fn on_init() { throw "init failed"; }"#);
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let result = inst.call_on_init(&engine, &mut budget);
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::RuntimeError {
                script_id, message, ..
            } => {
                assert_eq!(script_id, "test");
                assert!(message.contains("init failed"));
            }
            other => panic!("預期 RuntimeError，實際: {:?}", other),
        }
    }

    #[test]
    fn test_on_tick_runtime_error_no_disable() {
        let engine = test_engine();
        let ast = compile_script(&engine, r#"fn on_tick(dt) { throw "tick error"; }"#);
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let result = inst.call_on_tick(&engine, standard_dt(), &mut budget);
        assert!(matches!(result, Err(ScriptError::RuntimeError { .. })));
    }

    #[test]
    fn test_budget_exhausted_skips_callback() {
        let engine = test_engine();
        let ast = compile_script(&engine, "let x = 0;\nfn on_tick(dt) { x = 1; }");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = FrameBudget::new();
        // 手動消耗所有預算
        budget.deduct_elapsed(5.0);
        let result = inst.call_on_tick(&engine, standard_dt(), &mut budget);
        assert!(matches!(result, Err(ScriptError::BudgetExhausted)));
    }

    #[test]
    fn test_budget_deduction_after_callback() {
        let engine = test_engine();
        let ast = compile_script(&engine, "let x = 0;\nfn on_tick(dt) { x = 1; }");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let before = budget.remaining_ms();
        inst.call_on_tick(&engine, standard_dt(), &mut budget)
            .unwrap();
        // 執行後預算應減少（至少一點點）
        assert!(budget.remaining_ms() <= before);
    }

    #[test]
    fn test_scope_limit_error_after_callback() {
        let engine = test_engine();
        // 動態建立超過 256 個 let 變數
        // 注意：on_def_var 限制器在 SandboxedEngine 上沒有註冊 ScopeLimiter，
        // 因此用 Scope 注入方式測試 check_scope_sizes
        let ast = compile_script(&engine, "let x = 0;\nfn on_tick(dt) { x = 1; }");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        // 預先注入 257 個變數到 scope
        for i in 0..257 {
            inst.scope_mut().push(format!("v_{}", i), 0_i64);
        }
        let mut budget = test_budget();
        let result = inst.call_on_tick(&engine, standard_dt(), &mut budget);
        assert!(matches!(
            result,
            Err(ScriptError::ScopeLimitExceeded { .. })
        ));
    }

    // ═══════════════════════════════════════════════════════════════
    // 確定性
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_dt_softf32_deterministic() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let sum = 0.0;
            fn on_init() { sum = 0.0; }
            fn on_tick(dt) { sum += dt; }
            "#,
        );
        let dt = SoftF32::from_f64(1.0 / 60.0);

        // 執行 100 次
        let mut inst1 = ScriptInstance::new("test1".into(), ast.clone(), 0);
        let mut budget1 = test_budget();
        inst1.call_on_init(&engine, &mut budget1).unwrap();
        for _ in 0..100 {
            budget1.reset();
            inst1.call_on_tick(&engine, dt, &mut budget1).unwrap();
        }
        let sum1 = inst1.scope().get_value::<f64>("sum").unwrap();

        // 再次執行 100 次
        let mut inst2 = ScriptInstance::new("test2".into(), ast, 0);
        let mut budget2 = test_budget();
        inst2.call_on_init(&engine, &mut budget2).unwrap();
        for _ in 0..100 {
            budget2.reset();
            inst2.call_on_tick(&engine, dt, &mut budget2).unwrap();
        }
        let sum2 = inst2.scope().get_value::<f64>("sum").unwrap();

        assert_eq!(
            sum1.to_bits(),
            sum2.to_bits(),
            "兩次執行結果應 bit-for-bit 一致"
        );
    }

    #[test]
    fn test_scope_state_deterministic() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let a = 0;
            let b = "";
            fn on_init() { a = 0; b = ""; }
            fn on_tick(dt) { a += 1; b += "x"; }
            "#,
        );
        let dt = standard_dt();

        let run = |ast: &AST| -> (i64, String) {
            let mut inst = ScriptInstance::new("test".into(), ast.clone(), 0);
            let mut budget = test_budget();
            inst.call_on_init(&engine, &mut budget).unwrap();
            for _ in 0..10 {
                budget.reset();
                inst.call_on_tick(&engine, dt, &mut budget).unwrap();
            }
            let a = inst.scope().get_value::<i64>("a").unwrap();
            let b = inst
                .scope()
                .get_value::<rhai::ImmutableString>("b")
                .unwrap()
                .to_string();
            (a, b)
        };

        let (a1, b1) = run(&ast);
        let (a2, b2) = run(&ast);
        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
    }

    // ═══════════════════════════════════════════════════════════════
    // Task 02 額外測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_lifecycle_hook_name_variants() {
        let variants = [
            LifecycleHookName::Init,
            LifecycleHookName::Tick,
            LifecycleHookName::Event,
            LifecycleHookName::Input,
            LifecycleHookName::Unload,
        ];
        assert_eq!(variants.len(), 5);
        // PartialEq
        assert_eq!(LifecycleHookName::Init, LifecycleHookName::Init);
        assert_ne!(LifecycleHookName::Init, LifecycleHookName::Tick);
        // Debug
        for v in &variants {
            let _ = format!("{:?}", v);
        }
    }

    #[test]
    fn test_priority_roundtrip_0_255() {
        let engine = test_engine();
        let ast = compile_script(&engine, "");
        let inst0 = ScriptInstance::new("p0".into(), ast.clone(), 0);
        let inst255 = ScriptInstance::new("p255".into(), ast, 255);
        assert_eq!(inst0.priority, 0);
        assert_eq!(inst255.priority, 255);
    }

    #[test]
    fn test_priority_same_value_sorted_by_script_id() {
        let engine = test_engine();
        let ast = compile_script(&engine, "");
        let inst_b = ScriptInstance::new("b".into(), ast.clone(), 5);
        let inst_a = ScriptInstance::new("a".into(), ast, 5);

        let mut scripts = vec![inst_b, inst_a];
        scripts.sort_by(|a, b| (a.priority, &a.script_id).cmp(&(b.priority, &b.script_id)));
        assert_eq!(scripts[0].script_id, "a");
        assert_eq!(scripts[1].script_id, "b");
    }

    #[test]
    fn test_call_on_init_scope_retains_vars() {
        let engine = test_engine();
        let ast = compile_script(&engine, "let x = 0;\nfn on_init() { x = 42; }");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        inst.call_on_init(&engine, &mut budget).unwrap();
        assert_eq!(inst.scope().get_value::<i64>("x"), Some(42));
    }

    #[test]
    fn test_call_on_tick_modifies_scope() {
        let engine = test_engine();
        let ast = compile_script(
            &engine,
            r#"
            let c = 0;
            fn on_init() { c = 0; }
            fn on_tick(dt) { c += 1; }
            "#,
        );
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        inst.call_on_init(&engine, &mut budget).unwrap();
        for _ in 0..3 {
            inst.call_on_tick(&engine, standard_dt(), &mut budget)
                .unwrap();
        }
        assert_eq!(inst.scope().get_value::<i64>("c"), Some(3));
    }

    #[test]
    fn test_call_lifecycle_fn_missing_returns_ok() {
        let engine = test_engine();
        let ast = compile_script(&engine, "");
        let mut inst = ScriptInstance::new("test".into(), ast, 0);
        let mut budget = test_budget();
        let before = budget.remaining_ms();
        assert!(inst.call_on_init(&engine, &mut budget).is_ok());
        assert!(inst
            .call_on_tick(&engine, standard_dt(), &mut budget)
            .is_ok());
        assert!(inst
            .call_on_event(&engine, "e", Dynamic::UNIT, &mut budget)
            .is_ok());
        assert!(inst
            .call_on_input(&engine, "i", Dynamic::UNIT, &mut budget)
            .is_ok());
        assert!(inst.call_on_unload(&engine).is_ok());
        // 未執行腳本，budget 未扣減
        assert_eq!(budget.remaining_ms(), before);
    }

    #[test]
    fn test_runtime_error_maps_to_script_error() {
        let engine = test_engine();
        let ast = compile_script(&engine, r#"fn on_init() { throw "test error"; }"#);
        let mut inst = ScriptInstance::new("s1".into(), ast, 0);
        let mut budget = test_budget();
        let result = inst.call_on_init(&engine, &mut budget);
        match result {
            Err(ScriptError::RuntimeError {
                script_id,
                message,
                tick,
            }) => {
                assert_eq!(script_id, "s1");
                assert!(message.contains("test error"));
                assert_eq!(tick, 0);
            }
            other => panic!("預期 RuntimeError，實際: {:?}", other),
        }
    }
}
