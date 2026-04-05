//! WASM memory page 追蹤記憶體洩漏偵測
//!
//! warmup 100 frames → 測量基準線 → 1,000 frames → 驗證頁數穩定（≤ 1 page 增長）。
//! WASM 記憶體頁面為單調遞增（規範限制），故只能偵測「過多增長」。
//!
//! 執行方式：
//! ```bash
//! wasm-pack test --chrome --headless -p memory-leak-tests
//! ```

#![cfg(target_arch = "wasm32")]

use wasm_bindgen::JsCast;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// 測量目前 WASM 記憶體頁數（使用 memory().grow(0) 不實際成長）
///
/// 與 08-property-stress/03-memory-leak-detection.md 的
/// `measure_wasm_memory_pages` 介面一致。
/// 1 page = 64 KB。
pub fn measure_wasm_memory_pages() -> u32 {
    let memory = wasm_bindgen::memory()
        .dyn_into::<js_sys::WebAssembly::Memory>()
        .unwrap();
    memory.grow(0)
}

/// 模擬一個完整 game frame（WASM 版本）
///
/// WASM 限制：不可使用 std::thread、std::time::Instant。
/// 只能同步執行（README.md WASM Constraints）。
fn simulate_one_frame() {
    // Placeholder：模擬 frame 分配模式（alloc + dealloc 平衡）
    use std::collections::BTreeMap;
    let mut mock_state: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for i in 0..10 {
        mock_state.insert(i, vec![0u8; 256]);
    }
    // frame 結束，mock_state drop
    // TODO(Phase 9): 替換為正式 GameLoop::tick() 呼叫
}

/// 驗證 measure_wasm_memory_pages 基本功能
#[wasm_bindgen_test]
fn test_measure_wasm_memory_pages_returns_positive() {
    let pages = measure_wasm_memory_pages();
    assert!(pages > 0, "WASM memory pages 應至少為 1，實際值：{pages}");
}

/// WASM 記憶體穩定性測試：
/// warmup 100 frames → 測量基準線 → 執行 1000 frames → 驗證頁數穩定
///
/// 閾值：≤ 1 頁（64 KB），對齊 Phase 20 README 定義。
#[wasm_bindgen_test]
fn test_wasm_memory_stability() {
    // Warmup：讓 allocator 達到穩定狀態
    for _ in 0..100 {
        simulate_one_frame();
    }
    let baseline_pages = measure_wasm_memory_pages();

    // Run：執行 1000 frames
    for _ in 0..1_000 {
        simulate_one_frame();
    }
    let final_pages = measure_wasm_memory_pages();

    // WASM 記憶體頁數不應持續成長（容許 1 頁 = 64 KB 波動）
    assert!(
        final_pages <= baseline_pages + 1,
        "WASM 記憶體洩漏：{baseline_pages} → {final_pages} pages\n漂移：{} KB",
        (final_pages as i64 - baseline_pages as i64) * 64
    );
}

/// 驗證 warmup 後 page count 不低於初始值（單調遞增性質）
#[wasm_bindgen_test]
fn test_warmup_monotonic_page_growth() {
    let before_warmup = measure_wasm_memory_pages();
    for _ in 0..100 {
        simulate_one_frame();
    }
    let after_warmup = measure_wasm_memory_pages();
    assert!(
        after_warmup >= before_warmup,
        "WASM page count 違反單調遞增：warmup 前 {before_warmup} → 後 {after_warmup}"
    );
}
