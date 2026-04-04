//! 腳本停用 fallback 資料結構
//!
//! 定義腳本被自動停用後的 fallback 行為所需型別：
//! - [`DisableReason`]: 停用原因（4 種 variant）
//! - [`ScriptDisabled`]: 標記腳本已停用（含 script_id、原因、tick）
//! - [`AnimationDefault`]: 停用後播放的預設動畫 ID
//! - [`ServerPosition`]: 停用後使用 Server 權威位置渲染
//!
//! # 設計決策
//!
//! - 純 Rust struct，不 derive `Component`（Bevy 整合由 Phase 9 負責）
//! - `DisableReason` 分為兩類語義：
//!   - 閾值停用（Timeout, OperationLimit）：連續 10 幀超限後觸發
//!   - 立即停用（ScopeLimitExceeded, InitFailed）：單次違規即停用
//! - `AnimationDefault.animation_id` 為 `Option<u32>`，對齊 ecs-mirror.md MirroredEntity

use bridge_types::BridgeEvent;
use deterministic::SoftVec3;

use crate::ops_cost::FrameOpsEntry;

/// 腳本停用原因
///
/// 對應 auto-disable.md §6.4 觸發條件：
/// - Timeout: 單一腳本連續 10 幀超時（2ms 硬上限）
/// - OperationLimit: Rhai 引擎層或 Bridge API 層 ops 超限（連續 10 幀）
/// - ScopeLimitExceeded: Scope 大小限制違反（立即停用，見 04-lifecycle/scope-persistence.md）
/// - InitFailed: on_init() 執行失敗（立即停用，不需 10 幀計數）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisableReason {
    /// 連續超時達到閾值
    Timeout,
    /// 連續 operation limit 達到閾值
    OperationLimit,
    /// Scope 大小限制違反（立即停用）
    ScopeLimitExceeded,
    /// on_init() 拋出錯誤（立即停用）
    InitFailed,
}

/// 標記腳本已停用的資料結構
///
/// 純 Rust struct，不 derive Component。
/// Bevy Component derive 與 system 實作由 Phase 9 負責。
/// Phase 9 Task 11 負責建立 Component wrapper：
/// ```rust,ignore
/// // engine/vm_bevy_bridge/src/fallback_components.rs
/// #[derive(Component)]
/// pub struct ScriptDisabledComponent(pub ScriptDisabled);
/// ```
#[derive(Debug, Clone)]
pub struct ScriptDisabled {
    /// 被停用的腳本 ID
    pub script_id: String,
    /// 停用原因
    pub reason: DisableReason,
    /// 停用時的 tick 號（用於診斷）
    pub disabled_at_tick: u64,
}

impl ScriptDisabled {
    pub fn new(script_id: impl Into<String>, reason: DisableReason, tick: u64) -> Self {
        Self {
            script_id: script_id.into(),
            reason,
            disabled_at_tick: tick,
        }
    }
}

/// 動畫退化預設值 — 腳本停用後播放的預設動畫 ID
///
/// 對應 auto-disable.md §6.4 Degradation 行為表：
/// 「播放 type_id 對應的預設動畫（AnimationDefault component）」
/// 型別對齊上游 `MirroredEntity.animation_id: Option<u32>`（ecs-mirror.md）。
#[derive(Debug, Clone)]
pub struct AnimationDefault {
    /// 對應 asset 中的預設動畫 ID（None = 無預設動畫，停用後停止播放）
    pub animation_id: Option<u32>,
}

/// Server 權威位置 — 腳本停用後直接使用 Server 位置渲染
///
/// 對應 auto-disable.md §6.4 Degradation 行為表：
/// 「使用 Server 權威位置直接渲染（有延遲感但不影響正確性）」
#[derive(Debug, Clone)]
pub struct ServerPosition {
    /// Server 最後同步的位置（SoftVec3 確保確定性）
    pub pos: SoftVec3,
}

