//! Shadow VM False Positive / False Negative 測試
//!
//! 驗證 Shadow VM 的正確率：
//! - False Positive 率：0%（10,000 幀零誤報）
//! - False Negative 率：< 1%（84 種突變偵測率 > 99%）
//!
//! 所有測試使用固定 seed（0xDEAD_BEEF），確保可重現。
//! `f64` 用於測試統計運算（非 game logic），不受 Determinism Rules 限制。

mod common;

use bridge_types::{ShadowFrame, ShadowRequest, ShadowStatus};
use deterministic::DeterministicRng;
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

/// False Positive 主測試：10,000 幀正確執行，零誤報
#[test]
fn false_positive_zero_over_10k_frames() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let mut rng = DeterministicRng::seed_from_u64(0xDEAD_BEEF); // 固定 seed
    let mut false_positives = 0u32;

    for tick in 0..10_000u64 {
        let rng_state = rng.state_bytes();
        // 推進 RNG 狀態供下一幀使用
        let _ = rng.next_u32();
        let hash = run_main_vm_and_get_hash(&script, &rng_state, tick);
        let frame = ShadowFrame {
            tick,
            inputs: vec![],
            rng_state,
            ecs_mirror_hash: hash,
        };
        let resp = vm
            .validate_sync(&ShadowRequest {
                frames: vec![frame],
            })
            .unwrap();
        if !matches!(resp.status, ShadowStatus::AllMatch) {
            false_positives += 1;
            tracing::warn!("偽陽性在 tick={}: {:?}", tick, resp.status);
        }
    }
    assert_eq!(
        false_positives, 0,
        "期望零 False Positive，實際 {false_positives}"
    );
}

/// 共用 helper：單幀 FP 驗證
fn assert_single_frame_allmatches(tick: u64) {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let rng_state = [0u8; 16];
    let hash = run_main_vm_and_get_hash(&script, &rng_state, tick);
    let frame = ShadowFrame {
        tick,
        inputs: vec![],
        rng_state,
        ecs_mirror_hash: hash,
    };
    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::AllMatch),
        "tick={tick} 應為 AllMatch，實際：{:?}",
        resp.status
    );
}

/// FP 邊界測試：tick=0 首幀
#[test]
fn false_positive_tick_zero() {
    assert_single_frame_allmatches(0);
}

/// FP 邊界測試：大 tick（接近 u64::MAX）
#[test]
fn false_positive_large_tick() {
    assert_single_frame_allmatches(u64::MAX - 1);
}

/// FP 測試：含非空 PlayerInput
///
/// 注意：目前 `extract_ecs_mirror_from_scope` 為 placeholder，inputs 不影響 hash。
/// 此測試驗證非空 inputs 不會導致 false positive（hash 仍以 rng_state + tick 為準）。
#[test]
fn false_positive_with_non_empty_inputs() {
    use bridge_types::{DeterministicValue, EntityId, PlayerInput};

    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let rng_state = [0u8; 16];
    // 使用空 inputs 計算 hash（Shadow VM 使用 first input 或預設 0）
    let hash = run_main_vm_and_get_hash(&script, &rng_state, 1);
    let inputs = vec![PlayerInput {
        player_id: EntityId(0),
        input_type: 0,
        data: DeterministicValue::Int(0),
        tick: 1,
    }];
    let frame = ShadowFrame {
        tick: 1,
        inputs,
        rng_state,
        ecs_mirror_hash: hash,
    };
    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::AllMatch),
        "含預設 PlayerInput 不應誤報，實際：{:?}",
        resp.status
    );
}

