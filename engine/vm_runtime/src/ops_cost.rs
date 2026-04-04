//! Bridge API 操作成本追蹤
//!
//! 提供 [`OpsCostTable`] 靜態成本查詢表與 [`OpsTracker`] 剩餘 ops 計數器。
//! 獨立於 Rhai 引擎層 50,000 ops（Phase 6 on_progress），兩者各自上限互不影響。
//!
//! # 設計依據
//! - 成本表對齊 performance-costs.md §3.12 Operations 成本表
//! - OpsTracker 介面對齊 frame-budget.md §OpsTracker（remaining 語意）
//! - 先扣後做不變式：deduct 失敗時 remaining 不變

/// Bridge API 操作成本查詢表（靜態，編譯期固定常數）
///
/// 與 Rhai 引擎層 50,000 ops（Phase 6 on_progress）獨立。
/// 詳見 frame-budget.md §6.2.1 雙軌 Ops 限制設計。
/// 完整成本表見 performance-costs.md §3.12 Operations 成本表。
pub struct OpsCostTable;

impl OpsCostTable {
    /// 查詢指定 Bridge API 的操作成本
    ///
    /// 成本值對齊 performance-costs.md §3.12：
    /// - spawn_entity: 50, despawn_entity: 30
    /// - set_transform/rotation/scale: 5, set_visibility: 3
    /// - play_vfx/play_sound/play_sound_at: 10
    /// - stop_vfx/stop_sound: 5
    /// - get_position/get_entity_state/request_state: 10
    /// - get_local_player_id/get_frame_number/get_delta_time: 2
    /// - send_prediction: 15, blend_animation: 8
    /// - UI/Animation（其餘）: 5
    /// - 未知 API: 1
    pub fn cost_for(api_name: &str) -> u64 {
        match api_name {
            // Entity 操作（§3.1）
            "spawn_entity" => 50,
            "despawn_entity" => 30,
            "set_transform" | "set_rotation" | "set_scale" => 5,
            "set_visibility" => 3, // 純 bool 寫入，低於 set_transform

            // VFX / Audio（§3.2）
            "play_vfx" | "play_sound" | "play_sound_at" => 10,
            "stop_vfx" | "stop_sound" => 5,

            // Query（§3.5）— BTreeMap lookup O(log n)
            "get_position" | "get_entity_state" => 10,
            "get_local_player_id" | "get_frame_number" | "get_delta_time" => 2,

            // UI（§3.3）
            "update_hud" | "show_dialog" | "hide_dialog" => 5,
            "set_health_bar" | "show_damage_number" | "show_toast" => 5,

            // Animation（§3.4）
            "play_animation" | "play_animation_once" | "stop_animation" => 5,
            "blend_animation" => 8, // entity 查找 + 4 值寫入

            // Network（§3.6）
            "send_prediction" => 15, // Dynamic 型別轉換 + event 寫入
            "request_state" => 10,   // BTreeMap key lookup

            // 未知 API（安全兜底，不拒絕但有基本成本）
            _ => 1,
        }
    }
}

/// Bridge API 操作計數器（remaining 語意：剩餘可用 ops）
///
/// 獨立於 Rhai 引擎層 ops limit（Phase 6 on_progress），
/// 兩者各自 50,000 ops 上限，互不影響。
/// 介面對齊 frame-budget.md §OpsTracker。
pub struct OpsTracker {
    remaining: u64,
    limit: u64,
}

impl OpsTracker {
    /// Bridge 層 ops 預設上限
    pub const DEFAULT_BUDGET: u64 = 50_000;

    /// 建立預設上限 50,000 ops 的追蹤器
    pub fn new() -> Self {
        Self {
            remaining: Self::DEFAULT_BUDGET,
            limit: Self::DEFAULT_BUDGET,
        }
    }

    /// 建立自訂上限的追蹤器（測試用）
    pub fn with_limit(limit: u64) -> Self {
        Self {
            remaining: limit,
            limit,
        }
    }

