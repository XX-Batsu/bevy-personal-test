//! 腳本效能監控與自動停用模組
//!
//! 提供兩大功能：
//! - [`DiagnosticsResource`] / [`ScriptDiagnostics`]：腳本執行統計（ops、時間、超時/錯誤累計）
//! - [`AutoDisableManager`]：連續超時自動停用管理（閾值 10 次）
//!
//! # 設計依據
//! - diagnostics.md §ScriptDiagnostics / §DiagnosticsResource
//! - auto-disable.md §介面定義 / §不變式 #1~#7
//! - README.md Determinism Rules：BTreeMap 確保確定性迭代順序
//!
//! # f64 隔離
//! `exec_time_ms` / `current_frame_exec_time_ms` 等 f64 欄位僅用於診斷輸出，
//! 不參與 game logic 或 state hash 計算（diagnostics.md 不變式 #4）。

use std::collections::BTreeMap;

use deterministic::clock::Clock;

use crate::fallback::DisableReason;
use crate::ops_cost::{FrameOpsEntry, FrameOpsMetric};

// ═══════════════════════════════════════════════════════════════════════
// DiagnosticsResource + ScriptDiagnostics
// ═══════════════════════════════════════════════════════════════════════

/// 單一腳本的每幀執行統計
///
/// 權威定義：diagnostics.md §ScriptDiagnostics
/// 注意：不含 consecutive_timeouts / disabled（屬 AutoDisableManager 職責）
#[derive(Debug, Clone)]
pub struct ScriptDiagnostics {
    /// 腳本唯一識別碼
    pub script_id: String,
    /// 本幀消耗的 Rhai ops 數（Rhai 引擎層）
    pub rhai_ops: u64,
    /// 本幀消耗的 Bridge ops 數（Bridge API 層）
    pub bridge_ops: u64,
    /// 本幀執行時間（毫秒）
    /// 注意：f64 僅用於診斷輸出，不參與 game logic / state hash
    pub exec_time_ms: f64,
    /// 本幀超時次數（2ms 硬上限觸發）
    pub timeout_count: u32,
    /// 本幀錯誤次數（non-timeout runtime error）
    pub error_count: u32,
}

/// 全域腳本效能統計資源（Bevy Resource）
///
/// 權威定義：diagnostics.md §DiagnosticsResource
/// 儲存全部腳本的累計統計與最近 60 幀歷史。
/// Debug build 時可在 console 查看；Release build 僅在 auto-disable 時上報 Server。
pub struct DiagnosticsResource {
    /// 各腳本的累計超時次數（BTreeMap 確保確定性迭代順序）
    pub script_timeout_counts: BTreeMap<String, u32>,
    /// 各腳本的累計錯誤次數
    pub script_error_counts: BTreeMap<String, u32>,
    /// 每幀 ops 統計歷史（ring buffer，由 Phase 7 task-05a 定義）
    pub frame_ops_metric: FrameOpsMetric,
    /// 本幀腳本總執行時間（毫秒）
    pub current_frame_exec_time_ms: f64,
    /// 本幀所有腳本 ops 總數（Rhai 層）
    pub current_frame_rhai_ops: u64,
}

impl DiagnosticsResource {
    /// 建立空的診斷資源
    pub fn new() -> Self {
        Self {
            script_timeout_counts: BTreeMap::new(),
            script_error_counts: BTreeMap::new(),
            frame_ops_metric: FrameOpsMetric::new(),
            current_frame_exec_time_ms: 0.0,
            current_frame_rhai_ops: 0,
        }
    }

    /// 記錄一次腳本 callback 的統計
    ///
    /// 更新 frame_ops_metric（委派至 FrameOpsMetric::record）和當前幀累計值（current_frame_*）。
    /// 同幀多次呼叫會累加 current_frame_* 值。
    pub fn record_script_execution(
        &mut self,
        frame: u64,
        script_id: &str,
        rhai_ops: u64,
        bridge_ops: u64,
        time_ms: f64,
    ) {
        // 委派至 FrameOpsMetric::record()（Phase 7 task-05a 實作）
        self.frame_ops_metric
            .record(frame, script_id.to_string(), rhai_ops, bridge_ops, time_ms);
        // 累加當前幀統計
        self.current_frame_rhai_ops += rhai_ops;
        self.current_frame_exec_time_ms += time_ms;

        tracing::debug!(
            "腳本 {} 執行統計：rhai_ops={}, bridge_ops={}, 耗時={:.3}ms",
            script_id,
            rhai_ops,
            bridge_ops,
            time_ms
        );
    }

