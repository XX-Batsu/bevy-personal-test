//! 確定性 RNG — PCG-XSH-RR 自行實作。
//! 提供完整狀態序列化能力，用於 state hash、rollback、replay。
//!
//! # 演算法
//!
//! PCG-XSH-RR（Permuted Congruential Generator, XorShift + Random Rotation）：
//! - LCG multiplier = 6364136223846793005（Knuth 常數）
//! - increment = 3（固定，由 stream=1 計算：1 * 2 + 1 = 3）
//! - State 64-bit → Output 32-bit
//!
//! # 為何自行實作
//!
//! `rand_pcg::Pcg32` 的 state/increment 為 private，無法直接序列化。
//! 我們需要 `state_bytes()` / `from_state_bytes()` 進行 state hash 與 rollback，
//! 演算法僅 ~30 行，自行實作的維護成本低於繞過 private 欄位的 hack。

use serde::{Deserialize, Serialize};

/// PCG LCG 乘數（Knuth 常數，廣泛用於 PCG 家族）
const PCG_MULTIPLIER: u64 = 6364136223846793005;

/// 確定性 RNG — 自行實作 PCG-XSH-RR 演算法。
///
/// 演算法參數：
/// - LCG 乘數（multiplier）：6364136223846793005
/// - 固定 increment：3（由 stream=1 計算：stream * 2 + 1）
/// - State：64-bit，Output：32-bit
///
/// 自行實作而非使用 rand_pcg，以確保 state / increment 可完整序列化，
/// 用於 state hash、rollback、replay 等場景。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeterministicRng {
    /// PCG 內部狀態（64-bit LCG state）
    state: u64,
    /// LCG 增量（固定為 3，但參與序列化以維持格式一致性）
    increment: u64,
}

impl DeterministicRng {
    /// 從 seed 建立 RNG 實例。
    ///
    /// 初始化流程（PCG 標準）：
    /// 1. increment = stream * 2 + 1 = 1 * 2 + 1 = 3
    /// 2. state = 0
    /// 3. advance()（LCG 推進一次）
    /// 4. state += seed
    /// 5. advance()（LCG 再推進一次）
    pub fn seed_from_u64(seed: u64) -> Self {
        // stream = 1 → increment = 1 * 2 + 1 = 3
        let increment = 1u64.wrapping_mul(2).wrapping_add(1); // = 3
        let mut rng = Self {
            state: 0,
            increment,
        };

        // 初始化第一步：推進一次 LCG
        rng.advance();

        // 混入 seed
        rng.state = rng.state.wrapping_add(seed);

        // 初始化第二步：再推進一次 LCG
        rng.advance();

        rng
    }

    /// LCG 推進（internal）。
    ///
    /// state = state * 6364136223846793005 + increment
    ///
    /// 使用 wrapping_mul / wrapping_add 處理 u64 溢位。
    fn advance(&mut self) {
        self.state = self
            .state
            .wrapping_mul(PCG_MULTIPLIER)
            .wrapping_add(self.increment);
    }

    /// 產生下一個 32-bit 隨機數。
    ///
    /// XSH-RR 輸出函式：
    /// - xorshifted = ((old_state >> 18) ^ old_state) >> 27
    /// - rot = old_state >> 59
    /// - output = xorshifted.rotate_right(rot)
    pub fn next_u32(&mut self) -> u32 {
        let old_state = self.state;
        self.advance();

        // XSH-RR (XorShift + Random Rotation) 輸出函式
        // 從 64-bit state 萃取 32-bit output
        let xorshifted = (((old_state >> 18) ^ old_state) >> 27) as u32;
        let rot = (old_state >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// 產生 [low, high]（含兩端）範圍內的隨機 i32。
    ///
    /// 使用 rejection sampling 消除 modulo bias：
    /// - range = (high - low + 1) as u64
    /// - threshold = 0x1_0000_0000u64 - (0x1_0000_0000u64 % range)
    /// - loop: r = next_u32() as u64, if r < threshold → return low + (r % range) as i32
    ///
    /// # Panics
    /// - low > high 時 panic
    pub fn gen_range_i32(&mut self, low: i32, high: i32) -> i32 {
        assert!(
            low <= high,
            "gen_range_i32: low ({}) 必須 <= high ({})",
            low,
            high
        );
        let range = (high as i64 - low as i64 + 1) as u64;
        let threshold = 0x1_0000_0000u64 - (0x1_0000_0000u64 % range);
        loop {
            let r = self.next_u32() as u64;
            if r < threshold {
                return low + (r % range) as i32;
            }
        }
    }

    /// 產生 [0.0, 1.0) 範圍的確定性浮點數。
    ///
    /// 實作：取 next_u32() 高 23 bit 作為 mantissa，
    /// 組成 IEEE 754 [1.0, 2.0) 後減 1.0。
    /// 恰好消耗一個 next_u32()。
    pub fn gen_soft_f32(&mut self) -> crate::soft_float::SoftF32 {
        let mantissa = self.next_u32() >> 9;
        let bits = mantissa | 0x3F80_0000;
        let one_to_two = crate::soft_float::SoftF32(bits);
        one_to_two - crate::soft_float::SoftF32::from_f32(1.0)
    }

    /// 將內部狀態序列化為 16 bytes。
    ///
    /// 格式：state(u64 LE, 8 bytes) + increment(u64 LE, 8 bytes)
    /// 即使 increment 固定為 3，仍序列化以保持格式一致性，
    /// 方便 state hash 與未來 multi-stream 擴展。
    pub fn state_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&self.state.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.increment.to_le_bytes());
        bytes
    }

