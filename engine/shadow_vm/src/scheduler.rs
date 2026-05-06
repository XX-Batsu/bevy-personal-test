//! SamplingScheduler — Shadow VM 確定性抽樣排程器
//!
//! 以 PCG（DeterministicRng）決定每次 Shadow VM 驗證的間隔（[30, 60] 幀），
//! 同一 seed 產生相同抽樣序列（確定性保證）。

use deterministic::DeterministicRng;

/// Shadow VM 抽樣排程器
///
/// 持有獨立 RNG 實例（與遊戲邏輯 RNG 完全隔離），
/// 每次觸發後自動推進至下一個抽樣 tick。
pub struct SamplingScheduler {
    rng: DeterministicRng,
    next_sample_tick: u64,
}

impl SamplingScheduler {
    /// 以 seed 初始化排程器，計算首次抽樣 tick
    ///
    /// seed 應獨立於遊戲邏輯的 game_seed，避免共用 RNG 狀態破壞確定性。
    pub fn new(seed: u64) -> Self {
        let mut rng = DeterministicRng::seed_from_u64(seed);
        let next_sample_tick = next_interval(&mut rng);
        Self {
            rng,
            next_sample_tick,
        }
    }

    /// 若 tick >= next_sample_tick，推進下一個 sample tick 並回傳 true；否則 false
    ///
    /// 有副作用：回傳 true 時內部 next_sample_tick 向前推進。
    /// 使用絕對 tick 模型（非倒數計數），支援非連續 tick 查詢。
    pub fn should_sample(&mut self, tick: u64) -> bool {
        if tick >= self.next_sample_tick {
            // 使用 saturating_add 防止 tick 接近 u64::MAX 時溢位
            self.next_sample_tick = tick.saturating_add(next_interval(&mut self.rng));
            tracing::debug!("Shadow VM 抽樣排程觸發：tick={}", tick);
            true
        } else {
            false
        }
    }
}

/// 計算下一個抽樣間隔（[30, 60] 範圍內的 u64）
///
/// 公式：`30 + (rng.next_u32() % 31)`
/// modulo bias 約 7.2 × 10⁻⁹，遠低於可觀測門檻（< 0.04%），已由上游接受。
fn next_interval(rng: &mut DeterministicRng) -> u64 {
    30 + (rng.next_u32() % 31) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test A: 確定性序列
    #[test]
    fn scheduler_same_seed_same_sequence() {
        let mut s1 = SamplingScheduler::new(42);
        let mut s2 = SamplingScheduler::new(42);
        for tick in 0..1000 {
            assert_eq!(s1.should_sample(tick), s2.should_sample(tick));
        }
    }

    // Test B: 不同 seed 通常產生不同序列
    #[test]
    fn scheduler_different_seeds_different_sequences() {
        let mut s1 = SamplingScheduler::new(1);
        let mut s2 = SamplingScheduler::new(999);
        let seq1: Vec<u64> = (0..200u64).filter(|&t| s1.should_sample(t)).collect();
        let seq2: Vec<u64> = (0..200u64).filter(|&t| s2.should_sample(t)).collect();
        assert_ne!(seq1, seq2);
    }

    // Test C: 平均間隔在 [30, 60] 範圍內（統計測試）
    #[test]
    fn scheduler_average_interval_within_30_60() {
        let mut scheduler = SamplingScheduler::new(12345);
        let samples: Vec<u64> = (0..2000u64)
            .filter(|&t| scheduler.should_sample(t))
            .collect();
        // 至少要有幾個抽樣點
        assert!(samples.len() >= 30, "2000 tick 應有至少 30 個抽樣點");
        // 計算相鄰抽樣點間隔
        let intervals: Vec<u64> = samples.windows(2).map(|w| w[1] - w[0]).collect();
        // 注：as f64 計算平均值僅限測試程式碼，非遊戲邏輯，不違反 Determinism Rules
        let avg = intervals.iter().sum::<u64>() as f64 / intervals.len() as f64;
        assert!(
            (35.0..=55.0).contains(&avg),
            "平均抽樣間隔 {} 應在 [35, 55] 幀內（規格 [30, 60]，3σ 統計邊界）",
            avg
        );
    }

    // Test D: 首次抽樣 tick 在 [30, 60] 範圍
    #[test]
    fn scheduler_first_sample_within_range() {
        for seed in 0..50u64 {
            let mut scheduler = SamplingScheduler::new(seed);
            let first = (0..200u64).find(|&t| scheduler.should_sample(t)).unwrap();
            assert!((30..=60).contains(&first), "seed={seed}, first={first}");
        }
    }

    // Test E: should_sample 有副作用，連續呼叫同一 tick 只觸發一次
    #[test]
    fn scheduler_should_sample_has_side_effect() {
        let mut scheduler = SamplingScheduler::new(0);
        // 找到第一個觸發的 tick
        let trigger = (0..200u64).find(|&t| scheduler.should_sample(t)).unwrap();
        // 再次呼叫同一 tick，應回傳 false（已推進 next_sample_tick）
        assert!(!scheduler.should_sample(trigger));
    }

    // Test F: tick=0 不觸發抽樣（零值邊界）
    #[test]
    fn scheduler_tick_zero() {
        // 首次抽樣 tick 在 [30, 60]，tick=0 必定小於任何首次抽樣 tick
        for seed in 0..100u64 {
            let mut scheduler = SamplingScheduler::new(seed);
            assert!(
                !scheduler.should_sample(0),
                "seed={seed}: tick=0 不應觸發抽樣（首次 tick >= 30）"
            );
        }
    }

    // Test G: tick 接近 u64::MAX 不發生溢位
    #[test]
    fn scheduler_all_intervals_at_least_30_frames() {
        // 驗證不變式：相鄰兩次抽樣之間至少間隔 30 幀（設計文件 scheduling.md §不變式 #5）
        let mut scheduler = SamplingScheduler::new(42);
        let mut last_sample_tick: Option<u64> = None;
        let total_ticks = 3000u64;

        for tick in 0..total_ticks {
            if scheduler.should_sample(tick) {
                if let Some(last) = last_sample_tick {
                    let interval = tick - last;
                    assert!(
                        interval >= 30,
                        "相鄰兩次抽樣間隔 {} 幀，違反最小間隔 30 幀不變式（tick {} vs {}）",
                        interval,
                        last,
                        tick
                    );
                }
                last_sample_tick = Some(tick);
            }
        }

        // 確認確實有發生抽樣（避免測試因排程器從未觸發而假通過）
        assert!(last_sample_tick.is_some(), "3000 幀內應至少有一次抽樣");
    }

    #[test]
    fn scheduler_no_overflow_near_u64_max() {
        let mut scheduler = SamplingScheduler::new(77);
        let big_tick = u64::MAX - 100;
        // 首次呼叫必觸發（big_tick 遠大於任何初始 next_sample_tick）
        let triggered = scheduler.should_sample(big_tick);
        assert!(triggered, "極大 tick 應觸發抽樣");
        // 觸發後 next_sample_tick = big_tick + interval（[30,60]）
        // 若 big_tick + interval 溢位，saturating_add 保證不 wrap-around
        let next_tick = big_tick + 1;
        let _ = scheduler.should_sample(next_tick);
        // 不 panic 即通過
    }
}
