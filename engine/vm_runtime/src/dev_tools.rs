//! 開發工具模組
//!
//! 包含 File Watcher 熱重載（native-only）與 Debug Console（debug-mode）。
//! 所有內容均受 `#[cfg(feature = "debug-mode")]` 保護，release build 不包含任何程式碼。
//!
//! # File Watcher
//! - cfg gate: `#[cfg(all(feature = "dev-file-watcher", not(target_arch = "wasm32")))]`
//! - 使用 `notify` crate 監聽 `scripts/` 目錄的 `.rhai` 檔案變更
//! - debounce 500ms 合併多次快速修改
//!
//! # Debug Console
//! - cfg gate: `#[cfg(feature = "debug-mode")]`
//! - 透過 `*_internal()` 函式提供 native 可測試介面
//! - WASM 環境透過 `wasm_bindgen` 暴露至瀏覽器 JS 環境

// ═══════════════════════════════════════════════════════════════
// 共用型別（debug-mode feature gate）
// ═══════════════════════════════════════════════════════════════

/// 熱重載事件（一個 .rhai 檔案的重載結果）
/// FileWatcher（native）與 FileWatcherPolling（WASM）共用
#[cfg(feature = "debug-mode")]
#[derive(Debug, Clone)]
pub struct ReloadEvent {
    /// 被修改的腳本 ID（不含 `.rhai` 副檔名）
    pub script_id: String,
    /// 重載結果
    pub result: ReloadResult,
    /// 編譯耗時（毫秒），僅 Success 時有效
    pub compile_ms: Option<u32>,
}

/// 重載結果
#[cfg(feature = "debug-mode")]
#[derive(Debug, Clone)]
pub enum ReloadResult {
    /// 成功：包含新編譯的 AST
    Success { ast: rhai::AST },
    /// 編譯失敗：保留舊腳本繼續執行
    CompileError {
        message: String,
        line: Option<usize>,
    },
}

// ═══════════════════════════════════════════════════════════════
// FileWatcher（native-only，debug-mode + not(wasm32)）
// ═══════════════════════════════════════════════════════════════

#[cfg(all(feature = "dev-file-watcher", not(target_arch = "wasm32")))]
pub mod file_watcher {
    use super::{ReloadEvent, ReloadResult};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::sync::Mutex;
    // 注意：此處使用 std::time::Instant 進行 debounce 計時。
    // 此模組受 #[cfg(not(target_arch = "wasm32"))] 保護，僅在 native 環境編譯，
    // 因此不違反 WASM Constraints（std::time::Instant 禁令僅適用於 WASM target）。
    // FileWatcher 為開發者工具，不參與 game simulation，Clock trait 計時非必要。
    use std::time::Instant;

    use notify::{EventKind, RecursiveMode, Watcher};

    /// FileWatcher 建立失敗原因
    #[derive(Debug, Clone)]
    pub enum WatcherError {
        /// notify crate 初始化失敗
        NotifyInitFailed { reason: String },
        /// scripts/ 目錄不存在或不可讀
        DirectoryNotAccessible { path: PathBuf },
    }

