// client/wasm_loader/src/single_player.rs
// Single-player 嵌入式 server 模式（#[cfg(feature = "single-player")]）

use bridge_types::{EcsMirror, EntityId, PlayerInput};
use deterministic::SoftF32;
use netcode::snapshot::GameSnapshot;
use simulation::AuthoritativeSimulation;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// Single-player 初始化錯誤
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // wasm_init() 整合時使用
pub enum SinglePlayerInitError {
    #[error("Single-player 已初始化，不可重複呼叫")]
    AlreadyInitialized,
}

/// Single-player 全域狀態
#[allow(dead_code)] // local_player_id 待 input 收集整合時使用
struct SinglePlayerState {
    sim: AuthoritativeSimulation,
    local_player_id: EntityId,
}

thread_local! {
    static SP_STATE: RefCell<Option<SinglePlayerState>> = const { RefCell::new(None) };
}

/// 初始化 single-player 模式。
#[allow(dead_code)] // wasm_init() 整合時呼叫
pub fn init_single_player() -> Result<(), SinglePlayerInitError> {
    SP_STATE.with(|s| {
        let mut state = s.borrow_mut();
        if state.is_some() {
            return Err(SinglePlayerInitError::AlreadyInitialized);
        }

        let initial_state = GameSnapshot {
            tick: 0,
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 0,
                delta_time: SoftF32::from_f32(0.01667), // 60Hz
            },
            rng_state: [0u8; 16],
        };

        let sim = AuthoritativeSimulation::new(initial_state, 0);
        let local_player_id = EntityId(1);

        *state = Some(SinglePlayerState {
            sim,
            local_player_id,
        });

        tracing::info!("Single-player 模式已初始化");
        Ok(())
    })
}

/// Single-player lockstep tick（由 wasm_tick 委派）。
///
/// Lockstep 流程：
///   1. 收集本地 input（目前為空 placeholder）
///   2. 包裝為 BTreeMap<EntityId, Vec<PlayerInput>>
///   3. AuthoritativeSimulation::advance(inputs)
///   4. Client 直接套用 authoritative state（無 prediction、無 rollback）
pub fn tick(timestamp: f64) {
    SP_STATE.with(|s| {
        let mut state = s.borrow_mut();
        let sp = match state.as_mut() {
            Some(sp) => sp,
            None => {
                tracing::warn!("Single-player tick 但尚未初始化");
                return;
            }
        };

        let _ = timestamp; // 僅供基礎設施計時，不用於遊戲邏輯

        // 收集本地 input（TODO: 從瀏覽器鍵盤/滑鼠事件收集）
        let inputs: BTreeMap<EntityId, Vec<PlayerInput>> = BTreeMap::new();

        // Lockstep：直接驅動 authoritative simulation
        sp.sim.advance(&inputs);

        tracing::trace!("Single-player tick {} 完成", sp.sim.current_tick());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_state() {
        SP_STATE.with(|s| *s.borrow_mut() = None);
    }

    #[test]
    fn test_init_single_player() {
        reset_state();
        assert!(init_single_player().is_ok());
    }

    #[test]
    fn test_init_twice_fails() {
        reset_state();
        assert!(init_single_player().is_ok());
        assert!(matches!(
            init_single_player(),
            Err(SinglePlayerInitError::AlreadyInitialized)
        ));
    }

    #[test]
    fn test_tick_after_init() {
        reset_state();
        init_single_player().unwrap();
        tick(16.67); // 不 panic
        tick(33.34);
        SP_STATE.with(|s| {
            let state = s.borrow();
            assert_eq!(state.as_ref().unwrap().sim.current_tick(), 2);
        });
    }

    #[test]
    fn test_tick_without_init_no_panic() {
        reset_state();
        tick(16.67); // 不 panic，僅 log warning
    }
}
