//! Replay 回放引擎

use std::collections::BTreeMap;

use bridge_types::{Blake3Hash, EntityId, PlayerInput};

use crate::recorder::ReplayFile;

/// 確定性模擬介面（replay 回放注入）
pub trait DeterministicSimulation {
    type Error: std::fmt::Display;

    /// 還原 RNG 狀態
    fn restore_rng(&mut self, rng_state: &[u8; 16]);

    /// 以給定輸入推進一幀
    fn step(&mut self, inputs: &BTreeMap<EntityId, PlayerInput>) -> Result<(), Self::Error>;

    /// 計算當前 state hash
    fn compute_state_hash(&self) -> Blake3Hash;
}

/// 回放模式
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackMode {
    /// 驗證模式：逐幀比對 hash
    Validation,
    /// Debug 模式：逐幀比對 + 詳細 log
    Debug,
    /// 觀戰模式（speed_multiplier 僅用於渲染層）
    Spectator { speed_multiplier: f32 },
}

/// 單幀回放結果
#[derive(Debug, Clone)]
pub struct FrameStepResult {
    pub tick: u64,
    pub hash_match: bool,
    pub expected_hash: Blake3Hash,
    pub actual_hash: Blake3Hash,
    pub is_last_frame: bool,
}

/// Replay 驗證結果
#[derive(Debug, PartialEq)]
pub enum ReplayValidationResult {
    /// 所有幀驗證通過
    Ok { frames_verified: usize },
    /// 發現 desync
    Desync {
        first_desync_tick: u64,
        expected_hash: Blake3Hash,
        actual_hash: Blake3Hash,
    },
    /// 模擬過程中發生錯誤
    SimulationError { at_tick: u64, reason: String },
}

/// Replay 回放器
pub struct ReplayPlayer {
    replay: ReplayFile,
    current_frame_idx: usize,
    mode: PlaybackMode,
}

impl ReplayPlayer {
    /// 建立回放器
    pub fn new(replay: ReplayFile, mode: PlaybackMode) -> Self {
        Self {
            replay,
            current_frame_idx: 0,
            mode,
        }
    }

    /// 執行完整回放驗證
    pub fn run_full<S: DeterministicSimulation>(&mut self, sim: &mut S) -> ReplayValidationResult {
        self.current_frame_idx = 0;

        if self.replay.frames.is_empty() {
            return ReplayValidationResult::Ok { frames_verified: 0 };
        }

        let mut verified = 0;

        for frame in &self.replay.frames {
            sim.restore_rng(&frame.rng_state);

            let input_map = Self::inputs_to_map(&frame.inputs);

            if let Err(e) = sim.step(&input_map) {
                return ReplayValidationResult::SimulationError {
                    at_tick: frame.tick,
                    reason: format!("{}", e),
                };
            }

            let actual_hash = sim.compute_state_hash();
            if actual_hash != frame.state_hash {
                return ReplayValidationResult::Desync {
                    first_desync_tick: frame.tick,
                    expected_hash: frame.state_hash,
                    actual_hash,
                };
            }

            verified += 1;
        }

        self.current_frame_idx = self.replay.frames.len();
        ReplayValidationResult::Ok {
            frames_verified: verified,
        }
    }

    /// 逐幀推進
    pub fn step_frame<S: DeterministicSimulation>(
        &mut self,
        sim: &mut S,
    ) -> Option<FrameStepResult> {
        if self.current_frame_idx >= self.replay.frames.len() {
            return None;
        }

        let frame = &self.replay.frames[self.current_frame_idx];
        sim.restore_rng(&frame.rng_state);

        let input_map = Self::inputs_to_map(&frame.inputs);
        if sim.step(&input_map).is_err() {
            return None;
        }

        let actual_hash = sim.compute_state_hash();
        let is_last = self.current_frame_idx + 1 >= self.replay.frames.len();

        let result = FrameStepResult {
            tick: frame.tick,
            hash_match: actual_hash == frame.state_hash,
            expected_hash: frame.state_hash,
            actual_hash,
            is_last_frame: is_last,
        };

        self.current_frame_idx += 1;
        Some(result)
    }

    /// 回放進度 (current, total)
    pub fn progress(&self) -> (usize, usize) {
        (self.current_frame_idx, self.replay.frames.len())
    }

    /// 取得回放模式
    pub fn mode(&self) -> &PlaybackMode {
        &self.mode
    }

