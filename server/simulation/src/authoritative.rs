//! Server 端權威模擬骨架

use std::collections::BTreeMap;

use bridge_types::{Blake3Hash, EntityId, PlayerInput};
use deterministic::DeterministicRng;
use netcode::connection::{ConnectionState, FullSyncPacket};
use netcode::snapshot::GameSnapshot;
use server_types::{DeterministicSimulation, SimulationError, StepResult};
use state_hash::compute_state_hash;

/// 玩家資訊
#[derive(Debug, Clone)]
pub struct PlayerInfo {
    /// 連線狀態（來自 netcode::ConnectionState）
    pub connection_state: ConnectionState,
    /// 玩家加入時的 tick 號（用於 timeout 判斷）
    pub joined_at_tick: u64,
}

/// 完整伺服器快照（含遊戲狀態與玩家 metadata）
///
/// 用於 rollback：還原此 snapshot 可完整重現當時的伺服器狀態，
/// 包含 `GameSnapshot`（tick、ECS、RNG）以及所有玩家的連線資訊。
#[derive(Debug, Clone)]
pub struct ServerSnapshot {
    /// 遊戲核心狀態（tick、ECS mirror、RNG state）
    pub game: GameSnapshot,
    /// 玩家 metadata（BTreeMap 保證迭代確定性）
    pub players: BTreeMap<EntityId, PlayerInfo>,
}

/// Server 端權威模擬
///
/// Phase 12 將填充完整遊戲邏輯。
/// 此骨架僅定義介面與基本測試。
#[derive(Debug)]
pub struct AuthoritativeSimulation {
    /// Session 唯一識別碼
    session_id: u64,
    /// 當前遊戲狀態
    state: GameSnapshot,
    /// 確定性 RNG
    rng: DeterministicRng,
    /// 當前 tick
    current_tick: u64,
    /// 參與玩家（BTreeMap 保證迭代確定性）
    players: BTreeMap<EntityId, PlayerInfo>,
}

impl AuthoritativeSimulation {
    /// 建立新的權威模擬
    ///
    /// `initial_state.tick` 決定起始 tick（支援從非零 tick 繼續執行）。
    pub fn new(session_id: u64, initial_state: GameSnapshot, seed: u64) -> Self {
        let current_tick = initial_state.tick;
        let rng = DeterministicRng::seed_from_u64(seed);
        let mut state = initial_state;
        // 確保 state.rng_state 與 rng 的初始狀態一致，
        // 避免從 initial_state 帶入舊有 rng_state 造成兩者分歧。
        state.rng_state = rng.state_bytes();
        Self {
            session_id,
            state,
            rng,
            current_tick,
            players: BTreeMap::new(),
        }
    }

    /// 取得當前 tick
    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// 取得當前狀態的不可變引用
    pub fn state(&self) -> &GameSnapshot {
        &self.state
    }

    /// 取得 RNG 狀態
    pub fn rng_state_bytes(&self) -> [u8; 16] {
        self.rng.state_bytes()
    }

    /// 取得 session_id
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// 加入玩家
    pub fn add_player(&mut self, entity_id: EntityId) {
        self.players.insert(
            entity_id,
            PlayerInfo {
                connection_state: ConnectionState::Connected,
                joined_at_tick: self.current_tick,
            },
        );
    }

    /// 移除玩家
    pub fn remove_player(&mut self, entity_id: &EntityId) {
        self.players.remove(entity_id);
    }

    /// 查詢玩家資訊
    pub fn player_info(&self, entity_id: &EntityId) -> Option<&PlayerInfo> {
        self.players.get(entity_id)
    }