    /// 記錄一次超時事件
    ///
    /// 遞增 script_timeout_counts[script_id]。若 key 不存在則自動建立（初始值 1）。
    pub fn record_timeout(&mut self, script_id: &str) {
        *self
            .script_timeout_counts
            .entry(script_id.to_string())
            .or_insert(0) += 1;
        tracing::debug!(
            "腳本 {} 超時，累計超時次數：{}",
            script_id,
            self.script_timeout_counts[script_id]
        );
    }

    /// 記錄一次錯誤事件
    ///
    /// 遞增 script_error_counts[script_id]。若 key 不存在則自動建立（初始值 1）。
    pub fn record_error(&mut self, script_id: &str) {
        *self
            .script_error_counts
            .entry(script_id.to_string())
            .or_insert(0) += 1;
        tracing::debug!(
            "腳本 {} 錯誤，累計錯誤次數：{}",
            script_id,
            self.script_error_counts[script_id]
        );
    }

    /// 重置本幀統計（每幀開始時呼叫）
    ///
    /// 僅重置 current_frame_* 累計值，不清空 frame_ops_metric 歷史（不變式 #6）。
    /// frame 參數保留供未來 audit 用途。
    pub fn reset_frame(&mut self, _frame: u64) {
        self.current_frame_exec_time_ms = 0.0;
        self.current_frame_rhai_ops = 0;
        // 不清空 frame_ops_metric 歷史（不變式 #6）
        // 不清空 script_timeout_counts / script_error_counts（累計計數器）
    }

    /// 取得最近 n 幀的歷史統計
    ///
    /// 委派至 FrameOpsMetric::history(n)。若 n > 歷史長度，返回所有可用歷史。
    pub fn recent_history(&mut self, n: usize) -> &[FrameOpsEntry] {
        self.frame_ops_metric.history(n)
    }
}

impl Default for DiagnosticsResource {
    fn default() -> Self {
        Self::new()
    }
}

/// 計時包裝函式：以 Clock trait 量測腳本執行時間
///
/// execute_fn 回傳 (rhai_ops, bridge_ops, timed_out, had_error)。
/// 計時單位為微秒（Clock::now_micros），轉換為 f64 毫秒後存入 DiagnosticsResource。
pub fn record_script_timing<C: Clock>(
    diagnostics: &mut DiagnosticsResource,
    frame: u64,
    script_id: &str,
    clock: &C,
    execute_fn: impl FnOnce() -> (u64, u64, bool, bool),
) -> f64 {
    let start = clock.now_micros();
    let (rhai_ops, bridge_ops, timed_out, had_error) = execute_fn();
    let elapsed_us = clock.now_micros().saturating_sub(start);
    let elapsed_ms = elapsed_us as f64 / 1000.0;

    diagnostics.record_script_execution(frame, script_id, rhai_ops, bridge_ops, elapsed_ms);

    if timed_out {
        diagnostics.record_timeout(script_id);
    }
    if had_error {
        diagnostics.record_error(script_id);
    }

    elapsed_ms
}

// ═══════════════════════════════════════════════════════════════════════
// AutoDisableManager
// ═══════════════════════════════════════════════════════════════════════

/// 腳本自動停用狀態
///
/// 權威定義見 auto-disable.md §ScriptState
#[derive(Debug, Clone, PartialEq)]
pub enum AutoDisableState {
    /// 正常執行中
    Active,
    /// 已停用（tick: 觸發停用的幀號）
    Disabled { reason: DisableReason, tick: u64 },
}

/// 連續超時自動停用管理器
///
/// 不變式遵循：auto-disable.md §不變式 #1~#7
/// - #1: record_success() 歸零計數器
/// - #2: record_skipped() 不修改計數器
/// - #3: 停用僅能透過 re_enable() 恢復
/// - #4: 已停用腳本 record_timeout 不遞增
pub struct AutoDisableManager {
    /// 連續超時計數器（BTreeMap 確保確定性，README.md）
    timeout_counters: BTreeMap<String, u32>,
    /// 各腳本的停用狀態
    script_statuses: BTreeMap<String, AutoDisableState>,
    /// 連續超時停用閾值
    disable_threshold: u32,
}

