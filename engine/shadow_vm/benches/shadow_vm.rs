//! Shadow VM 效能 Benchmark
//!
//! 驗證 Shadow VM 符合效能要求：4 幀重播 < 20ms，單幀重播 < 5ms。
//! 僅在 native target 執行（WASM 無 std::time::Instant）。
#![cfg(not(target_arch = "wasm32"))]

use bridge_types::{ShadowFrame, ShadowRequest};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use shadow_vm::{executor::ShadowScript, InProcessShadowVm};
use std::collections::BTreeMap;

/// 60 Hz 固定時間步長（16.666...ms）。
/// Shadow VM 重播時使用固定 delta_time，與主線程相同（架構 §固定時間步長）。
const FIXED_DELTA_TIME_60HZ_F32: f32 = 1.0 / 60.0;

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
    use bridge_types::{EcsMirror, EntityId};
    use deterministic::DeterministicRng;
    use state_hash::compute_state_hash;
    use vm_runtime::SandboxedEngine;

    let engine = SandboxedEngine::new();
    let ast = engine
        .engine()
        .compile(&script.0)
        .expect("AST 編譯不應失敗");
    let _rng = DeterministicRng::from_state_bytes(rng_state);

    let mut scope = rhai::Scope::new();
    scope.push("input_type", 0i64);
    scope.push("input_data_int", 0i64);
    scope.push("rng_state", _rng.state_bytes().to_vec());
    scope.push("tick", tick as i64);

    let _: rhai::Dynamic = engine
        .call_fn_with_scope(&mut scope, &ast, "on_tick", ())
        .expect("on_tick 執行不應失敗");

    let ecs_mirror = EcsMirror {
        entities: BTreeMap::new(),
        local_player_id: EntityId(0),
        frame_number: 0,
        delta_time: deterministic::SoftF32::from_f32(FIXED_DELTA_TIME_60HZ_F32),
    };
    compute_state_hash(&ecs_mirror.entities, rng_state, tick)
}

fn make_test_frames(script: &ShadowScript, count: usize) -> Vec<ShadowFrame> {
    (0..count as u64)
        .map(|tick| {
            let rng = [0u8; 16]; // 固定 seed，確保確定性
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

fn bench_4_frame_replay(c: &mut Criterion) {
    let script = compile_test_script();
    let frames = make_test_frames(&script, 4);
    // VM 建立移至 iter 外部，僅量測重播本身（不含引擎初始化）
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    c.bench_function("shadow_vm/4_frame_replay", |b| {
        b.iter(|| {
            let request = ShadowRequest {
                frames: black_box(frames.clone()),
            };
            black_box(vm.validate_sync(&request).unwrap())
        })
    });
}

fn bench_single_frame_replay(c: &mut Criterion) {
    let script = compile_test_script();
    let frames = make_test_frames(&script, 1);
    // VM 建立移至 iter 外部，僅量測重播本身（不含引擎初始化）
    let mut vm = InProcessShadowVm::new(script.clone()).unwrap();

    c.bench_function("shadow_vm/single_frame_replay", |b| {
        b.iter(|| {
            let request = ShadowRequest {
                frames: black_box(frames.clone()),
            };
            black_box(vm.validate_sync(&request).unwrap())
        })
    });
}

fn bench_replay_scaling(c: &mut Criterion) {
    let script = compile_test_script();
    let mut group = c.benchmark_group("shadow_vm/replay_scaling");

    for frame_count in [1usize, 2, 4, 8].iter() {
        let frames = make_test_frames(&script, *frame_count);
        // VM 建立移至 iter 外部，僅量測重播本身（不含引擎初始化）
        let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
        group.bench_with_input(
            BenchmarkId::from_parameter(frame_count),
            frame_count,
            |b, _| {
                b.iter(|| {
                    let request = ShadowRequest {
                        frames: black_box(frames.clone()),
                    };
                    black_box(vm.validate_sync(&request).unwrap())
                })
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_4_frame_replay,
    bench_single_frame_replay,
    bench_replay_scaling
);
criterion_main!(benches);
