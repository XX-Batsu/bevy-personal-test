//! Rhai 沙箱引擎核心模組
//!
//! 包裝 `rhai::Engine`，設定操作限制、超時檢測、eval 阻擋等安全機制。
//! 執行上下文不含 script_id/tick（由 Phase 8 ScriptManager 補充）。
//!
//! # 資源限制速查
//! | 項目 | 限制值 |
//! |------|--------|
//! | 最大操作數/次 | 50,000 ops |
//! | 最大呼叫深度 | 32 levels |
//! | 最大字串長度 | 4,096 bytes |
//! | 最大陣列大小 | 1,024 elements |
//! | 單次超時 | 2ms（on_progress 檢查） |
//!
//! # 設計依據
//! - `docs/design/script-engine/02-sandbox/engine-configuration.md`
//! - `docs/design/script-engine/02-sandbox/resource-limits.md`

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bridge_types::ScriptError;
use deterministic::clock::Clock;
#[cfg(not(target_arch = "wasm32"))]
use deterministic::clock::NativeClock;

/// 單次超時閾值：2ms = 2,000 微秒
const TIMEOUT_MICROS: u64 = 2_000;

/// 每幀預算預設值（毫秒）
const FRAME_BUDGET_MS: f64 = 4.0;

/// 單次腳本超時上限（毫秒）
const SINGLE_SCRIPT_TIMEOUT_MS: f64 = 2.0;

/// 每幀時間預算管理器
///
/// 管理每幀 4ms 的腳本執行時間上限。所有腳本共享同一個 FrameBudget。
/// 單次腳本超時取 min(2ms, remaining_ms)。
///
/// # Phase 6 簡化版
/// cross-references.md 定義 Phase 6 Public API 為 new()/remaining_ms()/reset() 三方法。
/// Design doc [06-performance/frame-budget.md] 定義了完整版（含 CallToken、
/// begin_call/end_call、deduct_bridge_ops、BudgetError），屬 Phase 7/8 擴充範疇。
///
/// # 使用流程
/// ```rust,no_run
/// # use vm_runtime::sandbox::{SandboxedEngine, FrameBudget};
/// let engine = SandboxedEngine::new();
/// let mut budget = FrameBudget::new();
/// engine.execute_with_budget("let x = 1;", &mut budget).ok();
/// engine.execute_with_budget("let y = 2;", &mut budget).ok();
/// budget.reset(); // 下一幀開始前重置
/// ```
pub struct FrameBudget {
    /// 剩餘毫秒數（初始值 FRAME_BUDGET_MS = 4.0）
    remaining_ms: f64,
}

impl FrameBudget {
    /// 建立新的每幀預算（4.0ms）
    pub fn new() -> Self {
        Self {
            remaining_ms: FRAME_BUDGET_MS,
        }
    }

    /// 取得剩餘毫秒（保證 >= 0.0）
    ///
    /// 使用 `.max(0.0)` 防止浮點精度導致的微量負值。
    /// 內部 `remaining_ms` 可能因 f64 減法精度略低於 0（例如 -1e-15），
    /// 對外一律回傳 0.0 作為下界。
    pub fn remaining_ms(&self) -> f64 {
        self.remaining_ms.max(0.0)
    }

    /// 重置預算至初始值（4.0ms）
    ///
    /// 每幀開始時由 Phase 8 ScriptManager 呼叫。
    pub fn reset(&mut self) {
        self.remaining_ms = FRAME_BUDGET_MS;
    }

    /// 建立指定剩餘時間的預算（僅測試用）
    ///
    /// 用於模擬預算耗盡場景，避免直接存取 private 欄位。
    #[cfg(test)]
    pub fn with_remaining(remaining_ms: f64) -> Self {
        Self { remaining_ms }
    }
}

impl Default for FrameBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// 沙箱化 Rhai 引擎
///
/// 透過 Rhai Engine 的原生安全機制（set_max_operations、set_max_call_levels、
/// disable_symbol）加上自定義 on_progress callback（合併 ops counting + timeout）
/// 實現完整沙箱。
pub struct SandboxedEngine {
    engine: rhai::Engine,
    /// 起始時間戳（微秒），每次 execute() 前原子更新。
    /// 使用 Arc<AtomicU64> 因 Rhai on_progress callback 要求 Send + Sync。
    start_time: Arc<AtomicU64>,
    /// 動態超時閾值（微秒），execute() 使用固定 TIMEOUT_MICROS，
    /// execute_with_budget()（Task 5）會動態更新此值。
    timeout_micros: Arc<AtomicU64>,
    /// 時間來源抽象（Native: Instant, WASM: Performance.now()）
    clock: Arc<dyn Clock>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for SandboxedEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxedEngine {
    /// 建立含完整沙箱配置的 Rhai Engine（使用預設 NativeClock）
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self::with_clock(Arc::new(NativeClock::new()))
    }

