//! Server 端抽樣 replay 驗證

use std::collections::BTreeSet;

use deterministic::DeterministicRng;
use replay_engine::{PlaybackMode, ReplayFile, ReplayPlayer, ReplayValidationResult};
use server_types::{DeterministicSimulation, SimulationError};

/// 抽樣 replay 驗證器
///
/// 決定哪些 session 需要執行 replay 驗證。
pub struct ReplaySampler {
    /// 確定性 RNG（非 thread_rng，抽樣行為可重現）
    rng: DeterministicRng,
    /// 全量驗證 session 集合（BTreeSet 確定性）
    full_validation_sessions: BTreeSet<u64>,
    /// 抽樣率：每 N 次中有 1 次返回 true（0 = 停用抽樣，永遠 false）
    ///
    /// private：執行期間修改此值會立即影響後續的抽樣結果，
    /// 可能造成語意不一致，故僅透過 `sample_rate()` getter 存取。
    sample_rate: u32,
}

impl ReplaySampler {
    pub fn new(seed: u64, sample_rate: u32) -> Self {
        Self {
            rng: DeterministicRng::seed_from_u64(seed),
            full_validation_sessions: BTreeSet::new(),
            sample_rate,
        }
    }

    /// 決定 session 是否需要 replay 驗證
    ///
    /// - `full_validation_sessions` 中的 session 永遠返回 true
    /// - sample_rate == 0：永遠返回 false（停用抽樣）
    /// - 一般 session：`roll.is_multiple_of(sample_rate)`
    ///
    /// **RNG 消耗一致性**：無論 session 是否在 `full_validation_sessions` 中，
    /// RNG 在每次呼叫時均會先消耗一次。此設計確保升級某 session 為全量驗證
    /// 前後，對其他 session 的抽樣序列不發生斷層，維持確定性。
    ///
    /// **已知限制**：此設計假設每個 session 在其生命週期內通常只被查詢一次。
    /// 若升級後的 session 被重複查詢（每次仍消耗 RNG），會與未升級的參照端
    /// 產生 RNG 消耗量差異，導致序列分叉。
    pub fn should_validate(&mut self, session_id: u64) -> bool {
        let roll = self.rng.next_u32();
        if self.full_validation_sessions.contains(&session_id) {
            return true;
        }
        if self.sample_rate == 0 {
            return false;
        }
        roll.is_multiple_of(self.sample_rate)
    }

    /// 取得目前的抽樣率設定
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 升級 session 為全量驗證（當其 client 被標記為可疑時呼叫）
    pub fn upgrade_to_full_validation(&mut self, session_id: u64) {
        self.full_validation_sessions.insert(session_id);
    }

