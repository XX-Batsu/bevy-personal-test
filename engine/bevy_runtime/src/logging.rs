//! Tracing 統一設定模組。
//!
//! 根據 feature（`debug-mode`）和 target（WASM/Native）設定正確的 tracing level：
//! - Dev build（`debug-mode`）→ DEBUG level
//! - Prod build → INFO level
//! - Native → `tracing_subscriber::fmt()`
//! - WASM → `tracing-wasm` 橋接至 console.log
//!
//! # 不變式
//! - `init_logging()` 僅可呼叫一次（重複呼叫會 panic）

use std::collections::BTreeMap;
use std::sync::Once;

/// 確保 init_logging 僅執行一次的全域 guard
static INIT_LOGGING: Once = Once::new();

/// Tracing backend 列舉
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggingBackend {
    /// Native: tracing_subscriber::fmt()
    Fmt,
    /// WASM: tracing-wasm 橋接至 console.log
    TracingWasm,
}

/// Tracing 設定值（純資料，可測試）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingConfig {
    /// 日誌最大等級
    pub max_level: tracing::Level,
    /// 使用的 backend
    pub backend: LoggingBackend,
}

/// 回傳目前 cfg 組合對應的 LoggingConfig（純函式，不設定全域 subscriber）
///
/// cfg 分支結構（feature 為最外層，target 在內層）：
/// 1. `#[cfg(feature = "debug-mode")]` → DEBUG level
/// 2. `#[cfg(not(feature = "debug-mode"))]` → INFO level
/// - Native: backend = Fmt
/// - WASM:   backend = TracingWasm
pub fn logging_config() -> LoggingConfig {
    let max_level = if cfg!(feature = "debug-mode") {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    let backend = if cfg!(target_arch = "wasm32") {
        LoggingBackend::TracingWasm
    } else {
        LoggingBackend::Fmt
    };
    LoggingConfig { max_level, backend }
}

/// 回傳所有 log 訊息常數（category → 中文訊息模板）
///
/// 涵蓋 observability.md §tracing 埋點規範全部 8 個類別。
/// 使用 `BTreeMap` 確保確定性迭代順序（README.md §Determinism Rules）。
pub fn log_message_constants() -> BTreeMap<&'static str, &'static str> {
    let mut m = BTreeMap::new();
    m.insert("startup", "遊戲引擎初始化完成");
    m.insert("script_load", "載入腳本：{}");
    m.insert("hash_mismatch", "狀態雜湊不一致：本地={:?} 伺服器={:?}");
    m.insert("script_timeout", "腳本超時：{} ms（上限 2 ms）");
    m.insert("player_connect", "玩家連線：connection_id={}");
    m.insert("player_disconnect", "玩家斷線：connection_id={}，原因={}");
    m.insert("ota_start", "OTA 更新開始：version={}");
    m.insert("ota_complete", "OTA 更新完成：version={}，耗時={} ms");
    m
}

/// 初始化 tracing（統一設定）
///
/// cfg 分支結構（feature 為最外層，target 在內層）：
/// 1. `#[cfg(feature = "debug-mode")]` → DEBUG level
///    - Native: `tracing_subscriber::fmt().with_max_level(DEBUG).init()`
///    - WASM:   `tracing_wasm::set_as_global_default()`（預設 DEBUG）
/// 2. `#[cfg(not(feature = "debug-mode"))]` → INFO level
///    - Native: `tracing_subscriber::fmt().with_max_level(INFO).init()`
///    - WASM: `tracing_wasm::set_as_global_default_with_config(config)`（max_level = INFO）
///
/// 在 `GamePlugin::build()` 最前面呼叫。
///
/// # Panics
/// 重複呼叫會 panic（設定全域 subscriber 第二次會 panic）。
///
/// # 依據
/// - cfg 結構對齊 `observability.md` §Tracing 設定
/// - 不變式 #6：init_logging 僅呼叫一次
pub fn init_logging() {
    INIT_LOGGING.call_once(|| {
        #[cfg(feature = "debug-mode")]
        {
            // Dev build: DEBUG level，顯示所有子系統活動
            #[cfg(not(target_arch = "wasm32"))]
            {
                tracing_subscriber::fmt()
                    .with_max_level(tracing::Level::DEBUG)
                    .init();
            }
            #[cfg(target_arch = "wasm32")]
            {
                // tracing-wasm 預設 max_level = DEBUG，直接設定即可
                tracing_wasm::set_as_global_default();
            }
        }
        #[cfg(not(feature = "debug-mode"))]
        {
            // Prod build: INFO level，只顯示關鍵事件
            // 關鍵事件：startup、connections、errors、hash mismatch、OTA updates
            #[cfg(not(target_arch = "wasm32"))]
            {
                tracing_subscriber::fmt()
                    .with_max_level(tracing::Level::INFO)
                    .init();
            }
            #[cfg(target_arch = "wasm32")]
            {
                let config = tracing_wasm::WASMLayerConfigBuilder::new()
                    .set_max_level(tracing::Level::INFO)
                    .build();
                tracing_wasm::set_as_global_default_with_config(config);
            }
        }
    });
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- Native level 測試 ---

    /// Dev build → log level 為 DEBUG
    #[test]
    #[cfg(feature = "debug-mode")]
    fn test_logging_debug_level() {
        let config = logging_config();
        assert_eq!(config.max_level, tracing::Level::DEBUG);
    }

    /// Release build → log level 為 INFO
    #[test]
    #[cfg(not(feature = "debug-mode"))]
    fn test_logging_info_level() {
        let config = logging_config();
        assert_eq!(config.max_level, tracing::Level::INFO);
    }

    // --- Backend 類型測試 ---

    /// Native target → 使用 fmt subscriber
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_native_backend_is_fmt() {
        let config = logging_config();
        assert_eq!(config.backend, LoggingBackend::Fmt);
    }

    // --- 中文驗證 ---

    /// Log messages 包含中文（zh-TW）
    #[test]
    fn test_log_messages_contain_chinese() {
        let messages = log_message_constants();
        for (key, msg) in &messages {
            assert!(
                msg.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
                "log 訊息 '{}' 未包含中文字元：'{}'",
                key,
                msg,
            );
        }
    }

    /// 驗證中文訊息涵蓋所有必要類別
    #[test]
    fn test_log_message_categories_complete() {
        let messages = log_message_constants();
        let required_categories = [
            "startup",
            "script_load",
            "hash_mismatch",
            "script_timeout",
            "player_connect",
            "player_disconnect",
            "ota_start",
            "ota_complete",
        ];
        for cat in &required_categories {
            assert!(messages.contains_key(*cat), "缺少必要 log 類別：'{}'", cat,);
        }
    }

    // --- double-init 安全性測試 ---

    /// 重複呼叫 init_logging 不會 panic（Once guard 保護）
    /// 標記 #[ignore] 因為會設定全域 subscriber，與其他測試互相干擾
    #[test]
    #[ignore]
    fn test_double_init_is_safe() {
        // Once guard 確保只執行一次，第二次呼叫安全地跳過
        init_logging();
        init_logging(); // 第二次呼叫不應 panic
    }
}

// ── WASM 測試 ────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[cfg(test)]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// WASM target: backend 為 tracing-wasm
    #[wasm_bindgen_test]
    fn test_wasm_backend_is_tracing_wasm() {
        let config = logging_config();
        assert_eq!(config.backend, LoggingBackend::TracingWasm);
    }

    /// WASM + debug-mode → DEBUG level
    #[cfg(feature = "debug-mode")]
    #[wasm_bindgen_test]
    fn test_wasm_debug_level() {
        let config = logging_config();
        assert_eq!(config.max_level, tracing::Level::DEBUG);
    }

    /// WASM + release → INFO level
    #[cfg(not(feature = "debug-mode"))]
    #[wasm_bindgen_test]
    fn test_wasm_release_level() {
        let config = logging_config();
        assert_eq!(config.max_level, tracing::Level::INFO);
    }

    /// WASM tracing output 橋接至 console.log（不 panic 即為通過）
    #[wasm_bindgen_test]
    fn test_wasm_tracing_bridges_to_console() {
        init_logging();
        tracing::info!("WASM tracing 橋接測試");
    }
}
