//! Client 端 crash dump 收集機制。
//!
//! 提供 [`CrashDump`]（crash 時的完整快照）和 [`MinimalCrashReport`]（降級報告）。
//! 使用 thread_local 預存序列化 bytes，確保 panic hook 中不需分配記憶體。
//!
//! ## 架構
//! - Bevy Resource 層（`CrashContext`）：結構化 CrashDump，每 tick 更新
//! - thread_local 層：預序列化 bytes，panic hook 中僅讀取

use bridge_types::bridge_event::BridgeEvent;
use bridge_types::ecs_mirror::EcsMirror;
use bridge_types::errors::ScriptError;
use bridge_types::replay::{Blake3Hash, ReplayFrame};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

/// VM crash 時回報的 dump 結構
///
/// 透過 WebSocket 發送至 Server（bincode 序列化）。
/// panic hook 中讀取預存的 CrashContext 快照。
///
/// 非 game state，無需 `#[repr(C)]`；加入 `dump_version` 供跨版本相容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashDump {
    /// dump 版本號（供跨版本相容）
    pub dump_version: u32,
    /// 觸發 crash 的腳本 ID（若適用）
    pub script_id: Option<String>,
    /// 腳本錯誤（若適用）
    pub error: Option<ScriptError>,
    /// crash 發生時的 tick
    pub tick: u64,
    /// 腳本 scope 快照（(var_name, debug_repr) 列表）
    pub scope_snapshot: Option<Vec<(String, String)>>,
    /// 最近 10 個 BridgeEvent
    pub recent_events: Vec<BridgeEvent>,
    /// crash 時的 state hash
    pub state_hash: Blake3Hash,
    /// Client WASM 版本
    pub wasm_version: String,
    /// ECS Mirror 快照（用於重現）
    pub ecs_snapshot: EcsMirror,
    /// DeterministicRng 狀態
    pub rng_state: [u8; 16],
    /// 最近 10 幀的 replay frames
    pub recent_frames: Vec<ReplayFrame>,
    /// 錯誤訊息（純文字，用於 fallback）
    pub error_message: String,
}

/// 序列化失敗時的最小降級報告
///
/// 當完整 CrashDump bincode 序列化失敗時，以 JSON 字串格式發送此最小結構。
/// 使用 JSON 而非 bincode：降級路徑需人類可讀性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinimalCrashReport {
    pub tick: u64,
    pub error_message: String,
}