/// 發送腳本停用通知
///
/// 回傳 `Vec<BridgeEvent>`，由呼叫端（Phase 8 ScriptManager）
/// 合併到該幀的事件佇列中。
///
/// 行為對齊 degradation-notification.md §6.5：
/// - 所有 build：`ShowToast` 通知 + `tracing::warn!` 日誌
/// - `debug-mode` feature：額外 `tracing::debug!` 輸出最後 N 幀統計
/// - Server 端：disable 回報由呼叫者（ScriptManager）透過獨立通道處理（Phase 12）
///
/// 設計決策：tick 不作為此函數的參數。
/// tick 已記錄在 `ScriptDisabled.disabled_at_tick` 中（由呼叫端持有），
/// 通知函數僅負責「事件生成 + 日誌」，不需要額外的 tick 冗餘參數。
pub fn emit_disable_notification(
    script_id: &str,
    reason: &DisableReason,
    recent_history: &[FrameOpsEntry],
) -> Vec<BridgeEvent> {
    // 所有 build：日誌記錄停用事件
    tracing::warn!(
        script_id = %script_id,
        reason = ?reason,
        "腳本已自動停用"
    );

    // Debug build: 輸出最後 N 幀統計
    #[cfg(feature = "debug-mode")]
    {
        tracing::debug!(
            "停用腳本 {} 最近 {} 幀統計：",
            script_id,
            recent_history.len()
        );
        for entry in recent_history {
            if entry.time_ms > 2.0 {
                tracing::debug!(
                    "  幀 {}: rhai_ops={}, bridge_ops={}, time_ms={:.3} [超時]",
                    entry.frame,
                    entry.total_rhai_ops,
                    entry.total_bridge_ops,
                    entry.time_ms
                );
            } else {
                tracing::debug!(
                    "  幀 {}: rhai_ops={}, bridge_ops={}, time_ms={:.3}",
                    entry.frame,
                    entry.total_rhai_ops,
                    entry.total_bridge_ops,
                    entry.time_ms
                );
            }
        }
    }

    // 抑制 non-debug-mode 下 recent_history 未使用警告
    #[cfg(not(feature = "debug-mode"))]
    let _ = recent_history;

    // 無論 build mode，都回傳 ShowToast 事件
    vec![BridgeEvent::ShowToast {
        msg: "腳本異常，部分效果暫時關閉".to_string(),
        duration_ms: 3000,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops_cost::FrameOpsEntry;
    use bridge_types::BridgeEvent;
    use deterministic::{SoftF32, SoftVec3};

    // ── ScriptDisabled 建構測試 ─────────────────────────────────────

    #[test]
    fn test_script_disabled_timeout() {
        let disabled = ScriptDisabled::new("test.rhai", DisableReason::Timeout, 100);
        assert_eq!(disabled.script_id, "test.rhai");
        assert_eq!(disabled.reason, DisableReason::Timeout);
        assert_eq!(disabled.disabled_at_tick, 100);
    }

    #[test]
    fn test_script_disabled_ops_limit() {
        let disabled = ScriptDisabled::new("heavy.rhai", DisableReason::OperationLimit, 200);
        assert_eq!(disabled.script_id, "heavy.rhai");
        assert_eq!(disabled.reason, DisableReason::OperationLimit);
        assert_eq!(disabled.disabled_at_tick, 200);
    }

    #[test]
    fn test_script_disabled_scope_limit() {
        // ScopeLimitExceeded 為立即停用路徑，不走 10 幀閾值
        let disabled = ScriptDisabled::new("scope.rhai", DisableReason::ScopeLimitExceeded, 50);
        assert_eq!(disabled.script_id, "scope.rhai");
        assert_eq!(disabled.reason, DisableReason::ScopeLimitExceeded);
        assert_eq!(disabled.disabled_at_tick, 50);
    }

    #[test]
    fn test_script_disabled_init_failed() {
        // InitFailed 為立即停用路徑，通常 tick=0（載入時）
        let disabled = ScriptDisabled::new("broken.rhai", DisableReason::InitFailed, 0);
        assert_eq!(disabled.script_id, "broken.rhai");
        assert_eq!(disabled.reason, DisableReason::InitFailed);
        assert_eq!(disabled.disabled_at_tick, 0);
    }

    #[test]
    fn test_script_disabled_contains_tick() {
        let disabled = ScriptDisabled::new("x.rhai", DisableReason::Timeout, 42);
        assert_eq!(disabled.disabled_at_tick, 42);
    }

    #[test]
    fn test_script_disabled_empty_id() {
        // 空字串由上層驗證（Phase 8 ScriptManager），此處接受不 panic
        let disabled = ScriptDisabled::new("", DisableReason::Timeout, 0);
        assert_eq!(disabled.script_id, "");
    }

    // ── DisableReason 等價比較測試 ──────────────────────────────────

    #[test]
    fn test_disable_reason_eq() {
        assert_eq!(DisableReason::Timeout, DisableReason::Timeout);
    }

    #[test]
    fn test_disable_reason_ne() {
        assert_ne!(DisableReason::Timeout, DisableReason::OperationLimit);
    }

    #[test]
    fn test_disable_reason_all_variants_distinct() {
        let variants = [
            DisableReason::Timeout,
            DisableReason::OperationLimit,
            DisableReason::ScopeLimitExceeded,
            DisableReason::InitFailed,
        ];
        for i in 0..variants.len() {
            for j in 0..variants.len() {
                if i == j {
                    assert_eq!(variants[i], variants[j]);
                } else {
                    assert_ne!(variants[i], variants[j]);
                }
            }
        }
    }

    // ── AnimationDefault 測試 ───────────────────────────────────────

    #[test]
    fn test_animation_default_some() {
        let anim = AnimationDefault {
            animation_id: Some(42),
        };
        assert_eq!(anim.animation_id, Some(42));
    }

    #[test]
    fn test_animation_default_none() {
        let anim = AnimationDefault { animation_id: None };
        assert_eq!(anim.animation_id, None);
    }

    #[test]
    fn test_animation_default_zero() {
        let anim = AnimationDefault {
            animation_id: Some(0),
        };
        assert_eq!(anim.animation_id, Some(0));
    }

    // ── ServerPosition 測試 ─────────────────────────────────────────

    #[test]
    fn test_server_position() {
        let pos = ServerPosition {
            pos: SoftVec3::new(
                SoftF32::from_f32(1.0),
                SoftF32::from_f32(2.0),
                SoftF32::from_f32(3.0),
            ),
        };
        assert_eq!(pos.pos.x, SoftF32::from_f32(1.0));
        assert_eq!(pos.pos.y, SoftF32::from_f32(2.0));
        assert_eq!(pos.pos.z, SoftF32::from_f32(3.0));
    }

    // ── emit_disable_notification 測試 ──────────────────────────────

    /// 輔助函數：建構測試用 FrameOpsEntry
    fn make_entry(frame: u64, rhai_ops: u64, bridge_ops: u64, time_ms: f64) -> FrameOpsEntry {
        FrameOpsEntry {
            frame,
            per_script: vec![("test.rhai".to_string(), rhai_ops, bridge_ops, time_ms)],
            total_rhai_ops: rhai_ops,
            total_bridge_ops: bridge_ops,
            time_ms,
        }
    }

    /// 輔助函數：驗證回傳的 ShowToast 事件
    fn assert_show_toast(events: &[BridgeEvent]) {
        assert_eq!(events.len(), 1);
        match &events[0] {
            BridgeEvent::ShowToast { msg, duration_ms } => {
                assert_eq!(msg, "腳本異常，部分效果暫時關閉");
                assert_eq!(*duration_ms, 3000);
            }
            _ => panic!("預期 ShowToast 事件"),
        }
    }

    #[test]
    fn test_disable_notification_timeout() {
        let events = emit_disable_notification("test.rhai", &DisableReason::Timeout, &[]);
        assert_show_toast(&events);
    }

    #[test]
    fn test_disable_notification_ops_limit() {
        let history: Vec<FrameOpsEntry> =
            (0..5).map(|i| make_entry(i, 50000, 12000, 1.9)).collect();
        let events =
            emit_disable_notification("heavy.rhai", &DisableReason::OperationLimit, &history);
        assert_show_toast(&events);
    }

    #[test]
    fn test_disable_notification_scope_limit() {
        let events =
            emit_disable_notification("scope.rhai", &DisableReason::ScopeLimitExceeded, &[]);
        assert_show_toast(&events);
    }

    #[test]
    fn test_disable_notification_init_failed() {
        let events = emit_disable_notification("broken.rhai", &DisableReason::InitFailed, &[]);
        assert_show_toast(&events);
    }

    #[test]
    fn test_disable_notification_all_reasons_same_toast() {
        let reasons = [
            DisableReason::Timeout,
            DisableReason::OperationLimit,
            DisableReason::ScopeLimitExceeded,
            DisableReason::InitFailed,
        ];
        for reason in &reasons {
            let events = emit_disable_notification("any.rhai", reason, &[]);
            assert_show_toast(&events);
        }
    }

    #[test]
    fn test_disable_notification_empty_history() {
        let events = emit_disable_notification("test.rhai", &DisableReason::Timeout, &[]);
        assert_show_toast(&events);
    }

    #[test]
    fn test_disable_notification_empty_script_id() {
        let events = emit_disable_notification("", &DisableReason::Timeout, &[]);
        assert_show_toast(&events);
    }

    #[test]
    fn test_disable_notification_with_history() {
        // 建構 10 筆歷史，其中 frame 1055~1059 超時（time_ms > 2.0）
        let history: Vec<FrameOpsEntry> = (0..10)
            .map(|i| {
                let frame = 1050 + i;
                let time_ms = if i >= 5 { 2.1 + (i as f64) * 0.05 } else { 1.8 };
                make_entry(frame, 48000 + i * 200, 12000 + i * 500, time_ms)
            })
            .collect();
        let events = emit_disable_notification("perf.rhai", &DisableReason::Timeout, &history);
        // Toast 不受歷史數據影響
        assert_show_toast(&events);
        // debug-mode 下的 tracing 輸出含 [超時] 標記——
        // 需使用 cargo test --features debug-mode 並搭配 tracing capture 驗證
    }
}
