//! Shadow VM 效能目標驗證測試（CI assert）
//!
//! 合規說明：
//! - `std::time::Instant`：用於 benchmark/test 情境（infrastructure timing），
//!   非 game state computation，符合 README.md Determinism Rules。
//! - `f64`：用於平均值計算（benchmark 量測），非 game logic，符合 README.md 禁止項。
#![cfg(not(target_arch = "wasm32"))]

mod common;

use bridge_types::{ShadowFrame, ShadowRequest};
use shadow_vm::{executor::ShadowScript, InProcessShadowVm};
use std::time::Instant;

fn compile_test_script() -> ShadowScript {
    ShadowScript(
        r#"
        fn on_tick() {
            let x = input_type + input_data_int;
            x
        }
    "#
        .to_string(),
    )
}

fn run_main_vm_and_get_hash(script: &ShadowScript, rng_state: &[u8; 16], tick: u64) -> [u8; 32] {
    common::run_main_vm_and_get_hash(script, rng_state, tick)
}

fn make_test_frames(script: &ShadowScript, count: usize) -> Vec<ShadowFrame> {
    (0..count as u64)
        .map(|tick| {
            let rng = [0u8; 16];
            let hash = run_main_vm_and_get_hash(script, &rng, tick);
            ShadowFrame {
                tick,
                inputs: vec![],
                rng_state: rng,
                ecs_mirror_hash: hash,
            }
        })
        .collect()
}

/// CI assert：4 幀重播平均 < 20ms
#[test]
fn benchmark_4_frame_replay_under_20ms() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let frames = make_test_frames(&script, 4);

    // 預熱 10 次，消除 JIT/快取冷啟動效應
    for _ in 0..10 {
        vm.validate_sync(&ShadowRequest {
            frames: frames.clone(),
        })
        .unwrap();
    }

    // 量測 100 次取平均
    let start = Instant::now();
    for _ in 0..100 {
        vm.validate_sync(&ShadowRequest {
            frames: frames.clone(),
        })
        .unwrap();
    }
    let avg_ms: f64 = start.elapsed().as_millis() as f64 / 100.0;

    assert!(avg_ms < 20.0, "4 幀重播平均 {avg_ms:.1}ms 超過 20ms 預算");
}

/// CI assert：單幀重播平均 < 5ms
#[test]
fn benchmark_single_frame_replay_under_5ms() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let frames = make_test_frames(&script, 1);

    // 預熱 10 次
    for _ in 0..10 {
        vm.validate_sync(&ShadowRequest {
            frames: frames.clone(),
        })
        .unwrap();
    }

    // 量測 100 次取平均
    let start = Instant::now();
    for _ in 0..100 {
        vm.validate_sync(&ShadowRequest {
            frames: frames.clone(),
        })
        .unwrap();
    }
    let avg_ms: f64 = start.elapsed().as_millis() as f64 / 100.0;

    assert!(avg_ms < 5.0, "單幀重播平均 {avg_ms:.1}ms 超過 5ms 預算");
}

/// 記憶體使用量測試（手動執行，標記 #[ignore]）
///
/// 執行方式：
/// - macOS: `/usr/bin/time -l cargo test -p shadow_vm -- memory_usage_under_32mb --ignored`
/// - Linux: `/usr/bin/time -v cargo test -p shadow_vm -- memory_usage_under_32mb --ignored`
///   觀察 "maximum resident set size" 是否 < 32 MB
#[test]
#[ignore = "手動執行：需要 /usr/bin/time 或 valgrind 量測 RSS"]
fn memory_usage_under_32mb() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    // 執行 1000 次 4 幀重播，讓記憶體使用量達到穩態
    for _ in 0..1000 {
        let frames = make_test_frames(&script, 4);
        vm.validate_sync(&ShadowRequest { frames }).unwrap();
    }
    // RSS 由外部工具量測，此處確保程式正常執行不 panic
}