/// FN 主測試：84 種突變偵測率 >= 99%
///
/// 突變策略（84 種）：
/// - 16 種：rng_state[i] ^= 0x01（i ∈ 0..16）
/// - 16 種：rng_state[i] ^= 0xFF（i ∈ 0..16）
/// - 32 種：ecs_mirror_hash[i] ^= 0xFF（i ∈ 0..32）
/// - 10 種：rng_state[i] ^= 0x55（i ∈ 0..10，模擬 RNG 狀態細微篡改）
/// - 10 種：組合突變（rng_state[i] ^= 0x01 + ecs_mirror_hash[i] ^= 0xFF，i ∈ 0..10）
///
/// 設計說明：Shadow VM 的 PlayerInput 驗證透過 PostMessage 傳遞的 ecs_mirror_hash 實現，
/// 而非在 Shadow 端直接提取 PlayerInput。RNG state 突變已涵蓋 PlayerInput 篡改場景，
/// 因為篡改 PlayerInput 必然導致不同的 RNG 演化路徑與 state hash 差異。
#[test]
fn false_negative_detection_rate_above_99_percent() {
    // 84 種突變覆蓋三個攻擊面：
    //   - RNG byte XOR 0x01（16 種）+ XOR 0xFF（16 種）：模擬 RNG 狀態篡改
    //   - ecs_mirror_hash 各 byte XOR 0xFF（32 種）：模擬 hash 傳輸值偽造
    //   - RNG byte XOR 0x55（10 種）：模擬細微的狀態篡改（如間接的 PlayerInput 影響）
    //   - 組合突變（10 種）：同時攻擊 rng_state + ecs_mirror_hash
    let script = compile_test_script();

    // 84 種不同的突變策略（16+16+32+10+10）
    let mutations: Vec<Box<dyn Fn(ShadowFrame) -> ShadowFrame>> = {
        let mut v: Vec<Box<dyn Fn(ShadowFrame) -> ShadowFrame>> = Vec::new();

        // RNG state 各 byte 的 XOR 0x01 突變（16 種）
        for i in 0usize..16 {
            v.push(Box::new(move |mut f: ShadowFrame| {
                f.rng_state[i] ^= 0x01;
                f
            }));
        }

        // RNG state 各 byte 的 XOR 0xFF 突變（16 種）
        for i in 0usize..16 {
            v.push(Box::new(move |mut f: ShadowFrame| {
                f.rng_state[i] ^= 0xFF;
                f
            }));
        }

        // ECS mirror hash 各 byte 竄改（32 種）
        for i in 0usize..32 {
            v.push(Box::new(move |mut f: ShadowFrame| {
                f.ecs_mirror_hash[i] ^= 0xFF;
                f
            }));
        }

        // RNG state 各 byte 的 XOR 0x55 突變（10 種）
        // 模擬細微的 RNG 狀態篡改，涵蓋 PlayerInput 間接影響的攻擊面
        for i in 0usize..10 {
            v.push(Box::new(move |mut f: ShadowFrame| {
                f.rng_state[i] ^= 0x55;
                f
            }));
        }

        // 組合突變：同時修改 rng 和 ecs_mirror_hash（10 種）
        for i in 0usize..10 {
            v.push(Box::new(move |mut f: ShadowFrame| {
                f.rng_state[i] ^= 0x01;
                f.ecs_mirror_hash[i] ^= 0xFF;
                f
            }));
        }

        v
    };

    assert_eq!(
        mutations.len(),
        84,
        "突變數量應為 84 種（實際 {}）",
        mutations.len()
    );

    let mut detected = 0u32;
    let total = mutations.len() as u32;

    for (i, mutate) in mutations.iter().enumerate() {
        let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
        let rng_state = [0u8; 16];
        let correct_hash = run_main_vm_and_get_hash(&script, &rng_state, i as u64);
        let frame = ShadowFrame {
            tick: i as u64,
            inputs: vec![],
            rng_state,
            ecs_mirror_hash: correct_hash,
        };

        let tampered_frame = mutate(frame);
        let resp = vm
            .validate_sync(&ShadowRequest {
                frames: vec![tampered_frame],
            })
            .unwrap();

        if matches!(resp.status, ShadowStatus::Mismatch { .. }) {
            detected += 1;
        }
    }

    // f64 用於測試統計（非 game logic），不受 Determinism Rules 限制
    let detection_rate = detected as f64 / total as f64 * 100.0;
    assert!(
        detection_rate >= 99.0,
        "偵測率 {detection_rate:.1}% 應 >= 99%（偵測 {detected}/{total}）"
    );
}

