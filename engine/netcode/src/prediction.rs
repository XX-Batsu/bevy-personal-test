//! Client-side prediction 管理器

use bridge_types::PlayerInput;

use crate::error::NetcodeError;
use crate::snapshot::{GameSnapshot, InputBuffer, SnapshotBuffer};

/// 最大 rollback 深度（幀數）
/// 8 × 16.67ms ≈ 133ms RTT，覆蓋大部分 4G 行動網路延遲
pub const MAX_ROLLBACK_DEPTH: u64 = 8;

/// Client-side prediction 管理器
///
/// 不持有 buffer，透過參數接收 &mut SnapshotBuffer 和 &mut InputBuffer，
/// 避免所有權衝突（NetcodeState 統一持有）。
pub struct PredictionManager {
    /// 待 rollback 的 tick 號（若有 hash mismatch 則設定）
    pending_rollback: Option<u64>,
    /// 上一次從 server 接收的確認 tick
    last_confirmed_tick: u64,
}

impl PredictionManager {
    pub fn new() -> Self {
        Self {
            pending_rollback: None,
            last_confirmed_tick: 0,
        }
    }

    /// 套用本地輸入：
    /// 1. 將輸入 clone 後存入 input_buffer
    /// 2. 將當前 snapshot 存入 snapshot_buffer
    /// 3. 清理過舊的 input（retain_recent）
    pub fn apply_input(
        &mut self,
        input: &[PlayerInput],
        tick: u64,
        snapshot_buffer: &mut SnapshotBuffer<GameSnapshot>,
        input_buffer: &mut InputBuffer,
        current_snapshot: GameSnapshot,
    ) {
        input_buffer.insert(tick, input.to_vec());
        snapshot_buffer.push(tick, current_snapshot);
        input_buffer.retain_recent(tick);
    }

    /// 接收 server 的權威狀態：
    /// 1. 檢查 rollback 深度是否超過 MAX_ROLLBACK_DEPTH(8)
    /// 2. 比對 server_hash 與 local_hash
    /// 3. hash 不符 → 設定 pending_rollback；hash 一致 → 更新 last_confirmed_tick
    pub fn receive_server_state(
        &mut self,
        server_tick: u64,
        server_hash: [u8; 32],
        local_hash: [u8; 32],
        current_tick: u64,
    ) -> Result<(), NetcodeError> {
        if current_tick > server_tick {
            let depth = current_tick - server_tick;
            if depth > MAX_ROLLBACK_DEPTH {
                return Err(NetcodeError::RollbackDepthExceeded(depth));
            }
        }

        if server_hash != local_hash {
            self.pending_rollback = Some(server_tick);
            tracing::warn!(
                server_tick = server_tick,
                "預測 hash 不符：設定 pending_rollback"
            );
        } else {
            self.last_confirmed_tick = server_tick;
            self.pending_rollback = None;
        }

        Ok(())
    }

    /// 回傳需要 rollback 的 tick（若無需 rollback 則 None）
    pub fn needs_rollback(&self) -> Option<u64> {
        self.pending_rollback
    }

    /// 清除 rollback 標記（rollback 完成後呼叫）
    pub fn clear_rollback(&mut self) {
        self.pending_rollback = None;
    }

    /// 最後確認的 tick
    pub fn last_confirmed_tick(&self) -> u64 {
        self.last_confirmed_tick
    }
}