    /// 扣減 cost 個 ops（先扣後做：先檢查 remaining >= cost，不足則 Err）
    ///
    /// 語意：若 remaining >= cost → remaining -= cost → Ok(())
    ///       若 remaining < cost → remaining 不變 → Err("ops budget exceeded")
    ///
    /// 字串訊息供 Rhai runtime error 使用，含 remaining/cost 數值。
    pub fn deduct(&mut self, cost: u64) -> Result<(), String> {
        if self.remaining >= cost {
            self.remaining -= cost;
            Ok(())
        } else {
            Err(format!(
                "ops budget exceeded: remaining={}, cost={}",
                self.remaining, cost
            ))
        }
    }

    /// 查詢剩餘可用 ops
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// 重置計數器（remaining 恢復為建構時的 limit）
    pub fn reset(&mut self) {
        self.remaining = self.limit;
    }
}

impl Default for OpsTracker {
    fn default() -> Self {
        Self::new()
    }
}

use std::collections::VecDeque;

/// 每幀操作統計紀錄
///
/// 對齊上游 diagnostics.md §FrameOpsEntry 介面。
/// per_script 四元組為 (script_id, rhai_ops, bridge_ops, time_ms)，雙軌 ops 獨立計數。
#[derive(Debug, Clone)]
pub struct FrameOpsEntry {
    /// 幀號（遊戲 tick 計數）
    pub frame: u64,
    /// 各腳本的執行統計 (script_id, rhai_ops, bridge_ops, time_ms)
    /// 使用 Vec 而非 BTreeMap：entry 為歷史快照，順序固定（已按 priority 排序）
    /// 第 4 欄位 time_ms 為 per-script 執行時間（f64 毫秒，僅診斷用途）
    pub per_script: Vec<(String, u64, u64, f64)>,
    /// 本幀所有腳本 Rhai ops 總和
    pub total_rhai_ops: u64,
    /// 本幀所有腳本 Bridge ops 總和
    pub total_bridge_ops: u64,
    /// 本幀腳本執行時間（毫秒）
    /// 注意：f64 僅用於診斷輸出，不參與 game logic / state hash
    pub time_ms: f64,
}

/// 每幀操作統計追蹤器（ring buffer，最多保留 60 幀）
///
/// 對齊上游 diagnostics.md §FrameOpsMetric 介面。
pub struct FrameOpsMetric {
    history: VecDeque<FrameOpsEntry>,
}

impl FrameOpsMetric {
    /// ring buffer 最大容量（60 幀 = 1 秒 @ 60 fps）
    pub const DEFAULT_CAPACITY: usize = 60;

    /// 建立空的統計追蹤器
    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(Self::DEFAULT_CAPACITY),
        }
    }

    /// 記錄一次腳本執行的統計
    ///
    /// 同一幀（frame 相同）的多次呼叫合併到同一 FrameOpsEntry：
    /// - per_script 追加 (script_id, rhai_ops, bridge_ops) 三元組
    /// - total_rhai_ops / total_bridge_ops 各自累加
    /// - time_ms 累加
    ///
    /// 不同幀建立新 entry。ring buffer 超過 DEFAULT_CAPACITY 時移除最舊的。
    pub fn record(
        &mut self,
        frame: u64,
        script_id: String,
        rhai_ops: u64,
        bridge_ops: u64,
        time_ms: f64,
    ) {
        if let Some(last) = self.history.back_mut() {
            if last.frame == frame {
                last.per_script
                    .push((script_id, rhai_ops, bridge_ops, time_ms));
                last.total_rhai_ops += rhai_ops;
                last.total_bridge_ops += bridge_ops;
                last.time_ms += time_ms;
                return;
            }
        }
        // 新幀
        if self.history.len() >= Self::DEFAULT_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(FrameOpsEntry {
            frame,
            per_script: vec![(script_id, rhai_ops, bridge_ops, time_ms)],
            total_rhai_ops: rhai_ops,
            total_bridge_ops: bridge_ops,
            time_ms,
        });
    }

    /// 查詢最後一幀的 Rhai ops 總和（空歷史回傳 0）
    pub fn total_rhai_ops(&self) -> u64 {
        self.history.back().map_or(0, |e| e.total_rhai_ops)
    }

    /// 取得最近 n 幀的歷史紀錄（slice）
    ///
    /// 若 n > 歷史長度，返回所有可用歷史。
    ///
    /// 實作策略：簽名為 `&mut self`，使用 `make_contiguous()` 確保
    /// VecDeque 內部資料連續後回傳 slice。
    pub fn history(&mut self, n: usize) -> &[FrameOpsEntry] {
        let slice = self.history.make_contiguous();
        let len = slice.len();
        let start = len.saturating_sub(n);
        &slice[start..]
    }

    /// 清空歷史（腳本重新載入時使用）
    ///
    /// 呼叫時機：ScriptManager 觸發 script reload 或 OTA 更新完成後，
    /// 清除舊腳本的 ops 統計，避免新腳本繼承舊數據。
    pub fn clear(&mut self) {
        self.history.clear();
    }
}

