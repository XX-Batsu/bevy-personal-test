//! demo — 終端機能力展示 CLI
//!
//! 不需 server、素材或瀏覽器，直接驗證四項核心保證：
//!   1. SoftF32          — 軟體浮點，位元層級可重現
//!   2. DeterministicRng — PCG-XSH-RR，可由 16-byte state 精確還原
//!   3. State hash chain — blake3 逐幀雜湊鏈，同 seed 收斂、異 seed 發散
//!   4. Rhai sandbox     — eval/module 阻擋與 ops/timeout 上限
//!
//! 用法：
//!   cargo run -p demo_cli --release -- [--ticks 8] [--only softfloat|rng|hash|sandbox]

use std::collections::BTreeMap;

use bridge_types::{EntityId, EntityState, MirroredEntity, ScriptError};
use clap::Parser;
use deterministic::{DeterministicRng, SoftF32, SoftVec3};
use state_hash::compute_state_hash;
use vm_runtime::SandboxedEngine;

#[derive(Parser)]
#[command(
    name = "demo",
    about = "Terminal demo of the framework's determinism, state-hash and sandbox guarantees"
)]
struct Cli {
    /// State hash chain 的模擬幀數
    #[arg(long, default_value_t = 8)]
    ticks: u64,

    /// 只執行單一段落（softfloat / rng / hash / sandbox）
    #[arg(long)]
    only: Option<String>,
}

fn main() {
    let cli = Cli::parse();
    let only = cli.only.as_deref();

    if section_enabled(only, "softfloat") {
        section_softfloat();
    }
    if section_enabled(only, "rng") {
        section_rng();
    }
    if section_enabled(only, "hash") {
        section_hash_chain(cli.ticks);
    }
    if section_enabled(only, "sandbox") {
        section_sandbox();
    }
}

fn section_enabled(only: Option<&str>, name: &str) -> bool {
    only.is_none_or(|s| s == name)
}

fn header(title: &str) {
    println!("\n== {title} ==");
}

// ── 1. SoftF32 ──────────────────────────────────────────────────────────

/// 對照軟體浮點與原生浮點的位元表示。
///
/// 重點不是「兩者相等」，而是 SoftF32 的位元輸出不依賴平台的 FPU/SIMD 設定，
/// 因此可跨 x86 / ARM / WASM 重現；原生 f32 則無此保證。
fn section_softfloat() {
    header("1. SoftF32 — software float, bit-reproducible across targets");

    let a = SoftF32::from_f32(0.1);
    let b = SoftF32::from_f32(0.2);
    let two = SoftF32::from_f32(2.0);
    let one = SoftF32::from_f32(1.0);

    let rows: [(&str, SoftF32, f32); 4] = [
        ("0.1 + 0.2", a + b, 0.1f32 + 0.2f32),
        ("0.1 * 0.2", a * b, 0.1f32 * 0.2f32),
        ("sqrt(2.0)", two.sqrt(), 2.0f32.sqrt()),
        ("sin(1.0)", one.sin(), 1.0f32.sin()),
    ];

    println!(
        "  {:<12} {:<14} {:<14} bits equal",
        "expr", "SoftF32", "native f32"
    );
    for (expr, soft, native) in rows {
        let equal = soft.to_bits() == native.to_bits();
        println!(
            "  {:<12} 0x{:08X}     0x{:08X}     {}",
            expr,
            soft.to_bits(),
            native.to_bits(),
            if equal { "yes" } else { "no" }
        );
    }
    println!("  note: game logic uses SoftF32 only; the render layer may use native floats.");
}

// ── 2. DeterministicRng ─────────────────────────────────────────────────

/// 展示 seed → 序列可重現，且可由 16-byte state 還原並續接同一序列。
fn section_rng() {
    header("2. DeterministicRng — PCG-XSH-RR, seeded and restorable");

    let mut rng = DeterministicRng::seed_from_u64(42);
    let first: Vec<u32> = (0..4).map(|_| rng.next_u32()).collect();
    let state = rng.state_bytes();
    let after: Vec<u32> = (0..4).map(|_| rng.next_u32()).collect();

    let mut restored = DeterministicRng::from_state_bytes(&state);
    let replayed: Vec<u32> = (0..4).map(|_| restored.next_u32()).collect();

    let fresh: Vec<u32> = {
        let mut r = DeterministicRng::seed_from_u64(42);
        (0..4).map(|_| r.next_u32()).collect()
    };

    println!("  seed 42, draws 1-4     : {first:?}");
    println!(
        "  same seed, replayed    : {fresh:?}  (identical: {})",
        first == fresh
    );
    println!("  captured state (16B)   : {}", hex::encode(state));
    println!("  draws 5-8 from rng     : {after:?}");
    println!(
        "  draws 5-8 from state   : {replayed:?}  (identical: {})",
        after == replayed
    );
}

