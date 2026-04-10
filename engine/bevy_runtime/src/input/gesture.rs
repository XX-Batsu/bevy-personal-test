//! 手勢辨識層 — 事件類型、閾值常數、距離函數與狀態機。
//!
//! 設計規格：`docs/design/2026-04-10-gesture-recognizer-design.md`
//!
//! # 架構
//! - [`GestureEvent`] — 對外發布的手勢事件
//! - [`GestureRecognizer`] — 單一按鍵遮罩的狀態機，每 tick 呼叫 [`GestureRecognizer::update`]
//! - [`GestureRecognizers`] — Bevy Resource，持有所有 recognizer

use bevy::prelude::*;

use crate::fixed_update::FixedTickCounter;
use crate::frame_orchestrator::{PendingInputs, ScriptInput};

// ── 閾值常數 ──────────────────────────────────────────────────────────────────

/// 點擊最大持續 tick 數（按下到釋放）
pub const CLICK_MAX_TICKS: u64 = 18;
/// 點擊最大曼哈頓位移（像素）
pub const CLICK_MAX_DISTANCE: i32 = 5;
/// 雙擊兩次點擊之間最大間隔 tick 數
pub const DOUBLE_CLICK_MAX_TICKS: u64 = 24;
/// 雙擊兩次點擊落點最大曼哈頓距離（像素）
pub const DOUBLE_CLICK_MAX_DISTANCE: i32 = 10;
/// 長按最小持續 tick 數
pub const LONG_PRESS_MIN_TICKS: u64 = 30;
/// 拖曳觸發最小持續 tick 數
pub const DRAG_MIN_TICKS: u64 = 9;
/// 拖曳觸發最小曼哈頓位移（像素）
pub const DRAG_MIN_DISTANCE: i32 = 5;

// ── 距離函數 ──────────────────────────────────────────────────────────────────

/// 計算兩點的曼哈頓距離，使用 i32 算術避免 i16 溢位。
pub fn manhattan_distance(a: (i16, i16), b: (i16, i16)) -> i32 {
    let dx = (a.0 as i32 - b.0 as i32).abs();
    let dy = (a.1 as i32 - b.1 as i32).abs();
    dx + dy
}

// ── GestureEvent ─────────────────────────────────────────────────────────────

/// 手勢辨識器輸出事件。由 [`GestureRecognizer::update`] 產出，透過 Bevy EventWriter 廣播。
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub enum GestureEvent {
    /// 快速點擊（按下並迅速釋放且位移小）
    Click {
        /// 釋放時的滑鼠座標
        pos: (i16, i16),
        /// 觸發此手勢的按鍵遮罩
        button_mask: u8,
    },
    /// 雙擊（兩次連續快速點擊）
    DoubleClick {
        /// 第二次釋放時的滑鼠座標
        pos: (i16, i16),
        /// 觸發此手勢的按鍵遮罩
        button_mask: u8,
    },
    /// 長按（靜止按住超過閾值 tick 數）
    LongPress {
        /// 按下時的滑鼠座標
        pos: (i16, i16),
        /// 觸發此手勢的按鍵遮罩
        button_mask: u8,
    },
    /// 拖曳開始（按住移動超過閾值距離）
    DragStart {
        /// 按下時的滑鼠座標
        start_pos: (i16, i16),
        /// 觸發此手勢的按鍵遮罩
        button_mask: u8,
    },
    /// 拖曳中（每 tick 位移有變化時發出）
    Dragging {
        /// 拖曳起點座標
        start_pos: (i16, i16),
        /// 當前滑鼠座標
        current_pos: (i16, i16),
        /// 觸發此手勢的按鍵遮罩
        button_mask: u8,
    },
    /// 拖曳結束（拖曳中釋放按鍵）
    DragEnd {
        /// 拖曳起點座標
        start_pos: (i16, i16),
        /// 釋放時的滑鼠座標
        end_pos: (i16, i16),
        /// 觸發此手勢的按鍵遮罩
        button_mask: u8,
    },
}

// ── GestureState（內部狀態機） ─────────────────────────────────────────────────

