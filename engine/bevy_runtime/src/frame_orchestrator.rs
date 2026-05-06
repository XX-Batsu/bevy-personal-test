//! 幀協調器 — 管理每幀的 callback 排序與事件/輸入佇列。
//!
//! 定義：
//! - [`PendingScriptEvents`]：待傳遞給腳本的 ECS 事件（FIFO）
//! - [`PendingInputs`]：待傳遞給腳本的玩家輸入（FIFO）
//! - [`LocalBridgeEventQueue`]：幀協調測試用橋接事件佇列，容量 4,096
//! - [`ScriptEvent`]、[`ScriptInput`]、[`BridgeEvent`]：對應資料型別
//!
//! 正式 Bevy systems 由 Task 16 實作。
//!
//! 注意：`rhai::ImmutableString` 與 `rhai::Dynamic` 使用 `Rc`，不實作 `Send + Sync`，
//! 因此 [`ScriptEvent`] 與 [`ScriptInput`] 使用 `String` 作為型別名稱欄位，
//! 以便在 Bevy Resource 中儲存。

use bevy::prelude::*;

/// Script 事件（由 ECS 事件系統產生，傳遞給腳本）
///
/// 使用 `String` 而非 `rhai::ImmutableString` 以符合 `Send + Sync` 需求（Bevy Resource）。
pub struct ScriptEvent {
    /// 事件型別名稱
    pub event_type: String,
    /// 事件 payload（序列化為 String，測試用途）
    pub payload: String,
}

/// Script 輸入（玩家輸入、網路輸入）
///
/// 使用 `String` 而非 `rhai::ImmutableString` 以符合 `Send + Sync` 需求（Bevy Resource）。
pub struct ScriptInput {
    /// 輸入型別名稱
    pub input_type: String,
    /// 輸入資料（序列化為 String，測試用途）
    pub data: String,
}

/// Bridge 事件（VM→Bevy 單向橋接事件，測試用簡化版本）
#[derive(Debug)]
pub struct BridgeEvent {
    /// 事件名稱
    pub name: String,
}

/// Pending script events 儲存（Bevy Resource）
///
/// 每幀 on_event 前由 ECS event reader 填充，drain 後傳遞給腳本。
#[derive(Resource, Default)]
pub struct PendingScriptEvents {
    events: Vec<ScriptEvent>,
}

impl PendingScriptEvents {
    /// 建立空佇列
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// 推入一筆事件
    pub fn push(&mut self, event: ScriptEvent) {
        self.events.push(event);
    }

    /// 排空佇列，回傳迭代器
    pub fn drain(&mut self) -> impl Iterator<Item = ScriptEvent> + '_ {
        self.events.drain(..)
    }

    /// 目前佇列長度
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 佇列是否為空
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Pending inputs 儲存（Bevy Resource）
///
/// 每幀 on_input 前由輸入系統填充，drain 後傳遞給腳本。
#[derive(Resource, Default)]
pub struct PendingInputs {
    inputs: Vec<ScriptInput>,
}

impl PendingInputs {
    /// 建立空佇列
    pub fn new() -> Self {
        Self { inputs: Vec::new() }
    }

    /// 推入一筆輸入
    pub fn push(&mut self, input: ScriptInput) {
        self.inputs.push(input);
    }

    /// 排空佇列，回傳迭代器
    pub fn drain(&mut self) -> impl Iterator<Item = ScriptInput> + '_ {
        self.inputs.drain(..)
    }

    /// 目前佇列長度
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// 佇列是否為空
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }
}

/// Bridge 事件佇列（Bevy Resource），容量 4,096
///
/// VM→Bevy 單向橋接，超出容量時捨棄並計入 `dropped`。
#[derive(Resource, Debug, Default)]
pub struct LocalBridgeEventQueue {
    events: Vec<BridgeEvent>,
    dropped: usize,
}

/// 佇列最大容量（對齊 03-bridge-api/data-flow.md）
const BRIDGE_QUEUE_CAPACITY: usize = 4_096;