// ── 3. State hash chain ─────────────────────────────────────────────────

/// 以 SoftF32 位移 + RNG 抖動推進實體，逐幀輸出 blake3 狀態雜湊。
///
/// 框架本身把遊戲邏輯留為插入點（`AuthoritativeSimulation::step_full` 的 TODO），
/// 因此此處由 demo 提供一段最小遊戲邏輯，用來驅動雜湊鏈。
fn section_hash_chain(ticks: u64) {
    header("3. State hash chain — blake3 over (tick, rng state, entities)");

    let chain_a = simulate(42, ticks);
    let chain_b = simulate(42, ticks);
    let chain_c = simulate(43, ticks);

    for (i, hash) in chain_a.iter().enumerate() {
        let tick = i as u64 + 1;
        let same = chain_b[i] == *hash;
        let diverged = chain_c[i] != *hash;
        println!(
            "  tick {tick:>3}  {}  seed42-rerun: {}  seed43: {}",
            short_hash(hash),
            if same { "match" } else { "MISMATCH" },
            if diverged { "diverged" } else { "match" }
        );
    }

    println!("  seed 42 replay is bit-identical: {}", chain_a == chain_b);
    println!(
        "  seed 43 diverges from tick {}",
        chain_a
            .iter()
            .zip(&chain_c)
            .position(|(x, y)| x != y)
            .map(|i| (i + 1).to_string())
            .unwrap_or_else(|| "never".to_string())
    );
}

/// 最小確定性模擬：3 個實體，每幀依 SoftF32 速度移動並加上 RNG 抖動。
fn simulate(seed: u64, ticks: u64) -> Vec<[u8; 32]> {
    let mut rng = DeterministicRng::seed_from_u64(seed);
    let mut entities: BTreeMap<EntityId, MirroredEntity> =
        (1..=3).map(|i| (EntityId(i), spawn_entity(i))).collect();

    let dt = SoftF32::from_f32(1.0 / 60.0);
    let mut chain = Vec::with_capacity(ticks as usize);

    for tick in 1..=ticks {
        for entity in entities.values_mut() {
            // 抖動取自 RNG，確保 rng state 每幀前進且進入雜湊
            let jitter = rng.gen_soft_f32();
            let velocity = SoftVec3::new(jitter, dt, jitter * dt);
            entity.position = entity.position + velocity.scale(dt);
            entity.hp -= 1;
            entity.state = if entity.hp > 0 {
                EntityState::MOVING
            } else {
                EntityState::DEAD
            };
        }
        chain.push(compute_state_hash(&entities, &rng.state_bytes(), tick));
    }
    chain
}

fn spawn_entity(i: u64) -> MirroredEntity {
    let f = SoftF32::from_f32(i as f32);
    MirroredEntity {
        position: SoftVec3::new(f, SoftF32::from_f32(0.0), f),
        rotation: SoftVec3::zero(),
        scale: SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(1.0),
        ),
        hp: 100,
        max_hp: 100,
        state: EntityState::IDLE,
        animation_id: None,
        custom: BTreeMap::new(),
    }
}

fn short_hash(hash: &[u8; 32]) -> String {
    format!("blake3:{}…", hex::encode(&hash[..8]))
}

// ── 4. Rhai sandbox ─────────────────────────────────────────────────────

/// 驗證沙箱三道防線：符號阻擋、模組阻擋、資源上限。
fn section_sandbox() {
    header("4. Rhai sandbox — symbol blocking and resource limits");

    let engine = SandboxedEngine::new();

    let cases: [(&str, &str); 5] = [
        (
            "plain arithmetic",
            "let x = 0; for i in 0..10 { x += i; } x",
        ),
        ("eval()", r#"eval("1 + 1")"#),
        ("module import", r#"import "std" as std; 1"#),
        ("Fn() indirection", r#"let f = Fn("foo"); 1"#),
        ("unbounded loop", "let x = 0; loop { x += 1; }"),
    ];

    for (label, script) in cases {
        match engine.execute(script) {
            Ok(value) => println!("  {label:<18} -> allowed, result = {value}"),
            Err(err) => println!("  {label:<18} -> rejected ({})", err_kind(&err)),
        }
    }
    println!("  limits: 50,000 ops, 32 call levels, 4 KiB strings, 2 ms per script.");
}

/// 只取錯誤種類，避免把耗時毫秒數印進輸出（保持輸出可重現）。
fn err_kind(err: &ScriptError) -> &'static str {
    match err {
        ScriptError::CompileError(_) => "CompileError",
        ScriptError::Timeout { .. } => "Timeout",
        ScriptError::OperationLimit { .. } => "OperationLimit",
        ScriptError::RuntimeError { .. } => "RuntimeError",
        _ => "SandboxError",
    }
}