/// 手勢辨識狀態機的內部狀態。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum GestureState {
    /// 閒置，等待按下
    #[default]
    Idle,
    /// 已按下，等待釋放或超過閾值
    Pressed,
    /// 拖曳中
    Dragging,
    /// 等待釋放（長按後）
    WaitForRelease,
}

// ── GestureRecognizer ────────────────────────────────────────────────────────

/// 單一按鍵遮罩的手勢辨識狀態機。
///
/// 每個 fixed tick 呼叫 [`GestureRecognizer::update`] 並傳入目前的 tick、按鍵狀態與滑鼠位置，
/// 回傳本 tick 觸發的手勢事件列表。
#[derive(Clone, Debug)]
pub struct GestureRecognizer {
    /// 此 recognizer 監聽的按鍵遮罩（位元對應 [`crate::input::raw_input::RawPlayerInput`]）
    pub button_mask: u8,
    state: GestureState,
    /// 按下時的 tick
    press_tick: u64,
    /// 按下時的滑鼠座標
    press_pos: (i16, i16),
    /// 上一個 tick 的滑鼠座標
    last_pos: (i16, i16),
    /// 上一次點擊的 tick 與座標（用於雙擊偵測）
    last_click: Option<(u64, (i16, i16))>,
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        Self::new(0b001)
    }
}

impl GestureRecognizer {
    /// 建立指定按鍵遮罩的 recognizer。
    pub fn new(button_mask: u8) -> Self {
        Self {
            button_mask,
            state: GestureState::Idle,
            press_tick: 0,
            press_pos: (0, 0),
            last_pos: (0, 0),
            last_click: None,
        }
    }

    /// 重置為初始閒置狀態，清除所有暫存欄位。
    pub fn reset(&mut self) {
        self.state = GestureState::Idle;
        self.press_tick = 0;
        self.press_pos = (0, 0);
        self.last_pos = (0, 0);
        self.last_click = None;
    }

    /// 每 tick 更新狀態機，回傳本 tick 產生的手勢事件。
    ///
    /// # 參數
    /// - `current_tick`：當前遊戲邏輯 tick（單調遞增）
    /// - `mouse_buttons`：當前所有按鍵的位元遮罩
    /// - `mouse_pos`：當前滑鼠座標（螢幕像素，i16）
    pub fn update(
        &mut self,
        current_tick: u64,
        mouse_buttons: u8,
        mouse_pos: (i16, i16),
    ) -> Vec<GestureEvent> {
        let pressed = (mouse_buttons & self.button_mask) != 0;
        let mut events = Vec::new();

        match self.state {
            GestureState::Idle => {
                if pressed {
                    // 按下 → 進入 Pressed 狀態
                    self.state = GestureState::Pressed;
                    self.press_tick = current_tick;
                    self.press_pos = mouse_pos;
                    self.last_pos = mouse_pos;
                }
            }
            GestureState::Pressed => {
                let elapsed = current_tick.saturating_sub(self.press_tick);
                let distance = manhattan_distance(mouse_pos, self.press_pos);

                if !pressed {
                    // 按鍵釋放
                    if elapsed < CLICK_MAX_TICKS && distance < CLICK_MAX_DISTANCE {
                        // 符合點擊條件 → 檢查是否為雙擊
                        let is_double = self.last_click.is_some_and(|(last_tick, last_pos)| {
                            let tick_gap = current_tick.saturating_sub(last_tick);
                            let pos_dist = manhattan_distance(mouse_pos, last_pos);
                            tick_gap < DOUBLE_CLICK_MAX_TICKS
                                && pos_dist < DOUBLE_CLICK_MAX_DISTANCE
                        });

                        if is_double {
                            self.last_click = None;
                            events.push(GestureEvent::DoubleClick {
                                pos: mouse_pos,
                                button_mask: self.button_mask,
                            });
                        } else {
                            self.last_click = Some((current_tick, mouse_pos));
                            events.push(GestureEvent::Click {
                                pos: mouse_pos,
                                button_mask: self.button_mask,
                            });
                        }
                    }
                    // 不符合點擊（距離或時間過大）→ 靜默回 Idle
                    self.state = GestureState::Idle;
                } else {
                    // 按鍵仍按住
                    if elapsed >= LONG_PRESS_MIN_TICKS && distance < DRAG_MIN_DISTANCE {
                        // 長按
                        events.push(GestureEvent::LongPress {
                            pos: self.press_pos,
                            button_mask: self.button_mask,
                        });
                        self.state = GestureState::WaitForRelease;
                    } else if elapsed >= DRAG_MIN_TICKS && distance >= DRAG_MIN_DISTANCE {
                        // 拖曳開始
                        events.push(GestureEvent::DragStart {
                            start_pos: self.press_pos,
                            button_mask: self.button_mask,
                        });
                        self.last_pos = mouse_pos;
                        self.state = GestureState::Dragging;
                    }
                }
            }
            GestureState::Dragging => {
                if !pressed {
                    // 釋放 → 拖曳結束
                    events.push(GestureEvent::DragEnd {
                        start_pos: self.press_pos,
                        end_pos: mouse_pos,
                        button_mask: self.button_mask,
                    });
                    self.state = GestureState::Idle;
                } else if mouse_pos != self.last_pos {
                    // 位置改變 → 發出 Dragging 事件
                    events.push(GestureEvent::Dragging {
                        start_pos: self.press_pos,
                        current_pos: mouse_pos,
                        button_mask: self.button_mask,
                    });
                    self.last_pos = mouse_pos;
                }
                // 位置未改變 → 無事件
            }
            GestureState::WaitForRelease => {
                if !pressed {
                    // 釋放 → 回 Idle
                    self.state = GestureState::Idle;
                }
                // 仍按住 → 無事件
            }
        }

        events
    }
}

