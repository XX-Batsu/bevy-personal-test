//! jemalloc RSS 追蹤記憶體洩漏偵測（Linux only）
//!
//! 10,000 frames 後 RSS 漂移 < 1 MB。
//! 使用 nightly-perf feature 啟用，以 `--ignored` 執行。
//!
//! 執行方式：
//! ```bash
//! cargo test -p memory-leak-tests --features nightly-perf --test native_leak_test -- --ignored
//! ```

#![cfg(target_os = "linux")]

#[cfg(feature = "nightly-perf")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// 模擬一個完整 game frame
///
/// ## Placeholder 策略
/// Phase 9 GameLoop::tick() 完成後，替換為實際呼叫：
/// 1. `GameLoop::tick()` — engine/vm_bevy_bridge/src/
/// 2. `ScriptManager::tick_all()` — engine/vm_runtime/src/
/// 3. `update_ecs_mirror()` — engine/vm_bevy_bridge/src/
fn simulate_one_frame() {
    use std::collections::BTreeMap;
    let mut mock_state: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for i in 0..10 {
        mock_state.insert(i, vec![0u8; 256]);
    }
    // frame 結束，mock_state drop — 確保無洩漏
    // TODO: Phase 9 完成後替換為 GameLoop::tick() 呼叫
}

/// 記憶體穩定性測試：10,000 frames 後 RSS 漂移 < 1 MB
#[cfg(feature = "nightly-perf")]
#[test]
#[ignore]
fn test_10k_frames_memory_stability() {
    // 暖機（讓 allocator 穩定，避免初始化分配影響量測）
    for _ in 0..100 {
        simulate_one_frame();
    }

    // 刷新 jemalloc 統計
    let epoch_mib = tikv_jemalloc_ctl::epoch::mib().expect("無法取得 epoch MIB");
    epoch_mib.advance().expect("無法推進 jemalloc epoch");

    // 記錄初始 RSS（bytes）
    let initial_rss =
        tikv_jemalloc_ctl::stats::resident::read().expect("無法讀取 jemalloc RSS 統計");

    // 模擬 10,000 frames
    for _ in 0..10_000 {
        simulate_one_frame();
    }

    // 刷新 jemalloc 統計（需先更新 epoch）
    epoch_mib.advance().expect("無法推進 jemalloc epoch");

    let final_rss = tikv_jemalloc_ctl::stats::resident::read().expect("無法讀取 jemalloc RSS 統計");

    let drift_bytes = final_rss as i64 - initial_rss as i64;
    let drift_mb = drift_bytes as f64 / (1024.0 * 1024.0);

    // RSS 漂移 < 1 MB（允許負值：jemalloc 可能歸還記憶體給 OS）
    assert!(
        drift_mb < 1.0,
        "記憶體洩漏偵測：{drift_mb:.2} MB drift（上限 1 MB）\n\
         初始 RSS：{initial_rss} bytes，最終 RSS：{final_rss} bytes"
    );
}