    /// 從 16 bytes 還原 RNG 狀態。
    pub fn from_state_bytes(bytes: &[u8; 16]) -> Self {
        let state = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let increment = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        Self { state, increment }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_seed_same_sequence() {
        let mut rng1 = DeterministicRng::seed_from_u64(12345);
        let mut rng2 = DeterministicRng::seed_from_u64(12345);
        let seq1: Vec<u32> = (0..100).map(|_| rng1.next_u32()).collect();
        let seq2: Vec<u32> = (0..100).map(|_| rng2.next_u32()).collect();
        assert_eq!(seq1, seq2);
    }

    #[test]
    fn test_different_seed_different_sequence() {
        let mut rng1 = DeterministicRng::seed_from_u64(12345);
        let mut rng2 = DeterministicRng::seed_from_u64(54321);
        let seq1: Vec<u32> = (0..100).map(|_| rng1.next_u32()).collect();
        let seq2: Vec<u32> = (0..100).map(|_| rng2.next_u32()).collect();
        assert_ne!(seq1, seq2);
    }

    #[test]
    fn test_gen_range_i32_bounds() {
        let mut rng = DeterministicRng::seed_from_u64(42);
        for _ in 0..1000 {
            let v = rng.gen_range_i32(10, 20);
            assert!(
                (10..=20).contains(&v),
                "gen_range_i32(10, 20) 產生了超出範圍的值: {}",
                v
            );
        }
    }

    #[test]
    fn test_state_serialization_round_trip() {
        let mut rng = DeterministicRng::seed_from_u64(12345);

        // 推進 50 次
        for _ in 0..50 {
            rng.next_u32();
        }

        // 儲存狀態
        let state = rng.state_bytes();

        // 繼續產生 100 個值
        let mut rng_continued = rng.clone();
        let expected: Vec<u32> = (0..100).map(|_| rng_continued.next_u32()).collect();

        // 從狀態還原
        let mut rng_restored = DeterministicRng::from_state_bytes(&state);
        let restored: Vec<u32> = (0..100).map(|_| rng_restored.next_u32()).collect();

        assert_eq!(expected, restored);
    }

    #[test]
    fn test_seed_zero() {
        let mut rng = DeterministicRng::seed_from_u64(0);
        let vals: Vec<u32> = (0..10).map(|_| rng.next_u32()).collect();
        assert!(vals.iter().any(|&v| v != 0), "seed=0 不應全部產生 0");
    }

    #[test]
    fn test_seed_max() {
        let mut rng = DeterministicRng::seed_from_u64(u64::MAX);
        for _ in 0..10 {
            rng.next_u32();
        }
    }

    #[test]
    fn test_range_single_value() {
        let mut rng = DeterministicRng::seed_from_u64(42);
        for _ in 0..100 {
            assert_eq!(rng.gen_range_i32(5, 5), 5_i32);
        }
    }

    #[test]
    #[should_panic]
    fn test_gen_range_i32_min_greater_than_max_panics() {
        let mut rng = DeterministicRng::seed_from_u64(42);
        rng.gen_range_i32(20, 10);
    }

    #[test]
    fn test_gen_soft_f32_range() {
        let mut rng = DeterministicRng::seed_from_u64(42);
        for _ in 0..1000 {
            let v = rng.gen_soft_f32();
            let f = v.to_f64();
            assert!(
                (0.0..1.0).contains(&f),
                "gen_soft_f32() 產生了超出 [0.0, 1.0) 範圍的值: {}",
                f
            );
        }
    }

    #[test]
    fn test_gen_soft_f32_determinism() {
        let mut rng1 = DeterministicRng::seed_from_u64(12345);
        let mut rng2 = DeterministicRng::seed_from_u64(12345);
        let seq1: Vec<crate::soft_float::SoftF32> = (0..100).map(|_| rng1.gen_soft_f32()).collect();
        let seq2: Vec<crate::soft_float::SoftF32> = (0..100).map(|_| rng2.gen_soft_f32()).collect();
        assert_eq!(seq1, seq2);
    }
}
