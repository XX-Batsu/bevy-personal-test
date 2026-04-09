//! NativeWatcher — 原生平台檔案系統監聽器（hot-reload-native feature）。
//!
//! 監聽 `assets/` 目錄的變更，透過 500ms debounce 後回傳受影響的 [`AssetId`] 清單。
//! 此模組僅在 `hot-reload-native` feature 啟用時編譯，不會進入 release build。

#[cfg(feature = "hot-reload-native")]
pub use inner::NativeWatcher;

#[cfg(feature = "hot-reload-native")]
mod inner {
    use asset_manifest::AssetId;
    use crossbeam_channel::Receiver;
    use notify::{Event, RecursiveMode, Watcher};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// 500ms debounce 視窗。
    const DEBOUNCE: Duration = Duration::from_millis(500);

    /// 原生檔案系統監聽器共享狀態。
    struct WatcherState {
        /// 監聽到但尚未回傳的變更集合。
        pending: BTreeSet<String>,
        /// 最後一次收到事件的時間。
        last_event: Option<Instant>,
    }

    impl WatcherState {
        fn new() -> Self {
            Self {
                pending: BTreeSet::new(),
                last_event: None,
            }
        }
    }

    /// 原生檔案系統監聽器。
    ///
    /// 使用 [`notify`] crate 遞迴監聽 `assets/` 目錄。
    /// 變更的檔案路徑以去掉副檔名的相對路徑（= AssetId 字串）收集，
    /// 並在最後一個事件超過 500ms 後由 [`poll_changes`](NativeWatcher::poll_changes) 回傳。
    pub struct NativeWatcher {
        /// 共享狀態（pending 集合 + 最後事件時間）。
        state: Arc<Mutex<WatcherState>>,
        /// notify watcher 持有者（drop 時停止監聽）。
        _watcher: Box<dyn Watcher + Send>,
        /// 被監聽的 assets 根目錄（供診斷與測試使用）。
        #[allow(dead_code)]
        assets_root: PathBuf,
    }

    impl NativeWatcher {
        /// 建立並啟動監聽器，監聽 `assets_root` 下的所有檔案變更。
        pub fn start_watching(assets_root: impl AsRef<Path>) -> notify::Result<Self> {
            // 正規化路徑，避免在 macOS 下 /var → /private/var 符號連結造成
            // strip_prefix 失敗（notify 回傳的路徑為正規化路徑）。
            let assets_root = assets_root
                .as_ref()
                .canonicalize()
                .unwrap_or_else(|_| assets_root.as_ref().to_path_buf());
            let state = Arc::new(Mutex::new(WatcherState::new()));

            let state_clone = state.clone();
            let assets_root_clone = assets_root.clone();

            // 建立 crossbeam channel 接收 notify 事件。
            let (tx, rx): (
                crossbeam_channel::Sender<notify::Result<Event>>,
                Receiver<notify::Result<Event>>,
            ) = crossbeam_channel::unbounded();

            let mut watcher = notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            })?;

            watcher.watch(&assets_root, RecursiveMode::Recursive)?;

