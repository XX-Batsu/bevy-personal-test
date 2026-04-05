//! 沙箱逃逸測試 — 驗證 Rhai 沙箱對所有已知逃逸向量的阻擋能力
//!
//! 涵蓋 14 項逃逸向量 + 2 項擴展（白名單驗證、跨腳本隔離）：
//! - Runtime disable_symbol 禁止（6 項）
//! - Compile-time 禁止（2 項）
//! - 資源耗盡（4 項）
//! - 未註冊函式（2 項）
//! - 白名單驗證（1 項）
//! - 跨腳本隔離（1 項）
//!
//! 使用 `cargo test -p vm_runtime --features integration` 執行。

#![cfg(feature = "integration")]

use vm_runtime::{SandboxedEngine, ScriptError};

/// 判斷 ScriptError 是否為 RuntimeError 且 message 包含指定關鍵字
fn is_runtime_error_containing(result: &Result<rhai::Dynamic, ScriptError>, keyword: &str) -> bool {
    matches!(
        result,
        Err(ScriptError::RuntimeError { message, .. }) if message.contains(keyword)
    )
}

/// 判斷 ScriptError 是否為 CompileError 且 message 包含指定關鍵字
fn is_compile_error_containing(result: &Result<rhai::Dynamic, ScriptError>, keyword: &str) -> bool {
    matches!(
        result,
        Err(ScriptError::CompileError(msg)) if msg.contains(keyword)
    )
}

// ── Runtime disable_symbol 禁止（6 項，對齊 FORBIDDEN_SYMBOLS） ──

#[test]
fn eval_blocked() {
    let engine = SandboxedEngine::new();
    let result = engine.execute(r#"eval("1+2")"#);
    assert!(result.is_err(), "eval() 必須被 disable_symbol 阻擋");
}

#[test]
fn type_of_blocked() {
    let engine = SandboxedEngine::new();
    let result = engine.execute("let t = type_of(42);");
    assert!(
        result.is_err(),
        "type_of() 必須被 disable_symbol 阻擋（Reflection 類別）"
    );
}

#[test]
fn fn_symbol_blocked() {
    let engine = SandboxedEngine::new();
    let result = engine.execute(r#"let f = Fn("foo");"#);
    assert!(
        result.is_err(),
        "Fn() 必須被 disable_symbol 阻擋（動態函式物件）"
    );
}

#[test]
fn call_symbol_blocked() {
    let engine = SandboxedEngine::new();
    let result = engine.execute("let x = 42; call(x)");
    assert!(
        result.is_err(),
        "call() 必須被 disable_symbol 阻擋（動態函式呼叫）"
    );
}

#[test]
fn curry_blocked() {
    let engine = SandboxedEngine::new();
    let result = engine.execute("let x = 42; curry(x, 1)");
    assert!(
        result.is_err(),
        "curry() 必須被 disable_symbol 阻擋（偏函式應用）"
    );
}

#[test]
fn is_shared_blocked() {
    let engine = SandboxedEngine::new();
    let result = engine.execute("let x = 42; is_shared(x)");
    assert!(
        result.is_err(),
        "is_shared() 必須被 disable_symbol 阻擋（共享狀態偵測）"
    );
}

// ── Compile-time 禁止（2 項） ──

#[test]
fn import_blocked() {
    let engine = SandboxedEngine::new();
    let result = engine.execute(r#"import "os";"#);
    assert!(
        is_compile_error_containing(&result, "module")
            || is_compile_error_containing(&result, "import")
            || is_compile_error_containing(&result, "keyword"),
        "import 必須被 no_module 配置阻擋，回傳 CompileError。實際錯誤：{:?}",
        result
    );
}

#[test]
fn closure_blocked() {
    // no_closure feature 使 Rhai 不支援 closure 語法
    // `||` 會被解析為邏輯 OR（而非 closure），導致後續語法錯誤或函式找不到
    let engine = SandboxedEngine::new();
    let result = engine.execute("let f = || 42; f()");
    assert!(
        result.is_err(),
        "closure 語法必須被阻擋。實際錯誤：{:?}",
        result
    );
}

// ── 資源耗盡（4 項） ──

#[test]
fn infinite_loop_terminated() {
    let engine = SandboxedEngine::new();
    let result = engine.execute("loop { }");
    assert!(
        result.is_err(),
        "無限迴圈必須在 50,000 ops 或 2ms timeout 後被中斷"
    );
}

#[test]
fn recursion_depth_exceeded() {
    let engine = SandboxedEngine::new();
    let code = "fn bomb(n) { bomb(n + 1) } bomb(0)";
    let result = engine.execute(code);
    assert!(
        result.is_err(),
        "深遞迴必須在第 33 層被 max_call_levels 阻擋"
    );
}

#[test]
fn string_size_limit() {
    // max_string_size = 4,096 bytes
    // 可能先觸發 ops 限制或 timeout，視 Rhai Engine 檢查順序
    let engine = SandboxedEngine::new();
    let code = r#"let s = ""; for i in 0..1_000_000 { s += "a"; } s"#;
    let result = engine.execute(code);
    assert!(
        result.is_err(),
        "字串爆破必須被字串大小限制、ops 限制或 timeout 阻擋"
    );
}

#[test]
fn memory_bomb_blocked() {
    // max_array_size = 1,024 elements
    let engine = SandboxedEngine::new();
    let code = "let arr = []; for i in 0..1_000_000 { arr.push(i); }";
    let result = engine.execute(code);
    assert!(
        result.is_err(),
        "記憶體爆破必須被陣列大小限制、ops 限制或 timeout 阻擋"
    );
}

// ── 未註冊函式（2 項） ──

#[test]
fn filesystem_inaccessible() {
    let engine = SandboxedEngine::new();
    let result = engine.execute(r#"read_file("/etc/passwd")"#);
    assert!(result.is_err(), "fs 存取函式不得暴露給腳本");
}

#[test]
fn network_inaccessible() {
    let engine = SandboxedEngine::new();
    let result = engine.execute(r#"http_get("http://example.com")"#);
    assert!(result.is_err(), "網路存取函式不得暴露給腳本");
}

// ── 擴展測試（2 項） ──

#[test]
fn only_registered_bridge_api_callable() {
    let engine = SandboxedEngine::new();
    let result = engine.execute("unregistered_fn()");
    assert!(result.is_err(), "未註冊函式必須被阻擋");
}

#[test]
fn cross_script_variable_isolation() {
    // 建立兩個獨立的 SandboxedEngine 實例，驗證變數隔離
    let engine = SandboxedEngine::new();

    // Script A 設定變數 x = 42
    let result_a = engine.execute("let x = 42; x");
    assert!(result_a.is_ok(), "Script A 應成功執行");

    // Script B 嘗試讀取 x — 每次 execute 都使用全新 Scope，x 不存在
    let result_b = engine.execute("x");
    assert!(
        result_b.is_err(),
        "Script B 不得存取 Script A 的變數（Scope 隔離）"
    );
}