    /// Server 主迴圈用：推進一幀並返回完整結果（含 state hash）
    ///
    /// 1. 驗證所有輸入的 tick 號與 current_tick 一致（不符合回傳 TickMismatch）
    /// 2. TODO(game-logic): 遍歷 inputs，執行玩家移動、攻擊、技能等遊戲邏輯
    ///    插入點：docs/handbook/game-logic-stubs.md §遊戲邏輯插入點
    /// 3. TODO(game-logic): 消耗 rng 產生隨機事件（掉落、暴擊等）
    ///    使用 self.rng.next_u32() 等方法
    /// 4. 推進 current_tick
    /// 5. 計算並回傳 StepResult
    pub fn step_full(
        &mut self,
        inputs: &BTreeMap<EntityId, Vec<PlayerInput>>,
    ) -> Result<StepResult, SimulationError> {
        // 驗證輸入 tick 序號
        for inputs_vec in inputs.values() {
            for input in inputs_vec {
                if input.tick != self.current_tick {
                    return Err(SimulationError::TickMismatch {
                        expected: self.current_tick,
                        got: input.tick,
                    });
                }
            }
        }

        // TODO(game-logic): 遍歷 inputs，執行玩家移動、攻擊、技能等遊戲邏輯
        // 範例：for (entity_id, player_inputs) in inputs { ... }
        // 插入點：docs/handbook/game-logic-stubs.md §遊戲邏輯插入點

        // TODO(game-logic): 消耗 rng 產生隨機事件（掉落、暴擊等）
        // 範例：let roll = self.rng.next_u32() % 100;

        self.current_tick += 1;
        self.state.tick = self.current_tick;
        self.state.ecs_mirror.frame_number = self.current_tick;
        self.state.rng_state = self.rng.state_bytes();

        let state_hash = compute_state_hash(
            &self.state.ecs_mirror.entities,
            &self.state.rng_state,
            self.current_tick,
        );

        Ok(StepResult {
            tick: self.current_tick,
            state_hash,
            events: Vec::new(),
        })
    }

    /// 組裝 ServerSnapshot（供 SnapshotBuffer 使用）
    ///
    /// 包含完整伺服器狀態（GameSnapshot + players BTreeMap），
    /// 確保 rollback 後玩家 metadata 與遊戲狀態保持一致。
    pub fn make_snapshot(&self) -> ServerSnapshot {
        ServerSnapshot {
            game: self.state.clone(),
            players: self.players.clone(),
        }
    }

    /// 從 ServerSnapshot 還原（rollback 還原點）
    ///
    /// 完整還原遊戲狀態與玩家 metadata，確保還原後的伺服器狀態
    /// 與 snapshot 時刻完全一致。
    pub fn restore_snapshot(&mut self, snapshot: &ServerSnapshot) {
        self.state = snapshot.game.clone();
        self.current_tick = self.state.tick;
        self.rng = DeterministicRng::from_state_bytes(&self.state.rng_state);
        self.players = snapshot.players.clone();
    }

    /// 產生 FullSyncPacket（斷線重連 / 硬同步）
    pub fn create_full_sync_packet(&self) -> FullSyncPacket {
        FullSyncPacket {
            tick: self.state.tick,
            ecs_mirror: self.state.ecs_mirror.clone(),
            rng_state: self.state.rng_state,
        }
    }
}

impl DeterministicSimulation for AuthoritativeSimulation {
    type Error = SimulationError;

    fn restore_rng(&mut self, rng_state: &[u8; 16]) {
        self.rng = DeterministicRng::from_state_bytes(rng_state);
        self.state.rng_state = *rng_state;
    }

    /// trait 實作：委派 step_full()，供回放/驗證泛型綁定使用
    fn step(
        &mut self,
        inputs: &BTreeMap<EntityId, Vec<PlayerInput>>,
    ) -> Result<(), SimulationError> {
        self.step_full(inputs).map(|_| ())
    }

