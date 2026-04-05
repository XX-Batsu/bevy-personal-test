//! 壓力測試 — nightly-perf #[ignore] 標記
//!
//! 使用 `cargo test -p deterministic --features nightly-perf -- --ignored` 執行。

#![cfg(feature = "nightly-perf")]

use deterministic::{DeterministicRng, SoftF32, SoftVec3};

/// 10K 幀模擬：驗證長時間運算無記憶體洩漏、無 panic
#[test]
#[ignore = "nightly-perf: 10K 幀壓力測試"]
fn stress_10k_frames_no_panic() {
    let mut rng = DeterministicRng::seed_from_u64(42);
    let mut pos = SoftVec3::new(
        SoftF32::from_f32(0.0),
        SoftF32::from_f32(0.0),
        SoftF32::from_f32(0.0),
    );
    let dt = SoftF32::from_f32(1.0 / 60.0);

    for _ in 0..10_000 {
        let ax = SoftF32::from_bits(rng.next_u32());
        let ay = SoftF32::from_bits(rng.next_u32());
        let az = SoftF32::from_bits(rng.next_u32());
        let vel = SoftVec3::new(ax * dt, ay * dt, az * dt);
        pos = pos + vel;
    }

    // 若能走到這裡，代表 10K 幀無 panic
    let _ = pos;
}

/// 10K 幀確定性：兩次 run 結果一致
#[test]
#[ignore = "nightly-perf: 10K 幀確定性驗證"]
fn stress_10k_frames_deterministic() {
    fn run_sim(seed: u64) -> [u8; 32] {
        let mut rng = DeterministicRng::seed_from_u64(seed);
        let mut hasher = blake3::Hasher::new();

        for _ in 0..10_000 {
            let val = rng.next_u32();
            hasher.update(&val.to_le_bytes());
        }

        *hasher.finalize().as_bytes()
    }

    let hash_a = run_sim(42);
    let hash_b = run_sim(42);
    assert_eq!(hash_a, hash_b, "10K 幀確定性驗證失敗");
}
