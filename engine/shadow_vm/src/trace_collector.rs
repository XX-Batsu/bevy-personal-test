//! TraceCollector — Shadow VM 主線程 snapshot 收集器
//!
//! 在主線程收集每幀的 snapshot 資料（inputs、RNG state、EcsMirror hash），
//! 維持滑動視窗緩衝區，組裝為 `ShadowRequest` 供 Shadow Worker 驗證使用。

use std::collections::VecDeque;

use bridge_types::{ShadowFrame, ShadowRequest};

/// Shadow VM 主線程 snapshot 收集器
///
/// 維持最近 `window_size` 幀的 `ShadowFrame` 緩衝區（環形）。
/// 迭代順序：front → back = tick 遞增（不變式 #4, #6）。
pub struct TraceCollector {
    /// 環形緩衝區，維持最近 window_size 幀的 snapshot
    window: VecDeque<ShadowFrame>,
    /// 視窗大小，預設 4（validation-flow.md §重播流程）
    window_size: usize,
}

impl TraceCollector {
    /// 建立 TraceCollector，window_size 通常為 4
    pub fn new(window_size: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size),
            window_size,
        }
    }

    /// 記錄一幀的 snapshot，緩衝區超過 window_size 時自動捨棄最舊的幀
    ///
    /// 呼叫端應按 tick 遞增順序呼叫，以維持不變式 #4（tick 單調遞增）。
    pub fn record(&mut self, frame: ShadowFrame) {
        if self.window.len() >= self.window_size {
            // O(1) pop_front — VecDeque 環形緩衝區的核心優勢
            self.window.pop_front();
        }
        self.window.push_back(frame);
    }

    /// 取出已收集的 frames 並組裝為 ShadowRequest
    ///
    /// 緩衝區已達 window_size 幀時回傳 Some；不足時回傳 None。
    /// VecDeque::iter() 保證 front→back 順序 = tick 遞增（不變式 #4, #6）。
    pub fn take_request(&self) -> Option<ShadowRequest> {
        if self.window.len() >= self.window_size {
            Some(ShadowRequest {
                frames: self.window.iter().cloned().collect(),
            })
        } else {
            None
        }
    }

    /// 清空已消費的 frames（發送 ShadowRequest 後呼叫）
    ///
    /// 注意：必須先呼叫 `take_request()` 取得 `ShadowRequest`，
    /// 再呼叫 `clear()` 清空，否則已收集的 frames 會丟失。
    pub fn clear(&mut self) {
        self.window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(tick: u64) -> ShadowFrame {
        ShadowFrame {
            tick,
            inputs: vec![],
            rng_state: [0u8; 16],
            ecs_mirror_hash: [0u8; 32],
        }
    }

    // Test A: 收集不足 window_size 幀 → None
    #[test]
    fn collector_insufficient_frames_returns_none() {
        let mut collector = TraceCollector::new(4);
        for tick in 0..3 {
            collector.record(make_frame(tick));
        }
        assert!(collector.take_request().is_none());
    }

    // Test B: 收集恰好 window_size 幀 → Some
    #[test]
    fn collector_sufficient_frames_returns_some() {
        let mut collector = TraceCollector::new(4);
        for tick in 0..4 {
            collector.record(make_frame(tick));
        }
        let req = collector.take_request();
        assert!(req.is_some());
        assert_eq!(req.unwrap().frames.len(), 4);
    }

    // Test C: 每幀資料正確（tick、rng_state、ecs_mirror_hash）
    #[test]
    fn collector_frame_data_correct() {
        let mut collector = TraceCollector::new(2);
        collector.record(ShadowFrame {
            tick: 10,
            inputs: vec![],
            rng_state: [1u8; 16],
            ecs_mirror_hash: [2u8; 32],
        });
        collector.record(ShadowFrame {
            tick: 11,
            inputs: vec![],
            rng_state: [3u8; 16],
            ecs_mirror_hash: [4u8; 32],
        });
        let req = collector.take_request().unwrap();
        assert_eq!(req.frames[0].tick, 10);
        assert_eq!(req.frames[0].rng_state, [1u8; 16]);
        assert_eq!(req.frames[1].tick, 11);
        assert_eq!(req.frames[1].ecs_mirror_hash, [4u8; 32]);
    }

    // Test D: clear 後需重新收集
    #[test]
    fn collector_clear_resets_buffer() {
        let mut collector = TraceCollector::new(4);
        for tick in 0..4 {
            collector.record(make_frame(tick));
        }
        assert!(collector.take_request().is_some());
        collector.clear();
        assert!(collector.take_request().is_none());
    }

    // Test E: 超過 window_size 幀時，只保留最新的 window_size 幀
    #[test]
    fn collector_drops_oldest_frame() {
        let mut collector = TraceCollector::new(4);
        for tick in 0..6 {
            collector.record(make_frame(tick));
        }
        let req = collector.take_request().unwrap();
        // 應只保留 tick 2, 3, 4, 5
        assert_eq!(req.frames[0].tick, 2);
        assert_eq!(req.frames[3].tick, 5);
    }

    // Test F: 最小窗口 window_size=1
    #[test]
    fn collector_window_size_1() {
        let mut collector = TraceCollector::new(1);
        collector.record(make_frame(42));
        let req = collector.take_request().unwrap();
        assert_eq!(req.frames.len(), 1);
        assert_eq!(req.frames[0].tick, 42);

        // 再記錄一幀，舊幀被淘汰
        collector.record(make_frame(43));
        let req = collector.take_request().unwrap();
        assert_eq!(req.frames.len(), 1);
        assert_eq!(req.frames[0].tick, 43);
    }

    // Test G: tick 遞增保證（不變式 #4）
    #[test]
    fn collector_frames_tick_monotonically_increasing() {
        let mut collector = TraceCollector::new(4);
        // 依序記錄 tick 10, 11, 12, 13
        for tick in 10..14 {
            collector.record(make_frame(tick));
        }
        let req = collector.take_request().unwrap();
        // 驗證 frames 內 tick 嚴格遞增
        for i in 1..req.frames.len() {
            assert!(
                req.frames[i].tick > req.frames[i - 1].tick,
                "frames[{}].tick ({}) 應嚴格大於 frames[{}].tick ({})",
                i,
                req.frames[i].tick,
                i - 1,
                req.frames[i - 1].tick,
            );
        }
    }

    // Test H: 滾動後 tick 遞增仍成立
    #[test]
    fn collector_rolling_preserves_tick_order() {
        let mut collector = TraceCollector::new(3);
        // 記錄 tick 100..108（超過 window_size 多次滾動）
        for tick in 100..108 {
            collector.record(make_frame(tick));
        }
        let req = collector.take_request().unwrap();
        // 應只保留 tick 105, 106, 107
        assert_eq!(req.frames[0].tick, 105);
        assert_eq!(req.frames[1].tick, 106);
        assert_eq!(req.frames[2].tick, 107);
    }
}