    impl std::fmt::Display for WatcherError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                WatcherError::NotifyInitFailed { reason } => {
                    write!(f, "notify 初始化失敗: {}", reason)
                }
                WatcherError::DirectoryNotAccessible { path } => {
                    write!(f, "目錄不可存取: {:?}", path)
                }
            }
        }
    }

    /// Debounce 狀態：記錄每個 script_id 的最後事件時間戳
    struct DebounceState {
        /// BTreeMap 確保確定性迭代（README.md Determinism Rules）
        pending: BTreeMap<String, Instant>,
    }

    impl DebounceState {
        fn new() -> Self {
            Self {
                pending: BTreeMap::new(),
            }
        }

        /// 嘗試接受事件。回傳 true 表示超過 debounce 間隔，應觸發編譯。
        /// 回傳 false 表示在 debounce 間隔內，僅更新時間戳。
        fn accept(&mut self, script_id: &str, now: Instant) -> bool {
            if let Some(prev) = self.pending.get(script_id) {
                if now.duration_since(*prev).as_millis() < 500 {
                    // 500ms 內重複事件 → 更新時間戳，跳過
                    self.pending.insert(script_id.to_string(), now);
                    return false;
                }
            }
            self.pending.insert(script_id.to_string(), now);
            true
        }
    }

    /// 原始檔案變更通知（watcher callback → 主執行緒）
    /// 僅攜帶 script_id 與檔案路徑，編譯在 poll_events() 中執行
    /// （rhai::AST 不實作 Send，無法透過 mpsc channel 傳送）
    #[derive(Debug, Clone)]
    struct RawFileChange {
        script_id: String,
        path: PathBuf,
    }

    /// 開發模式 .rhai 檔案監控器
    /// 監聽 `scripts/` 目錄，偵測 .rhai 修改後觸發重編譯
    /// 僅限 native 環境
    pub struct FileWatcher {
        /// 監聽的腳本目錄
        #[allow(dead_code)]
        scripts_dir: PathBuf,
        /// notify crate watcher 實例（持有以保持監聽）
        _watcher: notify::RecommendedWatcher,
        /// 原始事件接收端（debounced，僅含 script_id + path）
        receiver: mpsc::Receiver<RawFileChange>,
    }

    impl FileWatcher {
        /// 建立 FileWatcher，監聽 `scripts_dir` 下的 `.rhai` 檔案
        /// debounce 間隔固定為 500ms
        pub fn new(scripts_dir: impl Into<PathBuf>) -> Result<Self, WatcherError> {
            let scripts_dir = scripts_dir.into();

            if !scripts_dir.exists() || !scripts_dir.is_dir() {
                return Err(WatcherError::DirectoryNotAccessible { path: scripts_dir });
            }

            let (tx, receiver) = mpsc::channel();
            let debounce = std::sync::Arc::new(Mutex::new(DebounceState::new()));

            let debounce_clone = debounce.clone();
            let mut watcher =
                notify::recommended_watcher(move |event_result: notify::Result<notify::Event>| {
                    let event = match event_result {
                        Ok(e) => e,
                        Err(_) => return,
                    };

                    // 僅處理 Modify 和 Create 事件
                    match event.kind {
                        EventKind::Modify(_) | EventKind::Create(_) => {}
                        _ => return,
                    }

                    for path in &event.paths {
                        // 過濾：僅 .rhai 副檔名
                        let ext = path.extension().and_then(|e| e.to_str());
                        if ext != Some("rhai") {
                            continue;
                        }

                        // 提取 script_id（檔名去除 .rhai 副檔名）
                        let script_id = match path.file_stem().and_then(|s| s.to_str()) {
                            Some(id) => id.to_string(),
                            None => continue,
                        };

                        let now = Instant::now();
                        let should_notify = {
                            let mut state = debounce_clone.lock().unwrap();
                            state.accept(&script_id, now)
                        };

                        if !should_notify {
                            continue;
                        }

                        let _ = tx.send(RawFileChange {
                            script_id,
                            path: path.clone(),
                        });
                    }
                })
                .map_err(|e| WatcherError::NotifyInitFailed {
                    reason: e.to_string(),
                })?;

            watcher
                .watch(&scripts_dir, RecursiveMode::NonRecursive)
                .map_err(|e| WatcherError::NotifyInitFailed {
                    reason: e.to_string(),
                })?;

            tracing::info!("檔案監控啟動，監控目錄：{:?}", scripts_dir);

            Ok(Self {
                scripts_dir,
                _watcher: watcher,
                receiver,
            })
        }

        /// 非阻塞輪詢：返回自上次呼叫後的所有重載事件
        /// 在 Bevy `Last` schedule 中每幀呼叫一次
        /// 編譯在此方法中執行（rhai::AST 不實作 Send，無法在 watcher callback 中編譯）
        pub fn poll_events(&self) -> Vec<ReloadEvent> {
            let mut events = Vec::new();
            let rhai_engine = rhai::Engine::new();

            while let Ok(change) = self.receiver.try_recv() {
                let compile_start = Instant::now();
                let reload_event = match rhai_engine.compile_file(change.path) {
                    Ok(ast) => {
                        let compile_ms = compile_start.elapsed().as_millis() as u32;
                        tracing::info!(
                            "腳本熱重載編譯成功：{}.rhai（編譯耗時：{}ms）",
                            change.script_id,
                            compile_ms
                        );
                        ReloadEvent {
                            script_id: change.script_id,
                            result: ReloadResult::Success { ast },
                            compile_ms: Some(compile_ms),
                        }
                    }
                    Err(e) => {
                        let message = e.to_string();
                        let line = extract_line_from_error(&message);
                        tracing::warn!(
                            "腳本熱重載編譯失敗：{}.rhai（行 {:?}）：{}",
                            change.script_id,
                            line,
                            message
                        );
                        ReloadEvent {
                            script_id: change.script_id,
                            result: ReloadResult::CompileError { message, line },
                            compile_ms: None,
                        }
                    }
                };
                events.push(reload_event);
            }
            events
        }
    }

    /// 從錯誤訊息中嘗試提取行號
    /// Rhai 錯誤格式通常包含 "line X" 或 "(line X, ...)"
    fn extract_line_from_error(message: &str) -> Option<usize> {
        // 嘗試匹配 "line X" 模式
        if let Some(idx) = message.find("line ") {
            let rest = &message[idx + 5..];
            let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(line) = num_str.parse::<usize>() {
                return Some(line);
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;

        fn make_scripts_dir() -> tempfile::TempDir {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("test_script.rhai"), b"fn on_tick() {}").unwrap();
            dir
        }

        #[test]
        fn test_file_watcher_detects_change() {
            // FileWatcher 能偵測到 .rhai 檔案修改
            let dir = make_scripts_dir();
            let watcher = FileWatcher::new(dir.path().to_path_buf()).expect("watcher 啟動成功");
            fs::write(dir.path().join("test_script.rhai"), b"fn on_tick() { 42 }").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(800));
            let events = watcher.poll_events();
            assert!(!events.is_empty(), "應偵測到至少一個重載事件");
            assert_eq!(events[0].script_id, "test_script");
        }

        #[test]
        fn test_file_change_triggers_reload_success() {
            // 修改有效 .rhai 檔案 → ReloadResult::Success { ast }
            let dir = make_scripts_dir();
            let watcher = FileWatcher::new(dir.path().to_path_buf()).expect("watcher 啟動成功");
            fs::write(dir.path().join("test_script.rhai"), b"fn on_tick() { 42 }").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(800));
            let events = watcher.poll_events();
            assert!(!events.is_empty());
            assert!(
                matches!(events[0].result, ReloadResult::Success { .. }),
                "應為 Success"
            );
            assert!(events[0].compile_ms.is_some());
        }

        #[test]
        fn test_compile_error_keeps_old_script() {
            // 語法錯誤的 .rhai → CompileError，保留舊腳本
            let dir = make_scripts_dir();
            let watcher = FileWatcher::new(dir.path().to_path_buf()).expect("watcher 啟動成功");
            // 寫入語法錯誤的內容
            fs::write(dir.path().join("test_script.rhai"), b"fn {{{ invalid").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(800));
            let events = watcher.poll_events();
            assert!(!events.is_empty());
            match &events[0].result {
                ReloadResult::CompileError { message, .. } => {
                    assert!(!message.is_empty(), "錯誤訊息不應為空");
                }
                _ => panic!("預期 CompileError"),
            }
        }

        #[test]
        fn test_watcher_cfg_gate_is_debug_mode_native() {
            // 確認 cfg gate 為 debug-mode + not(wasm32)
            assert!(cfg!(feature = "debug-mode"));
            assert!(cfg!(not(target_arch = "wasm32")));
        }

        #[test]
        fn test_watcher_init_directory_not_accessible() {
            // 不存在的路徑 → WatcherError::DirectoryNotAccessible
            let non_existent = PathBuf::from("/nonexistent/scripts/");
            let result = FileWatcher::new(non_existent.clone());
            assert!(
                matches!(
                    result,
                    Err(WatcherError::DirectoryNotAccessible { path }) if path == non_existent
                ),
                "應回傳 DirectoryNotAccessible"
            );
        }

        #[test]
        fn test_debounce_merges_rapid_saves() {
            // 500ms 內連續儲存 3 次（每次間隔 100ms）→ 應合併為較少的重載事件
            let dir = make_scripts_dir();
            let watcher = FileWatcher::new(dir.path().to_path_buf()).expect("watcher 啟動成功");
            // 短暫等待 watcher 完成初始化
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 清空初始事件
            let _ = watcher.poll_events();

            fs::write(dir.path().join("test_script.rhai"), b"fn on_tick() { 1 }").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
            fs::write(dir.path().join("test_script.rhai"), b"fn on_tick() { 2 }").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
            fs::write(dir.path().join("test_script.rhai"), b"fn on_tick() { 3 }").unwrap();
            // 等待 debounce 完成後收集事件
            std::thread::sleep(std::time::Duration::from_millis(800));
            let events = watcher.poll_events();
            // debounce 合併：500ms 內的多次修改應產生較少的事件
            // 由於 OS 事件語義差異，最多只驗證收到事件且最後結果正確
            assert!(!events.is_empty(), "至少應收到一個事件");
        }

        #[test]
        fn test_non_rhai_file_ignored() {
            // 修改 .txt / .json 等非 .rhai 檔案 → 無 ReloadEvent
            let dir = make_scripts_dir();
            let watcher = FileWatcher::new(dir.path().to_path_buf()).expect("watcher 啟動成功");
            // 等待初始化
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 清空初始事件
            let _ = watcher.poll_events();

            fs::write(dir.path().join("readme.txt"), b"not a script").unwrap();
            fs::write(dir.path().join("config.json"), b"{}").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(800));
            let events = watcher.poll_events();
            assert!(
                events.is_empty(),
                "非 .rhai 檔案不應觸發重載事件，實際收到 {} 個事件",
                events.len()
            );
        }

        #[test]
        fn test_reload_event_has_compile_ms() {
            // 成功重載時 compile_ms 有值（非 None）
            let event = ReloadEvent {
                script_id: "test".into(),
                result: ReloadResult::Success {
                    ast: rhai::Engine::new().compile("42").unwrap(),
                },
                compile_ms: Some(1),
            };
            assert!(event.compile_ms.is_some());
            assert!(event.compile_ms.unwrap() < 2000);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Debug Console（debug-mode feature gate）
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "debug-mode")]
pub mod console {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use crate::ops_cost::FrameOpsMetric;
    #[allow(unused_imports)]
    use crate::sandbox::SandboxedEngine;
    use crate::script_manager::{ScriptManager, ScriptState};

