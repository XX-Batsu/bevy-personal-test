// client/wasm_loader/src/single_player.rs
// Single-player 嵌入式 server 模式（#[cfg(feature = "single-player")]）

use bridge_types::{EcsMirror, EntityId, PlayerInput};
use deterministic::SoftF32;
use netcode::snapshot::GameSnapshot;
use simulation::AuthoritativeSimulation;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// Single-player 的 session id（無 server，故不需唯一性）
const SINGLE_PLAYER_SESSION_ID: u64 = 0;

/// Single-player 的 RNG seed（固定值，確保每次啟動可重現）
const SEED: u64 = 0;

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

        // session_id 於 single-player 無對外意義，固定為 0；seed 亦固定，
        // 使同一版本的 client 每次啟動都跑出相同的模擬序列。
        let sim = AuthoritativeSimulation::new(SINGLE_PLAYER_SESSION_ID, initial_state, SEED);
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
///   3. AuthoritativeSimulation::step_full(inputs)
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

        // 收集本地 input。
        // 瀏覽器鍵盤/滑鼠事件由 JS 層（transport.js）透過 addEventListener 監聽，
        // 經 wasm-bindgen 轉發至 WASM。目前 single-player 模式使用空 input 驅動模擬，
        // 完整 input pipeline 需要 JS 層配合：
        //   1. JS addEventListener('keydown'/'mousemove') 收集原始事件
        //   2. JS 呼叫 wasm_on_input(encoded_bytes) 傳入 WASM
        //   3. WASM 解碼為 PlayerInput 並填入此 BTreeMap
        // 此架構跨越 JS/WASM 邊界，不適合在純 Rust 側單獨實作。
        let inputs: BTreeMap<EntityId, Vec<PlayerInput>> = BTreeMap::new();

        // Lockstep：直接驅動 authoritative simulation
        match sp.sim.step_full(&inputs) {
            Ok(result) => {
                tracing::trace!("Single-player tick {} 完成", result.tick);
            }
            Err(e) => {
                tracing::error!("Single-player tick 失敗：{e}");
            }
        }
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