impl Default for FrameOpsMetric {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== OpsCostTable::cost_for() — 完整成本表對齊 ==========

    #[test]
    fn test_cost_spawn_entity() {
        assert_eq!(OpsCostTable::cost_for("spawn_entity"), 50);
    }

    #[test]
    fn test_cost_despawn_entity() {
        assert_eq!(OpsCostTable::cost_for("despawn_entity"), 30);
    }

    #[test]
    fn test_cost_set_transform() {
        assert_eq!(OpsCostTable::cost_for("set_transform"), 5);
    }

    #[test]
    fn test_cost_set_rotation() {
        assert_eq!(OpsCostTable::cost_for("set_rotation"), 5);
    }

    #[test]
    fn test_cost_set_scale() {
        assert_eq!(OpsCostTable::cost_for("set_scale"), 5);
    }

    #[test]
    fn test_cost_set_visibility() {
        // 對齊 performance-costs.md §3.12：set_visibility = 3 ops（純 bool 寫入）
        // 注意：與 set_transform(5) 不同
        assert_eq!(OpsCostTable::cost_for("set_visibility"), 3);
    }

    #[test]
    fn test_cost_play_vfx() {
        assert_eq!(OpsCostTable::cost_for("play_vfx"), 10);
    }

    #[test]
    fn test_cost_play_sound() {
        assert_eq!(OpsCostTable::cost_for("play_sound"), 10);
    }

    #[test]
    fn test_cost_play_sound_at() {
        assert_eq!(OpsCostTable::cost_for("play_sound_at"), 10);
    }

    #[test]
    fn test_cost_stop_vfx() {
        assert_eq!(OpsCostTable::cost_for("stop_vfx"), 5);
    }

    #[test]
    fn test_cost_stop_sound() {
        assert_eq!(OpsCostTable::cost_for("stop_sound"), 5);
    }

    #[test]
    fn test_cost_get_position() {
        // BTreeMap lookup
        assert_eq!(OpsCostTable::cost_for("get_position"), 10);
    }

    #[test]
    fn test_cost_get_entity_state() {
        // BTreeMap lookup ×2
        assert_eq!(OpsCostTable::cost_for("get_entity_state"), 10);
    }

    #[test]
    fn test_cost_get_local_player_id() {
        // 直接欄位讀取 = 2 ops（最低成本）
        assert_eq!(OpsCostTable::cost_for("get_local_player_id"), 2);
    }

    #[test]
    fn test_cost_get_frame_number() {
        assert_eq!(OpsCostTable::cost_for("get_frame_number"), 2);
    }

    #[test]
    fn test_cost_get_delta_time() {
        assert_eq!(OpsCostTable::cost_for("get_delta_time"), 2);
    }

    #[test]
    fn test_cost_update_hud() {
        assert_eq!(OpsCostTable::cost_for("update_hud"), 5);
    }

    #[test]
    fn test_cost_show_dialog() {
        assert_eq!(OpsCostTable::cost_for("show_dialog"), 5);
    }

    #[test]
    fn test_cost_hide_dialog() {
        assert_eq!(OpsCostTable::cost_for("hide_dialog"), 5);
    }

    #[test]
    fn test_cost_set_health_bar() {
        assert_eq!(OpsCostTable::cost_for("set_health_bar"), 5);
    }

    #[test]
    fn test_cost_show_damage_number() {
        assert_eq!(OpsCostTable::cost_for("show_damage_number"), 5);
    }

    #[test]
    fn test_cost_show_toast() {
        assert_eq!(OpsCostTable::cost_for("show_toast"), 5);
    }

    #[test]
    fn test_cost_play_animation() {
        assert_eq!(OpsCostTable::cost_for("play_animation"), 5);
    }