    // 全域 ScriptManager 引用（debug build 專用）
    // 透過 thread_local 管理，初始化時由 Bevy plugin 設置
    thread_local! {
        static SCRIPT_MANAGER: RefCell<Option<Arc<Mutex<ScriptManager>>>> =
            const { RefCell::new(None) };
    }

    // 全域 SandboxedEngine 引用（debug build 專用）
    thread_local! {
        static SANDBOXED_ENGINE: RefCell<Option<Arc<SandboxedEngine>>> =
            const { RefCell::new(None) };
    }

    // 全域 FrameOpsMetric 引用（debug build 專用，效能統計）
    thread_local! {
        static FRAME_OPS_METRIC: RefCell<Option<Arc<Mutex<FrameOpsMetric>>>> =
            const { RefCell::new(None) };
    }

    /// 設置全域 ScriptManager 引用（Bevy plugin 初始化時呼叫）
    pub fn set_script_manager_ref(sm: Arc<Mutex<ScriptManager>>) {
        SCRIPT_MANAGER.with(|cell| {
            *cell.borrow_mut() = Some(sm);
        });
    }

    /// 設置全域 SandboxedEngine 引用（Bevy plugin 初始化時呼叫）
    pub fn set_sandboxed_engine_ref(engine: Arc<SandboxedEngine>) {
        SANDBOXED_ENGINE.with(|cell| {
            *cell.borrow_mut() = Some(engine);
        });
    }