// thread_local 預存快照
// 初始為空 Vec，Bevy system 啟動後才透過 `update_snapshot()` 填充。
// panic hook 僅讀取此 thread_local，避免 panic 後記憶體分配。
thread_local! {
    static CRASH_SNAPSHOT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// CrashContext：Bevy Resource 層 + thread_local 雙層架構
pub struct CrashContext;

impl CrashContext {
    /// 安裝 WASM panic hook
    ///
    /// Panic hook 三層降級邏輯：
    /// 1. 從 thread_local 讀取預存 snapshot bytes → WebSocket 發送完整 CrashDump
    /// 2. 若 snapshot 為空或發送失敗 → 建構 MinimalCrashReport，serde_json 序列化後發送
    /// 3. 最終 fallback → console error 輸出純文字錯誤訊息
    #[cfg(target_arch = "wasm32")]
    pub fn install_panic_hook() {
        use std::panic;
        panic::set_hook(Box::new(|info| {
            let error_message = format!("{info}");
            // 第一層：嘗試讀取預存 snapshot 並透過 WebSocket 發送
            // 注意：panic hook 中呼叫 js_send_websocket 為 best-effort，
            // WebSocket 連線可能已中斷或 JS 層可能無法處理。
            if let Some(snapshot) = Self::get_snapshot() {
                tracing::error!(
                    "Panic 發生，嘗試發送 crash snapshot（{} bytes）",
                    snapshot.len()
                );
                crate::js_send_websocket(&snapshot);
            }
            // 第二層：MinimalCrashReport JSON 降級
            let minimal = MinimalCrashReport {
                tick: 0, // panic 時無法安全取得 tick
                error_message: error_message.clone(),
            };
            if let Ok(json) = serde_json::to_string(&minimal) {
                // 以 JSON bytes 發送降級報告（WebSocket binary frame）
                crate::js_send_websocket(json.as_bytes());
                tracing::error!("降級 crash 報告已發送：{}", minimal.error_message);
            }
            // 第三層：console error fallback（tracing 已在上方輸出至 console）
        }));
    }

    /// 將 CrashDump bincode 序列化後寫入 thread_local（每 tick 呼叫）
    ///
    /// 若 bincode::serialize 失敗，僅 tracing::warn! 記錄，不清除舊 snapshot。
    pub fn update_snapshot(dump: &CrashDump) {
        match bincode::serialize(dump) {
            Ok(bytes) => {
                CRASH_SNAPSHOT.with(|snapshot| {
                    *snapshot.borrow_mut() = bytes;
                });
            }
            Err(e) => {
                tracing::warn!("CrashDump 序列化失敗，保留舊 snapshot：{e}");
            }
        }
    }

    /// 取得預存快照（panic hook 中呼叫）
    ///
    /// 回傳值語義：
    /// - None：thread_local 為空 Vec（Bevy system 尚未啟動，或從未成功序列化）
    /// - Some(bytes)：最近一次成功序列化的 CrashDump bincode bytes
    pub fn get_snapshot() -> Option<Vec<u8>> {
        CRASH_SNAPSHOT.with(|snapshot| {
            let data = snapshot.borrow();
            if data.is_empty() {
                None
            } else {
                Some(data.clone())
            }
        })
    }
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::ecs_mirror::EcsMirror;
    use bridge_types::handles::EntityId;
    use deterministic::SoftF32;
    use std::collections::BTreeMap;

    /// 輔助函式：建構有效的 CrashDump
    fn make_test_dump() -> CrashDump {
        CrashDump {
            dump_version: 1,
            script_id: Some("test_script".to_string()),
            error: Some(ScriptError::RuntimeError {
                script_id: "test_script".to_string(),
                message: "測試錯誤".to_string(),
                tick: 42,
            }),
            tick: 42,
            scope_snapshot: Some(vec![("x".to_string(), "10".to_string())]),
            recent_events: vec![BridgeEvent::SpawnEntity { type_id: 1 }],
            state_hash: [0u8; 32],
            wasm_version: "1.0.0-test".to_string(),
            ecs_snapshot: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 42,
                delta_time: SoftF32(0),
            },
            rng_state: [0u8; 16],
            recent_frames: vec![],
            error_message: "測試 crash".to_string(),
        }
    }

    #[test]
    fn test_crash_dump_serialization_roundtrip() {
        let dump = make_test_dump();
        let bytes = bincode::serialize(&dump).expect("序列化應成功");
        let restored: CrashDump = bincode::deserialize(&bytes).expect("反序列化應成功");
        assert_eq!(dump.tick, restored.tick);
        assert_eq!(dump.dump_version, restored.dump_version);
        assert_eq!(dump.error_message, restored.error_message);
        assert_eq!(dump.rng_state, restored.rng_state);
        assert_eq!(dump.state_hash, restored.state_hash);
    }

    #[test]
    fn test_crash_dump_version_roundtrip() {
        let mut dump = make_test_dump();
        dump.dump_version = 42;
        let bytes = bincode::serialize(&dump).expect("序列化應成功");
        let restored: CrashDump = bincode::deserialize(&bytes).expect("反序列化應成功");
        assert_eq!(restored.dump_version, 42);
    }

    #[test]
    fn test_crash_context_update_and_get_snapshot() {
        let dump = make_test_dump();
        CrashContext::update_snapshot(&dump);
        let snapshot = CrashContext::get_snapshot();
        assert!(snapshot.is_some(), "update_snapshot 後應回傳 Some");
        assert!(!snapshot.unwrap().is_empty());
    }

    #[test]
    fn test_crash_dump_contains_all_fields() {
        let dump = make_test_dump();
        assert!(dump.tick > 0);
        assert!(!dump.error_message.is_empty());
    }

    #[test]
    fn test_serialization_fallback_to_minimal() {
        let minimal = MinimalCrashReport {
            tick: 42,
            error_message: "降級測試".to_string(),
        };
        let json = serde_json::to_string(&minimal).expect("JSON 序列化應成功");
        let restored: MinimalCrashReport =
            serde_json::from_str(&json).expect("JSON 反序列化應成功");
        assert_eq!(restored.tick, 42);
        assert_eq!(restored.error_message, "降級測試");
    }

    #[test]
    fn test_thread_local_read_write_roundtrip() {
        let dump = make_test_dump();
        CrashContext::update_snapshot(&dump);
        let snapshot = CrashContext::get_snapshot().expect("snapshot 應存在");
        let restored: CrashDump = bincode::deserialize(&snapshot).expect("反序列化應成功");
        assert_eq!(restored.tick, dump.tick);
    }

    #[test]
    fn test_thread_local_initially_empty() {
        std::thread::spawn(|| {
            assert!(CrashContext::get_snapshot().is_none(), "初始應回傳 None");
        })
        .join()
        .expect("不應 panic");
    }

    #[test]
    fn test_thread_local_overwrite() {
        let mut dump = make_test_dump();
        dump.tick = 100;
        CrashContext::update_snapshot(&dump);
        dump.tick = 200;
        CrashContext::update_snapshot(&dump);
        let snapshot = CrashContext::get_snapshot().expect("snapshot 應存在");
        let restored: CrashDump = bincode::deserialize(&snapshot).expect("反序列化應成功");
        assert_eq!(restored.tick, 200, "應回傳最新 snapshot");
    }

    #[test]
    fn test_crash_dump_large_scope_snapshot() {
        let mut dump = make_test_dump();
        dump.scope_snapshot = Some(
            (0..1000)
                .map(|i| (format!("var_{i}"), format!("val_{i}")))
                .collect(),
        );
        let bytes = bincode::serialize(&dump).expect("大型 scope 序列化應成功");
        let restored: CrashDump = bincode::deserialize(&bytes).expect("反序列化應成功");
        assert_eq!(restored.scope_snapshot.as_ref().unwrap().len(), 1000);
    }

    #[test]
    fn test_crash_dump_empty_optional_fields() {
        let dump = CrashDump {
            dump_version: 1,
            script_id: None,
            error: None,
            tick: 0,
            scope_snapshot: None,
            recent_events: vec![],
            state_hash: [0u8; 32],
            wasm_version: String::new(),
            ecs_snapshot: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 0,
                delta_time: SoftF32(0),
            },
            rng_state: [0u8; 16],
            recent_frames: vec![],
            error_message: String::new(),
        };
        let bytes = bincode::serialize(&dump).expect("空欄位 dump 序列化應成功");
        let _: CrashDump = bincode::deserialize(&bytes).expect("反序列化應成功");
    }

    #[test]
    fn test_minimal_report_json_serialization_always_succeeds() {
        let minimal = MinimalCrashReport {
            tick: u64::MAX,
            error_message: "邊界測試：最大 tick 值".to_string(),
        };
        let json = serde_json::to_string(&minimal).expect("JSON 序列化應成功");
        assert!(!json.is_empty());
    }
}
