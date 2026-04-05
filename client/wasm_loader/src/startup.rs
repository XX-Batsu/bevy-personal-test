//! 冷啟動 6 階段分段載入狀態機。
//!
//! 對應 18-non-functional/startup-performance.md 六個時間目標。
//! 提供 [`StartupStage`]（階段列舉）與 [`StartupProgress`]（狀態機管理器）。

/// 啟動階段（對應 18-non-functional/startup-performance.md 六個時間目標）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum StartupStage {
    /// JS Loader 下載 + 執行（0-10%）
    JsLoader = 0,
    /// WASM bootstrap 下載 + instantiate（10-30%）
    WasmBootstrap = 1,
    /// ECDH 金鑰交換（30-45%）
    EcdhHandshake = 2,
    /// Bevy Engine 初始化（45-60%）
    BevyInit = 3,
    /// 主 Assets 解密 + 載入（60-90%）
    AssetDecrypt = 4,
    /// Game Ready — 首幀渲染（90-100%）
    GameReady = 5,
}

impl StartupStage {
    /// 每個 stage 的進度範圍（起始 %, 結束 %）
    pub fn progress_range(self) -> (u8, u8) {
        match self {
            StartupStage::JsLoader => (0, 10),
            StartupStage::WasmBootstrap => (10, 30),
            StartupStage::EcdhHandshake => (30, 45),
            StartupStage::BevyInit => (45, 60),
            StartupStage::AssetDecrypt => (60, 90),
            StartupStage::GameReady => (90, 100),
        }
    }

    /// Stage 完成後的進度百分比（= progress_range().1）
    pub fn completion_percent(self) -> u8 {
        self.progress_range().1
    }

    /// Stage 的人類可讀名稱（zh-TW）
    pub fn display_name(self) -> &'static str {
        match self {
            StartupStage::JsLoader => "載入器初始化",
            StartupStage::WasmBootstrap => "WASM 啟動",
            StartupStage::EcdhHandshake => "金鑰交換",
            StartupStage::BevyInit => "引擎初始化",
            StartupStage::AssetDecrypt => "資產解密",
            StartupStage::GameReady => "遊戲準備就緒",
        }
    }

    /// 取得下一個 stage（若已是最後 stage 回傳 None）
    pub fn next(self) -> Option<StartupStage> {
        match self {
            StartupStage::JsLoader => Some(StartupStage::WasmBootstrap),
            StartupStage::WasmBootstrap => Some(StartupStage::EcdhHandshake),
            StartupStage::EcdhHandshake => Some(StartupStage::BevyInit),
            StartupStage::BevyInit => Some(StartupStage::AssetDecrypt),
            StartupStage::AssetDecrypt => Some(StartupStage::GameReady),
            StartupStage::GameReady => None,
        }
    }
}

/// 啟動流程錯誤
#[derive(Debug, Clone)]
pub enum StartupError {
    /// JS Loader 失敗
    JsLoaderFailed(String),
    /// WASM bootstrap 失敗（可重試，最多 3 次）
    WasmBootstrapFailed(String),
    /// ECDH 金鑰交換失敗（可重試，最多 1 次，共 2 次嘗試）
    EcdhFailed(String),
    /// Bevy 初始化失敗（不可重試，需重載頁面）
    BevyInitFailed(String),
    /// Asset 解密���敗（不可重試，需重載頁面）
    AssetDecryptFailed(String),
    /// 所有 stage 已完成，不可再推進
    AlreadyComplete,
}

/// 失敗原因（用於決定是否可重試）
#[derive(Debug, Clone)]
pub enum StartupFailReason {
    /// 網路錯誤（通常可重試）
    NetworkError(String),
    /// 解密失敗（資料損毀，需重載）
    DecryptionError(String),
    /// 初始化失敗（邏輯錯誤，需重載）
    InitError(String),
}

/// JS 側 callback 函式簽名（僅 WASM 環境）
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    /// 更新前端載入進度條
    #[wasm_bindgen(js_namespace = ["window", "__game"])]
    fn js_update_progress(percent: u8, stage_name: &str);
}

/// 進度回報 callback 型別（percent: u8, stage_name: &str）
pub type ProgressCallback = Box<dyn Fn(u8, &str)>;

/// 啟動進度管理器（WASM client 側）
///
/// ## Callback 機制（Native vs WASM）
///
/// - **Native 測試**：透過 `progress_callback: Option<ProgressCallback>`
///   進行進度回報。
/// - **WASM 生產**：額外呼叫 `#[wasm_bindgen]` FFI
///   `js_update_progress(percent, stage_name)`。
pub struct StartupProgress {
    current_stage: StartupStage,
    /// 所有 stage 是否已完成（GameReady stage 完成後設為 true）
    completed: bool,
    progress_callback: Option<ProgressCallback>,
}