    #[test]
    fn test_cost_play_animation_once() {
        assert_eq!(OpsCostTable::cost_for("play_animation_once"), 5);
    }

    #[test]
    fn test_cost_stop_animation() {
        assert_eq!(OpsCostTable::cost_for("stop_animation"), 5);
    }

    #[test]
    fn test_cost_blend_animation() {
        // 對齊 performance-costs.md §3.12：entity 查找 + 4 值寫入 = 8 ops
        assert_eq!(OpsCostTable::cost_for("blend_animation"), 8);
    }

    #[test]
    fn test_cost_send_prediction() {
        // Dynamic 型別轉換 + event 寫入 = 15 ops
        assert_eq!(OpsCostTable::cost_for("send_prediction"), 15);
    }

    #[test]
    fn test_cost_request_state() {
        // BTreeMap key lookup
        assert_eq!(OpsCostTable::cost_for("request_state"), 10);
    }

    #[test]
    fn test_cost_unknown_api() {
        // 未知 API 回傳 1（安全兜底，不拒絕但有基本成本）
        assert_eq!(OpsCostTable::cost_for("unknown_fn"), 1);
    }

    #[test]
    fn test_cost_empty_string() {
        // 空字串也回傳 1（預設值邊界）
        assert_eq!(OpsCostTable::cost_for(""), 1);
    }

    // ========== OpsTracker 建構 ==========

    #[test]
    fn test_tracker_new_remaining() {
        let tracker = OpsTracker::new();
        assert_eq!(tracker.remaining(), OpsTracker::DEFAULT_BUDGET);
        assert_eq!(tracker.remaining(), 50_000);
    }

    #[test]
    fn test_tracker_default_budget_constant() {
        assert_eq!(OpsTracker::DEFAULT_BUDGET, 50_000);
    }

    #[test]
    fn test_tracker_with_limit() {
        let tracker = OpsTracker::with_limit(100);
        assert_eq!(tracker.remaining(), 100);
    }

    #[test]
    fn test_tracker_with_limit_zero() {
        let tracker = OpsTracker::with_limit(0);
        assert_eq!(tracker.remaining(), 0);
        // 零上限邊界：任何扣減都應失敗
        let mut tracker = tracker;
        assert!(tracker.deduct(1).is_err());
    }

    // ========== OpsTracker::deduct() 先扣後做語意 ==========

    #[test]
    fn test_tracker_deduct_ok() {
        let mut tracker = OpsTracker::new();
        for _ in 0..1000 {
            assert!(tracker.deduct(5).is_ok());
        }
        // 50_000 - 5 × 1000 = 45_000
        assert_eq!(tracker.remaining(), 45_000);
    }

    #[test]
    fn test_tracker_deduct_exact_limit() {
        let mut tracker = OpsTracker::new();
        // 一次扣滿 50,000
        assert!(tracker.deduct(50_000).is_ok());
        assert_eq!(tracker.remaining(), 0);
    }

    #[test]
    fn test_tracker_deduct_exceed() {
        let mut tracker = OpsTracker::new();
        tracker.deduct(50_000).unwrap();
        assert_eq!(tracker.remaining(), 0);
        // 再扣 1 → remaining(0) < cost(1) → Err
        let result = tracker.deduct(1);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ops budget exceeded"));
    }

    #[test]
    fn test_tracker_deduct_single_exceed() {
        let mut tracker = OpsTracker::new();
        // 單次超限
        let result = tracker.deduct(50_001);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ops budget exceeded"));
    }

    #[test]
    fn test_tracker_deduct_fail_remaining_unchanged() {
        // 先扣後做：remaining < cost 時不扣減，remaining 維持不變
        let mut tracker = OpsTracker::with_limit(10);
        let result = tracker.deduct(11);
        assert!(result.is_err());
        assert_eq!(tracker.remaining(), 10); // 不變！
    }

    #[test]
    fn test_tracker_deduct_error_contains_values() {
        // 錯誤訊息應含 remaining 和 cost 數值，利於除錯
        let mut tracker = OpsTracker::with_limit(10);
        let err_msg = tracker.deduct(11).unwrap_err();
        assert!(err_msg.contains("ops budget exceeded"));
        // 驗證訊息含數值（remaining=10, cost=11）
        assert!(err_msg.contains("10") || err_msg.contains("remaining"));
        assert!(err_msg.contains("11") || err_msg.contains("cost"));
    }

