//! Shadow VM 整合測試共用 helper
//!
//! 此模組提供 `tests/` 目錄下所有整合測試共用的輔助函數。
//! 透過 `mod common;` 或 `#[path = "common.rs"] mod common;` 引入。

use std::collections::BTreeMap;

/// 60 Hz 固定時間步長（16.666...ms）。
/// Shadow VM 重播時使用固定 delta_time，與主線程相同（架構 §固定時間步長）。
const FIXED_DELTA_TIME_60HZ_F32: f32 = 1.0 / 60.0;

use shadow_vm::executor::ShadowScript;

/// 模擬主 VM 路徑：載入腳本 → 執行一幀 → 計算 state hash
///
/// 用途：動態計算 expected hash，確保主 VM 與 Shadow VM 兩條路徑一致。
/// 注意：此函數僅供測試使用，不受 Determinism Rules 限制（infrastructure timing）。
pub fn run_main_vm_and_get_hash(
    script: &ShadowScript,
    rng_state: &[u8; 16],
    tick: u64,
) -> [u8; 32] {
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