    // TODO(perf): 透過 replay 路徑呼叫時（step() → step_full() 內已計算一次，
    // 呼叫端再呼叫 compute_state_hash() 又算一次），hash 共計算兩次。
    // 改善方向：在 step_full() 中快取 last_state_hash 欄位，
    // 讓 compute_state_hash() 直接回傳快取值。
    fn compute_state_hash(&self) -> Blake3Hash {
        compute_state_hash(
            &self.state.ecs_mirror.entities,
            &self.rng.state_bytes(),
            self.current_tick,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::EcsMirror;
    use deterministic::SoftF32;

    fn make_initial_state() -> GameSnapshot {
        GameSnapshot {
            tick: 0,
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 0,
                delta_time: SoftF32::from_f32(0.01667),
            },
            rng_state: [0u8; 16],
        }
    }

    fn make_sim() -> AuthoritativeSimulation {
        AuthoritativeSimulation::new(42, make_initial_state(), 123)
    }

    // ── 基本功能 ──

    #[test]
    fn new_simulation_starts_at_tick_zero() {
        assert_eq!(make_sim().current_tick(), 0);
    }

    #[test]
    fn new_simulation_starts_at_initial_state_tick() {
        // initial_state.tick = 5 時，sim 起始 tick 應為 5
        let mut state = make_initial_state();
        state.tick = 5;
        let sim = AuthoritativeSimulation::new(1, state, 42);
        assert_eq!(sim.current_tick(), 5);
    }

    #[test]
    fn session_id_accessible() {
        assert_eq!(make_sim().session_id(), 42);
    }

    // ── step_full ──

    #[test]
    fn step_full_increments_tick() {
        let mut sim = make_sim();
        let result = sim.step_full(&BTreeMap::new()).unwrap();
        assert_eq!(result.tick, 1);
        assert_eq!(sim.current_tick(), 1);
    }

    #[test]
    fn step_full_returns_empty_events() {
        let mut sim = make_sim();
        let result = sim.step_full(&BTreeMap::new()).unwrap();
        assert!(result.events.is_empty());
    }

    #[test]
    fn step_full_state_hash_is_32_bytes() {
        let mut sim = make_sim();
        let result = sim.step_full(&BTreeMap::new()).unwrap();
        assert_eq!(result.state_hash.len(), 32);
    }

    #[test]
    fn step_full_tick_monotonically_increases() {
        let mut sim = make_sim();
        for expected_tick in 1..=5u64 {
            let result = sim.step_full(&BTreeMap::new()).unwrap();
            assert_eq!(result.tick, expected_tick);
        }
    }

    #[test]
    fn step_full_syncs_frame_number() {
        // Fix 1 驗收：step 後 state.ecs_mirror.frame_number 必須等於 current_tick
        let mut sim = make_sim();
        for expected_tick in 1..=3u64 {
            sim.step_full(&BTreeMap::new()).unwrap();
            assert_eq!(sim.state().ecs_mirror.frame_number, expected_tick);
            assert_eq!(sim.state().ecs_mirror.frame_number, sim.current_tick());
        }
    }

    // ── make_snapshot / restore_snapshot ──

    #[test]
    fn snapshot_roundtrip_preserves_tick() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap();
        sim.step_full(&BTreeMap::new()).unwrap();
        let snap = sim.make_snapshot();
        assert_eq!(snap.game.tick, 2);
        sim.step_full(&BTreeMap::new()).unwrap();
        assert_eq!(sim.current_tick(), 3);
        sim.restore_snapshot(&snap);
        assert_eq!(sim.current_tick(), 2);
    }

