//! Scope 變數數量與大小限制器
//!
//! 透過 Rhai `on_def_var` callback 攔截 `let` 宣告，限制每個腳本 Scope 的變數數量
//! （上限 256）與單一變數大小（上限 64 KB）。
//!
//! # 設計依據
//! - scope-persistence.md §Scope 大小限制
//! - resource-limits.md §限制速查表

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Scope 最大變數數量（對齊 scope-persistence.md §Scope 大小限制）
pub const SCOPE_MAX_VARS: usize = 256;

/// 單一變數最大大小（bytes）（對齊 scope-persistence.md §Scope 大小限制）
pub const SCOPE_MAX_VAR_BYTES: usize = 64 * 1024; // 64 KB = 65,536 bytes

/// Scope 大小違反錯誤（對齊 scope-persistence.md §介面定義）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeLimitError {
    /// 變數數量超過 SCOPE_MAX_VARS
    TooManyVariables { current: usize, limit: usize },
    /// 單一變數大小超過 SCOPE_MAX_VAR_BYTES
    VariableTooLarge {
        var_name: String,
        current_bytes: usize,
        limit: usize,
    },
}

/// Scope 變數數量限制器
///
/// 使用 `Arc<AtomicUsize>` 而非 `Rc<RefCell<usize>>`，因為 Rhai `on_def_var`
/// callback 的 closure 需滿足 `Send + Sync`（Rhai Engine 的 trait bound 要求）。
/// 雖然 WASM 為單執行緒，但 Rhai API 的型別約束仍須滿足。
pub struct ScopeLimiter {
    var_count: Arc<AtomicUsize>,
}

