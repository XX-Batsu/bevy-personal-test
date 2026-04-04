//! Anti-cheat 規則引擎與 session 管理
//!
//! `CheatDetector` 追蹤每位玩家的 mismatch 計數，根據閾值決定升級動作
//! （None → Resync → Flag → Disconnect）。
//!
//! ## 升級閾值（mismatch_count）
//! | 計數 | 動作 |
//! |------|------|
//! | 1 | None（記錄，繼續監控）|
//! | 2 | Resync（要求重新同步）|
//! | 3–5 | Flag（標記帳號）|
//! | >= 6 | Disconnect（踢出 + 記錄）|

use std::collections::BTreeMap;

use bridge_types::{ShadowResponse, ShadowStatus};

/// 玩家/Session 識別符（u64，與 server/gateway::SessionId 語義一致）
pub type SessionId = u64;

/// 單一玩家的作弊記錄
#[derive(Debug, Clone)]
struct PlayerCheatRecord {
    /// 累計 mismatch 次數（per-session lifetime，單調遞增）
    pub mismatch_count: u32,
    /// 最後一次 mismatch 的邏輯幀 tick
    pub last_mismatch_tick: u64,
    /// 當前裁決狀態
    pub status: CheatStatus,
}

/// 玩家作弊狀態（追蹤裁決歷程）
#[derive(Debug, Clone, PartialEq)]
enum CheatStatus {
    Clean,
    Warned,
    Flagged,
    Disconnected,
}

/// Server 端反作弊裁決（由 CheatDetector 產出）
///
/// 對齊上游 mismatch-handling.md 定義。
#[derive(Debug, Clone, PartialEq)]
pub enum CheatAction {
    /// 無異常或單次偶發，繼續監控
    None,
    /// 輕微異常：要求 client 重新同步 state（full snapshot resync）
    Resync,
    /// 多次異常：標記帳號，上報後台審核
    Flag { reason: String },
    /// 嚴重/重複異常：立即踢出玩家並記錄 anti-cheat log
    Disconnect { reason: String },
}

/// 反作弊偵測引擎（伺服器端，純確定性邏輯）
///
/// 使用 `BTreeMap` 符合 README.md Determinism Rules（禁止 HashMap/HashSet）。
pub struct CheatDetector {
    records: BTreeMap<SessionId, PlayerCheatRecord>,
}

impl CheatDetector {
    /// 建立空的 CheatDetector
    pub fn new() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }

    /// 收到 Shadow VM 回報時呼叫，回傳建議的 CheatAction
    ///
    /// - `ShadowStatus::Mismatch` → mismatch_count += 1，根據閾值回傳對應 CheatAction
    /// - `ShadowStatus::AllMatch` → 不更新計數，回傳 CheatAction::None
    /// - `ShadowStatus::Error` → 不增加計數（Worker 內部故障，不視為作弊），回傳 CheatAction::None
    /// - 若 session_id 不存在 → 自動建立新紀錄
    pub fn on_shadow_result(
        &mut self,
        session_id: SessionId,
        result: &ShadowResponse,
    ) -> CheatAction {
        match &result.status {
            ShadowStatus::Mismatch { tick, .. } => {
                let record = self
                    .records
                    .entry(session_id)
                    .or_insert_with(|| PlayerCheatRecord {
                        mismatch_count: 0,
                        last_mismatch_tick: 0,
                        status: CheatStatus::Clean,
                    });

                record.mismatch_count += 1;
                record.last_mismatch_tick = *tick;

                let count = record.mismatch_count;

                tracing::warn!(
                    "Shadow VM mismatch：session_id={session_id}, tick={tick}, count={count}"
                );

                // 閾值裁決
                if count >= 6 {
                    record.status = CheatStatus::Disconnected;
                    CheatAction::Disconnect {
                        reason: format!("累計 {count} 次 mismatch，超過閾值"),
                    }
                } else if count >= 3 {
                    record.status = CheatStatus::Flagged;
                    CheatAction::Flag {
                        reason: format!("累計 {count} 次 mismatch"),
                    }
                } else if count == 2 {
                    record.status = CheatStatus::Warned;
                    CheatAction::Resync
                } else {
                    // count == 1
                    CheatAction::None
                }
            }
            ShadowStatus::AllMatch => {
                // 正常通過，不更新計數
                CheatAction::None
            }
            ShadowStatus::Error(msg) => {
                // Worker 內部故障（Rhai 執行逾時、bytecode 載入失敗等），不計入 mismatch
                tracing::warn!(
                    "Shadow VM 回報錯誤（不計入 mismatch）：session_id={session_id}, error={msg}"
                );
                CheatAction::None
            }
        }
    }

    /// 取得指定玩家的累計 mismatch 次數
    ///
    /// 若 session_id 不存在 → 回傳 0
    pub fn mismatch_count(&self, session_id: SessionId) -> u32 {
        self.records
            .get(&session_id)
            .map(|r| r.mismatch_count)
            .unwrap_or(0)
    }

    /// 玩家離線時清除記錄
    pub fn remove_session(&mut self, session_id: SessionId) {
        self.records.remove(&session_id);
    }
}