// ── GestureRecognizers Resource ───────────────────────────────────────────────

/// Bevy Resource，持有全部手勢辨識器。預設包含左鍵辨識器（`0b001`）。
#[derive(Resource)]
pub struct GestureRecognizers(pub Vec<GestureRecognizer>);

impl Default for GestureRecognizers {
    fn default() -> Self {
        Self(vec![GestureRecognizer::new(0b001)])
    }
}

// ── 系統函數 ──────────────────────────────────────────────────────────────────

/// 手勢識別系統 — 每 FixedUpdate tick 消費 RawPlayerInput，產出 GestureEvent。
pub fn gesture_recognition_system(
    raw_input: Res<super::raw_input::RawPlayerInput>,
    tick: Res<FixedTickCounter>,
    mut recognizers: ResMut<GestureRecognizers>,
    mut gesture_events: EventWriter<GestureEvent>,
    mut pending_inputs: ResMut<PendingInputs>,
) {
    let current_tick = tick.count;
    let mouse_pos = raw_input.mouse_pos;
    let mouse_buttons = raw_input.mouse_buttons;

    for recognizer in recognizers.0.iter_mut() {
        let events = recognizer.update(current_tick, mouse_buttons, mouse_pos);

        for event in events {
            let json = gesture_event_to_json(&event);
            pending_inputs.push(ScriptInput {
                input_type: "gesture".to_string(),
                data: json,
            });
            gesture_events.send(event);
        }
    }
}

/// 離開 InGame 時重置所有 recognizer。
pub fn reset_gesture_recognizers(mut recognizers: ResMut<GestureRecognizers>) {
    for r in recognizers.0.iter_mut() {
        r.reset();
    }
}

/// 將 GestureEvent 轉換為 JSON 字串（供 Script 路徑）。
fn gesture_event_to_json(event: &GestureEvent) -> String {
    match event {
        GestureEvent::Click { pos, button_mask } => serde_json::json!({
            "type": "click", "x": pos.0, "y": pos.1, "button": button_mask
        })
        .to_string(),
        GestureEvent::DoubleClick { pos, button_mask } => serde_json::json!({
            "type": "double_click", "x": pos.0, "y": pos.1, "button": button_mask
        })
        .to_string(),
        GestureEvent::LongPress { pos, button_mask } => serde_json::json!({
            "type": "long_press", "x": pos.0, "y": pos.1, "button": button_mask
        })
        .to_string(),
        GestureEvent::DragStart {
            start_pos,
            button_mask,
        } => serde_json::json!({
            "type": "drag_start",
            "x": start_pos.0, "y": start_pos.1,
            "start_x": start_pos.0, "start_y": start_pos.1,
            "button": button_mask
        })
        .to_string(),
        GestureEvent::Dragging {
            start_pos,
            current_pos,
            button_mask,
        } => serde_json::json!({
            "type": "dragging",
            "x": current_pos.0, "y": current_pos.1,
            "start_x": start_pos.0, "start_y": start_pos.1,
            "button": button_mask
        })
        .to_string(),
        GestureEvent::DragEnd {
            start_pos,
            end_pos,
            button_mask,
        } => serde_json::json!({
            "type": "drag_end",
            "x": end_pos.0, "y": end_pos.1,
            "start_x": start_pos.0, "start_y": start_pos.1,
            "button": button_mask
        })
        .to_string(),
    }
}