            // 背景執行緒處理事件。
            std::thread::spawn(move || {
                for result in rx {
                    let event = match result {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::warn!("NativeWatcher 事件錯誤：{e}");
                            continue;
                        }
                    };

                    // 只處理實際修改內容的事件種類。
                    use notify::EventKind;
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
                        _ => continue,
                    }

                    let mut guard = state_clone.lock().unwrap();
                    let mut any_added = false;
                    for path in &event.paths {
                        if let Some(id) = Self::path_to_asset_id(path, &assets_root_clone) {
                            // 跳過 manifest.ron 變更。
                            if id == "manifest" {
                                continue;
                            }
                            guard.pending.insert(id);
                            any_added = true;
                        }
                    }
                    if any_added {
                        guard.last_event = Some(Instant::now());
                    }
                }
            });

            Ok(Self {
                state,
                _watcher: Box::new(watcher),
                assets_root,
            })
        }

        /// 若自最後一次事件已超過 500ms，則回傳並清空待處理的變更集合。
        /// 若仍在 debounce 視窗內，或尚無事件，則回傳空 Vec。
        pub fn poll_changes(&self) -> Vec<AssetId> {
            let mut guard = self.state.lock().unwrap();

            let last = match guard.last_event {
                Some(t) => t,
                None => return vec![],
            };

            if last.elapsed() < DEBOUNCE {
                return vec![];
            }

            if guard.pending.is_empty() {
                // 已超過 debounce 但 pending 為空（之前已被 drain）。
                guard.last_event = None;
                return vec![];
            }

            // 超過 debounce 視窗，drain pending。
            let ids: Vec<AssetId> = guard
                .pending
                .iter()
                .map(|s| AssetId::new(s.as_str()))
                .collect();
            guard.pending.clear();
            guard.last_event = None;

            ids
        }

        /// 停止監聽（目前透過 drop `_watcher` 自動完成，此方法供明確呼叫使用）。
        pub fn stop_owned(self) {
            // `_watcher` 在 drop 時自動停止；此方法讓呼叫端可以明確表達意圖。
            drop(self);
        }

        /// 將絕對路徑轉換為相對於 `assets_root` 的 AssetId 字串（去掉副檔名）。
        /// 若路徑不在 `assets_root` 下或無法解析，回傳 `None`。
        fn path_to_asset_id(path: &Path, assets_root: &Path) -> Option<String> {
            let rel = path.strip_prefix(assets_root).ok()?;
            // 去掉副檔名。
            let without_ext = rel.with_extension("");
            // 轉為以 `/` 分隔的字串（跨平台）。
            let id = without_ext
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/");
            if id.is_empty() {
                None
            } else {
                Some(id)
            }
        }

        /// 取得監聽的 assets 根目錄（測試用）。
        #[cfg(test)]
        #[allow(dead_code)]
        pub fn assets_root(&self) -> &Path {
            &self.assets_root
        }
    }

    impl super::super::AssetWatcher for NativeWatcher {
        fn poll_changes(&self) -> Vec<AssetId> {
            NativeWatcher::poll_changes(self)
        }

        fn stop(&mut self) {
            // NativeWatcher 的 watcher 在 drop 時自動停止。
            // 此處清空 pending 狀態以表示已停止。
            if let Ok(mut guard) = self.state.lock() {
                guard.pending.clear();
                guard.last_event = None;
            }
            tracing::info!("NativeWatcher 已透過 AssetWatcher::stop() 停止");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;
        use std::time::Duration;

        /// 輔助：重試 poll_changes，最多 `max_attempts` 次，每次間隔 `interval`。
        fn poll_with_retry(
            watcher: &NativeWatcher,
            max_attempts: u32,
            interval: Duration,
        ) -> Vec<AssetId> {
            for _ in 0..max_attempts {
                let result = watcher.poll_changes();
                if !result.is_empty() {
                    return result;
                }
                std::thread::sleep(interval);
            }
            vec![]
        }

        #[test]
        fn test_native_watcher_detects_file_change() {
            let dir = tempfile::tempdir().unwrap();
            let assets_dir = dir.path().join("assets");
            fs::create_dir_all(&assets_dir).unwrap();

            // 建立初始檔案。
            let file_path = assets_dir.join("sprites").join("player.png");
            fs::create_dir_all(file_path.parent().unwrap()).unwrap();
            fs::write(&file_path, b"initial content").unwrap();

            let watcher = NativeWatcher::start_watching(&assets_dir).unwrap();

            // 等待 watcher 啟動穩定（macOS FSEvents 需要初始化時間）。
            std::thread::sleep(Duration::from_millis(100));

            // 修改檔案，觸發事件。
            fs::write(&file_path, b"modified content").unwrap();

            // 等待超過 debounce 視窗（700ms 以上確保穩定）。
            std::thread::sleep(Duration::from_millis(800));

            // 最多重試 5 次，每次等 300ms。
            let changes = poll_with_retry(&watcher, 5, Duration::from_millis(300));
            assert!(
                !changes.is_empty(),
                "應偵測到檔案變更，但 poll_changes 回傳空"
            );
            let ids: Vec<&str> = changes.iter().map(|id| id.as_str()).collect();
            assert!(
                ids.contains(&"sprites/player"),
                "預期 sprites/player，實際：{ids:?}"
            );
        }

        #[test]
        fn test_native_watcher_debounce() {
            let dir = tempfile::tempdir().unwrap();
            let assets_dir = dir.path().join("assets");
            fs::create_dir_all(&assets_dir).unwrap();

            let file_a = assets_dir.join("audio").join("bgm.ogg");
            let file_b = assets_dir.join("audio").join("sfx.ogg");
            fs::create_dir_all(file_a.parent().unwrap()).unwrap();
            fs::write(&file_a, b"a").unwrap();
            fs::write(&file_b, b"b").unwrap();

            let watcher = NativeWatcher::start_watching(&assets_dir).unwrap();

            // 等待 watcher 啟動穩定。
            std::thread::sleep(Duration::from_millis(100));

            // 短時間內連續寫入兩個檔案（模擬快速連續儲存）。
            fs::write(&file_a, b"a modified").unwrap();
            std::thread::sleep(Duration::from_millis(50));
            fs::write(&file_b, b"b modified").unwrap();

            // debounce 視窗內 poll，應回傳空。
            std::thread::sleep(Duration::from_millis(100));
            let during = watcher.poll_changes();
            assert!(
                during.is_empty(),
                "debounce 視窗內不應回傳結果，實際：{during:?}"
            );

            // 等待超過 debounce 視窗後 poll，應回傳兩個 ID。
            std::thread::sleep(Duration::from_millis(800));
            let after = poll_with_retry(&watcher, 5, Duration::from_millis(300));
            assert!(
                !after.is_empty(),
                "debounce 後應回傳變更，但 poll_changes 回傳空"
            );
            let ids: Vec<&str> = after.iter().map(|id| id.as_str()).collect();
            assert!(ids.contains(&"audio/bgm"), "預期 audio/bgm，實際：{ids:?}");
            assert!(ids.contains(&"audio/sfx"), "預期 audio/sfx，實際：{ids:?}");
        }

        #[test]
        fn test_native_watcher_ignores_manifest_ron() {
            let dir = tempfile::tempdir().unwrap();
            let assets_dir = dir.path().join("assets");
            fs::create_dir_all(&assets_dir).unwrap();

            let manifest_path = assets_dir.join("manifest.ron");
            fs::write(&manifest_path, b"initial").unwrap();

            let watcher = NativeWatcher::start_watching(&assets_dir).unwrap();

            // 等待 watcher 啟動穩定。
            std::thread::sleep(Duration::from_millis(100));

            // 修改 manifest.ron。
            fs::write(&manifest_path, b"updated manifest").unwrap();

            // 等待超過 debounce 視窗。
            std::thread::sleep(Duration::from_millis(800));

            let changes = watcher.poll_changes();
            assert!(
                changes.is_empty(),
                "manifest.ron 的變更應被忽略，實際：{changes:?}"
            );
        }
    }
}