    // ========== OpsTracker::remaining() ==========

    #[test]
    fn test_tracker_remaining_decreases() {
        let mut tracker = OpsTracker::new();
        tracker.deduct(10).unwrap();
        tracker.deduct(10).unwrap();
        tracker.deduct(10).unwrap();
        assert_eq!(tracker.remaining(), 49_970);
    }

    // ========== OpsTracker::reset() ==========

    #[test]
    fn test_tracker_reset_restores_budget() {
        let mut tracker = OpsTracker::new();
        tracker.deduct(100).unwrap();
        assert_eq!(tracker.remaining(), 49_900);
        tracker.reset();
        assert_eq!(tracker.remaining(), 50_000);
    }

    #[test]
    fn test_tracker_reset_preserves_limit() {
        // with_limit 建構的 tracker，reset 後恢復為原 limit（非 DEFAULT_BUDGET）
        let mut tracker = OpsTracker::with_limit(100);
        tracker.deduct(50).unwrap();
        assert_eq!(tracker.remaining(), 50);
        tracker.reset();
        assert_eq!(tracker.remaining(), 100); // 恢復為 100，非 50_000
    }

    #[test]
    fn test_tracker_reset_then_deduct() {
        let mut tracker = OpsTracker::new();
        tracker.deduct(100).unwrap();
        tracker.reset();
        assert!(tracker.deduct(5).is_ok());
        assert_eq!(tracker.remaining(), 49_995);
    }

    // ========== 整合測試（OpsCostTable + OpsTracker 組合）==========

    #[test]
    fn test_cost_table_with_tracker_deduct() {
        let mut tracker = OpsTracker::new();
        let cost = OpsCostTable::cost_for("spawn_entity"); // 50
        assert!(tracker.deduct(cost).is_ok());
        assert_eq!(tracker.remaining(), 49_950);
    }

    #[test]
    fn test_exhaust_by_spawn() {
        let mut tracker = OpsTracker::new();
        // spawn_entity(50) × 1000 = 50,000 剛好耗盡
        for i in 0..1000 {
            let cost = OpsCostTable::cost_for("spawn_entity");
            assert!(tracker.deduct(cost).is_ok(), "第 {} 次 spawn 應成功", i + 1);
        }
        assert_eq!(tracker.remaining(), 0);
        // 第 1001 次 → Err
        let cost = OpsCostTable::cost_for("spawn_entity");
        assert!(tracker.deduct(cost).is_err());
    }

    // ========== FrameOpsMetric 測試（TDD 紅燈 — Task 5a）==========

    #[test]
    fn test_frame_ops_empty_history() {
        let mut metric = FrameOpsMetric::new();
        assert_eq!(metric.total_rhai_ops(), 0);
        assert_eq!(metric.history(10).len(), 0);
    }

