//! Rollback 回溯管理器與 NetcodeState 聚合

use bridge_types::Blake3Hash;
use state_hash::compute_state_hash;

use crate::error::NetcodeError;
use crate::prediction::{PredictionManager, MAX_ROLLBACK_DEPTH};
use crate::simulation_step::SimulationStep;
use crate::snapshot::{GameSnapshot, InputBuffer, SnapshotBuffer};

/// Rollback 回溯管理器
///
/// 負責從歷史快照還原並 re-simulate 至當前 tick。
/// 不持有 buffer——透過參數接收（NetcodeState 統一持有）。
pub struct RollbackManager {
    full_resync_requested: bool,
}

impl RollbackManager {
    pub fn new() -> Self {
        Self {
            full_resync_requested: false,
        }
    }

    /// 從 rollback_tick 還原快照，re-simulate 至 current_tick
    ///
    /// 流程：
    /// 1. 深度檢查 (depth > MAX_ROLLBACK_DEPTH → error)
    /// 2. 從 snapshot_buffer 取回 rollback_tick 的快照
    /// 3. 逐 tick re-simulate：step → 更新 snapshot_buffer
    /// 4. 回傳 re-simulated 後的最新快照
    pub fn rollback_to<S: SimulationStep>(
        &mut self,
        rollback_tick: u64,
        current_tick: u64,
        snapshot_buffer: &mut SnapshotBuffer<GameSnapshot>,
        input_buffer: &InputBuffer,
        step_fn: &S,
    ) -> Result<GameSnapshot, NetcodeError> {
        // 深度檢查
        if current_tick > rollback_tick {
            let depth = current_tick - rollback_tick;
            if depth > MAX_ROLLBACK_DEPTH {
                return Err(NetcodeError::RollbackDepthExceeded(depth));
            }
        }

        // 取回快照
        let base = snapshot_buffer
            .get(rollback_tick)
            .ok_or(NetcodeError::SnapshotNotFound(rollback_tick))?
            .clone();

        let mut state = base;

        // Re-simulate from rollback_tick+1 to current_tick (inclusive)
        for tick in (rollback_tick + 1)..=current_tick {
            let inputs = input_buffer.get(tick).map(|v| v.as_slice()).unwrap_or(&[]);
            step_fn.step(&mut state, inputs);
            state.tick = tick;
            snapshot_buffer.push(tick, state.clone());
        }

        Ok(state)
    }

    /// 請求完整重新同步（server 下發 FullSyncPacket）
    pub fn request_full_resync(&mut self) {
        self.full_resync_requested = true;
    }

    /// 是否需要完整重新同步
    pub fn needs_full_resync(&self) -> bool {
        self.full_resync_requested
    }

    /// 清除完整重新同步標記
    pub fn clear_full_resync(&mut self) {
        self.full_resync_requested = false;
    }
}

impl Default for RollbackManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Netcode 聚合狀態（GRASP Controller 模式）
///
/// 統一持有所有 buffer 和子管理器，避免 &mut 借用衝突。
pub struct NetcodeState<S: SimulationStep> {
    pub snapshot_buffer: SnapshotBuffer<GameSnapshot>,
    pub input_buffer: InputBuffer,
    pub prediction: PredictionManager,
    pub rollback: RollbackManager,
    pub step_fn: S,
}

impl<S: SimulationStep> NetcodeState<S> {
    pub fn new(step_fn: S) -> Self {
        Self {
            snapshot_buffer: SnapshotBuffer::new(),
            input_buffer: InputBuffer::new(),
            prediction: PredictionManager::new(),
            rollback: RollbackManager::new(),
            step_fn,
        }
    }

    /// 處理 server 權威狀態：
    /// 1. 從 snapshot_buffer 取出 local snapshot 並計算 hash
    /// 2. 呼叫 PredictionManager::receive_server_state 比對
    /// 3. 若需 rollback → 呼叫 RollbackManager::rollback_to
    pub fn handle_server_state(
        &mut self,
        server_tick: u64,
        server_hash: Blake3Hash,
        current_tick: u64,
    ) -> Result<Option<GameSnapshot>, NetcodeError> {
        // 取出 local snapshot 計算 hash
        let local_snapshot = self
            .snapshot_buffer
            .get(server_tick)
            .ok_or(NetcodeError::SnapshotNotFound(server_tick))?;

        let local_hash = compute_state_hash(
            &local_snapshot.ecs_mirror.entities,
            &local_snapshot.rng_state,
            local_snapshot.tick,
        );

        // 比對 hash
        self.prediction
            .receive_server_state(server_tick, server_hash, local_hash, current_tick)?;

        // 若需 rollback
        if let Some(rollback_tick) = self.prediction.needs_rollback() {
            let result = self.rollback.rollback_to(
                rollback_tick,
                current_tick,
                &mut self.snapshot_buffer,
                &self.input_buffer,
                &self.step_fn,
            )?;
            self.prediction.clear_rollback();
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation_step::{CountingSimulationStep, MockSimulationStep};
    use bridge_types::{EcsMirror, EntityId};
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

    // === RollbackManager 測試 ===

    #[test]
    fn rollback_restores_snapshot_state() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let ib = InputBuffer::new();
        let snap = make_snapshot(5);
        sb.push(5, snap.clone());
        let result = rm
            .rollback_to(5, 5, &mut sb, &ib, &MockSimulationStep)
            .unwrap();
        assert_eq!(result.tick, 5);
    }

    #[test]
    fn rollback_resimulates_inputs_from_tick() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let mut ib = InputBuffer::new();
        let counter = CountingSimulationStep::new();

        sb.push(5, make_snapshot(5));
        for t in 6..=8 {
            ib.insert(t, vec![]);
        }

        let result = rm.rollback_to(5, 8, &mut sb, &ib, &counter).unwrap();
        assert_eq!(counter.count(), 3); // re-simulate ticks 6, 7, 8
        assert_eq!(result.tick, 8);
    }

