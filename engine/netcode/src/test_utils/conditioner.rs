//! 穩態網路條件模擬 + Burst 丟包模擬
//!
//! 使用 DeterministicRng（seeded PCG-XSH-RR）確保測試可重現。
//! ⚠️ 此模組為測試基礎設施（`#[cfg(test)]`），非遊戲邏輯。
//! `packet_loss` 與 `reorder_rate` 使用 f64 是因為不參與確定性遊戲邏輯。

use deterministic::deterministic_rng::DeterministicRng;

/// 穩態網路條件配置（機率模型）
///
/// 描述延遲、抖動、丟包、亂序等網路行為。
/// 不含 burst loss——burst 為時間區間事件，由 [`BurstLossSimulator`] 獨立處理。
#[derive(Debug, Clone)]
pub struct NetworkCondition {
    /// 基礎延遲（毫秒）
    pub latency_ms: u64,
    /// Jitter（毫秒，實際延遲 = latency_ms ± jitter_ms）
    pub jitter_ms: u64,
    /// 丟包率（0.0 = 無丟包，1.0 = 全部丟失）
    pub packet_loss: f64,
    /// 封包亂序率（0.0-1.0）
    pub reorder_rate: f64,
    /// 確定性隨機數產生器
    rng: DeterministicRng,
}

/// 模擬 burst 丟包（連續一段時間內全部丟失）
///
/// 與 NetworkCondition 分離：穩態條件（機率模型）vs burst（時間區間事件）
pub struct BurstLossSimulator {
    /// burst 開始的 tick
    pub burst_start_tick: Option<u64>,
    /// burst 持續 tick 數
    pub burst_duration_ticks: u64,
}

/// 排程後的 packet 資訊
pub struct ConditionedPacket {
    /// 封包資料
    pub data: Vec<u8>,
    /// 預期送達時間（毫秒）
    pub deliver_at_ms: u64,
    /// 是否被丟棄
    pub dropped: bool,
}

impl NetworkCondition {
    /// 建立新的 condition，使用指定 seed 確保可重現性
    pub fn new_with_seed(seed: u64) -> Self {
        Self {
            latency_ms: 0,
            jitter_ms: 0,
            packet_loss: 0.0,
            reorder_rate: 0.0,
            rng: DeterministicRng::seed_from_u64(seed),
        }
    }

    /// 設定固定延遲（毫秒）
    pub fn with_latency(mut self, ms: u64) -> Self {
        assert!(
            ms <= i32::MAX as u64,
            "latency_ms 超過 i32::MAX，process() 內部轉型會溢位"
        );
        self.latency_ms = ms;
        self
    }

    /// 設定抖動範圍（±ms）
    pub fn with_jitter(mut self, ms: u64) -> Self {
        assert!(
            ms <= i32::MAX as u64,
            "jitter_ms 超過 i32::MAX，process() 內部轉型會溢位"
        );
        self.jitter_ms = ms;
        self
    }

    /// 設定隨機丟包率（0.0 到 1.0，clamp）
    pub fn with_packet_loss(mut self, rate: f64) -> Self {
        self.packet_loss = rate.clamp(0.0, 1.0);
        self
    }

    /// 設定亂序率（0.0 到 1.0，clamp）
    pub fn with_reorder(mut self, rate: f64) -> Self {
        self.reorder_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// 處理一個 packet，回傳排程資訊
    ///
    /// 演算法順序：隨機丟包 → 延遲計算（base + jitter, clamp >= 0）→ 亂序
    ///
    /// RNG 消耗：步驟 1 固定 1 次，步驟 2 jitter>0 時 1 次，步驟 3 reorder>0 時 1-2 次
    pub fn process(&mut self, data: Vec<u8>, send_time_ms: u64) -> ConditionedPacket {
        // === 步驟 1：隨機丟包 ===
        let dropped = self.rng.gen_soft_f32().to_f64() < self.packet_loss;

        // === 步驟 2：延遲計算 ===
        let jitter_offset = if self.jitter_ms > 0 {
            self.rng
                .gen_range_i32(-(self.jitter_ms as i32), self.jitter_ms as i32)
        } else {
            0
        };

        let base_delay = self.latency_ms as i64;
        // clamp >= 0：負延遲無物理意義
        let total_delay = (base_delay + jitter_offset as i64).max(0) as u64;
        let deliver_at_ms = send_time_ms + total_delay;

        // === 步驟 3：亂序處理 ===
        let deliver_at_ms =
            if self.reorder_rate > 0.0 && self.rng.gen_soft_f32().to_f64() < self.reorder_rate {
                // 加上額外隨機延遲（最多 1 倍 latency）
                let extra_delay = self.rng.gen_range_i32(0, self.latency_ms.max(1) as i32) as u64;
                deliver_at_ms + extra_delay
            } else {
                deliver_at_ms
            };

        ConditionedPacket {
            data,
            deliver_at_ms,
            dropped,
        }
    }

    // === 核心 presets ===

    /// 0ms 延遲、無 jitter、無丟包（基準線）
    pub fn perfect() -> Self {
        Self::new_with_seed(0)
    }

    /// 50ms + ±30ms jitter，無丟包
    pub fn jitter_only() -> Self {
        Self::new_with_seed(42).with_latency(50).with_jitter(30)
    }

    /// 無 jitter，5% 丟包
    pub fn loss_5pct() -> Self {
        Self::new_with_seed(42).with_packet_loss(0.05)
    }

    /// 無 jitter，20% 丟包
    pub fn loss_20pct() -> Self {
        Self::new_with_seed(42).with_packet_loss(0.20)
    }

    /// 10% 封包亂序，無丟包
    pub fn reorder_10pct() -> Self {
        Self::new_with_seed(42).with_latency(100).with_reorder(0.10)
    }

    // === 額外 presets ===

    /// 良好 WiFi（20ms 延遲，±5ms 抖動）
    pub fn good_wifi() -> Self {
        Self::new_with_seed(42).with_latency(20).with_jitter(5)
    }

    /// 4G 行動網路（80ms 延遲，±30ms 抖動，2% 丟包）
    pub fn mobile_4g() -> Self {
        Self::new_with_seed(42)
            .with_latency(80)
            .with_jitter(30)
            .with_packet_loss(0.02)
    }

    /// 差勁連線（200ms 延遲，±50ms 抖動，10% 丟包）
    pub fn poor_connection() -> Self {
        Self::new_with_seed(42)
            .with_latency(200)
            .with_jitter(50)
            .with_packet_loss(0.10)
    }
}

impl BurstLossSimulator {
    /// 建立新的 simulator（無 active burst）
    pub fn new() -> Self {
        Self {
            burst_start_tick: None,
            burst_duration_ticks: 0,
        }
    }