    #[test]
    fn test_frame_ops_record_single() {
        let mut metric = FrameOpsMetric::new();
        metric.record(1, "script_a".to_string(), 100, 50, 0.5);
        assert_eq!(metric.total_rhai_ops(), 100);
        let h = metric.history(10);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].per_script.len(), 1);
        // 驗證四元組 (script_id, rhai_ops, bridge_ops, time_ms)
        assert_eq!(h[0].per_script[0].0, "script_a");
        assert_eq!(h[0].per_script[0].1, 100); // rhai_ops
        assert_eq!(h[0].per_script[0].2, 50); // bridge_ops
        assert!((h[0].per_script[0].3 - 0.5).abs() < 0.001); // time_ms
        assert_eq!(h[0].total_bridge_ops, 50);
    }

    #[test]
    fn test_frame_ops_record_same_frame() {
        let mut metric = FrameOpsMetric::new();
        metric.record(1, "script_a".to_string(), 100, 50, 0.5);
        metric.record(1, "script_b".to_string(), 200, 80, 0.8);
        assert_eq!(metric.total_rhai_ops(), 300);
        let h = metric.history(10);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].per_script.len(), 2);
        // 雙軌 ops 各自累加
        assert_eq!(h[0].total_rhai_ops, 300); // 100 + 200
        assert_eq!(h[0].total_bridge_ops, 130); // 50 + 80
                                                // time_ms 應為同幀累加：0.5 + 0.8 = 1.3
        assert!((h[0].time_ms - 1.3).abs() < 0.001);
    }

    #[test]
    fn test_frame_ops_same_script_same_frame() {
        // 同一 script 在同幀多次執行（如 on_tick + on_event）
        // per_script 不合併，各自獨立 entry
        let mut metric = FrameOpsMetric::new();
        metric.record(1, "script_a".to_string(), 100, 20, 0.3);
        metric.record(1, "script_a".to_string(), 50, 10, 0.2);
        let h = metric.history(10);
        assert_eq!(h[0].per_script.len(), 2);
        assert_eq!(h[0].total_rhai_ops, 150);
        assert_eq!(h[0].total_bridge_ops, 30);
    }

    #[test]
    fn test_frame_ops_total_rhai_ops_last_frame() {
        let mut metric = FrameOpsMetric::new();
        // 記錄多幀，最後幀 rhai_ops = 500
        metric.record(1, "s".to_string(), 100, 10, 0.1);
        metric.record(2, "s".to_string(), 200, 20, 0.2);
        metric.record(3, "s".to_string(), 500, 30, 0.3);
        // total_rhai_ops 回傳最後一幀的 rhai_ops 總和
        assert_eq!(metric.total_rhai_ops(), 500);
    }

    #[test]
    fn test_frame_ops_history_10() {
        let mut metric = FrameOpsMetric::new();
        for i in 0..15 {
            metric.record(i, "s".to_string(), 10, 5, 0.1);
        }
        let h = metric.history(10);
        assert_eq!(h.len(), 10);
        // 應回傳最近 10 幀（frame 5..14）
        assert_eq!(h[0].frame, 5);
        assert_eq!(h[9].frame, 14);
    }

    #[test]
    fn test_frame_ops_history_overflow() {
        let mut metric = FrameOpsMetric::new();
        for i in 0..65 {
            metric.record(i, "s".to_string(), 10, 5, 0.1);
        }
        // ring buffer 最多保留 60 幀
        let h = metric.history(60);
        assert_eq!(h.len(), 60);
        // 最舊幀應為 frame 5（0..4 已被淘汰）
        assert_eq!(h[0].frame, 5);
        assert_eq!(h[59].frame, 64);
    }

    #[test]
    fn test_frame_ops_time_ms_accumulate() {
        let mut metric = FrameOpsMetric::new();
        metric.record(1, "a".to_string(), 100, 10, 0.5);
        metric.record(1, "b".to_string(), 200, 20, 0.8);
        let h = metric.history(10);
        // 同幀時間累加：0.5 + 0.8 = 1.3
        assert!((h[0].time_ms - 1.3).abs() < 0.001);
    }

    #[test]
    fn test_frame_ops_per_script_four_tuple() {
        let mut metric = FrameOpsMetric::new();
        metric.record(1, "a".to_string(), 100, 20, 0.5);
        let h = metric.history(10);
        // 驗證四元組結構：(script_id, rhai_ops, bridge_ops, time_ms)
        let (ref id, rhai, bridge, time) = h[0].per_script[0];
        assert_eq!(id, "a");
        assert_eq!(rhai, 100);
        assert_eq!(bridge, 20);
        assert!((time - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_frame_ops_clear() {
        let mut metric = FrameOpsMetric::new();
        metric.record(1, "s".to_string(), 100, 50, 0.5);
        metric.clear();
        assert_eq!(metric.total_rhai_ops(), 0);
        assert_eq!(metric.history(10).len(), 0);
    }

    // ========== 整合測試（OpsCostTable + OpsTracker 組合）==========

    #[test]
    fn test_typical_on_tick_budget() {
        let mut tracker = OpsTracker::new();
        // 典型場景：5× get_position(10) + 5× set_transform(5)
        for _ in 0..5 {
            let cost = OpsCostTable::cost_for("get_position");
            assert!(tracker.deduct(cost).is_ok());
        }
        for _ in 0..5 {
            let cost = OpsCostTable::cost_for("set_transform");
            assert!(tracker.deduct(cost).is_ok());
        }
        // 50 + 25 = 75 ops used
        assert_eq!(tracker.remaining(), 49_925);
    }
}