impl AutoDisableManager {
    /// 預設停用閾值：連續 10 次超時
    pub const DEFAULT_DISABLE_THRESHOLD: u32 = 10;

    /// 建立空的自動停用管理器
    pub fn new() -> Self {
        Self {
            timeout_counters: BTreeMap::new(),
            script_statuses: BTreeMap::new(),
            disable_threshold: Self::DEFAULT_DISABLE_THRESHOLD,
        }
    }

    /// 記錄一次超時事件
    ///
    /// 已 Disabled → 不遞增，返回 false（不變式 #4）。
    /// 達 threshold → 停用，返回 true。
    pub fn record_timeout(&mut self, script_id: &str, reason: DisableReason, tick: u64) -> bool {
        // 不變式 #4：已停用腳本不遞增計數器
        if self.is_disabled(script_id) {
            return false;
        }

        let counter = self
            .timeout_counters
            .entry(script_id.to_string())
            .or_insert(0);
        *counter += 1;

        if *counter >= self.disable_threshold {
            self.script_statuses.insert(
                script_id.to_string(),
                AutoDisableState::Disabled { reason, tick },
            );
            tracing::warn!(
                script_id = %script_id,
                counter = *counter,
                tick = tick,
                "腳本連續超時達到閾值，已自動停用"
            );
            true
        } else {
            false
        }
    }

    /// 記錄一次成功執行
    ///
    /// 不變式 #1：歸零計數器。已 Disabled 時不恢復狀態（不變式 #3）。
    pub fn record_success(&mut self, script_id: &str) {
        // 不變式 #3：已停用腳本 record_success 不恢復
        if self.is_disabled(script_id) {
            return;
        }
        // 不變式 #1：成功執行歸零連續超時計數器
        self.timeout_counters.insert(script_id.to_string(), 0);
    }

    /// 記錄一次跳過事件（幀預算耗盡）
    ///
    /// 不變式 #2：不修改計數器（不歸零也不遞增）。
    pub fn record_skipped(&mut self, _script_id: &str) {
        // 空操作：跳過不等於超時，不影響連續超時計數
    }

    /// 查詢腳本停用狀態
    ///
    /// 未知 script_id → &AutoDisableState::Active（安全預設值）
    pub fn status(&self, script_id: &str) -> &AutoDisableState {
        // 使用靜態常數避免 lifetime 問題
        const ACTIVE: AutoDisableState = AutoDisableState::Active;
        self.script_statuses.get(script_id).unwrap_or(&ACTIVE)
    }

    /// 便利方法：檢查腳本是否已停用
    ///
    /// 等同 matches!(status, Disabled { .. })，未知 → false
    pub fn is_disabled(&self, script_id: &str) -> bool {
        matches!(self.status(script_id), AutoDisableState::Disabled { .. })
    }

    /// 重新啟用已停用的腳本
    ///
    /// 不變式 #3：唯一恢復路徑。重置 statuses + counters。
    /// 對已 Active 或未知腳本冪等操作。
    pub fn re_enable(&mut self, script_id: &str) {
        self.script_statuses
            .insert(script_id.to_string(), AutoDisableState::Active);
        self.timeout_counters.insert(script_id.to_string(), 0);
    }
}

impl Default for AutoDisableManager {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 測試
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // ─── DiagnosticsResource::record_script_execution 測試 ───