    /// 將 Vec<PlayerInput> 轉為 BTreeMap<EntityId, PlayerInput>
    fn inputs_to_map(inputs: &[PlayerInput]) -> BTreeMap<EntityId, PlayerInput> {
        let mut map = BTreeMap::new();
        for input in inputs {
            map.insert(input.player_id, input.clone());
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{ReplayFile, REPLAY_FORMAT_VERSION};
    use bridge_types::{EntityId, ReplayFrame};

    struct MockSim {
        hash: Blake3Hash,
        rng_restored: bool,
        step_count: u64,
        fail_at_step: Option<u64>,
    }

    impl MockSim {
        fn new(hash: Blake3Hash) -> Self {
            Self {
                hash,
                rng_restored: false,
                step_count: 0,
                fail_at_step: None,
            }
        }

        fn failing_at(step: u64) -> Self {
            Self {
                hash: [0u8; 32],
                rng_restored: false,
                step_count: 0,
                fail_at_step: Some(step),
            }
        }
    }

    impl DeterministicSimulation for MockSim {
        type Error = String;

        fn restore_rng(&mut self, _rng_state: &[u8; 16]) {
            self.rng_restored = true;
        }

        fn step(&mut self, _inputs: &BTreeMap<EntityId, PlayerInput>) -> Result<(), Self::Error> {
            if let Some(fail_step) = self.fail_at_step {
                if self.step_count == fail_step {
                    return Err("模擬失敗".to_string());
                }
            }
            self.step_count += 1;
            Ok(())
        }

        fn compute_state_hash(&self) -> Blake3Hash {
            self.hash
        }
    }

    fn make_frame(tick: u64, hash: Blake3Hash) -> ReplayFrame {
        ReplayFrame {
            tick,
            inputs: vec![],
            rng_state: [0u8; 16],
            state_hash: hash,
        }
    }

    fn make_replay(frames: Vec<ReplayFrame>) -> ReplayFile {
        ReplayFile {
            version: REPLAY_FORMAT_VERSION,
            session_id: 1,
            seed: 42,
            player_count: 2,
            frames,
        }
    }

    #[test]
    fn run_full_all_match() {
        let hash = [0xAA; 32];
        let replay = make_replay(vec![
            make_frame(0, hash),
            make_frame(1, hash),
            make_frame(2, hash),
        ]);
        let mut sim = MockSim::new(hash);
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        let result = player.run_full(&mut sim);
        assert_eq!(result, ReplayValidationResult::Ok { frames_verified: 3 });
    }

    #[test]
    fn run_full_desync_detected() {
        let hash = [0xAA; 32];
        let wrong = [0xBB; 32];
        let replay = make_replay(vec![make_frame(0, hash), make_frame(1, wrong)]);
        let mut sim = MockSim::new(hash);
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        let result = player.run_full(&mut sim);
        assert_eq!(
            result,
            ReplayValidationResult::Desync {
                first_desync_tick: 1,
                expected_hash: wrong,
                actual_hash: hash,
            }
        );
    }

    #[test]
    fn run_full_empty_replay() {
        let replay = make_replay(vec![]);
        let mut sim = MockSim::new([0u8; 32]);
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        let result = player.run_full(&mut sim);
        assert_eq!(result, ReplayValidationResult::Ok { frames_verified: 0 });
    }

    #[test]
    fn run_full_simulation_error() {
        // MockSim::failing_at 的 hash 為 [0u8; 32]，frame hash 必須匹配
        let hash = [0u8; 32];
        let replay = make_replay(vec![make_frame(0, hash), make_frame(1, hash)]);
        let mut sim = MockSim::failing_at(1); // 第二次 step 時失敗
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        let result = player.run_full(&mut sim);
        match result {
            ReplayValidationResult::SimulationError { at_tick, .. } => {
                assert_eq!(at_tick, 1);
            }
            _ => panic!("Expected SimulationError"),
        }
    }

    #[test]
    fn step_frame_advances() {
        let hash = [0xAA; 32];
        let replay = make_replay(vec![make_frame(0, hash), make_frame(1, hash)]);
        let mut sim = MockSim::new(hash);
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        let r1 = player.step_frame(&mut sim).unwrap();
        assert_eq!(r1.tick, 0);
        assert!(r1.hash_match);
        assert!(!r1.is_last_frame);

        let r2 = player.step_frame(&mut sim).unwrap();
        assert_eq!(r2.tick, 1);
        assert!(r2.hash_match);
        assert!(r2.is_last_frame);

        assert!(player.step_frame(&mut sim).is_none());
    }

    #[test]
    fn progress_tracking() {
        let hash = [0xAA; 32];
        let replay = make_replay(vec![
            make_frame(0, hash),
            make_frame(1, hash),
            make_frame(2, hash),
        ]);
        let mut sim = MockSim::new(hash);
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        assert_eq!(player.progress(), (0, 3));
        player.step_frame(&mut sim);
        assert_eq!(player.progress(), (1, 3));
        player.step_frame(&mut sim);
        assert_eq!(player.progress(), (2, 3));
    }

    #[test]
    fn single_frame_replay() {
        let hash = [0xAA; 32];
        let replay = make_replay(vec![make_frame(0, hash)]);
        let mut sim = MockSim::new(hash);
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        let result = player.run_full(&mut sim);
        assert_eq!(result, ReplayValidationResult::Ok { frames_verified: 1 });
    }

    #[test]
    fn rng_restored_each_frame() {
        let hash = [0xAA; 32];
        let replay = make_replay(vec![make_frame(0, hash)]);
        let mut sim = MockSim::new(hash);
        let mut player = ReplayPlayer::new(replay, PlaybackMode::Validation);

        player.step_frame(&mut sim);
        assert!(sim.rng_restored);
    }
}