impl StartupProgress {
    /// 建���新的進度管理器
    ///
    /// 初始狀態：`current_stage = JsLoader`，`progress_percent() == 0`
    pub fn new(callback: Option<ProgressCallback>) -> Self {
        Self {
            current_stage: StartupStage::JsLoader,
            completed: false,
            progress_callback: callback,
        }
    }

    /// 取得目前所在 stage
    pub fn current_stage(&self) -> StartupStage {
        self.current_stage
    }

    /// 取得目前進度百分比（0-100）
    ///
    /// 回傳 `current_stage.progress_range().0`（stage 的起始百分比），
    /// 表示「已推進至此 stage 但尚未完成」。
    /// 特例：所有 stage 完成後回傳 100。
    pub fn progress_percent(&self) -> u8 {
        if self.completed {
            100
        } else {
            self.current_stage.progress_range().0
        }
    }

    /// 標記目��� stage 完成，自動推進至下一 stage
    ///
    /// 推進後觸發 callback（若有設定），參數為新 stage 的起始百分比與名稱。
    ///
    /// # Errors
    /// `StartupError::AlreadyComplete` — 已在 `GameReady` stage 且該 stage 已完成後仍呼叫
    pub fn on_stage_complete(&mut self) -> Result<(), StartupError> {
        if self.completed {
            return Err(StartupError::AlreadyComplete);
        }
        match self.current_stage.next() {
            Some(next) => {
                self.current_stage = next;
                let percent = next.progress_range().0;
                let name = next.display_name();
                if let Some(ref cb) = self.progress_callback {
                    cb(percent, name);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    js_update_progress(percent, name);
                }
                Ok(())
            }
            None => {
                // GameReady 完成：標記 completed
                self.completed = true;
                if let Some(ref cb) = self.progress_callback {
                    cb(100, self.current_stage.display_name());
                }
                #[cfg(target_arch = "wasm32")]
                {
                    js_update_progress(100, self.current_stage.display_name());
                }
                Ok(())
            }
        }
    }

    /// 標記目前 stage 失敗，回傳對應的 `StartupError`
    ///
    /// ## 語義約束
    /// - **不改變 `current_stage`**：失敗後 stage 不推進、不回退
    /// - **重試由呼叫端決定**
    /// - **`GameReady` stage 回傳 `AlreadyComplete`**
    pub fn on_stage_failed(&mut self, reason: StartupFailReason) -> StartupError {
        if self.completed {
            return StartupError::AlreadyComplete;
        }
        let msg = match &reason {
            StartupFailReason::NetworkError(s) => s.clone(),
            StartupFailReason::DecryptionError(s) => s.clone(),
            StartupFailReason::InitError(s) => s.clone(),
        };
        match self.current_stage {
            StartupStage::JsLoader => StartupError::JsLoaderFailed(msg),
            StartupStage::WasmBootstrap => StartupError::WasmBootstrapFailed(msg),
            StartupStage::EcdhHandshake => StartupError::EcdhFailed(msg),
            StartupStage::BevyInit => StartupError::BevyInitFailed(msg),
            StartupStage::AssetDecrypt => StartupError::AssetDecryptFailed(msg),
            StartupStage::GameReady => StartupError::AlreadyComplete,
        }
    }
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 建立帶有記錄功能的 callback，回傳 (callback, 記錄)
    fn tracking_callback() -> (Box<dyn Fn(u8, &str)>, Rc<RefCell<Vec<(u8, String)>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let log_clone = log.clone();
        let cb = Box::new(move |percent: u8, name: &str| {
            log_clone.borrow_mut().push((percent, name.to_string()));
        });
        (cb, log)
    }

    #[test]
    fn test_initial_state() {
        let sp = StartupProgress::new(None);
        assert_eq!(sp.current_stage(), StartupStage::JsLoader);
        assert_eq!(sp.progress_percent(), 0);
    }

    #[test]
    fn test_stages_execute_in_order() {
        let mut sp = StartupProgress::new(None);
        let expected_stages = [
            StartupStage::WasmBootstrap,
            StartupStage::EcdhHandshake,
            StartupStage::BevyInit,
            StartupStage::AssetDecrypt,
            StartupStage::GameReady,
        ];
        for expected in &expected_stages {
            sp.on_stage_complete().unwrap();
            assert_eq!(sp.current_stage(), *expected);
        }
        // GameReady 完成
        sp.on_stage_complete().unwrap();
        // 再次呼叫 → AlreadyComplete
        assert!(matches!(
            sp.on_stage_complete(),
            Err(StartupError::AlreadyComplete)
        ));
    }