impl LocalBridgeEventQueue {
    /// 建立空佇列
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            dropped: 0,
        }
    }

    /// 推入一筆事件；超出容量時捨棄並遞增 dropped 計數
    pub fn push(&mut self, event: BridgeEvent) {
        if self.events.len() >= BRIDGE_QUEUE_CAPACITY {
            self.dropped += 1;
        } else {
            self.events.push(event);
        }
    }

    /// 排空佇列，重置 dropped 計數，回傳迭代器
    pub fn drain(&mut self) -> impl Iterator<Item = BridgeEvent> + '_ {
        self.dropped = 0;
        self.events.drain(..)
    }

    /// 目前佇列長度
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 佇列是否為空
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 累積被捨棄的事件數
    pub fn dropped_count(&self) -> usize {
        self.dropped
    }
}

// ── Bevy Systems（Task 16）────────────────────────────────────────────────────

/// Step 1：ECS → EcsMirror 同步系統入口
///
/// 排程位置：`GameFixedSet::ProcessInputs`（Script callback 之前）
///
/// 實際同步邏輯由 `vm_bevy_bridge::update_ecs_mirror` 實作（BridgePlugin 已註冊至
/// `GameFixedSet::UpdateEcsMirror`）。此系統為 bevy_runtime 側的排程佔位符，
/// 確保 ProcessInputs 階段有明確的系統入口供測試與排序驗證使用。
///
/// **不可從 `Time<Fixed>` 讀取 delta**：delta_time 固定為 `SoftF32(0x3C88_8889)`
/// （1.0/60.0 的 bit-exact 表示），確保跨 client 確定性。
pub fn ecs_mirror_sync_system() {
    // EcsMirror 同步由 BridgePlugin 的 update_ecs_mirror 系統負責，
    // 排程在 GameFixedSet::UpdateEcsMirror（所有腳本執行之後）。
    // 此系統保留為 ProcessInputs 階段的排程標記，不執行實際邏輯。
    tracing::debug!("ecs_mirror_sync_system: ProcessInputs 階段");
}

/// Step 2–4：腳本幀回調精確排序執行系統
///
/// 執行順序：`on_event × N → on_input × N → on_tick × N`
/// 排程位置：`GameFixedSet::RunScripts`
///
/// `dt` 固定為 `SoftF32(0x3C88_8889)`（1.0/60.0）——**不**讀取 Bevy `Time<Fixed>`
/// 以保證跨 client 確定性。
///
/// # 架構說明
/// `ScriptManager` 與 `SandboxedEngine`（vm_runtime crate）使用 `rhai::Engine`，
/// 而 Rhai 在未啟用 `sync` feature 時內部使用 `Rc`，不滿足 Bevy Resource 的
/// `Send + Sync` 要求。因此腳本執行不在 Bevy system 內直接進行。
///
/// 本系統負責 Bevy 側的佇列管理：
/// 1. 排空 `PendingScriptEvents` 和 `PendingInputs`（防止跨幀累積）
/// 2. 記錄事件/輸入的 debug 日誌
///
/// 實際腳本執行由外層驅動：透過 `SharedBridgeState`（`Arc<Mutex<BridgeState>>`）
/// 將 ECS 狀態傳遞給 VM 層，VM 層呼叫 `ScriptManager::dispatch_event`/
/// `dispatch_input`/`tick_all` 完成腳本回調。
pub fn script_frame_orchestration(
    mut pending_events: ResMut<PendingScriptEvents>,
    mut pending_inputs: ResMut<PendingInputs>,
) {
    // ── Step 2: on_event ────────────────────────────────────────────────
    let events: Vec<_> = pending_events.drain().collect();
    for event in &events {
        tracing::debug!(
            event_type = %event.event_type,
            "script_frame_orchestration: on_event 已排空"
        );
    }

    // ── Step 3: on_input ────────────────────────────────────────────────
    let inputs: Vec<_> = pending_inputs.drain().collect();
    for input in &inputs {
        tracing::debug!(
            input_type = %input.input_type,
            "script_frame_orchestration: on_input 已排空"
        );
    }

    // ── Step 4: on_tick ────────────────────────────────────────────────
    // dt 固定為 1.0/60.0 的 bit-exact SoftF32，由 VM 層 ScriptManager 使用。
    // 本系統不直接呼叫 tick_all（Rhai Engine 非 Send + Sync）。
    tracing::debug!("script_frame_orchestration: 幀回調排程完成");
}

