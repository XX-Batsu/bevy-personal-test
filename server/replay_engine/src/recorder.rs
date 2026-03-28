//! Replay 錄製與讀寫（bincode + gzip）

use std::io::{Read, Write};

use bridge_types::{Blake3Hash, PlayerInput, ReplayFrame};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use crate::error::ReplayError;

/// Replay 格式版本
pub const REPLAY_FORMAT_VERSION: u16 = 1;

/// Replay 檔案結構
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayFile {
    /// 格式版本
    pub version: u16,
    /// Session ID
    pub session_id: u64,
    /// 初始 RNG seed
    pub seed: u64,
    /// 玩家數量
    pub player_count: u8,
    /// 所有幀資料
    pub frames: Vec<ReplayFrame>,
}

/// Replay 錄製器
pub struct ReplayRecorder {
    frames: Vec<ReplayFrame>,
    session_id: u64,
    seed: u64,
    player_count: u8,
}

impl ReplayRecorder {
    /// 建立新的錄製器
    pub fn new(session_id: u64, seed: u64, player_count: u8) -> Self {
        Self {
            frames: Vec::new(),
            session_id,
            seed,
            player_count,
        }
    }

    /// 錄製一幀
    pub fn record_frame(
        &mut self,
        tick: u64,
        inputs: Vec<PlayerInput>,
        rng_state: [u8; 16],
        state_hash: Blake3Hash,
    ) {
        self.frames.push(ReplayFrame {
            tick,
            inputs,
            rng_state,
            state_hash,
        });
    }

    /// 已錄製的幀數
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// 序列化並壓縮寫入 writer（bincode + gzip level 6）
    pub fn save(&self, writer: impl Write) -> Result<(), ReplayError> {
        let file = ReplayFile {
            version: REPLAY_FORMAT_VERSION,
            session_id: self.session_id,
            seed: self.seed,
            player_count: self.player_count,
            frames: self.frames.clone(),
        };

        let data = bincode::serialize(&file).map_err(ReplayError::SerializationFailed)?;

        let mut encoder = GzEncoder::new(writer, Compression::new(6));
        encoder
            .write_all(&data)
            .map_err(ReplayError::DecompressionFailed)?;
        encoder.finish().map_err(ReplayError::DecompressionFailed)?;

        Ok(())
    }

    /// 從 reader 解壓縮並反序列化（gzip + bincode）
    pub fn load(reader: impl Read) -> Result<ReplayFile, ReplayError> {
        let mut decoder = GzDecoder::new(reader);
        let mut data = Vec::new();
        decoder
            .read_to_end(&mut data)
            .map_err(ReplayError::DecompressionFailed)?;

        let file: ReplayFile =
            bincode::deserialize(&data).map_err(ReplayError::DeserializationFailed)?;

        if file.version != REPLAY_FORMAT_VERSION {
            return Err(ReplayError::UnsupportedVersion(file.version));
        }

        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, EntityId};

    fn make_input(tick: u64) -> PlayerInput {
        PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: DeterministicValue::Unit,
            tick,
        }
    }

    #[test]
    fn record_single_frame() {
        let mut recorder = ReplayRecorder::new(1, 42, 2);
        recorder.record_frame(0, vec![make_input(0)], [0u8; 16], [0xAA; 32]);
        assert_eq!(recorder.frame_count(), 1);
    }

    #[test]
    fn record_100_frames() {
        let mut recorder = ReplayRecorder::new(1, 42, 2);
        for i in 0..100 {
            recorder.record_frame(i, vec![make_input(i)], [0u8; 16], [0xAA; 32]);
        }
        assert_eq!(recorder.frame_count(), 100);
    }

    #[test]
    fn save_and_load_round_trip() {
        let mut recorder = ReplayRecorder::new(1, 42, 2);
        for i in 0..10 {
            recorder.record_frame(i, vec![make_input(i)], [i as u8; 16], [i as u8; 32]);
        }

        let mut buffer = Vec::new();
        recorder.save(&mut buffer).unwrap();

        let loaded = ReplayRecorder::load(buffer.as_slice()).unwrap();
        assert_eq!(loaded.version, REPLAY_FORMAT_VERSION);
        assert_eq!(loaded.session_id, 1);
        assert_eq!(loaded.seed, 42);
        assert_eq!(loaded.player_count, 2);
        assert_eq!(loaded.frames.len(), 10);
        assert_eq!(loaded.frames[0].tick, 0);
        assert_eq!(loaded.frames[9].tick, 9);
    }

    #[test]
    fn gzip_compression_reduces_size() {
        let mut recorder = ReplayRecorder::new(1, 42, 2);
        for i in 0..100 {
            recorder.record_frame(i, vec![make_input(i)], [0u8; 16], [0u8; 32]);
        }

        let mut compressed = Vec::new();
        recorder.save(&mut compressed).unwrap();

        let uncompressed = bincode::serialize(&ReplayFile {
            version: REPLAY_FORMAT_VERSION,
            session_id: 1,
            seed: 42,
            player_count: 2,
            frames: recorder.frames.clone(),
        })
        .unwrap();

        assert!(compressed.len() < uncompressed.len());
    }

    #[test]
    fn load_empty_replay() {
        let recorder = ReplayRecorder::new(1, 42, 2);
        let mut buffer = Vec::new();
        recorder.save(&mut buffer).unwrap();

        let loaded = ReplayRecorder::load(buffer.as_slice()).unwrap();
        assert_eq!(loaded.frames.len(), 0);
    }

    #[test]
    fn load_corrupted_gzip_fails() {
        let bad_data = vec![0xFF, 0xFE, 0xFD, 0xFC];
        let result = ReplayRecorder::load(bad_data.as_slice());
        assert!(result.is_err());
    }

    #[test]
    fn rng_state_preserved() {
        let mut recorder = ReplayRecorder::new(1, 42, 1);
        let rng = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        recorder.record_frame(0, vec![], rng, [0u8; 32]);

        let mut buffer = Vec::new();
        recorder.save(&mut buffer).unwrap();

        let loaded = ReplayRecorder::load(buffer.as_slice()).unwrap();
        assert_eq!(loaded.frames[0].rng_state, rng);
    }
}