    #[test]
    fn test_progress_percent_at_each_stage() {
        let mut sp = StartupProgress::new(None);
        assert_eq!(sp.progress_percent(), 0); // JsLoader 起始

        sp.on_stage_complete().unwrap(); // → WasmBootstrap
        assert_eq!(sp.progress_percent(), 10);

        sp.on_stage_complete().unwrap(); // → EcdhHandshake
        assert_eq!(sp.progress_percent(), 30);

        sp.on_stage_complete().unwrap(); // → BevyInit
        assert_eq!(sp.progress_percent(), 45);

        sp.on_stage_complete().unwrap(); // → AssetDecrypt
        assert_eq!(sp.progress_percent(), 60);

        sp.on_stage_complete().unwrap(); // → GameReady
        assert_eq!(sp.progress_percent(), 90);

        sp.on_stage_complete().unwrap(); // GameReady 完成
        assert_eq!(sp.progress_percent(), 100);
    }

    #[test]
    fn test_callback_called_each_stage() {
        let (cb, log) = tracking_callback();
        let mut sp = StartupProgress::new(Some(cb));

        // 完成 JsLoader → 推進至 WasmBootstrap
        sp.on_stage_complete().unwrap();
        sp.on_stage_complete().unwrap();
        sp.on_stage_complete().unwrap();
        sp.on_stage_complete().unwrap();
        sp.on_stage_complete().unwrap();
        // 完成 GameReady
        sp.on_stage_complete().unwrap();

        let log = log.borrow();
        assert_eq!(log.len(), 6); // 5 次推進 + 1 次 GameReady 完成
        assert_eq!(log[0], (10, "WASM 啟動".to_string()));
        assert_eq!(log[1], (30, "金鑰交換".to_string()));
        assert_eq!(log[2], (45, "引擎初始化".to_string()));
        assert_eq!(log[3], (60, "資產解密".to_string()));
        assert_eq!(log[4], (90, "遊戲準備就緒".to_string()));
        assert_eq!(log[5], (100, "遊戲準備就緒".to_string()));
    }

    #[test]
    fn test_callback_not_called_when_none() {
        let mut sp = StartupProgress::new(None);
        // 不 panic 即為通過
        for _ in 0..6 {
            sp.on_stage_complete().unwrap();
        }
        assert_eq!(sp.progress_percent(), 100);
    }

    #[test]
    fn test_stage_failed_maps_to_correct_error() {
        // JsLoader
        let mut sp = StartupProgress::new(None);
        assert!(matches!(
            sp.on_stage_failed(StartupFailReason::NetworkError("test".into())),
            StartupError::JsLoaderFailed(_)
        ));

        // WasmBootstrap
        sp.on_stage_complete().unwrap();
        assert!(matches!(
            sp.on_stage_failed(StartupFailReason::NetworkError("test".into())),
            StartupError::WasmBootstrapFailed(_)
        ));

        // EcdhHandshake
        sp.on_stage_complete().unwrap();
        assert!(matches!(
            sp.on_stage_failed(StartupFailReason::NetworkError("test".into())),
            StartupError::EcdhFailed(_)
        ));

        // BevyInit
        sp.on_stage_complete().unwrap();
        assert!(matches!(
            sp.on_stage_failed(StartupFailReason::InitError("test".into())),
            StartupError::BevyInitFailed(_)
        ));

        // AssetDecrypt
        sp.on_stage_complete().unwrap();
        assert!(matches!(
            sp.on_stage_failed(StartupFailReason::DecryptionError("test".into())),
            StartupError::AssetDecryptFailed(_)
        ));
    }

    #[test]
    fn test_stage_failed_preserves_reason_message() {
        let mut sp = StartupProgress::new(None);
        let err = sp.on_stage_failed(StartupFailReason::NetworkError("timeout".into()));
        match err {
            StartupError::JsLoaderFailed(msg) => assert!(msg.contains("timeout")),
            _ => panic!("預期 JsLoaderFailed"),
        }
    }

    #[test]
    fn test_stage_failed_does_not_advance() {
        let mut sp = StartupProgress::new(None);
        sp.on_stage_complete().unwrap(); // → WasmBootstrap
        sp.on_stage_failed(StartupFailReason::NetworkError("err".into()));
        assert_eq!(sp.current_stage(), StartupStage::WasmBootstrap);
    }