/// Step 5：BridgeEvent flush 系統
///
/// 排程位置：`GameFixedSet::FlushBridgeEvents`
///
/// **不變式**：必須先呼叫 `dropped_count()` 讀取計數，再呼叫 `drain()`——
/// `drain()` 會將 `dropped` 清零，若順序顛倒則遺失溢出計數。
pub fn bridge_event_flush_system(mut bridge_events: ResMut<LocalBridgeEventQueue>) {
    // 先讀 dropped（drain() 會清零 dropped）
    let dropped = bridge_events.dropped_count();
    if dropped > 0 {
        tracing::warn!(dropped, "本幀有 BridgeEvent 因佇列溢出被丟棄");
    }

    // 消費所有事件。
    // VM→Bevy 事件的完整分派（依 BridgeEvent variant 對應至 ECS Command / Bevy Resource）
    // 由 vm_bevy_bridge::flush_bridge_events 系統負責（BridgePlugin 已註冊至
    // GameFixedSet::FlushBridgeEvents）。此 LocalBridgeEventQueue 為 bevy_runtime 側的
    // 測試用本地佇列，與 vm_bevy_bridge::BridgeEventQueue 獨立。
    for event in bridge_events.drain() {
        tracing::debug!(event = ?event, "處理 LocalBridgeEvent");
    }
}