    /// 在指定 tick 開始 burst 丟包
    pub fn start_burst(&mut self, at_tick: u64, duration_ticks: u64) {
        self.burst_start_tick = Some(at_tick);
        self.burst_duration_ticks = duration_ticks;
    }

    /// 當前 tick 是否在 burst 期間
    pub fn is_in_burst(&self, current_tick: u64) -> bool {
        if let Some(start) = self.burst_start_tick {
            self.burst_duration_ticks > 0
                && current_tick >= start
                && current_tick < start + self.burst_duration_ticks
        } else {
            false
        }
    }
}

impl Default for BurstLossSimulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 基本功能測試 ===

    #[test]
    fn test_latency_delays_packet() {
        let mut c = NetworkCondition::new_with_seed(42).with_latency(100);
        let pkt = c.process(vec![1, 2, 3], 0);
        assert!(!pkt.dropped, "latency 模式不應丟棄 packet");
        assert!(
            pkt.deliver_at_ms >= 100,
            "100ms 延遲：deliver_at={}",
            pkt.deliver_at_ms
        );
    }

    #[test]
    fn test_jitter_varies_delay() {
        let mut c = NetworkCondition::new_with_seed(42)
            .with_latency(100)
            .with_jitter(30);
        let pkts: Vec<_> = (0..100).map(|i| c.process(vec![i as u8], i * 10)).collect();
        let delays: Vec<i64> = pkts
            .iter()
            .enumerate()
            .map(|(i, p)| p.deliver_at_ms as i64 - (i as i64 * 10))
            .collect();
        let min_delay = *delays.iter().min().unwrap();
        let max_delay = *delays.iter().max().unwrap();
        assert!(
            max_delay > min_delay,
            "jitter 應造成延遲變化：min={min_delay}, max={max_delay}"
        );
        assert!(
            min_delay >= 70,
            "延遲不應低於 latency - jitter = 70ms：{min_delay}"
        );
    }

    #[test]
    fn test_jitter_clamp_non_negative() {
        let mut c = NetworkCondition::new_with_seed(42)
            .with_latency(10)
            .with_jitter(50);
        for i in 0..100u64 {
            let pkt = c.process(vec![i as u8], i * 10);
            assert!(
                pkt.deliver_at_ms >= i * 10,
                "deliver_at_ms ({}) 不應早於 send_time_ms ({})",
                pkt.deliver_at_ms,
                i * 10
            );
        }
    }

    #[test]
    fn test_packet_loss_drops_approximately_5_percent() {
        let mut c = NetworkCondition::new_with_seed(42).with_packet_loss(0.05);
        let total = 10_000;
        let dropped = (0..total)
            .filter(|i| c.process(vec![*i as u8], *i as u64 * 16).dropped)
            .count();
        // 5% +/- 2%
        assert!(
            dropped >= 300 && dropped <= 700,
            "預期 ~5% 丟包，實際 {dropped}/{total}"
        );
    }

    #[test]
    fn test_reorder_creates_out_of_order_packets() {
        let mut c = NetworkCondition::new_with_seed(42)
            .with_latency(100)
            .with_reorder(0.10);
        let pkts: Vec<_> = (0..100)
            .map(|i| c.process(vec![i as u8], i as u64 * 16))
            .collect();
        let deliver_times: Vec<u64> = pkts.iter().map(|p| p.deliver_at_ms).collect();
        let has_reorder = deliver_times.windows(2).any(|w| w[0] > w[1]);
        assert!(has_reorder, "10% 亂序率應產生至少一個亂序 packet");
    }