    /// 使用自定義 Clock 建立沙箱引擎（測試用）
    ///
    /// 允許注入 MockClock 以測試超時行為，避免依賴真實壁鐘時間。
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        let mut engine = rhai::Engine::new();

        // --- 資源限制（四步配置順序 Step 1） ---
        engine.set_max_operations(50_000);
        engine.set_max_call_levels(32);
        engine.set_max_string_size(4_096);
        engine.set_max_array_size(1_024);

        // --- 危險符號阻擋（四步配置順序 Step 2） ---
        engine.disable_symbol("eval");
        engine.disable_symbol("type_of");
        engine.disable_symbol("Fn");
        engine.disable_symbol("call");
        engine.disable_symbol("curry");
        engine.disable_symbol("is_shared");

        // --- Step 3（Bridge API 掛載）Phase 6 不實作，延至 Phase 8 ---

        // --- on_progress callback（四步配置順序 Step 4：合併 ops + timeout） ---
        let start_time = Arc::new(AtomicU64::new(0));
        let start_time_clone = start_time.clone();
        let clock_clone = clock.clone();
        let timeout_micros = Arc::new(AtomicU64::new(TIMEOUT_MICROS));
        let timeout_clone = timeout_micros.clone();

        // on_progress 在每個 Rhai AST node 執行時觸發。
        // 回傳 Some(Dynamic) 表示終止執行，None 表示繼續。
        engine.on_progress(move |_ops_count: u64| {
            let start = start_time_clone.load(Ordering::Relaxed);
            let now = clock_clone.now_micros();
            let elapsed = now.saturating_sub(start);
            let limit = timeout_clone.load(Ordering::Relaxed);

            if elapsed >= limit {
                // 終止執行，攜帶 elapsed 微秒數供 map_rhai_error 使用
                Some(rhai::Dynamic::from(elapsed as i64))
            } else {
                None
            }
        });