    #[test]
    fn test_retry_after_failure() {
        let mut sp = StartupProgress::new(None);
        sp.on_stage_complete().unwrap(); // → WasmBootstrap
        sp.on_stage_failed(StartupFailReason::NetworkError("err".into()));
        assert_eq!(sp.current_stage(), StartupStage::WasmBootstrap);
        // 重試成功
        sp.on_stage_complete().unwrap(); // → EcdhHandshake
        assert_eq!(sp.current_stage(), StartupStage::EcdhHandshake);
    }

    #[test]
    fn test_game_ready_failed_returns_already_complete() {
        let mut sp = StartupProgress::new(None);
        // 推進至 GameReady
        for _ in 0..5 {
            sp.on_stage_complete().unwrap();
        }
        assert_eq!(sp.current_stage(), StartupStage::GameReady);
        let err = sp.on_stage_failed(StartupFailReason::InitError("err".into()));
        assert!(matches!(err, StartupError::AlreadyComplete));
    }

    #[test]
    fn test_already_complete_error() {
        let mut sp = StartupProgress::new(None);
        for _ in 0..6 {
            sp.on_stage_complete().unwrap();
        }
        assert!(matches!(
            sp.on_stage_complete(),
            Err(StartupError::AlreadyComplete)
        ));
        // 冪等
        assert!(matches!(
            sp.on_stage_complete(),
            Err(StartupError::AlreadyComplete)
        ));
    }

    #[test]
    fn test_already_complete_then_failed() {
        let mut sp = StartupProgress::new(None);
        for _ in 0..6 {
            sp.on_stage_complete().unwrap();
        }
        let err = sp.on_stage_failed(StartupFailReason::NetworkError("err".into()));
        assert!(matches!(err, StartupError::AlreadyComplete));
    }

    #[test]
    fn test_progress_range_coverage() {
        let stages = [
            StartupStage::JsLoader,
            StartupStage::WasmBootstrap,
            StartupStage::EcdhHandshake,
            StartupStage::BevyInit,
            StartupStage::AssetDecrypt,
            StartupStage::GameReady,
        ];
        let expected_ranges = [(0, 10), (10, 30), (30, 45), (45, 60), (60, 90), (90, 100)];
        for (stage, expected) in stages.iter().zip(expected_ranges.iter()) {
            assert_eq!(stage.progress_range(), *expected, "stage {:?}", stage);
        }
        // 首尾相接
        for i in 0..stages.len() - 1 {
            assert_eq!(
                stages[i].progress_range().1,
                stages[i + 1].progress_range().0,
                "stage {:?} 結束值應等於 {:?} 起始值",
                stages[i],
                stages[i + 1],
            );
        }
    }

    #[test]
    fn test_progress_range_starts_at_zero_ends_at_hundred() {
        assert_eq!(StartupStage::JsLoader.progress_range().0, 0);
        assert_eq!(StartupStage::GameReady.progress_range().1, 100);
    }

    #[test]
    fn test_display_name_all_stages() {
        let stages = [
            (StartupStage::JsLoader, "載入器初始化"),
            (StartupStage::WasmBootstrap, "WASM 啟動"),
            (StartupStage::EcdhHandshake, "金鑰交換"),
            (StartupStage::BevyInit, "引擎初始化"),
            (StartupStage::AssetDecrypt, "資產解密"),
            (StartupStage::GameReady, "遊戲準備就緒"),
        ];
        for (stage, expected) in &stages {
            assert_eq!(stage.display_name(), *expected);
            // 驗證包含中文字元
            assert!(
                expected
                    .chars()
                    .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
                "{:?} 的 display_name 未包含中文",
                stage,
            );
        }
    }

    #[test]
    fn test_stage_next() {
        assert_eq!(
            StartupStage::JsLoader.next(),
            Some(StartupStage::WasmBootstrap)
        );
        assert_eq!(
            StartupStage::WasmBootstrap.next(),
            Some(StartupStage::EcdhHandshake)
        );
        assert_eq!(
            StartupStage::EcdhHandshake.next(),
            Some(StartupStage::BevyInit)
        );
        assert_eq!(
            StartupStage::BevyInit.next(),
            Some(StartupStage::AssetDecrypt)
        );
        assert_eq!(
            StartupStage::AssetDecrypt.next(),
            Some(StartupStage::GameReady)
        );
        assert_eq!(StartupStage::GameReady.next(), None);
    }

    #[test]
    fn test_stage_ord_matches_discriminant() {
        assert!(StartupStage::JsLoader < StartupStage::WasmBootstrap);
        assert!(StartupStage::WasmBootstrap < StartupStage::EcdhHandshake);
        assert!(StartupStage::EcdhHandshake < StartupStage::BevyInit);
        assert!(StartupStage::BevyInit < StartupStage::AssetDecrypt);
        assert!(StartupStage::AssetDecrypt < StartupStage::GameReady);
    }
}
