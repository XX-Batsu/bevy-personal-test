//! 整合測試：TraceCollector → ShadowRequest → InProcessShadowVm.validate_sync

mod common;

use bridge_types::{ShadowFrame, ShadowStatus};
use shadow_vm::{executor::ShadowScript, InProcessShadowVm, TraceCollector};

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

/// 整合測試：TraceCollector 收集 4 幀 → ShadowRequest → InProcessShadowVm.validate_sync
#[test]
fn trace_collector_to_in_process_vm_integration() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let mut collector = TraceCollector::new(4);

    // 模擬收集 4 幀
    for tick in 0..4u64 {
        let rng = [0u8; 16];
        let hash = run_main_vm_and_get_hash(&script, &rng, tick);
        collector.record(ShadowFrame {
            tick,
            inputs: vec![],
            rng_state: rng,
            ecs_mirror_hash: hash,
        });
    }

    // 組裝並驗證
    let request = collector.take_request().expect("應有 4 幀資料");
    let resp = vm.validate_sync(&request).unwrap();
    assert!(matches!(resp.status, ShadowStatus::AllMatch));
    assert_eq!(resp.checked_ticks.len(), 4);

    // 清空後重置
    collector.clear();
    assert!(collector.take_request().is_none());
}

/// 整合測試：TraceCollector 收集不足 4 幀 → None（不觸發驗證）
#[test]
fn trace_collector_insufficient_frames_no_validation() {
    let script = compile_test_script();
    let mut collector = TraceCollector::new(4);

    for tick in 0..3u64 {
        let rng = [0u8; 16];
        let hash = run_main_vm_and_get_hash(&script, &rng, tick);
        collector.record(ShadowFrame {
            tick,
            inputs: vec![],
            rng_state: rng,
            ecs_mirror_hash: hash,
        });
    }

    assert!(collector.take_request().is_none(), "不足 4 幀不應觸發驗證");
}

/// 整合測試：TraceCollector 滾動視窗 + InProcessShadowVm 驗證 Mismatch
#[test]
fn trace_collector_rolling_with_mismatch() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let mut collector = TraceCollector::new(4);

    // 收集 6 幀（滾動後保留最新 4 幀：tick 2, 3, 4, 5）
    for tick in 0..6u64 {
        let rng = [0u8; 16];
        let hash = if tick == 4 {
            [0xFFu8; 32] // tick 4 故意錯誤的 hash
        } else {
            run_main_vm_and_get_hash(&script, &rng, tick)
        };
        collector.record(ShadowFrame {
            tick,
            inputs: vec![],
            rng_state: rng,
            ecs_mirror_hash: hash,
        });
    }

    let request = collector.take_request().expect("應有 4 幀資料（tick 2-5）");
    // 只保留 tick 2, 3, 4, 5，其中 tick 4 hash 錯誤
    assert_eq!(request.frames[0].tick, 2);
    assert_eq!(request.frames[3].tick, 5);

    let resp = vm.validate_sync(&request).unwrap();
    // tick 2, 3 通過，tick 4 不匹配
    match resp.status {
        ShadowStatus::Mismatch { tick, .. } => {
            assert_eq!(tick, 4, "應在 tick 4 發生 Mismatch");
        }
        other => panic!("預期 Mismatch，實際 {other:?}"),
    }
}