    #[test]
    fn test_record_execution_updates_frame_ops() {
        // 記錄一次執行後，frame_ops_metric 有對應 entry
        let mut res = DiagnosticsResource::new();
        res.record_script_execution(1, "skill_vfx", 500, 200, 0.8);
        let history = res.recent_history(1);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].total_rhai_ops, 500);
        assert_eq!(history[0].total_bridge_ops, 200);
        assert!((history[0].time_ms - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_record_execution_accumulates_current_frame() {
        // 同幀多次記錄，current_frame 累計值正確
        let mut res = DiagnosticsResource::new();
        res.record_script_execution(1, "script_a", 300, 100, 0.5);
        res.record_script_execution(1, "script_b", 200, 80, 0.3);
        assert_eq!(res.current_frame_rhai_ops, 500); // 300 + 200
        assert!((res.current_frame_exec_time_ms - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_record_execution_dual_ops_tracked() {
        // 驗證 rhai_ops 與 bridge_ops 雙軌分離追蹤
        let mut res = DiagnosticsResource::new();
        res.record_script_execution(1, "script_a", 1000, 500, 1.0);
        let history = res.recent_history(1);
        assert_eq!(history[0].total_rhai_ops, 1000);
        assert_eq!(history[0].total_bridge_ops, 500);
        // per_script 四元組驗證：(script_id, rhai_ops, bridge_ops, time_ms)
        assert_eq!(history[0].per_script[0].1, 1000); // rhai_ops
        assert_eq!(history[0].per_script[0].2, 500); // bridge_ops
        assert!((history[0].per_script[0].3 - 1.0).abs() < 0.001); // time_ms
    }

    // ─── DiagnosticsResource::record_timeout 測試 ───

    #[test]
    fn test_record_timeout_increments_counter() {
        // 超時事件遞增累計計數器
        let mut res = DiagnosticsResource::new();
        res.record_timeout("slow_script");
        res.record_timeout("slow_script");
        res.record_timeout("slow_script");
        assert_eq!(res.script_timeout_counts["slow_script"], 3);
    }

    #[test]
    fn test_record_timeout_unknown_script_creates_entry() {
        // 對未知 script_id 記錄超時，自動建立 entry
        let mut res = DiagnosticsResource::new();
        assert!(!res.script_timeout_counts.contains_key("new_script"));
        res.record_timeout("new_script");
        assert_eq!(res.script_timeout_counts["new_script"], 1);
    }

    // ─── DiagnosticsResource::record_error 測試 ───

    #[test]
    fn test_record_error_increments_counter() {
        // 錯誤事件遞增累計計數器
        let mut res = DiagnosticsResource::new();
        res.record_error("buggy");
        res.record_error("buggy");
        assert_eq!(res.script_error_counts["buggy"], 2);
    }

    #[test]
    fn test_record_error_isolated_from_timeout() {
        // 錯誤與超時計數器獨立，互不影響
        let mut res = DiagnosticsResource::new();
        res.record_timeout("script_a");
        res.record_error("script_a");
        assert_eq!(res.script_timeout_counts["script_a"], 1);
        assert_eq!(res.script_error_counts["script_a"], 1);
    }

    // ─── DiagnosticsResource::reset_frame 測試 ───

    #[test]
    fn test_reset_frame_clears_current_frame_stats() {
        // reset_frame 僅重置 current_frame_* 累計值
        let mut res = DiagnosticsResource::new();
        res.record_script_execution(1, "script_a", 300, 100, 0.5);
        assert!(res.current_frame_rhai_ops > 0);
        res.reset_frame(2);
        assert_eq!(res.current_frame_rhai_ops, 0);
        assert!((res.current_frame_exec_time_ms).abs() < 0.001);
    }

    #[test]
    fn test_reset_frame_preserves_history() {
        // reset_frame 不清空 frame_ops_metric 歷史（不變式 #6）
        let mut res = DiagnosticsResource::new();
        res.record_script_execution(1, "script_a", 300, 100, 0.5);
        res.reset_frame(2);
        let history = res.recent_history(10);
        assert_eq!(history.len(), 1); // frame 1 的歷史仍保留
        assert_eq!(history[0].frame, 1);
    }

    #[test]
    fn test_reset_frame_preserves_timeout_error_counts() {
        // reset_frame 不清空累計的超時/錯誤計數器
        let mut res = DiagnosticsResource::new();
        res.record_timeout("script_a");
        res.record_error("script_a");
        res.reset_frame(2);
        assert_eq!(res.script_timeout_counts["script_a"], 1);
        assert_eq!(res.script_error_counts["script_a"], 1);
    }

    // ─── DiagnosticsResource::recent_history 測試 ───

    #[test]
    fn test_recent_history_empty() {
        // 無記錄時回傳空 slice
        let mut res = DiagnosticsResource::new();
        assert_eq!(res.recent_history(10).len(), 0);
    }

    #[test]
    fn test_recent_history_truncates() {
        // 請求超過可用歷史時，回傳所有可用的
        let mut res = DiagnosticsResource::new();
        res.record_script_execution(1, "s", 100, 50, 0.1);
        res.record_script_execution(2, "s", 100, 50, 0.1);
        let history = res.recent_history(10);
        assert_eq!(history.len(), 2);
    }

    // ─── 多腳本隔離 ───

    #[test]
    fn test_multiple_scripts_isolated() {
        // 多腳本各自獨立，不互相影響
        let mut res = DiagnosticsResource::new();
        res.record_timeout("script_a");
        res.record_timeout("script_a");
        res.record_error("script_b");
        assert_eq!(res.script_timeout_counts.get("script_a"), Some(&2));
        assert_eq!(res.script_timeout_counts.get("script_b"), None);
        assert_eq!(res.script_error_counts.get("script_a"), None);
        assert_eq!(res.script_error_counts.get("script_b"), Some(&1));
    }

    // ─── 確定性驗證 ───

    #[test]
    fn test_btreemap_ordering_timeout_counts() {
        // BTreeMap 保證字典序排序（確定性要求）
        let mut res = DiagnosticsResource::new();
        res.record_timeout("z_script");
        res.record_timeout("a_script");
        res.record_timeout("m_script");
        let ids: Vec<&String> = res.script_timeout_counts.keys().collect();
        assert_eq!(ids, vec!["a_script", "m_script", "z_script"]);
    }

    #[test]
    fn test_btreemap_ordering_error_counts() {
        // script_error_counts 同樣保證字典序
        let mut res = DiagnosticsResource::new();
        res.record_error("z_script");
        res.record_error("a_script");
        let ids: Vec<&String> = res.script_error_counts.keys().collect();
        assert_eq!(ids, vec!["a_script", "z_script"]);
    }

    // ─── f64 時間精度 ───

    #[test]
    fn test_time_ms_accumulation_precision() {
        // 同幀多次記錄的 time_ms 累加精度
        let mut res = DiagnosticsResource::new();
        for _ in 0..100 {
            res.record_script_execution(1, "s", 10, 5, 0.01);
        }
        // 100 × 0.01 = 1.0，允許浮點誤差 < 0.001
        assert!((res.current_frame_exec_time_ms - 1.0).abs() < 0.001);
    }

    // ═══════════════════════════════════════════════════════════════
    // record_script_timing 測試
    // ═══════════════════════════════════════════════════════════════

    /// 測試用 MockClock：依序回傳預設的時間戳序列
    struct MockClock {
        /// 呼叫次數計數器
        call_count: AtomicU64,
        /// 每次呼叫回傳的時間戳序列
        timestamps: Vec<u64>,
    }

    impl MockClock {
        fn new(timestamps: Vec<u64>) -> Self {
            Self {
                call_count: AtomicU64::new(0),
                timestamps,
            }
        }
    }

    impl Clock for MockClock {
        fn now_micros(&self) -> u64 {
            let idx = self.call_count.fetch_add(1, Ordering::Relaxed) as usize;
            if idx < self.timestamps.len() {
                self.timestamps[idx]
            } else {
                // 超出範圍時回傳最後一個值
                *self.timestamps.last().unwrap_or(&0)
            }
        }
    }

    #[test]
    fn test_record_timing_uses_clock_micros_to_ms() {
        // 注入 mock Clock（now_micros 回傳 1000, 3500），elapsed = 2500μs = 2.5ms
        let clock = MockClock::new(vec![1000, 3500]);
        let mut diag = DiagnosticsResource::new();
        let elapsed = record_script_timing(&mut diag, 1, "test_script", &clock, || {
            (100, 50, false, false)
        });
        assert!((elapsed - 2.5).abs() < 0.001);
        assert!((diag.current_frame_exec_time_ms - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_record_timing_calls_record_timeout_on_timeout() {
        let clock = MockClock::new(vec![0, 500]);
        let mut diag = DiagnosticsResource::new();
        record_script_timing(&mut diag, 1, "slow", &clock, || (0, 0, true, false));
        assert_eq!(diag.script_timeout_counts["slow"], 1);
    }

    #[test]
    fn test_record_timing_calls_record_error_on_error() {
        let clock = MockClock::new(vec![0, 500]);
        let mut diag = DiagnosticsResource::new();
        record_script_timing(&mut diag, 1, "buggy", &clock, || (0, 0, false, true));
        assert_eq!(diag.script_error_counts["buggy"], 1);
    }

    #[test]
    fn test_record_timing_timeout_and_error_independent() {
        let clock = MockClock::new(vec![0, 500]);
        let mut diag = DiagnosticsResource::new();
        record_script_timing(&mut diag, 1, "both", &clock, || (0, 0, true, true));
        assert_eq!(diag.script_timeout_counts["both"], 1);
        assert_eq!(diag.script_error_counts["both"], 1);
    }

    #[test]
    fn test_record_timing_returns_elapsed_ms() {
        // mock Clock 差值 500μs = 0.5ms
        let clock = MockClock::new(vec![1000, 1500]);
        let mut diag = DiagnosticsResource::new();
        let elapsed = record_script_timing(&mut diag, 1, "s", &clock, || (10, 5, false, false));
        assert!((elapsed - 0.5).abs() < 0.001);
    }

    // ═══════════════════════════════════════════════════════════════
    // AutoDisableManager 測試
    // ═══════════════════════════════════════════════════════════════

    /// 輔助：建立 mgr 並連續 record_timeout n 次
    fn timeout_n(mgr: &mut AutoDisableManager, id: &str, n: u32) {
        for i in 0..n {
            mgr.record_timeout(id, DisableReason::Timeout, i as u64);
        }
    }

    // ─── 閾值邊界 ───

    #[test]
    fn test_nine_consecutive_timeouts_not_disabled() {
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        for i in 0..(t - 1) {
            assert!(!mgr.record_timeout("a", DisableReason::Timeout, i as u64));
        }
        assert!(!mgr.is_disabled("a"));
        assert_eq!(mgr.status("a"), &AutoDisableState::Active);
    }

    #[test]
    fn test_ten_consecutive_timeouts_auto_disabled() {
        let mut mgr = AutoDisableManager::new();
        timeout_n(&mut mgr, "b", AutoDisableManager::DEFAULT_DISABLE_THRESHOLD);
        assert!(mgr.is_disabled("b"));
        assert!(matches!(
            mgr.status("b"),
            &AutoDisableState::Disabled {
                reason: DisableReason::Timeout,
                ..
            }
        ));
    }

    #[test]
    fn test_tenth_timeout_returns_true() {
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        timeout_n(&mut mgr, "x", t - 1);
        assert!(mgr.record_timeout("x", DisableReason::Timeout, (t - 1) as u64));
    }

    // ─── 計數器重置 ───

    #[test]
    fn test_success_resets_consecutive_counter() {
        // 不變式 #1：record_success() 歸零
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        timeout_n(&mut mgr, "c", t - 1);
        mgr.record_success("c");
        assert!(!mgr.is_disabled("c"));
        assert_eq!(mgr.status("c"), &AutoDisableState::Active);
        // 重置後再 9 次仍不停用
        timeout_n(&mut mgr, "c", t - 1);
        assert!(!mgr.is_disabled("c"));
    }

    // ─── 跳過不等於超時 ───

    #[test]
    fn test_skipped_does_not_increment_counter() {
        // 不變式 #2：跳過 20 次不觸發停用
        let mut mgr = AutoDisableManager::new();
        for _ in 0..20 {
            mgr.record_skipped("e");
        }
        assert!(!mgr.is_disabled("e"));
    }

    #[test]
    fn test_skipped_preserves_existing_timeout_count() {
        // 5 次超時 → 3 次跳過 → 5 次超時 = 累計 10 次（跳過不歸零也不遞增）
        let mut mgr = AutoDisableManager::new();
        for i in 0..5u64 {
            mgr.record_timeout("f", DisableReason::Timeout, i);
        }
        for _ in 0..3 {
            mgr.record_skipped("f");
        }
        for i in 5..10u64 {
            mgr.record_timeout("f", DisableReason::Timeout, i);
        }
        assert!(mgr.is_disabled("f"), "跳過不中斷連續超時序列");
    }

    // ─── 停用後行為 ───

    #[test]
    fn test_disabled_does_not_increment_after_disable() {
        // 不變式 #4：已停用腳本 record_timeout 返回 false，狀態不變
        let mut mgr = AutoDisableManager::new();
        timeout_n(&mut mgr, "d", AutoDisableManager::DEFAULT_DISABLE_THRESHOLD);
        assert!(!mgr.record_timeout("d", DisableReason::Timeout, 100));
        assert!(mgr.is_disabled("d"));
    }

    #[test]
    fn test_disabled_tick_preserved() {
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        for i in 0..t {
            mgr.record_timeout("tk", DisableReason::Timeout, 42 + i as u64);
        }
        if let AutoDisableState::Disabled { tick, .. } = mgr.status("tk") {
            assert_eq!(*tick, 42 + (t - 1) as u64);
        } else {
            panic!("應為 Disabled");
        }
    }

    // ─── re_enable ───

    #[test]
    fn test_re_enable_clears_disabled_state() {
        // 不變式 #3：停用僅能透過 re_enable() 恢復
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        timeout_n(&mut mgr, "re", t);
        assert!(mgr.is_disabled("re"));
        mgr.re_enable("re");
        assert!(!mgr.is_disabled("re"));
        assert_eq!(mgr.status("re"), &AutoDisableState::Active);
        // 計數器已重置：再 9 次不停用
        timeout_n(&mut mgr, "re", t - 1);
        assert!(!mgr.is_disabled("re"));
    }

    #[test]
    fn test_re_enable_idempotent_on_active() {
        let mut mgr = AutoDisableManager::new();
        mgr.record_success("act");
        mgr.re_enable("act");
        assert_eq!(mgr.status("act"), &AutoDisableState::Active);
    }

    // ─── 未知腳本邊界 ───

    #[test]
    fn test_is_disabled_returns_false_for_unknown_script() {
        let mgr = AutoDisableManager::new();
        assert!(!mgr.is_disabled("nonexistent"));
    }

    #[test]
    fn test_status_returns_active_for_unknown_script() {
        let mgr = AutoDisableManager::new();
        assert_eq!(mgr.status("nonexistent"), &AutoDisableState::Active);
    }

    // ─── DisableReason 路徑覆蓋 ───

    #[test]
    fn test_operation_limit_path_triggers_disable() {
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        for i in 0..t {
            mgr.record_timeout("ops", DisableReason::OperationLimit, i as u64);
        }
        assert!(mgr.is_disabled("ops"));
        assert!(matches!(
            mgr.status("ops"),
            &AutoDisableState::Disabled {
                reason: DisableReason::OperationLimit,
                ..
            }
        ));
    }

    // ─── 多腳本交叉停用 ───

    #[test]
    fn test_multi_script_independent_counters() {
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        for i in 0..t {
            mgr.record_timeout("a", DisableReason::Timeout, i as u64);
            if i < 5 {
                mgr.record_timeout("b", DisableReason::Timeout, i as u64);
            }
        }
        assert!(mgr.is_disabled("a"), "A 應停用");
        assert!(!mgr.is_disabled("b"), "B 不應停用（僅 5 次）");
    }

    #[test]
    fn test_multi_script_disable_one_reenable_other_unaffected() {
        let mut mgr = AutoDisableManager::new();
        let t = AutoDisableManager::DEFAULT_DISABLE_THRESHOLD;
        for i in 0..t {
            mgr.record_timeout("a", DisableReason::Timeout, i as u64);
            mgr.record_timeout("b", DisableReason::OperationLimit, i as u64);
        }
        mgr.re_enable("a");
        assert!(!mgr.is_disabled("a"), "A 已恢復");
        assert!(mgr.is_disabled("b"), "B 不受影響");
    }

    // ─── 停用不可逆 ───

    #[test]
    fn test_success_does_not_reenable_disabled_script() {
        // 不變式 #3：record_success 不恢復已停用腳本
        let mut mgr = AutoDisableManager::new();
        timeout_n(
            &mut mgr,
            "sticky",
            AutoDisableManager::DEFAULT_DISABLE_THRESHOLD,
        );
        mgr.record_success("sticky");
        assert!(mgr.is_disabled("sticky"));
    }
}
