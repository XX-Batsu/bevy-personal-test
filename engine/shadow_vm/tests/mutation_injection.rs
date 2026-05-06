//! Shadow VM Mutation Injection 測試
//!
//! 驗證 Shadow VM 能偵測各種作弊突變。
//! 使用 `InProcessShadowVm` 進行 5 種突變類型 + 3 種邊界測試。
//!
//! ## 突變類型
//! 1. RNG state 竄改
//! 2. PlayerInput 竄改（待 Phase 8/9 Bridge API 完善後啟用）
//! 3. ECS Mirror hash 竄改
//! 4. 中間狀態竄改（多幀重播）
//! 5. 記憶體區塊竄改（多 byte 修改）

mod common;

use bridge_types::{
    DeterministicValue, EntityId, PlayerInput, ShadowFrame, ShadowRequest, ShadowStatus,
};
use shadow_vm::{executor::ShadowScript, InProcessShadowVm};

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

fn make_shadow_frame(
    tick: u64,
    inputs: Vec<PlayerInput>,
    rng_state: [u8; 16],
    ecs_mirror_hash: [u8; 32],
) -> ShadowFrame {
    ShadowFrame {
        tick,
        inputs,
        rng_state,
        ecs_mirror_hash,
    }
}

/// 突變 1: RNG State 竄改
#[test]
fn mutation_rng_state_tamper_detected() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    let correct_hash = run_main_vm_and_get_hash(&script, &rng, 1);

    // 修改 rng_state 的第 0 個 byte，模擬 RNG state 遭到竄改
    let mut frame = make_shadow_frame(1, vec![], rng, correct_hash);
    frame.rng_state[0] ^= 0x01;

    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 1, .. }),
        "RNG 竄改應被偵測為 Mismatch，實際：{:?}",
        resp.status
    );
}

/// 突變 2: PlayerInput 竄改
///
/// **注意：** 目前 `extract_ecs_mirror_from_scope` 為 Phase 14 placeholder（回傳空 EcsMirror），
/// inputs 的變更不影響 state hash，因此此測試標為 `#[ignore]`。
/// 待 Phase 8/9 Bridge API 完善後啟用。
#[test]
#[ignore = "待 Phase 8/9 Bridge API 完善 extract_ecs_mirror_from_scope 後啟用"]
fn mutation_inputs_tamper_detected() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    let correct_hash = run_main_vm_and_get_hash(&script, &rng, 1);

    // 主 VM 使用空 inputs，Shadow 使用不同 input_type（模擬 Client 篡改輸入）
    let tampered_inputs = vec![PlayerInput {
        player_id: EntityId(0),
        input_type: 99, // 非零 input_type
        data: DeterministicValue::Int(0),
        tick: 1,
    }];
    let frame = make_shadow_frame(1, tampered_inputs, rng, correct_hash);

    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 1, .. }),
        "Inputs 竄改應被偵測為 Mismatch，實際：{:?}",
        resp.status
    );
}

/// 突變 3: ECS Mirror Hash 竄改
#[test]
fn mutation_ecs_mirror_hash_tamper_detected() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    // 使用假造的（錯誤的）hash 作為 ecs_mirror_hash，模擬 ECS Mirror 遭到篡改
    let tampered_hash = [0xDEu8; 32];

    let frame = make_shadow_frame(1, vec![], rng, tampered_hash);
    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 1, .. }),
        "EcsMirror hash 竄改應被偵測為 Mismatch，實際：{:?}",
        resp.status
    );
}

/// 突變 4: 中間狀態竄改（多幀重播）
///
/// tick 1 正確，tick 2 的 ecs_mirror_hash 被竄改。
/// 重播流程：
///   1. tick 1：Shadow VM 重播 → hash 正確 → AllMatch
///   2. tick 2：Shadow VM 重播 → hash 不符 → Mismatch { tick: 2 }
///      checked_ticks = [1, 2]（tick 1 通過 + tick 2 不匹配但已檢查）
#[test]
fn mutation_intermediate_state_tamper_detected() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    let hash_tick1 = run_main_vm_and_get_hash(&script, &rng, 1);

    // tick 1 正確，tick 2 的 ecs_mirror_hash 被竄改
    let frame1 = make_shadow_frame(1, vec![], rng, hash_tick1);
    let frame2_tampered = make_shadow_frame(2, vec![], rng, [0xABu8; 32]);

    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame1, frame2_tampered],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 2, .. }),
        "中間狀態竄改應在 tick 2 被偵測，實際：{:?}",
        resp.status
    );
    assert_eq!(
        resp.checked_ticks.len(),
        2,
        "tick 1 和 tick 2 都應被檢查，checked_ticks：{:?}",
        resp.checked_ticks
    );
}

/// 突變 5: 記憶體區塊竄改（多 Byte 修改）
#[test]
fn mutation_multi_byte_tamper_detected() {
    // 同時修改 rng_state 多個 byte，模擬記憶體區塊竄改
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    let correct_hash = run_main_vm_and_get_hash(&script, &rng, 1);

    let mut tampered_rng = rng;
    tampered_rng[0] = 0xFF;
    tampered_rng[8] = 0xFF; // 同時修改 state 和 increment 欄位

    let frame = make_shadow_frame(1, vec![], tampered_rng, correct_hash);
    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 1, .. }),
        "多 byte 記憶體竄改應被偵測，實際：{:?}",
        resp.status
    );
}

/// 邊界測試 A: 零差異突變（未修改的 frame → AllMatch）
#[test]
fn mutation_no_tamper_all_match() {
    // 驗證 baseline：未注入任何突變的 frame 應回傳 AllMatch
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    let correct_hash = run_main_vm_and_get_hash(&script, &rng, 1);
    let frame = make_shadow_frame(1, vec![], rng, correct_hash);

    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::AllMatch),
        "未修改的 frame 應回傳 AllMatch，實際：{:?}",
        resp.status
    );
    assert_eq!(resp.checked_ticks, vec![1]);
}

/// 邊界測試 B: 全零 ecs_mirror_hash
#[test]
fn mutation_all_zero_ecs_mirror_hash_detected() {
    // 驗證全零 hash 不會與任何正確 hash 碰撞（機率 2^-256 可忽略）
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    let all_zero_hash = [0u8; 32];

    let frame = make_shadow_frame(1, vec![], rng, all_zero_hash);
    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    // 除非主 VM 恰好計算出全零 hash（機率 2^-256），否則必為 Mismatch
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 1, .. }),
        "全零 ecs_mirror_hash 應被偵測為 Mismatch，實際：{:?}",
        resp.status
    );
}

/// 邊界測試 C: tick=0 邊界
#[test]
fn mutation_tick_zero_tamper_detected() {
    // 驗證 tick=0（首幀）的突變也能被正確偵測
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    let rng = [0u8; 16];
    let correct_hash = run_main_vm_and_get_hash(&script, &rng, 0);

    // 竄改 rng_state
    let mut frame = make_shadow_frame(0, vec![], rng, correct_hash);
    frame.rng_state[0] ^= 0x01;

    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 0, .. }),
        "tick=0 的 RNG 竄改應被偵測為 Mismatch，實際：{:?}",
        resp.status
    );
}