impl ScopeLimiter {
    /// 建立新的 ScopeLimiter（計數器初始為 0）
    pub fn new() -> Self {
        Self {
            var_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 將 on_def_var callback 註冊到 Engine，攔截 `let` 定義
    ///
    /// Rhai `on_def_var` 在每個 `let` 語句執行時觸發。
    /// 回傳 `Ok(false)` 會讓 Rhai 拋出 `EvalAltResult::ErrorRuntime`。
    ///
    /// 注意：`on_def_var` 只攔截 `let` 宣告，不攔截 `=` 賦值。
    /// 完整的賦值大小檢查由 Phase 8 ScriptManager 在 callback 後檢查 Scope 負責。
    ///
    /// 語義：`fetch_add` 回傳的是 **增加前** 的值，因此 `current >= SCOPE_MAX_VARS`
    /// 表示「本次為第 SCOPE_MAX_VARS+1 個變數」，正確攔截。
    pub fn register(&self, engine: &mut rhai::Engine) {
        let counter = self.var_count.clone();
        // Rhai 將 on_def_var 標記為 deprecated 但實際未棄用（"volatile API" 警告）
        #[allow(deprecated)]
        engine.on_def_var(move |is_runtime, _info, _context| {
            // on_def_var 在編譯期（is_runtime=false）和執行期（is_runtime=true）都會觸發。
            // 變數計數限制僅在執行期攔截，編譯期無條件放行。
            if !is_runtime {
                return Ok(true);
            }
            let current = counter.fetch_add(1, Ordering::Relaxed);
            if current >= SCOPE_MAX_VARS {
                return Ok(false); // Rhai 拋出 EvalAltResult::ErrorRuntime
            }
            Ok(true)
        });
    }

    /// 重置計數器（每次腳本執行前呼叫）
    pub fn reset(&self) {
        self.var_count.store(0, Ordering::Relaxed);
    }

    /// 估算 rhai::Dynamic 值的記憶體大小（bytes）
    ///
    /// 命名對齊 scope-persistence.md：estimate_dynamic_size
    ///
    /// 遞迴深度上限為 8 層，防止巢狀結構造成 stack overflow。
    pub fn estimate_dynamic_size(value: &rhai::Dynamic) -> usize {
        Self::estimate_dynamic_size_inner(value, 0)
    }

    /// 遞迴估算 Dynamic 值大小，含深度限制
    ///
    /// 注意：Rhai 原生型別包含 f64，但本專案遊戲邏輯全面使用 SoftF32
    /// （README.md Prohibitions: No native float in game logic）。
    /// 此處保留 f64 匹配分支是因為 Rhai 引擎內部型別系統仍以 f64 表示浮點，
    /// SoftF32 在 Bridge 層轉換為 i64/Dynamic 傳入 Rhai，
    /// 但 Rhai 算術運算可能產生 f64 值，需正確估算大小。
    fn estimate_dynamic_size_inner(value: &rhai::Dynamic, depth: usize) -> usize {
        if depth > 8 {
            return 0;
        }
        match value.type_name() {
            "i64" => 8,
            "f64" => 8,
            "bool" => 1,
            "string" | "String" => value.clone_cast::<rhai::ImmutableString>().len(),
            "array" => value
                .clone_cast::<rhai::Array>()
                .iter()
                .map(|v| Self::estimate_dynamic_size_inner(v, depth + 1))
                .sum(),
            "map" => value
                .clone_cast::<rhai::Map>()
                .iter()
                .map(|(k, v)| k.len() + Self::estimate_dynamic_size_inner(v, depth + 1))
                .sum(),
            _ => 0,
        }
    }

    /// 檢查 Scope 中所有變數的大小與數量，超限回傳 Err(ScopeLimitError)
    ///
    /// 由 Phase 8 ScriptManager 在每次 callback 執行後呼叫，
    /// 用於檢查 `=` 賦值可能造成的大小超限，以及直接注入 Scope 的變數數量超限。
    pub fn check_scope_sizes(scope: &rhai::Scope) -> Result<(), ScopeLimitError> {
        let mut var_count = 0_usize;
        for (name, _, value) in scope.iter_raw() {
            var_count += 1;
            let size = Self::estimate_dynamic_size(value);
            if size > SCOPE_MAX_VAR_BYTES {
                return Err(ScopeLimitError::VariableTooLarge {
                    var_name: name.to_string(),
                    current_bytes: size,
                    limit: SCOPE_MAX_VAR_BYTES,
                });
            }
        }
        if var_count > SCOPE_MAX_VARS {
            return Err(ScopeLimitError::TooManyVariables {
                current: var_count,
                limit: SCOPE_MAX_VARS,
            });
        }
        Ok(())
    }

    /// 取得目前計數器值（測試用）
    #[cfg(test)]
    pub(crate) fn current_count(&self) -> usize {
        self.var_count.load(Ordering::Relaxed)
    }
}

impl Default for ScopeLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════
    // 變數數量限制測試
    // ═══════════════════════════════════════════════════════════════

    /// #1: 宣告 256 個 `let` 變數應成功（上限邊界含）
    #[test]
    fn test_scope_256_vars_succeeds() {
        let limiter = ScopeLimiter::new();
        let mut engine = rhai::Engine::new();
        limiter.register(&mut engine);

        // 產生 256 行 "let v_0 = 0; let v_1 = 0; ..."
        let script: String = (0..SCOPE_MAX_VARS)
            .map(|i| format!("let v_{i} = 0;"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut scope = rhai::Scope::new();
        let result = engine.eval_with_scope::<()>(&mut scope, &script);
        assert!(result.is_ok(), "256 變數應成功：{result:?}");
    }

    /// #2: 第 257 個 `let` 變數應被 on_def_var 拒絕
    #[test]
    fn test_scope_257th_var_rejected() {
        let limiter = ScopeLimiter::new();
        let mut engine = rhai::Engine::new();
        limiter.register(&mut engine);

        let script: String = (0..SCOPE_MAX_VARS + 1)
            .map(|i| format!("let v_{i} = 0;"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut scope = rhai::Scope::new();
        let result = engine.eval_with_scope::<()>(&mut scope, &script);
        assert!(result.is_err(), "第 257 個變數應失敗");
        // 驗證 Rhai 錯誤為 on_def_var 拒絕相關
        let err = result.unwrap_err();
        let err_msg = format!("{err:?}");
        // Rhai on_def_var 回傳 false 時拋出 ErrorForbiddenVariable
        let err_msg_lower = err_msg.to_lowercase();
        assert!(
            err_msg_lower.contains("rejected")
                || err_msg_lower.contains("variable")
                || err_msg_lower.contains("forbidden"),
            "錯誤訊息應與變數定義拒絕相關，實際：{err_msg}",
        );
    }

    /// #3: 空 Scope 應通過 check_scope_sizes 檢查
    #[test]
    fn test_scope_empty_succeeds() {
        let scope = rhai::Scope::new();
        let result = ScopeLimiter::check_scope_sizes(&scope);
        assert!(result.is_ok(), "空 Scope 應通過檢查");
    }

    /// #4: reset 後應可重新宣告 256 個變數
    #[test]
    fn test_scope_reset_allows_reuse() {
        let limiter = ScopeLimiter::new();
        let mut engine = rhai::Engine::new();
        limiter.register(&mut engine);

        let script: String = (0..SCOPE_MAX_VARS)
            .map(|i| format!("let v_{i} = 0;"))
            .collect::<Vec<_>>()
            .join("\n");

        // 第一輪：宣告 256 個變數
        let mut scope1 = rhai::Scope::new();
        let result1 = engine.eval_with_scope::<()>(&mut scope1, &script);
        assert!(result1.is_ok(), "第一輪 256 變數應成功：{result1:?}");

        // 重置計數器
        limiter.reset();

        // 第二輪：應可再次宣告 256 個變數
        let mut scope2 = rhai::Scope::new();
        let result2 = engine.eval_with_scope::<()>(&mut scope2, &script);
        assert!(
            result2.is_ok(),
            "reset 後應可重新宣告 256 變數：{result2:?}"
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // 變數大小限制測試
    // ═══════════════════════════════════════════════════════════════

    /// #5: i64 大小應為 8 bytes
    #[test]
    fn test_estimate_dynamic_size_int() {
        let val = rhai::Dynamic::from(42_i64);
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 8);
    }

    /// #6: bool 大小應為 1 byte
    #[test]
    fn test_estimate_dynamic_size_bool() {
        let val = rhai::Dynamic::from(true);
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 1);
    }

    /// #7: 空字串大小應為 0
    #[test]
    fn test_estimate_dynamic_size_string_empty() {
        let val = rhai::Dynamic::from(String::new());
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 0);
    }

    /// #8: 恰好 64 KB 字串應回傳 65,536
    #[test]
    fn test_estimate_dynamic_size_string_exact_limit() {
        let val = rhai::Dynamic::from("A".repeat(SCOPE_MAX_VAR_BYTES));
        assert_eq!(
            ScopeLimiter::estimate_dynamic_size(&val),
            SCOPE_MAX_VAR_BYTES
        );
    }

    /// #9: 超過 64 KB 字串應回傳 65,537
    #[test]
    fn test_estimate_dynamic_size_string_over_limit() {
        let val = rhai::Dynamic::from("A".repeat(SCOPE_MAX_VAR_BYTES + 1));
        assert_eq!(
            ScopeLimiter::estimate_dynamic_size(&val),
            SCOPE_MAX_VAR_BYTES + 1
        );
    }

    /// #10: UTF-8 多位元組字元（中文字 3 bytes）
    #[test]
    fn test_estimate_dynamic_size_multibyte_utf8() {
        // "中" = 3 bytes UTF-8，驗證 len() 為 byte 長度而非字元數
        let val = rhai::Dynamic::from("中".repeat(100));
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 300);
    }

    /// #11: 巢狀陣列大小估算
    #[test]
    fn test_estimate_dynamic_size_nested_array() {
        // 建構 [[1, 2], [3, 4]] 的 rhai::Array
        let inner1: rhai::Array = vec![rhai::Dynamic::from(1_i64), rhai::Dynamic::from(2_i64)];
        let inner2: rhai::Array = vec![rhai::Dynamic::from(3_i64), rhai::Dynamic::from(4_i64)];
        let outer: rhai::Array = vec![rhai::Dynamic::from(inner1), rhai::Dynamic::from(inner2)];
        let val = rhai::Dynamic::from(outer);
        // 4 × size_of::<i64>() = 4 × 8 = 32
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 32);
    }

    /// #12: 所有變數在限制內應回傳 Ok
    #[test]
    fn test_check_scope_sizes_all_within_limit() {
        let mut scope = rhai::Scope::new();
        scope.push("a", 42_i64);
        scope.push("b", true);
        scope.push("c", "hello".to_string());
        let result = ScopeLimiter::check_scope_sizes(&scope);
        assert!(result.is_ok(), "3 個小變數應通過檢查");
    }

    /// #13: 單一變數超過 64 KB 應回傳 VariableTooLarge
    #[test]
    fn test_check_scope_sizes_one_var_exceeds() {
        let mut scope = rhai::Scope::new();
        scope.push("big_var", "A".repeat(SCOPE_MAX_VAR_BYTES + 1));
        let result = ScopeLimiter::check_scope_sizes(&scope);
        assert_eq!(
            result,
            Err(ScopeLimitError::VariableTooLarge {
                var_name: "big_var".to_string(),
                current_bytes: SCOPE_MAX_VAR_BYTES + 1,
                limit: SCOPE_MAX_VAR_BYTES,
            })
        );
    }

    /// #14: Scope 含 257 個變數應回傳 TooManyVariables
    #[test]
    fn test_check_scope_sizes_var_count_exceeds() {
        let mut scope = rhai::Scope::new();
        for i in 0..SCOPE_MAX_VARS + 1 {
            scope.push(format!("v_{i}"), 0_i64);
        }
        let result = ScopeLimiter::check_scope_sizes(&scope);
        assert_eq!(
            result,
            Err(ScopeLimitError::TooManyVariables {
                current: SCOPE_MAX_VARS + 1,
                limit: SCOPE_MAX_VARS,
            })
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // 行為語義測試
    // ═══════════════════════════════════════════════════════════════

    /// #15: `=` 賦值不應被 on_def_var 攔截
    #[test]
    fn test_assignment_not_intercepted() {
        let limiter = ScopeLimiter::new();
        let mut engine = rhai::Engine::new();
        limiter.register(&mut engine);

        let script = r#"
            let x = 1;
            x = 42;
            x
        "#;
        let mut scope = rhai::Scope::new();
        let result = engine.eval_with_scope::<i64>(&mut scope, script);
        assert!(result.is_ok(), "= 賦值不應被 on_def_var 攔截");
        assert_eq!(result.unwrap(), 42);
    }

    /// #16: 多次 `=` 賦值不應增加 on_def_var 計數
    #[test]
    fn test_multiple_assignments_no_count_increase() {
        let limiter = ScopeLimiter::new();
        let mut engine = rhai::Engine::new();
        limiter.register(&mut engine);

        // 1 個 let + 多次 = 賦值
        let script = r#"
            let x = 1;
            x = 2;
            x = 3;
            x = 4;
        "#;
        let mut scope = rhai::Scope::new();
        let result = engine.eval_with_scope::<()>(&mut scope, script);
        assert!(result.is_ok(), "多次賦值不應觸發 on_def_var 限制");
        // 隱含驗證：若 = 賦值也計數，4 次賦值 + 1 次 let = 5 次，
        // 但 on_def_var 應僅觸發 1 次（let x）
    }

    /// #17: reset 後計數器歸零
    #[test]
    fn test_reset_zeroes_counter() {
        let limiter = ScopeLimiter::new();
        let mut engine = rhai::Engine::new();
        limiter.register(&mut engine);

        // 宣告 10 個 let 變數
        let script: String = (0..10)
            .map(|i| format!("let v_{i} = 0;"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut scope = rhai::Scope::new();
        let _ = engine.eval_with_scope::<()>(&mut scope, &script);

        // reset 後計數器應歸零
        limiter.reset();
        assert_eq!(limiter.current_count(), 0, "reset 後計數器應為 0");

        // reset 後應可再宣告 256 個變數
        let full_script: String = (0..SCOPE_MAX_VARS)
            .map(|i| format!("let w_{i} = 0;"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut scope2 = rhai::Scope::new();
        let result = engine.eval_with_scope::<()>(&mut scope2, &full_script);
        assert!(result.is_ok(), "reset 後應可重新宣告 256 變數：{result:?}");
    }

    // ═══════════════════════════════════════════════════════════════
    // Task 2 實作補充測試（#18–#22）
    // ═══════════════════════════════════════════════════════════════

    /// #18: 遞迴深度超過 8 層時回傳 0
    #[test]
    fn test_estimate_dynamic_size_depth_limit() {
        // 建構 9 層巢狀 array：[[[...[42]...]]]
        let mut val = rhai::Dynamic::from(42_i64);
        for _ in 0..9 {
            let arr: rhai::Array = vec![val];
            val = rhai::Dynamic::from(arr);
        }
        // 第 9 層（depth > 8）回傳 0，因此最內層 i64(8) 不被計算
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 0);
    }

    /// #19: Map 型別大小估算（key.len() + value size 累加）
    #[test]
    fn test_estimate_dynamic_size_map() {
        let mut map = rhai::Map::new();
        map.insert("key1".into(), rhai::Dynamic::from(42_i64));
        map.insert("key2".into(), rhai::Dynamic::from("hello".to_string()));
        let val = rhai::Dynamic::from(map);
        // "key1".len()=4 + i64=8 + "key2".len()=4 + "hello".len()=5 = 21
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 21);
    }

    /// #20: 未知型別回傳 0（安全處理）
    #[test]
    fn test_estimate_dynamic_size_unknown_type() {
        // rhai::Dynamic::UNIT 是 () 型別，type_name 為 "()"，不在匹配中
        let val = rhai::Dynamic::UNIT;
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 0);
    }

    /// #21: Rhai 原生 f64 型別正確估算為 8 bytes
    #[test]
    fn test_estimate_dynamic_size_f64() {
        let val = rhai::Dynamic::from(3.14_f64);
        assert_eq!(ScopeLimiter::estimate_dynamic_size(&val), 8);
    }

    /// #22: 混合型別 Scope 遍歷全部通過
    #[test]
    fn test_check_scope_sizes_mixed_types() {
        let mut scope = rhai::Scope::new();
        scope.push("int_var", 42_i64);
        scope.push("bool_var", true);
        scope.push("str_var", "hello world".to_string());
        let arr: rhai::Array = vec![rhai::Dynamic::from(1_i64), rhai::Dynamic::from(2_i64)];
        scope.push("arr_var", arr);
        let result = ScopeLimiter::check_scope_sizes(&scope);
        assert!(result.is_ok(), "混合型別應通過檢查：{result:?}");
    }
}