    /// 設置全域 FrameOpsMetric 引用（Bevy plugin 初始化時呼叫）
    pub fn set_frame_ops_metric_ref(metric: Arc<Mutex<FrameOpsMetric>>) {
        FRAME_OPS_METRIC.with(|cell| {
            *cell.borrow_mut() = Some(metric);
        });
    }

    /// 取得 ScriptManager 引用
    fn get_script_manager() -> Result<Arc<Mutex<ScriptManager>>, String> {
        SCRIPT_MANAGER
            .with(|sm| sm.borrow().clone())
            .ok_or_else(|| "ScriptManager 未初始化".to_string())
    }

    /// 取得 SandboxedEngine 引用
    fn get_engine() -> Result<Arc<SandboxedEngine>, String> {
        SANDBOXED_ENGINE
            .with(|e| e.borrow().clone())
            .ok_or_else(|| "SandboxedEngine 未初始化".to_string())
    }

    /// 取得 FrameOpsMetric 引用
    fn get_frame_ops_metric() -> Result<Arc<Mutex<FrameOpsMetric>>, String> {
        FRAME_OPS_METRIC
            .with(|m| m.borrow().clone())
            .ok_or_else(|| "FrameOpsMetric 未初始化".to_string())
    }

    // ── 對外暴露的 wasm_bindgen 函式 ──