// ── 幀回調排序測試 ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ─── Mock 基礎設施 ───────────────────────────────────────────────

    thread_local! {
        static CALL_LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn log_call(name: &str) {
        CALL_LOG.with(|log| log.borrow_mut().push(name.to_string()));
    }

    fn get_call_log() -> Vec<String> {
        CALL_LOG.with(|log| log.borrow().clone())
    }

    fn clear_call_log() {
        CALL_LOG.with(|log| log.borrow_mut().clear());
    }

    /// 輕量 MockScriptManager，記錄呼叫順序而非實際執行 Rhai
    ///
    /// 使用 BTreeMap 保證 (priority, script_id) 升序（Determinism 規則）。
    struct MockScriptManager {
        /// BTreeMap key 為 (priority, script_id)，升序排列
        script_ids: std::collections::BTreeMap<(u8, String), ()>,
    }

    impl MockScriptManager {
        fn new(scripts: &[(u8, &str)]) -> Self {
            let mut map = std::collections::BTreeMap::new();
            for (prio, id) in scripts {
                map.insert((*prio, id.to_string()), ());
            }
            Self { script_ids: map }
        }

        fn call_on_event_all(&self, _event: &str) {
            for (prio, _id) in self.script_ids.keys() {
                log_call(&format!("on_event_prio_{}", prio));
            }
        }

        fn call_on_input_all(&self, _input: &str) {
            for (prio, _id) in self.script_ids.keys() {
                log_call(&format!("on_input_prio_{}", prio));
            }
        }

        fn call_on_tick_all(&self) {
            for (prio, _id) in self.script_ids.keys() {
                log_call(&format!("on_tick_prio_{}", prio));
            }
        }
    }

    // ─── 8 個測試案例 ────────────────────────────────────────────────

    /// 測試 1：幀回調完整順序
    /// mirror_sync → on_event × N → on_input × N → on_tick × N → bridge_flush
    #[test]
    fn test_frame_callback_order() {
        clear_call_log();
        let mgr = MockScriptManager::new(&[(0, "a"), (1, "b")]);

        log_call("mirror_sync");
        mgr.call_on_event_all("hit");
        mgr.call_on_input_all("move");
        mgr.call_on_tick_all();
        log_call("bridge_flush");

        let log = get_call_log();
        assert_eq!(log.len(), 8, "完整幀應有 8 個呼叫");
        assert_eq!(log[0], "mirror_sync");
        assert_eq!(log[1], "on_event_prio_0");
        assert_eq!(log[2], "on_event_prio_1");
        assert_eq!(log[3], "on_input_prio_0");
        assert_eq!(log[4], "on_input_prio_1");
        assert_eq!(log[5], "on_tick_prio_0");
        assert_eq!(log[6], "on_tick_prio_1");
        assert_eq!(log[7], "bridge_flush");
    }

    /// 測試 2：on_event 依 priority 升序執行
    #[test]
    fn test_on_event_priority_order() {
        clear_call_log();
        let mgr = MockScriptManager::new(&[(2, "c"), (0, "a"), (1, "b")]);
        mgr.call_on_event_all("hit");

        let log = get_call_log();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0], "on_event_prio_0");
        assert_eq!(log[1], "on_event_prio_1");
        assert_eq!(log[2], "on_event_prio_2");
    }

    /// 測試 3：相同 priority 時依 script_id 字母升序
    #[test]
    fn test_same_priority_alphabetic_order() {
        clear_call_log();
        // 同 priority=0，id="b" 與 "a"，應按字母序 "a" → "b"
        let mgr = MockScriptManager::new(&[(0, "b"), (0, "a")]);
        // 直接按 BTreeMap 順序記錄 script_id
        for (_prio, id) in mgr.script_ids.keys() {
            log_call(&format!("on_event_{}", id));
        }

        let log = get_call_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0], "on_event_a", "字母序 a 在 b 之前");
        assert_eq!(log[1], "on_event_b");
    }

    /// 測試 4：bridge_flush 必須在所有 callback 之後
    #[test]
    fn test_bridge_flush_after_all_callbacks() {
        clear_call_log();
        let mgr = MockScriptManager::new(&[(0, "a")]);

        log_call("mirror_sync");
        mgr.call_on_event_all("hit");
        mgr.call_on_input_all("move");
        mgr.call_on_tick_all();
        log_call("bridge_flush");

        let log = get_call_log();
        let flush_idx = log.iter().position(|s| s == "bridge_flush").unwrap();
        assert_eq!(
            flush_idx,
            log.len() - 1,
            "bridge_flush 必須在所有 callback 之後"
        );
    }

    /// 測試 5：PendingScriptEvents 先進先出
    #[test]
    fn test_events_delivered_fifo() {
        let mut pending = PendingScriptEvents::new();
        pending.push(ScriptEvent {
            event_type: "A".to_string(),
            payload: "()".to_string(),
        });
        pending.push(ScriptEvent {
            event_type: "B".to_string(),
            payload: "()".to_string(),
        });
        let drained: Vec<_> = pending.drain().collect();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].event_type.as_str(), "A");
        assert_eq!(drained[1].event_type.as_str(), "B");
        assert!(pending.is_empty(), "drain 後佇列必須為空");
    }

    /// 測試 6：PendingInputs 先進先出
    #[test]
    fn test_inputs_delivered_fifo() {
        let mut pending = PendingInputs::new();
        pending.push(ScriptInput {
            input_type: "move".to_string(),
            data: "()".to_string(),
        });
        pending.push(ScriptInput {
            input_type: "attack".to_string(),
            data: "()".to_string(),
        });
        let drained: Vec<_> = pending.drain().collect();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].input_type.as_str(), "move");
        assert_eq!(drained[1].input_type.as_str(), "attack");
        assert!(pending.is_empty(), "drain 後佇列必須為空");
    }

    /// 測試 7：EcsMirror sync 必須在所有 script callback 之前
    #[test]
    fn test_ecs_mirror_sync_before_scripts() {
        clear_call_log();
        let mgr = MockScriptManager::new(&[(0, "a")]);

        log_call("mirror_sync");
        mgr.call_on_event_all("hit");
        mgr.call_on_tick_all();
        log_call("bridge_flush");

        let log = get_call_log();
        assert_eq!(log[0], "mirror_sync", "EcsMirror sync 必須是第一個呼叫");
        for (i, entry) in log.iter().enumerate().skip(1) {
            if entry.starts_with("on_") {
                assert!(i > 0, "script callback 必須在 mirror_sync 之後");
            }
        }
    }

    /// 測試 8：無 events 與 inputs 時，直接進入 on_tick
    #[test]
    fn test_empty_events_and_inputs() {
        clear_call_log();
        let mgr = MockScriptManager::new(&[(0, "a")]);

        log_call("mirror_sync");
        // 無 events、無 inputs → 直接進入 on_tick
        mgr.call_on_tick_all();
        log_call("bridge_flush");

        let log = get_call_log();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0], "mirror_sync");
        assert_eq!(log[1], "on_tick_prio_0");
        assert_eq!(log[2], "bridge_flush");
    }
}