    /// Replay 逐幀驗證入口
    ///
    /// `replay` 取 by-value：`ReplayPlayer::new()` 取得所有權，
    /// 由呼叫端傳入可省去 clone，符合 Rust 慣用。
    ///
    /// # TODO(replay-engine)
    /// 目前使用 ReplayPlayer 執行同步驗證，介面已定供呼叫端整合。
    /// 實作前需確認：
    /// 1. ReplayFile 結構完整度（server/replay_engine/src/recorder.rs）
    /// 2. 背景 worker 執行模型（Server 執行緒架構）
    ///
    /// 參考：docs/design/architecture/09-deterministic-replay/
    ///
    /// # 背景執行注意事項
    /// TODO(server-scaling): 最終需在背景 worker 執行，
    /// 不得阻塞 AuthoritativeSimulation::step_full() 主迴圈。
    pub fn validate_replay<S>(&self, replay: ReplayFile, sim: &mut S) -> ReplayValidationResult
    where
        S: DeterministicSimulation<Error = SimulationError>,
    {
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);
        player.run_full(sim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_validation_session_always_true() {
        let mut sampler = ReplaySampler::new(42, 100);
        sampler.upgrade_to_full_validation(99);
        // 重複呼叫多次應永遠 true
        for _ in 0..10 {
            assert!(sampler.should_validate(99));
        }
    }

    #[test]
    fn sample_rate_zero_always_false() {
        let mut sampler = ReplaySampler::new(42, 0);
        for session_id in 0..100 {
            assert!(!sampler.should_validate(session_id));
        }
    }

    #[test]
    fn sample_rate_one_always_true() {
        let mut sampler = ReplaySampler::new(42, 1);
        for session_id in 0..10 {
            assert!(sampler.should_validate(session_id));
        }
    }

    #[test]
    fn upgrade_to_full_validation_takes_effect() {
        let mut sampler = ReplaySampler::new(42, 0); // 停用抽樣
        assert!(!sampler.should_validate(5));
        sampler.upgrade_to_full_validation(5);
        assert!(sampler.should_validate(5));
    }

    #[test]
    fn deterministic_sampling_same_seed_same_result() {
        let mut s1 = ReplaySampler::new(12345, 10);
        let mut s2 = ReplaySampler::new(12345, 10);
        for session_id in 0..50 {
            assert_eq!(
                s1.should_validate(session_id),
                s2.should_validate(session_id)
            );
        }
    }

    // 全量 session 仍消耗 RNG（設計語意保證）
    #[test]
    fn full_validation_session_still_consumes_rng() {
        let mut s_full = ReplaySampler::new(42, 10);
        let mut s_ref = ReplaySampler::new(42, 10);
        s_full.upgrade_to_full_validation(999);
        // 兩者各對 session 999 呼叫一次（各消耗 1 次 RNG）
        assert!(s_full.should_validate(999)); // 全量，永遠 true
        s_ref.should_validate(999); // 普通路徑，也消耗 1 次 RNG
                                    // RNG 消耗量相同，後續其他 session 結果應一致
        for sid in 0..5u64 {
            assert_eq!(
                s_full.should_validate(sid),
                s_ref.should_validate(sid),
                "全量 session 的 RNG 消耗應與普通查詢一致"
            );
        }
    }

    // 記錄已知行為：升級後重複查詢同一 session 會消耗 RNG，導致與未升級端分叉
    #[test]
    fn rng_diverges_when_upgraded_session_queried_repeatedly() {
        let mut s1 = ReplaySampler::new(99999, 10);
        let mut s2 = ReplaySampler::new(99999, 10);
        // 兩者各查詢 session 100 一次（各消耗 1 次 RNG）
        s1.should_validate(100);
        s2.should_validate(100);
        s1.upgrade_to_full_validation(100);
        // s1 對 session 100 再查詢一次（額外消耗 1 次 RNG），s2 不查詢
        assert!(s1.should_validate(100));
        // 此後 s1 與 s2 的 RNG 消耗量差 1，後續序列應分叉（已知行為，非 bug）
        let mut diverged = false;
        for sid in 1..=20u64 {
            if s1.should_validate(sid) != s2.should_validate(sid) {
                diverged = true;
                break;
            }
        }
        assert!(
            diverged,
            "升級後重複查詢同一 session 應導致 RNG 分叉（已知行為）"
        );
    }

    /// 驗證將某 session 升級為全量驗證後，其他 session 的 RNG 序列不受影響。
    ///
    /// 原理：兩個 sampler 以相同 seed 初始化，s1 先查詢 session 100（尚未升級，
    /// 消耗 1 次 RNG），再將 session 100 升級為全量驗證。s2 也查詢 session 100
    /// 一次（消耗相同數量的 RNG）。之後兩者對其他 session 的查詢結果應完全一致，
    /// 因為 s1 對已升級 session 的後續呼叫雖然仍消耗 RNG，但兩個 sampler
    /// 只對 session 100 各呼叫一次（升級前），RNG 消耗量相同。
    #[test]
    fn rng_sequence_stable_across_upgrade() {
        let mut s1 = ReplaySampler::new(99999, 10);
        let mut s2 = ReplaySampler::new(99999, 10);

        // 兩個 sampler 都對 session 100 呼叫一次（此時 s1 尚未升級）
        // 兩者各消耗 1 次 RNG，結果應相同
        let r1 = s1.should_validate(100);
        let r2 = s2.should_validate(100);
        assert_eq!(r1, r2, "升級前兩個 sampler 對 session 100 結果應相同");

        // s1 升級 session 100 為全量驗證；s2 不升級
        s1.upgrade_to_full_validation(100);

        // 現在對其他 session 呼叫 10 次，比較兩個 sampler 結果
        // 兩者 RNG 狀態相同（各消耗了 1 次），後續序列應完全一致。
        // 注意：此迴圈選擇 0..10（排除 session 100）是刻意的——
        // 若包含 session 100，s1 因升級而永遠回傳 true，s2 走普通路徑，
        // 兩者回傳值本就不同，會使 assert_eq! 誤報為序列不一致。
        for session_id in 0..10u64 {
            assert_eq!(
                s1.should_validate(session_id),
                s2.should_validate(session_id),
                "升級 session 100 後，session {session_id} 的抽樣結果不應改變"
            );
        }
    }

    // ── sample_rate() getter ──

    #[test]
    fn sample_rate_getter_returns_configured_rate() {
        let s = ReplaySampler::new(42, 7);
        assert_eq!(s.sample_rate(), 7);
    }

    #[test]
    fn sample_rate_zero_getter() {
        let s = ReplaySampler::new(0, 0);
        assert_eq!(s.sample_rate(), 0);
    }

    // ── sample_rate=0 且 session 在 full_validation_sessions（組合測試）──

    #[test]
    fn full_validation_overrides_disabled_sample_rate() {
        // sample_rate=0 停用抽樣，但升級後的 session 應仍回傳 true
        let mut sampler = ReplaySampler::new(42, 0);
        sampler.upgrade_to_full_validation(77);
        // session 77（全量驗證）應永遠 true，即使 sample_rate=0
        assert!(sampler.should_validate(77));
        // 其他 session 仍為 false
        assert!(!sampler.should_validate(1));
        assert!(!sampler.should_validate(2));
    }

    // ── upgrade_to_full_validation 冪等性（多次 upgrade 同一 session 不 panic）──

    #[test]
    fn upgrade_idempotent_multiple_times() {
        let mut sampler = ReplaySampler::new(42, 10);
        sampler.upgrade_to_full_validation(5);
        sampler.upgrade_to_full_validation(5); // 重複升級不 panic
        assert!(sampler.should_validate(5));
    }

    // ── new() 初始狀態無任何 full_validation session ──

    #[test]
    fn new_sampler_has_no_full_validation_sessions() {
        // 任意 session 在未升級前不應因「全量驗證」路徑返回 true
        // （注意：sample_rate=1 時普通路徑也是 true，故用 sample_rate=0）
        let mut sampler = ReplaySampler::new(42, 0);
        for sid in 0..5u64 {
            assert!(
                !sampler.should_validate(sid),
                "新建 sampler 不應有任何預設的 full_validation session"
            );
        }
    }
}