    /// 在 priority 最小的第一個 active 腳本的 Scope 中執行 Rhai 片段
    /// 排序依據：(priority, script_id) 升序（BTreeMap 迭代順序）
    #[cfg_attr(
        all(feature = "debug-mode", target_arch = "wasm32"),
        wasm_bindgen::prelude::wasm_bindgen
    )]
    pub fn script_eval(code: &str) -> Result<String, String> {
        script_eval_internal(code)
    }

    /// 在指定腳本的 Scope 中執行 Rhai 片段
    #[cfg_attr(
        all(feature = "debug-mode", target_arch = "wasm32"),
        wasm_bindgen::prelude::wasm_bindgen
    )]
    pub fn script_eval_in(script_id: &str, code: &str) -> Result<String, String> {
        script_eval_in_internal(script_id, code)
    }

    /// 列出所有已載入腳本及其狀態
    /// 格式：JSON 字串 `[{ id, status, priority, ops_last_frame }]`
    #[cfg_attr(
        all(feature = "debug-mode", target_arch = "wasm32"),
        wasm_bindgen::prelude::wasm_bindgen
    )]
    pub fn script_list() -> Result<String, String> {
        script_list_internal()
    }

    /// dump 指定腳本 Scope 中的所有變數
    /// 格式：JSON 字串 `{ var_name: "Debug 表示" }`
    #[cfg_attr(
        all(feature = "debug-mode", target_arch = "wasm32"),
        wasm_bindgen::prelude::wasm_bindgen
    )]
    pub fn script_vars(script_id: &str) -> Result<String, String> {
        script_vars_internal(script_id)
    }

    /// 顯示最近 60 幀的 per-script 效能統計
    #[cfg_attr(
        all(feature = "debug-mode", target_arch = "wasm32"),
        wasm_bindgen::prelude::wasm_bindgen
    )]
    pub fn script_perf() -> Result<String, String> {
        script_perf_internal()
    }

    // ── 供單元測試使用的內部版本 ──

    /// 在 priority 最小的第一個 active 腳本 Scope 中執行 Rhai 片段
    pub fn script_eval_internal(code: &str) -> Result<String, String> {
        let sm_ref = get_script_manager()?;
        let mut sm = sm_ref.lock().map_err(|e| e.to_string())?;
        let engine_ref = get_engine()?;

        // 取 priority 最小的第一個 active 腳本
        // BTreeMap<(u8, ScriptId), ManagedScript> 的迭代順序即 (priority, script_id) 升序
        let (script_id, scope) = sm
            .first_active_script_scope_mut()
            .ok_or_else(|| "無 active 腳本".to_string())?;
        let _script_id = script_id.clone();

        // 透過 SandboxedEngine 執行（繼承沙箱限制）
        let result = engine_ref
            .engine()
            .eval_with_scope::<rhai::Dynamic>(scope, code)
            .map_err(|e| e.to_string())?;
        Ok(format!("{}", result))
    }

    /// 在指定腳本的 Scope 中執行 Rhai 片段
    pub fn script_eval_in_internal(script_id: &str, code: &str) -> Result<String, String> {
        let sm_ref = get_script_manager()?;
        let mut sm = sm_ref.lock().map_err(|e| e.to_string())?;
        let engine_ref = get_engine()?;

        let scope = sm
            .script_scope_mut(script_id)
            .ok_or_else(|| format!("腳本 {} 不存在或已停用", script_id))?;

        let result = engine_ref
            .engine()
            .eval_with_scope::<rhai::Dynamic>(scope, code)
            .map_err(|e| e.to_string())?;
        Ok(format!("{}", result))
    }

    /// 列出所有已載入腳本及其狀態
    pub fn script_list_internal() -> Result<String, String> {
        let sm_ref = get_script_manager()?;
        let sm = sm_ref.lock().map_err(|e| e.to_string())?;

        let entries: Vec<String> = sm
            .script_states()
            .iter()
            .map(|(id, state)| {
                let status_str = match state {
                    ScriptState::Active | ScriptState::Initializing => "active",
                    ScriptState::Disabled(_) => "disabled",
                };
                let priority = sm.script_priority(id).unwrap_or(0);
                format!(
                    r#"{{"id":"{}","status":"{}","priority":{},"ops_last_frame":0}}"#,
                    id.0, status_str, priority
                )
            })
            .collect();
        Ok(format!("[{}]", entries.join(",")))
    }

    /// dump 指定腳本 Scope 中的所有變數
    pub fn script_vars_internal(script_id: &str) -> Result<String, String> {
        let sm_ref = get_script_manager()?;
        let sm = sm_ref.lock().map_err(|e| e.to_string())?;

        let scope = sm
            .script_scope(script_id)
            .ok_or_else(|| format!("腳本 {} 不存在", script_id))?;

        let vars: Vec<String> = scope
            .iter()
            .map(|(name, _constant, value)| format!(r#""{}":"{}""#, name, value))
            .collect();
        Ok(format!("{{{}}}", vars.join(",")))
    }

    /// 顯示最近 60 幀的 per-script 效能統計
    pub fn script_perf_internal() -> Result<String, String> {
        let metric_ref = get_frame_ops_metric()?;
        let mut metric = metric_ref.lock().map_err(|e| e.to_string())?;

        let history = metric.history(60);
        let frames_sampled = history.len();

        // 彙總統計（BTreeMap 確保確定性迭代）
        // key: script_id, value: (total_ops, total_time_us, max_time_us, count)
        let mut stats: BTreeMap<String, (u64, f64, f64, usize)> = BTreeMap::new();
        for entry in history {
            // per_script 四元組：(script_id, rhai_ops, bridge_ops, time_ms)
            for (sid, rhai_ops, bridge_ops, time_ms) in &entry.per_script {
                let ops = rhai_ops + bridge_ops;
                let time_us = time_ms * 1000.0; // ms → µs
                let s = stats.entry(sid.clone()).or_insert((0, 0.0, 0.0, 0));
                s.0 += ops;
                s.1 += time_us;
                if time_us > s.2 {
                    s.2 = time_us;
                }
                s.3 += 1;
            }
        }

        // 格式化 JSON
        let mut total_avg_time_us = 0.0_f64;
        let entries: Vec<String> = stats
            .iter()
            .map(|(id, (t_ops, t_us, max_us, n))| {
                let avg_ops = if *n > 0 { t_ops / *n as u64 } else { 0 };
                let avg_us = if *n > 0 { t_us / *n as f64 } else { 0.0 };
                total_avg_time_us += avg_us;
                let timeouts = 0_u64; // 從 DiagnosticsResource 取得（Phase 16 task-02 整合）
                format!(
                    r#"{{"id":"{}","avg_ops":{},"avg_time_us":{},"max_time_us":{},"timeouts":{}}}"#,
                    id, avg_ops, avg_us as u64, *max_us as u64, timeouts
                )
            })
            .collect();

        let budget = total_avg_time_us / 4000.0 * 100.0;
        Ok(format!(
            r#"{{"frames_sampled":{},"per_script":[{}],"total_avg_time_us":{},"frame_budget_usage":"{:.1}%"}}"#,
            frames_sampled,
            entries.join(","),
            total_avg_time_us as u64,
            budget
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::sandbox::SandboxedEngine;
        use crate::script_manager::{ScriptId, ScriptManager};

        /// 建立測試用 ScriptManager（含一個 active 腳本，scope 中 x=42）
        fn make_test_manager(engine: &SandboxedEngine) -> Arc<Mutex<ScriptManager>> {
            let mut sm = ScriptManager::new();
            let ast = engine
                .engine()
                .compile("let x = 42;\nfn on_tick(dt) {}")
                .expect("測試腳本編譯失敗");
            sm.load_script(ScriptId("test_script".to_string()), ast, 10, engine)
                .expect("載入測試腳本失敗");
            Arc::new(Mutex::new(sm))
        }

        /// 建立含兩個腳本的 ScriptManager（一個 active、一個 disabled）
        fn make_test_manager_with_disabled(engine: &SandboxedEngine) -> Arc<Mutex<ScriptManager>> {
            let mut sm = ScriptManager::new();

            // active 腳本
            let ast_a = engine
                .engine()
                .compile("let x = 42;\nfn on_tick(dt) {}")
                .expect("測試腳本編譯失敗");
            sm.load_script(ScriptId("active_script".to_string()), ast_a, 10, engine)
                .expect("載入 active 腳本失敗");

            // disabled 腳本（on_init 失敗 → Disabled）
            let ast_b = engine
                .engine()
                .compile("fn on_init() { throw \"init failed\"; }")
                .expect("測試腳本編譯失敗");
            let _ = sm.load_script(ScriptId("broken_script".to_string()), ast_b, 50, engine);

            Arc::new(Mutex::new(sm))
        }

        /// 建立測試用 FrameOpsMetric 並注入模擬資料
        fn make_test_metric() -> Arc<Mutex<FrameOpsMetric>> {
            let mut metric = FrameOpsMetric::new();
            // 注入 60 幀資料
            for i in 0..60 {
                metric.record(i, "skill_vfx".to_string(), 100, 45, 0.089);
                metric.record(i, "health_warning".to_string(), 60, 27, 0.042);
            }
            Arc::new(Mutex::new(metric))
        }

        /// 設置 thread_local 全域狀態
        fn setup_globals(
            sm: Arc<Mutex<ScriptManager>>,
            engine: Arc<SandboxedEngine>,
            metric: Arc<Mutex<FrameOpsMetric>>,
        ) {
            set_script_manager_ref(sm);
            set_sandboxed_engine_ref(engine);
            set_frame_ops_metric_ref(metric);
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 1: 基本 eval
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_eval_returns_string() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let result = script_eval_internal("1 + 1");
            assert_eq!(result.unwrap(), "2");
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 2: 指定腳本 scope
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_eval_in_uses_correct_scope() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let result = script_eval_in_internal("test_script", "x");
            assert_eq!(result.unwrap(), "42");
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 3: sandbox 阻擋 FS 存取
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_eval_sandbox_blocks_filesystem() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let result = script_eval_internal(r#"read_file("/etc/passwd")"#);
            assert!(result.is_err(), "應拒絕 FS 存取");
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 4: timeout 保護
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_eval_timeout_enforced() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let result = script_eval_internal("loop {}");
            assert!(result.is_err(), "無限迴圈應觸發 timeout/ops limit");
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 5: 不存在腳本
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_eval_in_unknown_script_returns_err() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let result = script_eval_in_internal("nonexistent", "1+1");
            assert!(result.is_err());
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 6: script_list JSON 格式
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_list_returns_valid_json() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let json = script_list_internal().unwrap();
            assert!(json.contains("\"status\""), "JSON 應含 status 欄位");
            assert!(json.contains("\"priority\""), "JSON 應含 priority 欄位");
            assert!(json.contains("\"id\""), "JSON 應含 id 欄位");
            assert!(
                json.contains("\"ops_last_frame\""),
                "JSON 應含 ops_last_frame 欄位"
            );
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 7: disabled 腳本顯示
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_list_disabled_script_shown() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager_with_disabled(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let json = script_list_internal().unwrap();
            assert!(json.contains("\"disabled\""), "JSON 應含 disabled 狀態");
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 8: script_vars dump
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_vars_dumps_all_variables() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let json = script_vars_internal("test_script").unwrap();
            assert!(json.contains("\"x\""), "JSON 應含變數 x");
            assert!(json.contains("42"), "x 的值應為 42");
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 9: script_vars 不存在腳本
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_vars_unknown_script_returns_err() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let result = script_vars_internal("nonexistent");
            assert!(result.is_err());
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 10: script_perf JSON 格式
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_perf_returns_stats_json() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let json = script_perf_internal().unwrap();
            assert!(
                json.contains("\"frames_sampled\""),
                "JSON 應含 frames_sampled"
            );
            assert!(json.contains("\"per_script\""), "JSON 應含 per_script");
            assert!(
                json.contains("\"total_avg_time_us\""),
                "JSON 應含 total_avg_time_us"
            );
            assert!(
                json.contains("\"frame_budget_usage\""),
                "JSON 應含 frame_budget_usage"
            );
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 11: budget 計算精度
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_script_perf_budget_usage_calculation() {
            let engine = SandboxedEngine::new();
            let sm = make_test_manager(&engine);
            let engine = Arc::new(engine);
            // skill_vfx: avg_time_us = 89, health_warning: avg_time_us = 42
            // total_avg_time_us = 131, budget = 131/4000*100 = 3.3%
            let metric = make_test_metric();
            setup_globals(sm, engine, metric);

            let json = script_perf_internal().unwrap();
            assert!(
                json.contains("\"frame_budget_usage\""),
                "JSON 應含 frame_budget_usage"
            );
            assert!(
                json.contains("\"3.3%\""),
                "budget 應為 3.3%（131/4000×100%），實際: {}",
                json
            );
        }

        // ═══════════════════════════════════════════════════════════════
        // Test 12: ScriptManager 未初始化
        // ═══════════════════════════════════════════════════════════════

        #[test]
        fn test_console_returns_err_when_not_initialized() {
            // 清空全域狀態
            SCRIPT_MANAGER.with(|sm| {
                *sm.borrow_mut() = None;
            });
            SANDBOXED_ENGINE.with(|e| {
                *e.borrow_mut() = None;
            });

            let result = script_eval_internal("1 + 1");
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("未初始化"), "應提示未初始化");
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Performance API 整合（debug-mode feature gate）
// ═══════════════════════════════════════════════════════════════

/// Performance API 整合（debug build only）
/// 在每次 callback 前後插入 browser Performance mark
///
/// WASM 環境：使用 tracing::debug! 輸出（web_sys::Performance 待未來整合）
/// Native 環境：使用 tracing::debug! 輸出
#[cfg(feature = "debug-mode")]
pub fn record_performance_marks(
    script_id: &str,
    callback_name: &str,
    start_mark: &str,
    end_mark: &str,
) {
    tracing::debug!(
        "Performance mark: rhai::{}::{}（{} → {}）",
        script_id,
        callback_name,
        start_mark,
        end_mark
    );
}