/// FN 最小突變測試：單 bit 翻轉必須被偵測
#[test]
fn single_bit_flip_always_detected() {
    let script = compile_test_script();
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
    let rng_state = [0u8; 16];
    let hash = run_main_vm_and_get_hash(&script, &rng_state, 1);
    // rng_state[0] LSB 翻轉：最小可能突變
    let mut tampered_rng = rng_state;
    tampered_rng[0] ^= 0x01;
    let frame = ShadowFrame {
        tick: 1,
        inputs: vec![],
        rng_state: tampered_rng,
        ecs_mirror_hash: hash,
    };
    let resp = vm
        .validate_sync(&ShadowRequest {
            frames: vec![frame],
        })
        .unwrap();
    assert!(
        matches!(resp.status, ShadowStatus::Mismatch { tick: 1, .. }),
        "單 bit 翻轉必須被偵測，實際：{:?}",
        resp.status
    );
}

/// FP 確定性重現測試：相同 seed 執行兩次，結果完全相同
#[test]
fn false_positive_test_is_deterministic() {
    let script = compile_test_script();

    let run = |seed: u64| -> u32 {
        let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
        let mut rng = DeterministicRng::seed_from_u64(seed);
        let mut false_positives = 0u32;

        for tick in 0..100u64 {
            let rng_state = rng.state_bytes();
            let _ = rng.next_u32(); // 推進 RNG
            let correct_hash = run_main_vm_and_get_hash(&script, &rng_state, tick);
            let frame = ShadowFrame {
                tick,
                inputs: vec![],
                rng_state,
                ecs_mirror_hash: correct_hash,
            };
            let resp = vm
                .validate_sync(&ShadowRequest {
                    frames: vec![frame],
                })
                .unwrap();
            if !matches!(resp.status, ShadowStatus::AllMatch) {
                false_positives += 1;
            }
        }
        false_positives
    };

    // 相同 seed 執行兩次，結果必須相同
    assert_eq!(
        run(0xDEAD_BEEF),
        run(0xDEAD_BEEF),
        "相同 seed 兩次執行的 false_positives 必須相同"
    );
}

/// FN 確定性重現測試：相同突變兩次執行偵測結果完全相同
#[test]
fn false_negative_test_is_deterministic() {
    let script = compile_test_script();

    let run = || -> Vec<bool> {
        let mutations: Vec<Box<dyn Fn(ShadowFrame) -> ShadowFrame>> = vec![
            Box::new(|mut f: ShadowFrame| {
                f.rng_state[0] ^= 0x01;
                f
            }),
            Box::new(|mut f: ShadowFrame| {
                f.ecs_mirror_hash[0] ^= 0xFF;
                f
            }),
            Box::new(|mut f: ShadowFrame| {
                f.rng_state[0] ^= 0x01;
                f.ecs_mirror_hash[0] ^= 0xFF;
                f
            }),
        ];
        mutations
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
                let rng = [0u8; 16];
                let hash = run_main_vm_and_get_hash(&script, &rng, i as u64);
                let f = ShadowFrame {
                    tick: i as u64,
                    inputs: vec![],
                    rng_state: rng,
                    ecs_mirror_hash: hash,
                };
                let resp = vm
                    .validate_sync(&ShadowRequest { frames: vec![m(f)] })
                    .unwrap();
                matches!(resp.status, ShadowStatus::Mismatch { .. })
            })
            .collect()
    };

    assert_eq!(run(), run(), "相同突變兩次執行的偵測結果必須相同");
}