        Self {
            engine,
            start_time,
            timeout_micros,
            clock,
        }
    }

    /// 執行 Rhai 腳本
    ///
    /// # 流程
    /// 1. compile(script) → CompileError
    /// 2. 建立新 Scope
    /// 3. 更新 start_time（確保 on_progress 讀到正確起始值）
    /// 4. 重置 timeout_micros 為固定 2,000μs
    /// 5. eval_ast_with_scope → 錯誤映射
    pub fn execute(&self, script: &str) -> Result<rhai::Dynamic, ScriptError> {
        // 1. 編譯
        let ast = self.engine.compile(script).map_err(|e| {
            tracing::warn!("腳本編譯失敗: {}", e);
            ScriptError::CompileError(e.to_string())
        })?;

        // 2. 建立新 Scope
        let mut scope = rhai::Scope::new();

        // 3. 更新起始時間戳（必須在 eval 之前）
        let start = self.clock.now_micros();
        self.start_time.store(start, Ordering::Relaxed);

        // 4. 重置超時閾值為固定 2ms
        self.timeout_micros.store(TIMEOUT_MICROS, Ordering::Relaxed);

        // 5. 執行
        self.engine
            .eval_ast_with_scope(&mut scope, &ast)
            .map_err(|e| Self::map_rhai_error(*e, start, &self.clock))
    }

    /// 動態設定超時閾值（微秒）
    ///
    /// Phase 8 on_unload 需要 3ms timeout（3,000μs），呼叫後應恢復 2,000μs。
    pub fn set_timeout_micros(&self, micros: u64) {
        self.timeout_micros.store(micros, Ordering::Relaxed);
    }

    /// 取得內部 Engine 的可變引用（供 ScopeLimiter 等註冊 callback 使用）
    pub fn engine_mut(&mut self) -> &mut rhai::Engine {
        &mut self.engine
    }

    /// 取得內部 Engine 的不可變引用
    pub fn engine(&self) -> &rhai::Engine {
        &self.engine
    }

    /// 在幀預算內執行腳本
    ///
    /// # 流程
    /// 1. 檢查 `budget.remaining_ms <= 0.0` → `Err(ScriptError::BudgetExhausted)`
    /// 2. 計算本次超時閾值：`min(SINGLE_SCRIPT_TIMEOUT_MS, budget.remaining_ms)`
    /// 3. 透過 `self.timeout_micros`（Arc<AtomicU64>）更新 on_progress 超時閾值
    /// 4. compile（on_progress 不在 compile 階段觸發）
    /// 5. 記錄起始時間（compile 之後、eval 之前）
    /// 6. eval_ast_with_scope
    /// 7. 計算實際耗時並扣減 budget
    /// 8. 映射錯誤
    ///
    /// # 注意
    /// - 無論執行成功或失敗，budget 都會扣減實際耗時
    /// - BudgetExhausted 時不扣減（未執行腳本）
    /// - CompileError 時不扣減（compile 在 start_time 記錄前返回）
    pub fn execute_with_budget(
        &self,
        script: &str,
        budget: &mut FrameBudget,
    ) -> Result<rhai::Dynamic, ScriptError> {
        // 1. 預算檢查
        if budget.remaining_ms <= 0.0 {
            tracing::warn!("幀預算已耗盡，跳過腳本執行");
            return Err(ScriptError::BudgetExhausted);
        }

        // 2. 計算超時閾值
        let timeout_ms = SINGLE_SCRIPT_TIMEOUT_MS.min(budget.remaining_ms);
        let timeout_micros = (timeout_ms * 1000.0) as u64;

        // 3. 更新超時閾值（透過 AtomicU64 傳遞給 on_progress callback）
        self.timeout_micros.store(timeout_micros, Ordering::Relaxed);

        // 4. 編譯（on_progress 不在 compile 階段觸發）
        let ast = self.engine.compile(script).map_err(|e| {
            tracing::warn!("腳本編譯失敗: {}", e);
            ScriptError::CompileError(e.to_string())
        })?;

        // 5. 記錄起始時間（必須在 compile 之後、eval 之前，與 execute() 一致）
        let start = self.clock.now_micros();
        self.start_time.store(start, Ordering::Relaxed);

        // 6. 執行
        let mut scope = rhai::Scope::new();
        let result = self
            .engine
            .eval_ast_with_scope(&mut scope, &ast)
            .map_err(|e| Self::map_rhai_error(*e, start, &self.clock));

        // 7. 計算實際耗時並扣減 budget
        let elapsed_micros = self.clock.now_micros().saturating_sub(start);
        let elapsed_ms = elapsed_micros as f64 / 1000.0;
        budget.remaining_ms -= elapsed_ms;

        result
    }

    /// 將 Rhai EvalAltResult 映射為 ScriptError
    ///
    /// Phase 6 不含 script_id/tick 上下文，使用空字串和 0 作為佔位值。
    /// Phase 8 ScriptManager 會以 ScriptErrorContext 包裝補充上下文。
    fn map_rhai_error(
        error: rhai::EvalAltResult,
        start_micros: u64,
        clock: &Arc<dyn Clock>,
    ) -> ScriptError {
        use rhai::EvalAltResult::*;

        match &error {
            // ops limit（Rhai engine level 50,000 ops）
            ErrorTooManyOperations(_) => {
                tracing::warn!("腳本執行超過操作數限制 (50,000 ops)");
                ScriptError::OperationLimit {
                    script_id: String::new(),
                    ops: 50_000,
                    tick: 0,
                }
            }
            // on_progress 終止 → Timeout
            ErrorTerminated(_, _) => {
                let elapsed_micros = clock.now_micros().saturating_sub(start_micros);
                let elapsed_ms = elapsed_micros as f64 / 1000.0;
                tracing::warn!("腳本執行超時: {:.2}ms", elapsed_ms);
                ScriptError::Timeout {
                    script_id: String::new(),
                    elapsed_ms,
                    tick: 0,
                }
            }
            // 其餘 → RuntimeError（含 eval 被 disable_symbol 阻擋、
            // stack overflow、字串/陣列超限等）
            _ => {
                tracing::warn!("腳本執行期錯誤: {}", error);
                ScriptError::RuntimeError {
                    script_id: String::new(),
                    message: error.to_string(),
                    tick: 0,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// 測試用 MockClock：每次 now_micros() 呼叫推進固定微秒數
    struct MockClock {
        current: AtomicU64,
        step_micros: u64,
    }

    impl MockClock {
        fn new(step_micros: u64) -> Self {
            Self {
                current: AtomicU64::new(0),
                step_micros,
            }
        }
    }

    impl Clock for MockClock {
        fn now_micros(&self) -> u64 {
            self.current.fetch_add(self.step_micros, Ordering::Relaxed)
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // 基本執行
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_simple_arithmetic() {
        let engine = SandboxedEngine::new();
        let result = engine.execute("let x = 1 + 2; x").unwrap();
        assert_eq!(result.as_int().unwrap(), 3);
    }

    #[test]
    fn test_normal_script_within_limits() {
        let engine = SandboxedEngine::new();
        let script = r#"
            let a = 10;
            let b = 20;
            let c = if a > b { a } else { b };
            c * 2
        "#;
        let result = engine.execute(script).unwrap();
        assert_eq!(result.as_int().unwrap(), 40);
    }

    // ═══════════════════════════════════════════════════════════════
    // eval 阻擋
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_eval_blocked() {
        let engine = SandboxedEngine::new();
        let result = engine.execute(r#"eval("1 + 2")"#);
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::RuntimeError { .. } | ScriptError::CompileError(_) => {}
            other => panic!("預期 RuntimeError 或 CompileError，實際: {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // 操作數限制
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_infinite_loop_terminated() {
        let engine = SandboxedEngine::new();
        let result = engine.execute("loop {}");
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::OperationLimit { .. } | ScriptError::Timeout { .. } => {}
            other => panic!("預期 OperationLimit 或 Timeout，實際: {:?}", other),
        }
    }

    #[test]
    fn test_ops_limit_exceeded() {
        // 使用 MockClock（step=0）避免超時先於操作數限制觸發
        let clock = Arc::new(MockClock::new(0)); // 時間不推進，不會觸發超時
        let engine = SandboxedEngine::with_clock(clock);
        let result = engine.execute("let x = 0; while x < 100000 { x += 1; }");
        assert!(matches!(result, Err(ScriptError::OperationLimit { .. })));
    }

    // ═══════════════════════════════════════════════════════════════
    // 呼叫深度限制
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_deep_recursion_blocked() {
        let engine = SandboxedEngine::new();
        let result = engine.execute("fn recurse(n) { recurse(n + 1) } recurse(0)");
        assert!(matches!(result, Err(ScriptError::RuntimeError { .. })));
    }

    // ═══════════════════════════════════════════════════════════════
    // 危險符號阻擋
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_fn_symbol_blocked() {
        let engine = SandboxedEngine::new();
        let result = engine.execute(r#"Fn("x")"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_call_symbol_blocked() {
        let engine = SandboxedEngine::new();
        let result = engine.execute("let f = 1; call(f)");
        assert!(result.is_err());
    }

    #[test]
    fn test_type_of_blocked() {
        let engine = SandboxedEngine::new();
        let result = engine.execute("type_of(42)");
        assert!(result.is_err());
    }

    #[test]
    fn test_curry_blocked() {
        let engine = SandboxedEngine::new();
        let result = engine.execute("curry()");
        assert!(result.is_err());
    }

    #[test]
    fn test_is_shared_blocked() {
        let engine = SandboxedEngine::new();
        let result = engine.execute("is_shared(42)");
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════════════════════════
    // 資源限制
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_max_string_size() {
        let engine = SandboxedEngine::new();
        let script = r#"
            let s = "";
            while s.len() < 5000 {
                s += "aaaaaaaaaa";
            }
            s
        "#;
        let result = engine.execute(script);
        assert!(result.is_err());
    }

    #[test]
    fn test_max_array_size() {
        let engine = SandboxedEngine::new();
        let script = r#"
            let a = [];
            while a.len() < 1100 {
                a.push(1);
            }
            a
        "#;
        let result = engine.execute(script);
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════════════════════════
    // Clock 注入與超時
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_with_clock_executes_normally() {
        let clock = Arc::new(MockClock::new(1)); // 每次推進 1μs
        let engine = SandboxedEngine::with_clock(clock);
        let result = engine.execute("let x = 1 + 2; x").unwrap();
        assert_eq!(result.as_int().unwrap(), 3);
    }

    #[test]
    fn test_with_mock_clock_timeout() {
        // MockClock 每次推進 3000μs (3ms) > TIMEOUT_MICROS (2000μs)
        let clock = Arc::new(MockClock::new(3_000));
        let engine = SandboxedEngine::with_clock(clock);
        let result = engine.execute("let x = 1 + 2; x");
        assert!(matches!(result, Err(ScriptError::Timeout { .. })));
    }

    // ═══════════════════════════════════════════════════════════════
    // FrameBudget 基本 API
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_frame_budget_initial() {
        let budget = FrameBudget::new();
        assert_eq!(budget.remaining_ms(), 4.0);
    }

    #[test]
    fn test_frame_budget_default_trait() {
        let budget = FrameBudget::default();
        assert_eq!(budget.remaining_ms(), 4.0);
    }

    // ═══════════════════════════════════════════════════════════════
    // FrameBudget 扣減行為
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_frame_budget_deducted_after_execute() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::new();

        let _ = engine
            .execute_with_budget("let x = 1 + 2; x", &mut budget)
            .unwrap();

        // 執行後剩餘時間應小於初始值
        assert!(budget.remaining_ms() < 4.0);
        // 但對於簡單腳本，剩餘時間應大於 0
        assert!(budget.remaining_ms() > 0.0);
    }

    #[test]
    fn test_frame_budget_cumulative_deduction() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::new();

        let _ = engine
            .execute_with_budget("let a = 1;", &mut budget)
            .unwrap();
        let after_first = budget.remaining_ms();

        let _ = engine
            .execute_with_budget("let b = 2;", &mut budget)
            .unwrap();
        let after_second = budget.remaining_ms();

        // 累計扣減：每次執行後剩餘時間遞減
        assert!(after_first < 4.0);
        assert!(after_second < after_first);
    }

    // ═══════════════════════════════════════════════════════════════
    // FrameBudget 預算耗盡
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_frame_budget_exhausted_zero() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::with_remaining(0.0);

        let result = engine.execute_with_budget("let x = 1;", &mut budget);
        assert!(matches!(result, Err(ScriptError::BudgetExhausted)));
    }

    #[test]
    fn test_frame_budget_exhausted_negative() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::with_remaining(-1.0);

        let result = engine.execute_with_budget("let x = 1;", &mut budget);
        assert!(matches!(result, Err(ScriptError::BudgetExhausted)));
    }

    #[test]
    fn test_frame_budget_not_deducted_on_exhausted() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::with_remaining(0.0);

        let _ = engine.execute_with_budget("let x = 1;", &mut budget);
        // BudgetExhausted 路徑不執行腳本，不應扣減
        assert_eq!(budget.remaining_ms(), 0.0);
    }

    // ═══════════════════════════════════════════════════════════════
    // FrameBudget 重置
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_frame_budget_reset() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::new();

        // 使用一些預算
        let _ = engine
            .execute_with_budget("let a = 1;", &mut budget)
            .unwrap();
        assert!(budget.remaining_ms() < 4.0);

        // 重置
        budget.reset();
        assert_eq!(budget.remaining_ms(), 4.0);

        // 重置後可再次執行
        let result = engine.execute_with_budget("let b = 2;", &mut budget);
        assert!(result.is_ok());
    }

    // ═══════════════════════════════════════════════════════════════
    // FrameBudget 超時閾值受 remaining_ms 影響
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_frame_budget_timeout_capped_by_remaining() {
        // MockClock 每次推進 600μs (0.6ms)
        // remaining = 0.5ms → 超時閾值 = min(2ms, 0.5ms) = 0.5ms = 500μs
        // on_progress 第一次觸發時 elapsed ≈ 600μs > 500μs → Timeout
        let clock = Arc::new(MockClock::new(600));
        let engine = SandboxedEngine::with_clock(clock);
        let mut budget = FrameBudget::with_remaining(0.5);

        let result = engine.execute_with_budget("let x = 1 + 2; x", &mut budget);
        assert!(matches!(result, Err(ScriptError::Timeout { .. })));
    }

    // ═══════════════════════════════════════════════════════════════
    // FrameBudget 錯誤情境仍扣減 budget
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_frame_budget_deducted_on_timeout() {
        // MockClock 每次推進 3000μs (3ms) → 穩定觸發超時
        let clock = Arc::new(MockClock::new(3_000));
        let engine = SandboxedEngine::with_clock(clock);
        let mut budget = FrameBudget::new();

        let result = engine.execute_with_budget("let x = 1 + 2; x", &mut budget);
        assert!(matches!(result, Err(ScriptError::Timeout { .. })));
        // 超時仍應扣減實際耗時
        assert!(budget.remaining_ms() < 4.0);
    }

    #[test]
    fn test_frame_budget_deducted_on_ops_limit() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::new();

        // 超過 50,000 ops 的腳本
        let result =
            engine.execute_with_budget("let x = 0; while x < 100000 { x += 1; }", &mut budget);
        assert!(result.is_err());
        // ops limit 仍應扣減實際耗時
        assert!(budget.remaining_ms() < 4.0);
    }

    #[test]
    fn test_frame_budget_compile_error_deducts() {
        let engine = SandboxedEngine::new();
        let mut budget = FrameBudget::new();

        let result = engine.execute_with_budget("let = ;", &mut budget);
        assert!(matches!(result, Err(ScriptError::CompileError(_))));
        // 編譯失敗後 budget 可能略減（編譯耗時），至少不應增加
        assert!(budget.remaining_ms() <= 4.0);
    }
}