    #[test]
    fn rollback_depth_within_limit_succeeds() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let ib = InputBuffer::new();
        sb.push(5, make_snapshot(5));
        // depth = 13 - 5 = 8 == MAX_ROLLBACK_DEPTH
        assert!(rm
            .rollback_to(5, 13, &mut sb, &ib, &MockSimulationStep)
            .is_ok());
    }

    #[test]
    fn rollback_depth_exceeds_limit_fails() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let ib = InputBuffer::new();
        sb.push(5, make_snapshot(5));
        // depth = 14 - 5 = 9 > MAX_ROLLBACK_DEPTH
        assert_eq!(
            rm.rollback_to(5, 14, &mut sb, &ib, &MockSimulationStep),
            Err(NetcodeError::RollbackDepthExceeded(9))
        );
    }

    #[test]
    fn rollback_determinism_same_inputs_same_result() {
        let mut rm1 = RollbackManager::new();
        let mut rm2 = RollbackManager::new();
        let mut sb1 = SnapshotBuffer::new();
        let mut sb2 = SnapshotBuffer::new();
        let mut ib1 = InputBuffer::new();
        let mut ib2 = InputBuffer::new();

        sb1.push(5, make_snapshot(5));
        sb2.push(5, make_snapshot(5));
        for t in 6..=8 {
            ib1.insert(t, vec![]);
            ib2.insert(t, vec![]);
        }

        let r1 = rm1
            .rollback_to(5, 8, &mut sb1, &ib1, &MockSimulationStep)
            .unwrap();
        let r2 = rm2
            .rollback_to(5, 8, &mut sb2, &ib2, &MockSimulationStep)
            .unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn rollback_with_empty_inputs() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let ib = InputBuffer::new(); // 完全空的
        sb.push(3, make_snapshot(3));
        let result = rm
            .rollback_to(3, 6, &mut sb, &ib, &MockSimulationStep)
            .unwrap();
        assert_eq!(result.tick, 6);
    }

    #[test]
    fn rollback_to_same_tick() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let ib = InputBuffer::new();
        let counter = CountingSimulationStep::new();
        sb.push(5, make_snapshot(5));
        let result = rm.rollback_to(5, 5, &mut sb, &ib, &counter).unwrap();
        assert_eq!(counter.count(), 0); // 同 tick，不需 re-simulate
        assert_eq!(result.tick, 5);
    }

    #[test]
    fn full_resync_clear_resets_flag() {
        let mut rm = RollbackManager::new();
        assert!(!rm.needs_full_resync());
        rm.request_full_resync();
        assert!(rm.needs_full_resync());
        rm.clear_full_resync();
        assert!(!rm.needs_full_resync());
    }

    #[test]
    fn rollback_snapshot_not_found() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let ib = InputBuffer::new();
        assert_eq!(
            rm.rollback_to(99, 100, &mut sb, &ib, &MockSimulationStep),
            Err(NetcodeError::SnapshotNotFound(99))
        );
    }

    #[test]
    fn rollback_depth_far_exceeds_limit() {
        let mut rm = RollbackManager::new();
        let mut sb = SnapshotBuffer::new();
        let ib = InputBuffer::new();
        sb.push(0, make_snapshot(0));
        assert_eq!(
            rm.rollback_to(0, 100, &mut sb, &ib, &MockSimulationStep),
            Err(NetcodeError::RollbackDepthExceeded(100))
        );
    }

    // === NetcodeState 測試 ===

    #[test]
    fn netcode_state_handle_server_state_no_rollback() {
        let mut ns = NetcodeState::new(MockSimulationStep);
        let snap = make_snapshot(5);
        ns.snapshot_buffer.push(5, snap.clone());
        // 計算 server hash = local hash（相同 snapshot）
        let server_hash = compute_state_hash(&snap.ecs_mirror.entities, &snap.rng_state, snap.tick);
        let result = ns.handle_server_state(5, server_hash, 8).unwrap();
        assert!(result.is_none()); // 無需 rollback
    }

    #[test]
    fn netcode_state_handle_server_state_triggers_rollback() {
        let mut ns = NetcodeState::new(MockSimulationStep);
        let snap = make_snapshot(5);
        ns.snapshot_buffer.push(5, snap);
        // 使用完全不同的 hash 觸發 rollback
        let fake_hash = [0xFF; 32];
        let result = ns.handle_server_state(5, fake_hash, 8).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().tick, 8);
    }

    #[test]
    fn netcode_state_handle_server_state_snapshot_not_found() {
        let mut ns = NetcodeState::new(MockSimulationStep);
        let result = ns.handle_server_state(99, [0u8; 32], 100);
        assert_eq!(result, Err(NetcodeError::SnapshotNotFound(99)));
    }
}