    #[test]
    fn snapshot_roundtrip_preserves_rng() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap();
        let snap = sim.make_snapshot();
        let rng_after_snap = snap.game.rng_state;
        sim.step_full(&BTreeMap::new()).unwrap();
        sim.restore_snapshot(&snap);
        assert_eq!(sim.rng_state_bytes(), rng_after_snap);
    }

    #[test]
    fn snapshot_roundtrip_preserves_players() {
        // Fix 2 驗收：restore 後 players 內容與 snapshot 時完全一致
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 1
        sim.add_player(EntityId(1));
        sim.add_player(EntityId(2));

        let snap = sim.make_snapshot();

        // 繼續推進並修改玩家清單
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 2
        sim.add_player(EntityId(3));

        // 快照時應有 2 個玩家
        assert_eq!(snap.players.len(), 2);
        assert!(snap.players.contains_key(&EntityId(1)));
        assert!(snap.players.contains_key(&EntityId(2)));

        // 還原後，玩家清單回到 snapshot 時的狀態（2 個玩家，無 EntityId(3)）
        sim.restore_snapshot(&snap);
        assert_eq!(sim.players.len(), 2);
        assert!(sim.players.contains_key(&EntityId(1)));
        assert!(sim.players.contains_key(&EntityId(2)));
        assert!(!sim.players.contains_key(&EntityId(3)));

        // 驗證 joined_at_tick 也被還原
        assert_eq!(sim.players[&EntityId(1)].joined_at_tick, 1);
        assert_eq!(sim.players[&EntityId(2)].joined_at_tick, 1);
    }

    // ── create_full_sync_packet ──

    #[test]
    fn full_sync_packet_tick_matches_current() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap();
        let pkt = sim.create_full_sync_packet();
        assert_eq!(pkt.tick, 1);
    }

    #[test]
    fn full_sync_packet_rng_state_matches() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap();
        let pkt = sim.create_full_sync_packet();
        assert_eq!(pkt.rng_state, sim.rng_state_bytes());
    }

    // ── DeterministicSimulation trait ──

    #[test]
    fn trait_step_increments_tick() {
        let mut sim = make_sim();
        sim.step(&BTreeMap::new()).unwrap();
        assert_eq!(sim.current_tick(), 1);
    }

    #[test]
    fn trait_compute_state_hash_is_32_bytes() {
        let sim = make_sim();
        let hash = sim.compute_state_hash();
        assert_eq!(hash.len(), 32);
    }

    #[test]
    fn trait_restore_rng_then_hash_deterministic() {
        // 兩個相同 seed 的 sim 各自 step，hash 應相同（確定性驗證）
        let mut sim1 = make_sim();
        let mut sim2 = make_sim();
        sim1.step(&BTreeMap::new()).unwrap();
        sim2.step(&BTreeMap::new()).unwrap();
        assert_eq!(sim1.compute_state_hash(), sim2.compute_state_hash());

        // 從 snapshot 還原 RNG 後，hash 應與還原前時間點一致
        let snap = sim1.make_snapshot();
        sim1.step(&BTreeMap::new()).unwrap(); // 推進 sim1 到 tick=2
        sim1.restore_snapshot(&snap); // 還原回 tick=1
        assert_eq!(sim1.compute_state_hash(), sim2.compute_state_hash());
    }

    // ── add_player / remove_player / player_info ──

    #[test]
    fn add_player_records_join_tick() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 1
        sim.add_player(EntityId(5));
        // mod tests 與 struct 同 module，可合法存取 private 欄位；
        // 此處驗證內部不變式，不需對外暴露 players getter。
        let player = &sim.players[&EntityId(5)];
        assert_eq!(player.joined_at_tick, 1);
    }

    #[test]
    fn player_info_returns_some_after_add() {
        let mut sim = make_sim();
        sim.add_player(EntityId(7));
        assert!(sim.player_info(&EntityId(7)).is_some());
    }

    #[test]
    fn player_info_records_joined_at_tick() {
        let mut sim = make_sim();
        sim.add_player(EntityId(7));
        assert_eq!(sim.player_info(&EntityId(7)).unwrap().joined_at_tick, 0);
    }

    #[test]
    fn player_info_returns_none_for_unknown() {
        let sim = make_sim();
        assert!(sim.player_info(&EntityId(99)).is_none());
    }

    #[test]
    fn remove_player_removes_from_players() {
        let mut sim = make_sim();
        sim.add_player(EntityId(7));
        assert!(sim.player_info(&EntityId(7)).is_some());
        sim.remove_player(&EntityId(7));
        assert!(sim.player_info(&EntityId(7)).is_none());
    }

    #[test]
    fn remove_player_nonexistent_is_noop() {
        let mut sim = make_sim();
        // 移除不存在的玩家不應 panic
        sim.remove_player(&EntityId(99));
    }

    // ── 確定性驗證 ──

    #[test]
    fn compute_state_hash_is_deterministic_across_identical_simulations() {
        // 兩個相同 seed 的 sim 每幀 hash 應完全一致
        let mut sim1 = make_sim();
        let mut sim2 = make_sim();
        for _ in 0..5 {
            let r1 = sim1.step_full(&BTreeMap::new()).unwrap();
            let r2 = sim2.step_full(&BTreeMap::new()).unwrap();
            assert_eq!(
                r1.state_hash, r2.state_hash,
                "相同 tick 下 state_hash 應完全一致"
            );
            assert_eq!(
                sim1.compute_state_hash(),
                sim2.compute_state_hash(),
                "compute_state_hash() 應與 step_full 返回的 hash 一致"
            );
        }
    }

    // ── restore_snapshot 連續性 ──

    #[test]
    fn restore_snapshot_then_step_continues_from_correct_tick() {
        // rollback 後繼續推進，tick 從還原點（2）起算，不從還原前（4）起算
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 1
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 2
        let snap = sim.make_snapshot(); // snapshot at tick=2
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 3
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 4
        sim.restore_snapshot(&snap);
        assert_eq!(sim.current_tick(), 2);
        let result = sim.step_full(&BTreeMap::new()).unwrap();
        assert_eq!(result.tick, 3);
        assert_eq!(sim.current_tick(), 3);
        assert_eq!(sim.state().tick, 3);
        assert_eq!(sim.state().ecs_mirror.frame_number, 3);
    }

    // ── restore_rng trait method ──

    #[test]
    fn restore_rng_only_affects_rng_not_tick() {
        // trait method restore_rng() 只還原 RNG，不影響 tick
        // 骨架實作中 step_full() 不消耗 RNG，故直接用任意 bytes 驗證 restore_rng 語意
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 1
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 2
        assert_eq!(sim.current_tick(), 2);
        let target_rng: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        sim.restore_rng(&target_rng);
        assert_eq!(
            sim.rng_state_bytes(),
            target_rng,
            "restore_rng 應正確還原 RNG 狀態"
        );
        assert_eq!(sim.current_tick(), 2, "restore_rng 不應改變 tick");
        assert_eq!(sim.state().tick, 2, "restore_rng 不應改變 state.tick");
    }

    // ── step_full 多玩家輸入 ──

    #[test]
    fn step_full_with_multiple_valid_inputs_succeeds() {
        let mut sim = make_sim();
        let input1 = PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: bridge_types::DeterministicValue::Unit,
            tick: 0,
        };
        let input2 = PlayerInput {
            player_id: EntityId(2),
            input_type: 0,
            data: bridge_types::DeterministicValue::Unit,
            tick: 0,
        };
        let mut inputs = BTreeMap::new();
        inputs.insert(EntityId(1), vec![input1]);
        inputs.insert(EntityId(2), vec![input2]);
        let result = sim.step_full(&inputs);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().tick, 1);
    }

    // ── step_full tick 驗證 ──

    #[test]
    fn step_full_with_wrong_tick_returns_tick_mismatch() {
        let mut sim = make_sim();
        // current_tick = 0，但輸入帶 tick = 5
        let wrong_input = PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: bridge_types::DeterministicValue::Unit,
            tick: 5,
        };
        let mut inputs = BTreeMap::new();
        inputs.insert(EntityId(1), vec![wrong_input]);
        let result = sim.step_full(&inputs);
        assert!(matches!(
            result,
            Err(SimulationError::TickMismatch {
                expected: 0,
                got: 5
            })
        ));
    }

    // tick 落後（tick < current_tick）亦應回傳 TickMismatch
    #[test]
    fn step_full_with_past_tick_returns_tick_mismatch() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap(); // current_tick = 1
        // 輸入帶舊 tick=0（落後一幀）
        let stale_input = PlayerInput {
            player_id: EntityId(1),
            input_type: 0,
            data: bridge_types::DeterministicValue::Unit,
            tick: 0,
        };
        let mut inputs = BTreeMap::new();
        inputs.insert(EntityId(1), vec![stale_input]);
        let result = sim.step_full(&inputs);
        assert!(matches!(
            result,
            Err(SimulationError::TickMismatch {
                expected: 1,
                got: 0,
            })
        ));
    }

    // ── state() getter ──

    #[test]
    fn state_getter_tick_matches_current_tick() {
        let mut sim = make_sim();
        // 初始狀態
        assert_eq!(sim.state().tick, sim.current_tick());
        sim.step_full(&BTreeMap::new()).unwrap();
        // step 後 state().tick 必須與 current_tick() 一致
        assert_eq!(sim.state().tick, sim.current_tick());
        assert_eq!(sim.state().tick, 1);
    }

    // new() 以非零 tick 初始化時，state().tick 也應等於 initial_state.tick
    #[test]
    fn new_with_nonzero_tick_state_tick_matches() {
        let mut state = make_initial_state();
        state.tick = 10;
        let sim = AuthoritativeSimulation::new(1, state, 42);
        assert_eq!(sim.state().tick, 10);
        assert_eq!(sim.current_tick(), 10);
    }

    // ── rng_state_bytes() 與 state().rng_state 同步 ──

    #[test]
    fn rng_state_bytes_matches_state_rng_state_after_steps() {
        let mut sim = make_sim();
        // new() 時 state.rng_state 必須與 rng 同步（設計不變式）
        assert_eq!(sim.rng_state_bytes(), sim.state().rng_state);
        for _ in 0..3 {
            sim.step_full(&BTreeMap::new()).unwrap();
            // 每次 step 後兩者應持續同步
            assert_eq!(
                sim.rng_state_bytes(),
                sim.state().rng_state,
                "step 後 rng_state_bytes() 應與 state().rng_state 一致"
            );
        }
    }

    // ── make_snapshot() zero-player ──

    #[test]
    fn make_snapshot_with_no_players() {
        let sim = make_sim();
        let snap = sim.make_snapshot();
        assert_eq!(snap.players.len(), 0);
        assert_eq!(snap.game.tick, 0);
    }

    // make_snapshot 的 game.rng_state 與 rng_state_bytes() 一致
    #[test]
    fn make_snapshot_rng_state_matches_rng_state_bytes() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap();
        let snap = sim.make_snapshot();
        assert_eq!(snap.game.rng_state, sim.rng_state_bytes());
    }

    // ── restore_snapshot() 驗證 state.rng_state 也被還原 ──

    #[test]
    fn restore_snapshot_syncs_state_rng_state() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap();
        let snap = sim.make_snapshot();
        let expected_rng = snap.game.rng_state;

        sim.step_full(&BTreeMap::new()).unwrap(); // 繼續推進
        sim.restore_snapshot(&snap);

        // restore 後 state().rng_state 應等於 snapshot 時的 rng_state
        assert_eq!(
            sim.state().rng_state,
            expected_rng,
            "restore_snapshot 後 state().rng_state 應回到 snapshot 時的值"
        );
        // 且與 rng_state_bytes() 一致
        assert_eq!(sim.rng_state_bytes(), sim.state().rng_state);
    }

    // ── create_full_sync_packet() at tick=0 ──

    #[test]
    fn full_sync_packet_at_tick_zero() {
        let sim = make_sim();
        let pkt = sim.create_full_sync_packet();
        assert_eq!(pkt.tick, 0);
        assert_eq!(pkt.rng_state, sim.rng_state_bytes());
    }

    // create_full_sync_packet() 的 ecs_mirror 與 state() 一致
    #[test]
    fn full_sync_packet_ecs_mirror_matches_state() {
        let mut sim = make_sim();
        sim.step_full(&BTreeMap::new()).unwrap();
        let pkt = sim.create_full_sync_packet();
        assert_eq!(
            pkt.ecs_mirror.frame_number,
            sim.state().ecs_mirror.frame_number
        );
        assert_eq!(
            pkt.ecs_mirror.local_player_id,
            sim.state().ecs_mirror.local_player_id
        );
    }

    // ── add_player 重複加入同一 entity_id ──

    #[test]
    fn add_player_twice_overwrites_join_tick() {
        let mut sim = make_sim();
        sim.add_player(EntityId(10)); // joined_at_tick = 0
        sim.step_full(&BTreeMap::new()).unwrap(); // tick = 1
        sim.add_player(EntityId(10)); // 重複加入，應覆蓋 joined_at_tick = 1
        // player_info() 應存在且只有一筆
        assert!(sim.player_info(&EntityId(10)).is_some());
        assert_eq!(sim.players.len(), 1);
        // joined_at_tick 更新為第二次加入時的 tick
        assert_eq!(
            sim.player_info(&EntityId(10)).unwrap().joined_at_tick,
            1,
            "重複 add_player 應覆蓋 joined_at_tick 為最新的 current_tick"
        );
    }

    // ── ConnectionState 初始值 ──

    #[test]
    fn add_player_initial_connection_state_is_connected() {
        let mut sim = make_sim();
        sim.add_player(EntityId(3));
        assert!(matches!(
            sim.player_info(&EntityId(3)).unwrap().connection_state,
            ConnectionState::Connected
        ));
    }
}