    #[test]
    fn test_chaining_all_effects() {
        let mut c = NetworkCondition::new_with_seed(42)
            .with_latency(100)
            .with_jitter(30)
            .with_packet_loss(0.05);
        for i in 0..1000u64 {
            let _ = c.process(vec![1, 2, 3], i * 16);
        }
    }

    // === Burst loss 測試 ===

    #[test]
    fn test_burst_loss_covers_duration() {
        let mut burst = BurstLossSimulator::new();
        burst.start_burst(10, 5);
        for tick in 10..15 {
            assert!(burst.is_in_burst(tick), "tick {} 應在 burst 期間內", tick);
        }
        assert!(!burst.is_in_burst(15), "tick 15 應在 burst 期間外");
    }

    #[test]
    fn test_burst_loss_no_active_burst() {
        let burst = BurstLossSimulator::new();
        assert!(!burst.is_in_burst(0));
        assert!(!burst.is_in_burst(100));
    }

    #[test]
    fn test_burst_loss_zero_duration() {
        let mut burst = BurstLossSimulator::new();
        burst.start_burst(10, 0);
        assert!(!burst.is_in_burst(10), "duration=0 的 burst 不應丟包");
    }

    // === Edge case 測試 ===

    #[test]
    fn test_100_percent_loss_drops_all() {
        let mut c = NetworkCondition::new_with_seed(42).with_packet_loss(1.0);
        let all_dropped = (0..100).all(|i| c.process(vec![i as u8], i as u64 * 16).dropped);
        assert!(all_dropped, "100% 丟包率：所有 packet 都應被丟棄");
    }

    #[test]
    fn test_zero_latency_delivers_immediately() {
        let mut c = NetworkCondition::new_with_seed(42);
        let pkt = c.process(vec![1, 2, 3], 1000);
        assert_eq!(pkt.deliver_at_ms, 1000, "0 延遲應立即送達");
    }

    #[test]
    fn test_empty_packet_no_panic() {
        let mut c = NetworkCondition::new_with_seed(42)
            .with_latency(50)
            .with_packet_loss(0.1);
        let pkt = c.process(vec![], 0);
        assert!(pkt.data.is_empty(), "空 packet data 應保持空");
    }

    #[test]
    fn test_zero_packet_loss_delivers_all() {
        let mut c = NetworkCondition::new_with_seed(42).with_packet_loss(0.0);
        let all_delivered = (0..100).all(|i| !c.process(vec![i as u8], i as u64 * 16).dropped);
        assert!(all_delivered, "0% 丟包率：所有 packet 都應傳遞");
    }

    // === 確定性驗證測試 ===

    #[test]
    fn test_deterministic_same_seed_same_result() {
        let run = |seed: u64| -> Vec<(u64, bool)> {
            let mut c = NetworkCondition::new_with_seed(seed)
                .with_latency(100)
                .with_jitter(30)
                .with_packet_loss(0.05)
                .with_reorder(0.10);
            (0..200)
                .map(|i| {
                    let pkt = c.process(vec![i as u8], i as u64 * 16);
                    (pkt.deliver_at_ms, pkt.dropped)
                })
                .collect()
        };
        let results_a = run(42);
        let results_b = run(42);
        assert_eq!(results_a, results_b, "相同 seed 必須產生完全相同的結果序列");
    }

    #[test]
    fn test_deterministic_different_seed_different_result() {
        let run = |seed: u64| -> Vec<(u64, bool)> {
            let mut c = NetworkCondition::new_with_seed(seed)
                .with_latency(100)
                .with_jitter(30)
                .with_packet_loss(0.05);
            (0..100)
                .map(|i| {
                    let pkt = c.process(vec![i as u8], i as u64 * 16);
                    (pkt.deliver_at_ms, pkt.dropped)
                })
                .collect()
        };
        let results_a = run(1);
        let results_b = run(2);
        assert_ne!(results_a, results_b, "不同 seed 應產生不同結果序列");
    }

    // === Preset 測試 ===

    #[test]
    fn test_presets_do_not_panic() {
        for preset_fn in [
            NetworkCondition::perfect,
            NetworkCondition::jitter_only,
            NetworkCondition::loss_5pct,
            NetworkCondition::loss_20pct,
            NetworkCondition::reorder_10pct,
            NetworkCondition::good_wifi,
            NetworkCondition::mobile_4g,
            NetworkCondition::poor_connection,
        ] {
            let mut c = preset_fn();
            for i in 0..10u64 {
                let _ = c.process(vec![1], i * 16);
            }
        }
    }

    #[test]
    fn test_preset_perfect_no_loss_no_delay() {
        let mut c = NetworkCondition::perfect();
        for i in 0..100u64 {
            let pkt = c.process(vec![1], i * 16);
            assert!(!pkt.dropped, "perfect preset 不應丟包");
            assert_eq!(pkt.deliver_at_ms, i * 16, "perfect preset 不應延遲");
        }
    }
}
