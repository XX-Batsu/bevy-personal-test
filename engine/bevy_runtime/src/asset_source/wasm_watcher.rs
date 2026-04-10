//! WasmWatcher — WebSocket-based asset watcher for WASM target。
//! feature gate: hot-reload-wasm
//!
//! Dev server 在 `ws://localhost:<port>/ws` 推送變更通知：
//! `{ "changed": ["sprites/player/idle", "audio/bgm"] }`

#[cfg(all(target_arch = "wasm32", feature = "hot-reload-wasm"))]
mod inner {
    use asset_manifest::AssetId;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;
    use web_sys::{MessageEvent, WebSocket};

    /// Debounce 門檻（毫秒）。收到最後一個事件後，
    /// 須超過此時間 `poll_changes()` 才會回傳結果。
    const DEBOUNCE_MS: f64 = 500.0;

    /// WASM WebSocket watcher — 接收 dev server 的素材變更通知。
    pub struct WasmWatcher {
        ws_url: String,
        pending: Rc<RefCell<Vec<String>>>,
        last_event_time: Rc<RefCell<Option<f64>>>,
        ws: Option<WebSocket>,
    }

    impl WasmWatcher {
        pub fn new(ws_url: String) -> Self {
            Self {
                ws_url,
                pending: Rc::new(RefCell::new(Vec::new())),
                last_event_time: Rc::new(RefCell::new(None)),
                ws: None,
            }
        }

        /// 啟動 WebSocket 連線。只能呼叫一次。
        pub fn start_watching(&mut self) {
            if self.ws.is_some() {
                tracing::warn!("WasmWatcher 已在監聽中，忽略重複呼叫");
                return;
            }

            let ws = match WebSocket::new(&self.ws_url) {
                Ok(ws) => ws,
                Err(e) => {
                    tracing::error!("建立 WebSocket 失敗：{:?}", e);
                    return;
                }
            };

            let pending = self.pending.clone();
            let last_event_time = self.last_event_time.clone();

            let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                if let Ok(text) = e.data().dyn_into::<js_sys::JsString>() {
                    let text: String = text.into();
                    // 使用 serde_json 解析 JSON
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(value) => {
                            if let Some(changed) = value.get("changed").and_then(|v| v.as_array()) {
                                let mut guard = pending.borrow_mut();
                                for item in changed {
                                    if let Some(s) = item.as_str() {
                                        if !s.is_empty() {
                                            guard.push(s.to_string());
                                        }
                                    }
                                }
                                // 更新最後事件時間（用 performance.now()）
                                if let Some(perf) = web_sys::window().and_then(|w| w.performance())
                                {
                                    *last_event_time.borrow_mut() = Some(perf.now());
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("WasmWatcher JSON 解析失敗：{e}，原始訊息：{text}");
                        }
                    }
                }
            });

            ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            // Intentional leak：Closure 必須在 WebSocket 存活期間有效。
            // WebSocket 的 onmessage callback 會持續引用此 Closure，
            // 若 drop 會導致 callback 指向無效記憶體。
            onmessage.forget();

            let ws_url = self.ws_url.clone();
            let onerror = Closure::<dyn FnMut()>::new(move || {
                tracing::warn!("WasmWatcher WebSocket 連線錯誤：{ws_url}");
            });
            ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
            // Intentional leak：同 onmessage，onerror callback 須在 WebSocket 存活期間有效。
            onerror.forget();

            self.ws = Some(ws);
            tracing::info!("WasmWatcher 已啟動：{}", self.ws_url);
        }

        /// 輪詢自上次呼叫以來的變更清單（非阻塞）。
        /// 加入 debounce：最後一個事件超過 DEBOUNCE_MS 後才回傳。
        pub fn poll_changes(&self) -> Vec<AssetId> {
            let guard = self.pending.borrow();
            if guard.is_empty() {
                return Vec::new();
            }

            // Debounce 檢查
            if let Some(last_time) = *self.last_event_time.borrow() {
                if let Some(perf) = web_sys::window().and_then(|w| w.performance()) {
                    let now = perf.now();
                    if now - last_time < DEBOUNCE_MS {
                        return Vec::new();
                    }
                }
            }
            drop(guard);

            let mut pending = self.pending.borrow_mut();
            let ids: Vec<AssetId> = pending.drain(..).map(|s| AssetId::new(s)).collect();
            *self.last_event_time.borrow_mut() = None;
            tracing::debug!("WasmWatcher 偵測到 {} 個素材變更", ids.len());
            ids
        }

        /// 關閉 WebSocket。
        pub fn stop(&mut self) {
            if let Some(ws) = self.ws.take() {
                let _ = ws.close();
                tracing::info!("WasmWatcher 已停止");
            }
        }
    }

    impl super::super::AssetWatcher for WasmWatcher {
        fn poll_changes(&self) -> Vec<AssetId> {
            WasmWatcher::poll_changes(self)
        }

        fn stop(&mut self) {
            WasmWatcher::stop(self)
        }
    }
}

#[cfg(all(target_arch = "wasm32", feature = "hot-reload-wasm"))]
pub use inner::WasmWatcher;