impl Default for CheatDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mismatch_response(tick: u64) -> ShadowResponse {
        ShadowResponse {
            status: ShadowStatus::Mismatch {
                tick,
                expected_hash: [0u8; 32],
                actual_hash: [0xFFu8; 32],
            },
            checked_ticks: vec![tick],
        }
    }

    fn error_response() -> ShadowResponse {
        ShadowResponse {
            status: ShadowStatus::Error("測試錯誤".to_string()),
            checked_ticks: vec![],
        }
    }

    fn all_match_response() -> ShadowResponse {
        ShadowResponse {
            status: ShadowStatus::AllMatch,
            checked_ticks: vec![1, 2, 3],
        }
    }

    // Test: 單次 mismatch → None
    #[test]
    fn single_mismatch_no_action() {
        let mut detector = CheatDetector::new();
        let action = detector.on_shadow_result(1, &mismatch_response(1));
        assert_eq!(action, CheatAction::None);
        assert_eq!(detector.mismatch_count(1), 1);
    }

    // Test: 第 2 次 mismatch → Resync
    #[test]
    fn second_mismatch_resync() {
        let mut detector = CheatDetector::new();
        detector.on_shadow_result(1, &mismatch_response(1));
        let action = detector.on_shadow_result(1, &mismatch_response(2));
        assert_eq!(action, CheatAction::Resync);
    }

    // Test: 3-5 次 mismatch → Flag
    #[test]
    fn threshold_flag_at_3_to_5() {
        let mut detector = CheatDetector::new();
        for _ in 0..2 {
            detector.on_shadow_result(1, &mismatch_response(1));
        }
        let action = detector.on_shadow_result(1, &mismatch_response(3));
        assert!(
            matches!(action, CheatAction::Flag { .. }),
            "count=3 應為 Flag"
        );
        let action = detector.on_shadow_result(1, &mismatch_response(4));
        assert!(
            matches!(action, CheatAction::Flag { .. }),
            "count=4 應為 Flag"
        );
        let action = detector.on_shadow_result(1, &mismatch_response(5));
        assert!(
            matches!(action, CheatAction::Flag { .. }),
            "count=5 應為 Flag"
        );
    }

    // Test: 第 6 次 mismatch → Disconnect
    #[test]
    fn threshold_disconnect_above_5() {
        let mut detector = CheatDetector::new();
        for _ in 0..5 {
            detector.on_shadow_result(1, &mismatch_response(1));
        }
        let action = detector.on_shadow_result(1, &mismatch_response(6));
        assert!(
            matches!(action, CheatAction::Disconnect { .. }),
            "count=6 應為 Disconnect"
        );
    }

    // Test: 超大計數不溢出
    #[test]
    fn threshold_disconnect_large_count() {
        let mut detector = CheatDetector::new();
        for i in 0..100u64 {
            detector.on_shadow_result(1, &mismatch_response(i));
        }
        let action = detector.on_shadow_result(1, &mismatch_response(100));
        assert!(matches!(action, CheatAction::Disconnect { .. }));
    }

    // Test: Error status 不計入 mismatch
    #[test]
    fn error_status_does_not_increase_count() {
        let mut detector = CheatDetector::new();
        let error_resp = error_response();
        for _ in 0..10 {
            let action = detector.on_shadow_result(1, &error_resp);
            assert_eq!(action, CheatAction::None);
        }
        assert_eq!(detector.mismatch_count(1), 0);
    }

    // Test: AllMatch 不計入 mismatch
    #[test]
    fn all_match_does_not_increase_count() {
        let mut detector = CheatDetector::new();
        let all_match_resp = all_match_response();
        for _ in 0..10 {
            let action = detector.on_shadow_result(1, &all_match_resp);
            assert_eq!(action, CheatAction::None);
        }
        assert_eq!(detector.mismatch_count(1), 0);
    }

    // Test: Error + Mismatch 混合
    #[test]
    fn mixed_error_and_mismatch() {
        let mut detector = CheatDetector::new();
        let err = error_response();
        // 5 次 Error（不計入）
        for _ in 0..5 {
            detector.on_shadow_result(1, &err);
        }
        // 3 次 Mismatch
        detector.on_shadow_result(1, &mismatch_response(1));
        detector.on_shadow_result(1, &mismatch_response(2));
        let action = detector.on_shadow_result(1, &mismatch_response(3));
        // count=3 → Flag
        assert!(
            matches!(action, CheatAction::Flag { .. }),
            "count=3 應為 Flag"
        );
        assert_eq!(detector.mismatch_count(1), 3);
    }

    // Test: remove_session 清除記錄
    #[test]
    fn remove_session_clears_record() {
        let mut detector = CheatDetector::new();
        detector.on_shadow_result(1, &mismatch_response(1));
        detector.remove_session(1);
        assert_eq!(detector.mismatch_count(1), 0);
    }

    // Test: remove_session 對不存在的 session 不 panic
    #[test]
    fn remove_session_nonexistent_no_panic() {
        let mut detector = CheatDetector::new();
        detector.remove_session(999); // 不 panic
    }

    // Test: 自動建立記錄
    #[test]
    fn auto_create_record_on_shadow_result() {
        let mut detector = CheatDetector::new();
        let action = detector.on_shadow_result(99, &mismatch_response(1));
        assert_eq!(action, CheatAction::None);
        assert_eq!(detector.mismatch_count(99), 1);
    }

    // Test: mismatch_count 查詢未知 session → 0
    #[test]
    fn mismatch_count_nonexistent_session() {
        let detector = CheatDetector::new();
        assert_eq!(detector.mismatch_count(999), 0);
    }

    // Test: 多玩家互相隔離
    #[test]
    fn multiple_players_independent() {
        let mut detector = CheatDetector::new();
        // 玩家 1：7 次 mismatch
        for _ in 0..6 {
            detector.on_shadow_result(1, &mismatch_response(1));
        }
        let action_a = detector.on_shadow_result(1, &mismatch_response(7));
        assert!(
            matches!(action_a, CheatAction::Disconnect { .. }),
            "玩家 1 應 Disconnect"
        );
        // 玩家 2：第 1 次 mismatch
        let action_b = detector.on_shadow_result(2, &mismatch_response(1));
        assert_eq!(
            action_b,
            CheatAction::None,
            "玩家 2 獨立，count=1 應為 None"
        );
    }

    // Test: BTreeMap 確定性（不同插入順序，裁決一致）
    #[test]
    fn btreemap_determinism() {
        let mut d1 = CheatDetector::new();
        let mut d2 = CheatDetector::new();

        // d1：先插入 session 3，再插入 session 1
        d1.on_shadow_result(3, &mismatch_response(1));
        d1.on_shadow_result(1, &mismatch_response(1));

        // d2：先插入 session 1，再插入 session 3
        d2.on_shadow_result(1, &mismatch_response(1));
        d2.on_shadow_result(3, &mismatch_response(1));

        // 兩者裁決結果應一致
        assert_eq!(d1.mismatch_count(1), d2.mismatch_count(1));
        assert_eq!(d1.mismatch_count(3), d2.mismatch_count(3));
    }
}