impl Default for PredictionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, EcsMirror, EntityId};
    use deterministic::SoftF32;
    use std::collections::BTreeMap;

    fn make_snapshot(tick: u64) -> GameSnapshot {
        GameSnapshot {
            tick,
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: tick,
                delta_time: SoftF32::from_f32(0.01667),
            },
            rng_state: [0u8; 16],
        }
    }

    fn setup() -> (PredictionManager, SnapshotBuffer<GameSnapshot>, InputBuffer) {
        (
            PredictionManager::new(),
            SnapshotBuffer::new(),
            InputBuffer::new(),
        )
    }

    fn make_hash_zero() -> [u8; 32] {
        [0u8; 32]
    }

    fn make_hash_different() -> [u8; 32] {
        [0xFF; 32]
    }

    #[test]
    fn prediction_new_no_rollback() {
        let pm = PredictionManager::new();
        assert_eq!(pm.needs_rollback(), None);
        assert_eq!(pm.last_confirmed_tick(), 0);
    }

    #[test]
    fn apply_input_stores_in_input_buffer() {
        let (mut pm, mut sb, mut ib) = setup();
        let inputs = vec![PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: DeterministicValue::Unit,
            tick: 5,
        }];
        pm.apply_input(&inputs, 5, &mut sb, &mut ib, make_snapshot(5));
        assert_eq!(ib.get(5), Some(&inputs));
    }

    #[test]
    fn apply_input_stores_snapshot() {
        let (mut pm, mut sb, mut ib) = setup();
        let snap = make_snapshot(5);
        pm.apply_input(&[], 5, &mut sb, &mut ib, snap.clone());
        assert_eq!(sb.get(5), Some(&snap));
    }

    #[test]
    fn apply_input_retains_recent() {
        let (mut pm, mut sb, mut ib) = setup();
        for t in 1..=15 {
            pm.apply_input(&[], t, &mut sb, &mut ib, make_snapshot(t));
        }
        assert_eq!(ib.get(1), None);
        assert_eq!(ib.get(5), None);
        assert!(ib.get(6).is_some());
    }

    #[test]
    fn apply_input_tick_zero() {
        let (mut pm, mut sb, mut ib) = setup();
        let inputs = vec![PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: DeterministicValue::Unit,
            tick: 0,
        }];
        pm.apply_input(&inputs, 0, &mut sb, &mut ib, make_snapshot(0));
        assert_eq!(ib.get(0), Some(&inputs));
    }

    #[test]
    fn no_server_correction_no_rollback() {
        assert_eq!(PredictionManager::new().needs_rollback(), None);
    }

    #[test]
    fn server_correction_hash_match() {
        let (mut pm, mut sb, mut ib) = setup();
        pm.apply_input(&[], 5, &mut sb, &mut ib, make_snapshot(5));
        let hash = make_hash_zero();
        assert!(pm.receive_server_state(5, hash, hash, 8).is_ok());
        assert_eq!(pm.needs_rollback(), None);
    }

    #[test]
    fn server_correction_triggers_rollback() {
        let (mut pm, mut sb, mut ib) = setup();
        pm.apply_input(&[], 5, &mut sb, &mut ib, make_snapshot(5));
        assert!(pm
            .receive_server_state(5, make_hash_zero(), make_hash_different(), 8)
            .is_ok());
        assert_eq!(pm.needs_rollback(), Some(5));
    }

    #[test]
    fn receive_server_state_depth_exceeded() {
        let (mut pm, mut sb, mut ib) = setup();
        pm.apply_input(&[], 5, &mut sb, &mut ib, make_snapshot(5));
        let hash = make_hash_zero();
        assert_eq!(
            pm.receive_server_state(5, hash, hash, 14),
            Err(NetcodeError::RollbackDepthExceeded(9))
        );
    }

    #[test]
    fn receive_server_state_depth_at_limit() {
        let (mut pm, mut sb, mut ib) = setup();
        pm.apply_input(&[], 5, &mut sb, &mut ib, make_snapshot(5));
        let hash = make_hash_zero();
        assert!(pm.receive_server_state(5, hash, hash, 13).is_ok());
    }

    #[test]
    fn clear_rollback_resets() {
        let (mut pm, mut sb, mut ib) = setup();
        pm.apply_input(&[], 5, &mut sb, &mut ib, make_snapshot(5));
        let _ = pm.receive_server_state(5, make_hash_zero(), make_hash_different(), 8);
        assert!(pm.needs_rollback().is_some());
        pm.clear_rollback();
        assert_eq!(pm.needs_rollback(), None);
    }

    #[test]
    fn receive_server_updates_confirmed_tick() {
        let (mut pm, mut sb, mut ib) = setup();
        pm.apply_input(&[], 10, &mut sb, &mut ib, make_snapshot(10));
        let hash = make_hash_zero();
        let _ = pm.receive_server_state(10, hash, hash, 15);
        assert_eq!(pm.last_confirmed_tick(), 10);
    }

    #[test]
    fn rollback_depth_zero() {
        let mut pm = PredictionManager::new();
        let hash = make_hash_zero();
        assert!(pm.receive_server_state(5, hash, hash, 5).is_ok());
    }
}