// ── 測試 ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── manhattan_distance 測試 ───────────────────────────────────────────────

    #[test]
    fn manhattan_distance_相同點() {
        assert_eq!(manhattan_distance((10, 20), (10, 20)), 0);
    }

    #[test]
    fn manhattan_distance_正向位移() {
        assert_eq!(manhattan_distance((0, 0), (3, 4)), 7);
    }

    #[test]
    fn manhattan_distance_負向位移() {
        assert_eq!(manhattan_distance((5, 5), (2, 1)), 7);
    }

    #[test]
    fn manhattan_distance_混合正負() {
        assert_eq!(manhattan_distance((-10, 20), (10, -20)), 60);
    }

    #[test]
    fn manhattan_distance_極端值_i16_max_to_min() {
        assert_eq!(
            manhattan_distance((i16::MAX, i16::MAX), (i16::MIN, i16::MIN)),
            131070
        );
    }

    #[test]
    fn manhattan_distance_單軸() {
        // 僅 x 軸
        assert_eq!(manhattan_distance((0, 5), (10, 5)), 10);
        // 僅 y 軸
        assert_eq!(manhattan_distance((3, 0), (3, 7)), 7);
    }

    // ── 測試輔助函數 ──────────────────────────────────────────────────────────

    fn make_recognizer() -> GestureRecognizer {
        GestureRecognizer::default()
    }

    fn tick(
        r: &mut GestureRecognizer,
        tick: u64,
        buttons: u8,
        pos: (i16, i16),
    ) -> Vec<GestureEvent> {
        r.update(tick, buttons, pos)
    }

    // ── GestureRecognizer 測試 ────────────────────────────────────────────────

    #[test]
    fn idle_無按鍵_無事件() {
        let mut r = make_recognizer();
        let events = tick(&mut r, 1, 0b000, (0, 0));
        assert!(events.is_empty());
    }

    #[test]
    fn click_按下後快速釋放產出_click() {
        let mut r = make_recognizer();
        // 按下 tick 1
        tick(&mut r, 1, 0b001, (100, 100));
        // 釋放 tick 5（elapsed=4，距離=0）
        let events = tick(&mut r, 5, 0b000, (100, 100));
        assert_eq!(
            events,
            vec![GestureEvent::Click {
                pos: (100, 100),
                button_mask: 0b001
            }]
        );
    }

    #[test]
    fn click_距離過大_無事件() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (100, 100));
        // 釋放時位移 10（超過 CLICK_MAX_DISTANCE=5）
        let events = tick(&mut r, 5, 0b000, (110, 100));
        assert!(events.is_empty());
    }

    #[test]
    fn double_click_兩次快速點擊() {
        let mut r = make_recognizer();
        // 第一次點擊：按下 tick 1，釋放 tick 5
        tick(&mut r, 1, 0b001, (100, 100));
        let e1 = tick(&mut r, 5, 0b000, (100, 100));
        assert_eq!(e1.len(), 1);
        assert!(matches!(e1[0], GestureEvent::Click { .. }));

        // 第二次點擊：按下 tick 15，釋放 tick 20（間隔 15 tick，< DOUBLE_CLICK_MAX_TICKS=24）
        tick(&mut r, 15, 0b001, (100, 100));
        let e2 = tick(&mut r, 20, 0b000, (100, 100));
        assert_eq!(e2.len(), 1);
        assert!(matches!(e2[0], GestureEvent::DoubleClick { .. }));
    }

    #[test]
    fn double_click_間隔過久_兩個獨立_click() {
        let mut r = make_recognizer();
        // 第一次點擊
        tick(&mut r, 1, 0b001, (100, 100));
        let e1 = tick(&mut r, 5, 0b000, (100, 100));
        assert!(matches!(e1[0], GestureEvent::Click { .. }));

        // 第二次點擊：間隔 45 tick（>= DOUBLE_CLICK_MAX_TICKS=24）
        tick(&mut r, 40, 0b001, (100, 100));
        let e2 = tick(&mut r, 50, 0b000, (100, 100));
        assert_eq!(e2.len(), 1);
        assert!(matches!(e2[0], GestureEvent::Click { .. }));
    }

    #[test]
    fn double_click_距離過大_兩個獨立_click() {
        let mut r = make_recognizer();
        // 第一次點擊在 (100,100)
        tick(&mut r, 1, 0b001, (100, 100));
        let e1 = tick(&mut r, 5, 0b000, (100, 100));
        assert!(matches!(e1[0], GestureEvent::Click { .. }));

        // 第二次點擊在 (120,100)，距離 20（> DOUBLE_CLICK_MAX_DISTANCE=10）
        tick(&mut r, 15, 0b001, (120, 100));
        let e2 = tick(&mut r, 20, 0b000, (120, 100));
        assert_eq!(e2.len(), 1);
        assert!(matches!(e2[0], GestureEvent::Click { .. }));
    }

    #[test]
    fn 首次_click_不誤判_double_click() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (0, 0));
        let events = tick(&mut r, 5, 0b000, (0, 0));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], GestureEvent::Click { .. }));
    }

    #[test]
    fn long_press_靜止按住超過閾值() {
        let mut r = make_recognizer();
        // 按下 tick 1，持續至 tick 31（elapsed=30 >= LONG_PRESS_MIN_TICKS=30）
        tick(&mut r, 1, 0b001, (50, 50));
        for t in 2..31 {
            let events = tick(&mut r, t, 0b001, (50, 50));
            assert!(events.is_empty(), "tick {t} 不應產出事件");
        }
        let events = tick(&mut r, 31, 0b001, (50, 50));
        assert_eq!(
            events,
            vec![GestureEvent::LongPress {
                pos: (50, 50),
                button_mask: 0b001
            }]
        );
    }

    #[test]
    fn long_press_後釋放不觸發_click() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (50, 50));
        for t in 2..=31 {
            tick(&mut r, t, 0b001, (50, 50));
        }
        // 釋放
        let events = tick(&mut r, 32, 0b000, (50, 50));
        assert!(events.is_empty());
    }

    #[test]
    fn long_press_後不重複觸發() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (50, 50));
        // tick 31 觸發 LongPress
        for t in 2..=31 {
            tick(&mut r, t, 0b001, (50, 50));
        }
        // 繼續按住，不應再觸發
        for t in 32..=40 {
            let events = tick(&mut r, t, 0b001, (50, 50));
            assert!(events.is_empty(), "tick {t} 不應重複觸發");
        }
    }

    #[test]
    fn wait_for_release_釋放回_idle() {
        let mut r = make_recognizer();
        // 觸發 LongPress
        tick(&mut r, 1, 0b001, (50, 50));
        for t in 2..=31 {
            tick(&mut r, t, 0b001, (50, 50));
        }
        // 釋放
        tick(&mut r, 32, 0b000, (50, 50));
        // 重新點擊應可正常觸發 Click
        tick(&mut r, 40, 0b001, (50, 50));
        let events = tick(&mut r, 44, 0b000, (50, 50));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], GestureEvent::Click { .. }));
    }

    #[test]
    fn drag_時間且距離都滿足_觸發_drag_start() {
        let mut r = make_recognizer();
        // 按下 tick 1
        tick(&mut r, 1, 0b001, (100, 100));
        // tick 10：elapsed=9 >= DRAG_MIN_TICKS=9，距離=5 >= DRAG_MIN_DISTANCE=5
        let events = tick(&mut r, 10, 0b001, (105, 100));
        assert_eq!(
            events,
            vec![GestureEvent::DragStart {
                start_pos: (100, 100),
                button_mask: 0b001
            }]
        );
    }

    #[test]
    fn drag_僅時間不觸發() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (100, 100));
        // tick 10：elapsed=9，但距離=0（未達 DRAG_MIN_DISTANCE）
        let events = tick(&mut r, 10, 0b001, (100, 100));
        assert!(events.is_empty());
    }

    #[test]
    fn drag_僅距離不觸發() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (100, 100));
        // tick 5：elapsed=4（未達 DRAG_MIN_TICKS=9），距離=10
        let events = tick(&mut r, 5, 0b001, (110, 100));
        assert!(events.is_empty());
    }

    #[test]
    fn dragging_位移變化_產出事件() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (100, 100));
        // 觸發 DragStart
        tick(&mut r, 10, 0b001, (105, 100));
        // 繼續移動
        let events = tick(&mut r, 11, 0b001, (110, 100));
        assert_eq!(
            events,
            vec![GestureEvent::Dragging {
                start_pos: (100, 100),
                current_pos: (110, 100),
                button_mask: 0b001
            }]
        );
    }

    #[test]
    fn dragging_位移不變_無事件() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (100, 100));
        // 觸發 DragStart
        tick(&mut r, 10, 0b001, (105, 100));
        // 位置不變
        let events = tick(&mut r, 11, 0b001, (105, 100));
        assert!(events.is_empty());
    }

    #[test]
    fn drag_end_釋放產出事件() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (100, 100));
        // 觸發 DragStart
        tick(&mut r, 10, 0b001, (105, 100));
        // 釋放
        let events = tick(&mut r, 11, 0b000, (105, 100));
        assert_eq!(
            events,
            vec![GestureEvent::DragEnd {
                start_pos: (100, 100),
                end_pos: (105, 100),
                button_mask: 0b001
            }]
        );
    }

    #[test]
    fn drag_後不觸發_click() {
        let mut r = make_recognizer();
        tick(&mut r, 1, 0b001, (100, 100));
        tick(&mut r, 10, 0b001, (105, 100));
        // DragEnd
        tick(&mut r, 11, 0b000, (105, 100));
        // 之後的正常點擊應可觸發
        tick(&mut r, 20, 0b001, (100, 100));
        let events = tick(&mut r, 24, 0b000, (100, 100));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], GestureEvent::Click { .. }));
    }

    #[test]
    fn 可配置按鍵_右鍵() {
        // 右鍵 recognizer
        let mut r = GestureRecognizer::new(0b010);
        // 按左鍵 → 無反應
        tick(&mut r, 1, 0b001, (0, 0));
        let events = tick(&mut r, 5, 0b000, (0, 0));
        assert!(events.is_empty(), "左鍵不應觸發右鍵 recognizer");

        // 按右鍵 → 觸發 Click
        tick(&mut r, 10, 0b010, (0, 0));
        let events = tick(&mut r, 14, 0b000, (0, 0));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            GestureEvent::Click {
                button_mask: 0b010,
                ..
            }
        ));
    }

    #[test]
    fn 多_recognizer_獨立運作() {
        let mut left = GestureRecognizer::new(0b001);
        let mut right = GestureRecognizer::new(0b010);

        // 同時按下左右鍵
        tick(&mut left, 1, 0b011, (0, 0));
        tick(&mut right, 1, 0b011, (0, 0));

        // 僅釋放左鍵（0b010 仍按住）
        let left_events = tick(&mut left, 5, 0b010, (0, 0));
        let right_events = tick(&mut right, 5, 0b010, (0, 0));

        assert_eq!(left_events.len(), 1, "左鍵應產出 Click");
        assert!(
            matches!(
                left_events[0],
                GestureEvent::Click {
                    button_mask: 0b001,
                    ..
                }
            ),
            "應為左鍵 Click"
        );
        assert!(right_events.is_empty(), "右鍵仍按住，不應產出事件");
    }

    #[test]
    fn reset_清除所有狀態() {
        let mut r = make_recognizer();
        // 進入 Pressed 狀態
        tick(&mut r, 1, 0b001, (10, 10));
        assert_ne!(r.state, GestureState::Idle);
        // 重置
        r.reset();
        assert_eq!(r.state, GestureState::Idle);
        assert_eq!(r.press_tick, 0);
        assert_eq!(r.press_pos, (0, 0));
        assert_eq!(r.last_pos, (0, 0));
        assert!(r.last_click.is_none());
    }

    #[test]
    fn reset_清除_last_click() {
        let mut r = make_recognizer();
        // 觸發一次 Click，記錄 last_click
        tick(&mut r, 1, 0b001, (0, 0));
        tick(&mut r, 5, 0b000, (0, 0));
        assert!(r.last_click.is_some());
        // 重置
        r.reset();
        // 下一次點擊不應觸發 DoubleClick
        tick(&mut r, 10, 0b001, (0, 0));
        let events = tick(&mut r, 14, 0b000, (0, 0));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], GestureEvent::Click { .. }));
    }

    // ── gesture_event_to_json 測試 ────────────────────────────────────────

    #[test]
    fn json_click_格式正確() {
        let event = GestureEvent::Click {
            pos: (100, 200),
            button_mask: 1,
        };
        let json: serde_json::Value =
            serde_json::from_str(&super::gesture_event_to_json(&event)).unwrap();
        assert_eq!(json["type"], "click");
        assert_eq!(json["x"], 100);
        assert_eq!(json["y"], 200);
        assert_eq!(json["button"], 1);
    }

    #[test]
    fn json_double_click_格式正確() {
        let event = GestureEvent::DoubleClick {
            pos: (50, 60),
            button_mask: 1,
        };
        let json: serde_json::Value =
            serde_json::from_str(&super::gesture_event_to_json(&event)).unwrap();
        assert_eq!(json["type"], "double_click");
        assert_eq!(json["x"], 50);
        assert_eq!(json["y"], 60);
    }

    #[test]
    fn json_long_press_格式正確() {
        let event = GestureEvent::LongPress {
            pos: (10, 20),
            button_mask: 2,
        };
        let json: serde_json::Value =
            serde_json::from_str(&super::gesture_event_to_json(&event)).unwrap();
        assert_eq!(json["type"], "long_press");
        assert_eq!(json["button"], 2);
    }

    #[test]
    fn json_drag_start_格式正確() {
        let event = GestureEvent::DragStart {
            start_pos: (100, 100),
            button_mask: 1,
        };
        let json: serde_json::Value =
            serde_json::from_str(&super::gesture_event_to_json(&event)).unwrap();
        assert_eq!(json["type"], "drag_start");
        assert_eq!(json["x"], 100);
        assert_eq!(json["start_x"], 100);
    }

    #[test]
    fn json_dragging_格式正確() {
        let event = GestureEvent::Dragging {
            start_pos: (100, 100),
            current_pos: (150, 120),
            button_mask: 1,
        };
        let json: serde_json::Value =
            serde_json::from_str(&super::gesture_event_to_json(&event)).unwrap();
        assert_eq!(json["type"], "dragging");
        assert_eq!(json["x"], 150);
        assert_eq!(json["y"], 120);
        assert_eq!(json["start_x"], 100);
        assert_eq!(json["start_y"], 100);
    }

    #[test]
    fn json_drag_end_格式正確() {
        let event = GestureEvent::DragEnd {
            start_pos: (100, 100),
            end_pos: (200, 150),
            button_mask: 1,
        };
        let json: serde_json::Value =
            serde_json::from_str(&super::gesture_event_to_json(&event)).unwrap();
        assert_eq!(json["type"], "drag_end");
        assert_eq!(json["x"], 200);
        assert_eq!(json["y"], 150);
        assert_eq!(json["start_x"], 100);
    }

    #[test]
    fn pressed_釋放_elapsed_超過_click_但未達_drag_靜默回_idle() {
        let mut r = make_recognizer();
        // 按下 tick 1
        tick(&mut r, 1, 0b001, (100, 100));
        // 持續按住到 tick 20（elapsed=19 > CLICK_MAX_TICKS=18，距離僅 2）
        for t in 2..=20 {
            tick(&mut r, t, 0b001, (101, 101));
        }
        // 釋放 → 不符合 Click（超時），也不符合 Drag（距離不足）→ 靜默回 Idle
        let events = tick(&mut r, 21, 0b000, (101, 101));
        assert!(events.is_empty());
    }

    // ── 整合測試 ─────────────────────────────────────────────────────────

    use crate::fixed_update::FixedUpdatePlugin;
    use crate::frame_orchestrator::PendingInputs;
    use crate::input::InputPlugin;
    use crate::welcome::AppState;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// 建立最小 App，含 FixedUpdatePlugin + InputPlugin + States
    fn build_gesture_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(FixedUpdatePlugin);
        app.add_plugins(InputPlugin);
        app.init_state::<AppState>();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            20,
        )));
        app
    }

    #[test]
    fn 整合_gesture_event_寫入_bevy_events() {
        let mut app = build_gesture_test_app();
        // 進入 InGame 狀態
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
        app.update(); // Frame 0: 狀態轉換
        app.update(); // Frame 1: 觸發 FixedUpdate

        // 設定 RawPlayerInput：左鍵按下
        {
            let mut raw = app
                .world_mut()
                .resource_mut::<super::super::raw_input::RawPlayerInput>();
            raw.mouse_buttons = 0b001;
            raw.mouse_pos = (100, 100);
        }
        app.update(); // tick: 按下

        // 釋放左鍵
        {
            let mut raw = app
                .world_mut()
                .resource_mut::<super::super::raw_input::RawPlayerInput>();
            raw.mouse_buttons = 0b000;
            raw.mouse_pos = (100, 100);
        }
        app.update(); // tick: 釋放 → 應產出 Click

        // 驗證 GestureEvent 寫入 Bevy Events
        let events = app.world().resource::<Events<GestureEvent>>();
        let mut reader = events.get_cursor();
        let gesture_events: Vec<_> = reader.read(events).collect();
        assert!(
            gesture_events
                .iter()
                .any(|e| matches!(e, GestureEvent::Click { .. })),
            "應有 Click 事件寫入 Bevy Events，實際: {:?}",
            gesture_events
        );
    }

    #[test]
    fn 整合_script_input_推入_pending_inputs() {
        let mut app = build_gesture_test_app();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
        app.update();
        app.update();

        // 按下
        {
            let mut raw = app
                .world_mut()
                .resource_mut::<super::super::raw_input::RawPlayerInput>();
            raw.mouse_buttons = 0b001;
            raw.mouse_pos = (50, 50);
        }
        app.update();

        // 釋放
        {
            let mut raw = app
                .world_mut()
                .resource_mut::<super::super::raw_input::RawPlayerInput>();
            raw.mouse_buttons = 0b000;
            raw.mouse_pos = (50, 50);
        }
        app.update();

        // 驗證 PendingInputs 含有 gesture 類型的 ScriptInput
        let pending = app.world().resource::<PendingInputs>();
        assert!(
            !pending.is_empty(),
            "PendingInputs 應含有 gesture 類型的 ScriptInput"
        );
    }

    #[test]
    fn 整合_on_exit_重置_recognizer() {
        let mut app = build_gesture_test_app();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
        app.update();
        app.update();

        // 按下左鍵（進入 Pressed 狀態）
        {
            let mut raw = app
                .world_mut()
                .resource_mut::<super::super::raw_input::RawPlayerInput>();
            raw.mouse_buttons = 0b001;
            raw.mouse_pos = (100, 100);
        }
        app.update();

        // 切換回 Welcome → 觸發 OnExit(InGame) → reset
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Welcome);
        app.update();

        // 清除殘留按鍵狀態（模擬玩家在畫面切換期間已釋放按鍵）
        {
            let mut raw = app
                .world_mut()
                .resource_mut::<super::super::raw_input::RawPlayerInput>();
            raw.mouse_buttons = 0b000;
        }

        // 行為驗證：重新進入 InGame，釋放不應有殘留事件
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
        app.update();
        app.update();

        {
            let mut raw = app
                .world_mut()
                .resource_mut::<super::super::raw_input::RawPlayerInput>();
            raw.mouse_buttons = 0b000;
            raw.mouse_pos = (100, 100);
        }
        app.update();

        let events = app.world().resource::<Events<GestureEvent>>();
        let mut reader = events.get_cursor();
        let gesture_events: Vec<_> = reader.read(events).collect();
        assert!(
            gesture_events.is_empty(),
            "reset 後重新進入 InGame，釋放不應有殘留事件，實際: {:?}",
            gesture_events
        );
    }
}
