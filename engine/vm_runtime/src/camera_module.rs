//! Rhai camera bridge：5 個 native function（camera_shake / camera_push_override /
//! camera_pop_override / camera_clear_overrides / camera_clear_shakes）。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.8.4 / §6.3.1
//!
//! 處理順序：
//! 1. ops_tracker.deduct
//! 2. NaN / Inf 檢查 → reject
//! 3. handle 範圍檢查（0..=u32::MAX）
//! 4. trauma clamp、sentinel 套用（在 push 至 queue 前）
//! 5. 構造 CameraBridgeOp，push 至 BridgeState.camera_op_queue
//!
//! ## 為何先 deduct 後 reject（spec §3.8.4 設計選擇）
//!
//! 步驟 1（deduct）在步驟 2（NaN/Inf reject）之前是**有意設計**：
//! 防止腳本反覆觸發 NaN reject 來規避 ops_cost。若先 reject 後 deduct，
//! 腳本可以零成本執行 `loop { camera_shake(1, 0.0/0.0, ...); }` 占用 CPU。
//! 先 deduct 確保即使參數無效，呼叫本身仍消耗 budget，反覆呼叫會在 50K ops 內耗盡。

use bridge_types::CameraBridgeOp;
use rhai::{EvalAltResult, Position};

use crate::bridge_api::{BridgeState, SharedState};
use crate::ops_cost::OpsCostTable;

// ── 內部輔助 ──────────────────────────────────────────────────────
//
// TODO（Phase B 收尾或後續 housekeeping）: `check_finite` 與 `ops_err`
// 在 `bridge_api.rs` 已有等價實作（`validate_finite` line 305、`ops_err`
// line 147）。應把 bridge_api 內這兩個 helper 升級為 `pub(crate)` 並讓
// `camera_module` 共用，避免錯誤訊息字串雙份維護。
// 同時：`check_handle_range` 對齊 `bridge_api` 既有 `validate_*_id` 命名
// 慣例，建議改名為 `validate_handle` 並考慮搬到 bridge_api（其他未來
// bridge — audio handle / vfx handle — 也會需要相同 i64→u32 saturating
// bounded 轉換）。
//
// 留 TODO 不立即動手以縮小本次變更 scope。

fn invalid_param(msg: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(msg.into(), Position::NONE))
}

fn ops_err(msg: String) -> Box<EvalAltResult> {
    invalid_param(msg)
}

/// NaN / Inf 檢查；違反則 reject 為 `InvalidParameter`。
fn check_finite(value: f64, name: &str) -> Result<(), Box<EvalAltResult>> {
    if !value.is_finite() {
        return Err(invalid_param(format!(
            "InvalidParameter: {} 必須為有限數值，收到 {}",
            name, value
        )));
    }
    Ok(())
}

/// handle 範圍檢查：必須在 `0..=u32::MAX`。
fn check_handle_range(handle: i64) -> Result<u32, Box<EvalAltResult>> {
    if !(0..=(u32::MAX as i64)).contains(&handle) {
        return Err(invalid_param(format!(
            "InvalidParameter: handle 必須在 [0, u32::MAX]，收到 {}",
            handle
        )));
    }
    Ok(handle as u32)
}

// ── 5 個 register 函式 ──

/// 註冊 `camera_shake`（8 個參數，ops cost 10）。
///
/// Rhai 簽名：`camera_shake(handle, trauma, dir_x, dir_y, max_strength, decay_rate,
///             direction_bias, perpendicular_damping) -> ()`
///
/// 處理順序：deduct → NaN/Inf → handle range → trauma clamp + 各 strength 參數
/// `.max(0.0)` 飽和 → push queue。
/// - `trauma` clamp 至 `[0.0, 1.0]`
/// - `max_strength` / `decay_rate` / `direction_bias` / `perpendicular_damping`
///   負值 saturate 至 `0.0`（plan B4.2 Step 8 規定）
fn register_camera_shake(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "camera_shake",
        move |handle: i64,
              trauma: f64,
              dir_x: f64,
              dir_y: f64,
              max_strength: f64,
              decay_rate: f64,
              direction_bias: f64,
              perpendicular_damping: f64|
              -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();

            // 步驟 1：先扣 ops（先 deduct 後 reject，防 0-cost 攻擊）
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("camera_shake"))
                .map_err(ops_err)?;

            // 步驟 2：NaN / Inf 檢查
            check_finite(trauma, "trauma")?;
            check_finite(dir_x, "dir_x")?;
            check_finite(dir_y, "dir_y")?;
            check_finite(max_strength, "max_strength")?;
            check_finite(decay_rate, "decay_rate")?;
            check_finite(direction_bias, "direction_bias")?;
            check_finite(perpendicular_damping, "perpendicular_damping")?;

            // 步驟 3：handle 範圍檢查
            let handle = check_handle_range(handle)?;

            // 步驟 4：clamp / saturate
            let trauma = (trauma as f32).clamp(0.0, 1.0);
            let max_strength = (max_strength as f32).max(0.0);
            let decay_rate = (decay_rate as f32).max(0.0);
            let direction_bias = (direction_bias as f32).max(0.0);
            let perpendicular_damping = (perpendicular_damping as f32).max(0.0);

            // 步驟 5：push op
            st.camera_op_queue.push(CameraBridgeOp::Shake {
                handle,
                trauma,
                dir_x: dir_x as f32,
                dir_y: dir_y as f32,
                max_strength,
                decay_rate,
                direction_bias,
                perpendicular_damping,
            });

            tracing::debug!(handle, trauma, "camera_shake 已排入佇列");
            Ok(())
        },
    );
}

/// 註冊 `camera_push_override`（7 個參數，ops cost 8）。
///
/// Rhai 簽名：`camera_push_override(handle, x, y, zoom, speed, priority, duration) -> ()`
///
/// sentinel 規則（由 flush 層套用，此層保留原值）：
/// - `zoom <= 0.0` → flush 層解讀為 `target_zoom = None`
/// - `duration < 0.0` → flush 層解讀為無期限（`None`）
/// - `duration = 0.0` → flush 層解讀為 `Some(0.0)`（立即移除）
///
/// `priority` 為 i64，使用 `.clamp(i32::MIN, i32::MAX)` 飽和而非直接 `as i32`
/// 截斷，避免 wrap-around（plan B4.2 Step 8 規定）。
fn register_camera_push_override(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "camera_push_override",
        move |handle: i64,
              x: f64,
              y: f64,
              zoom: f64,
              speed: f64,
              priority: i64,
              duration: f64|
              -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();

            // 步驟 1：先扣 ops
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("camera_push_override"))
                .map_err(ops_err)?;

            // 步驟 2：NaN / Inf 檢查（zoom / duration 允許 sentinel，不要求正數，但仍須 finite）
            check_finite(x, "x")?;
            check_finite(y, "y")?;
            check_finite(zoom, "zoom")?;
            check_finite(speed, "speed")?;
            check_finite(duration, "duration")?;

            // 步驟 3：handle 範圍檢查
            let handle = check_handle_range(handle)?;

            // 步驟 4：priority saturating clamp（避免 i64→i32 wrap-around）
            let priority: i32 = priority.clamp(i32::MIN as i64, i32::MAX as i64) as i32;

            // 步驟 5：push op
            st.camera_op_queue.push(CameraBridgeOp::PushOverride {
                handle,
                x: x as f32,
                y: y as f32,
                zoom: zoom as f32,
                speed: speed as f32,
                priority,
                duration: duration as f32,
            });

            tracing::debug!(
                handle,
                x = x as f32,
                y = y as f32,
                "camera_push_override 已排入佇列"
            );
            Ok(())
        },
    );
}

/// 註冊 `camera_pop_override`（1 個參數，ops cost 3）。
///
/// Rhai 簽名：`camera_pop_override(handle) -> ()`
///
/// 移除最高 priority override；handle 0 為 broadcast。
fn register_camera_pop_override(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "camera_pop_override",
        move |handle: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();

            // 步驟 1：先扣 ops
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("camera_pop_override"))
                .map_err(ops_err)?;

            // 步驟 3：handle 範圍檢查
            let handle = check_handle_range(handle)?;

            // 步驟 5：push op
            st.camera_op_queue
                .push(CameraBridgeOp::PopOverride { handle });

            tracing::debug!(handle, "camera_pop_override 已排入佇列");
            Ok(())
        },
    );
}

/// 註冊 `camera_clear_overrides`（1 個參數，ops cost 5）。
///
/// Rhai 簽名：`camera_clear_overrides(handle) -> ()`
///
/// 清空 override stack；handle 0 為 broadcast。
fn register_camera_clear_overrides(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "camera_clear_overrides",
        move |handle: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();

            // 步驟 1：先扣 ops
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("camera_clear_overrides"))
                .map_err(ops_err)?;

            // 步驟 3：handle 範圍檢查
            let handle = check_handle_range(handle)?;

            // 步驟 5：push op
            st.camera_op_queue
                .push(CameraBridgeOp::ClearOverrides { handle });

            tracing::debug!(handle, "camera_clear_overrides 已排入佇列");
            Ok(())
        },
    );
}

/// 註冊 `camera_clear_shakes`（1 個參數，ops cost 5）。
///
/// Rhai 簽名：`camera_clear_shakes(handle) -> ()`
///
/// 清空所有 shake entries；handle 0 為 broadcast。
fn register_camera_clear_shakes(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "camera_clear_shakes",
        move |handle: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();

            // 步驟 1：先扣 ops
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("camera_clear_shakes"))
                .map_err(ops_err)?;

            // 步驟 3：handle 範圍檢查
            let handle = check_handle_range(handle)?;

            // 步驟 5：push op
            st.camera_op_queue
                .push(CameraBridgeOp::ClearShakes { handle });

            tracing::debug!(handle, "camera_clear_shakes 已排入佇列");
            Ok(())
        },
    );
}

/// 將所有 camera bridge native function 註冊至 Rhai Engine。
///
/// 呼叫於 `register_bridge_api` 末尾（§3.8 Camera Bridge，Phase B）。
/// `state` 由呼叫端 move 進來（最後一個 register fn，不需 clone）。
pub fn register_camera_module(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    register_camera_shake(engine, state.clone());
    register_camera_push_override(engine, state.clone());
    register_camera_pop_override(engine, state.clone());
    register_camera_clear_overrides(engine, state.clone());
    register_camera_clear_shakes(engine, state);
}

// ── 測試 ──

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::bridge_api::BridgeState;

    fn make_state() -> SharedState<BridgeState> {
        Rc::new(RefCell::new(BridgeState::new()))
    }

    fn make_engine_with_camera(state: SharedState<BridgeState>) -> rhai::Engine {
        let mut engine = rhai::Engine::new();
        register_camera_module(&mut engine, state);
        engine
    }

    // ── camera_shake ──

    #[test]
    fn shake_valid_call_pushes_to_queue() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> =
            engine.eval("camera_shake(1, 0.5, 1.0, 0.0, 25.0, 2.0, 1.5, 0.3)");
        assert!(result.is_ok(), "有效 camera_shake 呼叫應成功：{:?}", result);

        let queue_len = state.borrow().camera_op_queue.len();
        assert_eq!(queue_len, 1, "queue 應有 1 個 op");

        let ops = state.borrow_mut().camera_op_queue.drain();
        match &ops[0] {
            CameraBridgeOp::Shake {
                handle,
                trauma,
                dir_x,
                dir_y,
                ..
            } => {
                assert_eq!(*handle, 1u32);
                assert!((trauma - 0.5f32).abs() < 1e-5);
                assert!((dir_x - 1.0f32).abs() < 1e-5);
                assert!((dir_y - 0.0f32).abs() < 1e-5);
            }
            _ => panic!("期望 CameraBridgeOp::Shake"),
        }
    }

    #[test]
    fn shake_handle_negative_rejects() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> =
            engine.eval("camera_shake(-1, 0.5, 1.0, 0.0, 25.0, 2.0, 1.5, 0.3)");
        assert!(result.is_err(), "負值 handle 應被 reject");
    }

    #[test]
    fn shake_handle_exceeds_u32_rejects() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        // u32::MAX = 4294967295，i64 大於此值應 reject
        let result: Result<(), _> =
            engine.eval("camera_shake(4294967296, 0.5, 1.0, 0.0, 25.0, 2.0, 1.5, 0.3)");
        assert!(result.is_err(), "超出 u32::MAX 的 handle 應被 reject");
    }

    #[test]
    fn shake_trauma_nan_reject() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        // Rhai 中 0.0/0.0 = NaN
        let result: Result<(), _> =
            engine.eval("camera_shake(1, 0.0/0.0, 1.0, 0.0, 25.0, 2.0, 1.5, 0.3)");
        assert!(result.is_err(), "NaN trauma 應被 reject");
    }

    #[test]
    fn shake_nan_reject_still_deducts_ops_anti_attack() {
        // 攻擊防護驗證：即使 NaN 參數被 reject，ops_tracker 仍應扣 cost。
        // 這是「先 deduct 後 reject」設計的核心 invariant（spec §3.8.4，
        // camera_module.rs 模組 doc）— 防止腳本反覆觸發 NaN reject 來規避
        // ops_cost 占用 CPU。
        let state = make_state();
        let initial = state.borrow().ops_tracker.remaining();
        let engine = make_engine_with_camera(state.clone());

        let result = engine.run("camera_shake(1, 0.0/0.0, 0.0, 0.0, 25.0, 2.0, 1.5, 0.3);");
        assert!(result.is_err(), "NaN trauma 應 reject 為 InvalidParameter");

        let after = state.borrow().ops_tracker.remaining();
        assert_eq!(
            initial - after,
            10,
            "NaN-reject 仍應扣 cost(10)，否則攻擊者可 0-cost 反覆呼叫；\
             initial={initial}, after={after}"
        );
    }

    #[test]
    fn shake_trauma_inf_reject() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        // Rhai 中 1.0/0.0 = Inf
        let result: Result<(), _> =
            engine.eval("camera_shake(1, 1.0/0.0, 1.0, 0.0, 25.0, 2.0, 1.5, 0.3)");
        assert!(result.is_err(), "Inf trauma 應被 reject");
    }

    #[test]
    fn shake_trauma_clamps_to_zero_one() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        // trauma = 2.0 → clamp 至 1.0
        let result: Result<(), _> =
            engine.eval("camera_shake(1, 2.0, 0.0, 0.0, 25.0, 2.0, 1.5, 0.3)");
        assert!(
            result.is_ok(),
            "超出 [0,1] 的 trauma 應 clamp 後成功：{:?}",
            result
        );

        let ops = state.borrow_mut().camera_op_queue.drain();
        match &ops[0] {
            CameraBridgeOp::Shake { trauma, .. } => {
                assert!(
                    (trauma - 1.0f32).abs() < 1e-5,
                    "trauma 應 clamp 至 1.0，實際 {}",
                    trauma
                );
            }
            _ => panic!("期望 CameraBridgeOp::Shake"),
        }
    }

    #[test]
    fn shake_max_strength_saturates_negative_to_zero() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());
        // max_strength = -25.0 應 saturate 至 0.0；其餘正常路徑值原樣保留。
        engine
            .run("camera_shake(1, 0.5, 0.0, 0.0, -25.0, 2.0, 1.5, 0.3);")
            .unwrap();
        let ops = state.borrow_mut().camera_op_queue.drain();
        let CameraBridgeOp::Shake {
            max_strength,
            decay_rate,
            direction_bias,
            perpendicular_damping,
            ..
        } = ops[0]
        else {
            panic!("期望 CameraBridgeOp::Shake")
        };
        assert_eq!(max_strength, 0.0, "max_strength 負值應 clamp 至 0");
        // 同時驗證 decay_rate / direction_bias / perpendicular_damping 正常路徑
        assert_eq!(decay_rate, 2.0);
        assert_eq!(direction_bias, 1.5);
        assert_eq!(perpendicular_damping, 0.3);
    }

    #[test]
    fn shake_deducts_ops_cost_ten() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let before = state.borrow().ops_tracker.remaining();
        let result: Result<(), _> =
            engine.eval("camera_shake(1, 0.5, 1.0, 0.0, 25.0, 2.0, 1.5, 0.3)");
        assert!(result.is_ok());
        let after = state.borrow().ops_tracker.remaining();
        assert_eq!(before - after, 10, "camera_shake 應扣 10 ops");
    }

    // ── camera_push_override ──

    #[test]
    fn push_override_valid_call_pushes_to_queue() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> =
            engine.eval("camera_push_override(1, 100.0, 200.0, 1.5, 5.0, 50, -1.0)");
        assert!(
            result.is_ok(),
            "有效 camera_push_override 呼叫應成功：{:?}",
            result
        );

        let queue_len = state.borrow().camera_op_queue.len();
        assert_eq!(queue_len, 1);

        let ops = state.borrow_mut().camera_op_queue.drain();
        match &ops[0] {
            CameraBridgeOp::PushOverride {
                handle,
                x,
                y,
                priority,
                ..
            } => {
                assert_eq!(*handle, 1u32);
                assert!((x - 100.0f32).abs() < 1e-3);
                assert!((y - 200.0f32).abs() < 1e-3);
                assert_eq!(*priority, 50i32);
            }
            _ => panic!("期望 CameraBridgeOp::PushOverride"),
        }
    }

    #[test]
    fn push_override_handle_negative_rejects() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> =
            engine.eval("camera_push_override(-1, 100.0, 200.0, 1.5, 5.0, 50, -1.0)");
        assert!(result.is_err(), "負值 handle 應被 reject");
    }

    #[test]
    fn push_override_x_nan_reject() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> =
            engine.eval("camera_push_override(1, 0.0/0.0, 200.0, 1.5, 5.0, 50, -1.0)");
        assert!(result.is_err(), "NaN x 應被 reject");
    }

    #[test]
    fn push_override_deducts_ops_cost_eight() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let before = state.borrow().ops_tracker.remaining();
        let result: Result<(), _> =
            engine.eval("camera_push_override(1, 100.0, 200.0, 1.5, 5.0, 50, -1.0)");
        assert!(result.is_ok());
        let after = state.borrow().ops_tracker.remaining();
        assert_eq!(before - after, 8, "camera_push_override 應扣 8 ops");
    }

    #[test]
    fn push_override_priority_clamps_at_i32_max() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());
        // i64 大於 i32::MAX，應 saturating clamp 至 i32::MAX（非 wrap 至 i32::MIN）
        engine
            .run(&format!(
                "camera_push_override(1, 0.0, 0.0, -1.0, 5.0, {}, -1.0);",
                (i32::MAX as i64) + 1000
            ))
            .unwrap();
        let ops = state.borrow_mut().camera_op_queue.drain();
        let CameraBridgeOp::PushOverride { priority, .. } = ops[0] else {
            panic!("期望 CameraBridgeOp::PushOverride")
        };
        assert_eq!(
            priority,
            i32::MAX,
            "priority 應 saturating clamp 至 i32::MAX，非 wrap"
        );
    }

    // ── camera_pop_override ──

    #[test]
    fn pop_override_valid_call() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> = engine.eval("camera_pop_override(1)");
        assert!(
            result.is_ok(),
            "有效 camera_pop_override 呼叫應成功：{:?}",
            result
        );

        let queue_len = state.borrow().camera_op_queue.len();
        assert_eq!(queue_len, 1);

        let ops = state.borrow_mut().camera_op_queue.drain();
        match &ops[0] {
            CameraBridgeOp::PopOverride { handle } => {
                assert_eq!(*handle, 1u32);
            }
            _ => panic!("期望 CameraBridgeOp::PopOverride"),
        }
    }

    #[test]
    fn pop_override_deducts_ops_cost_three() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let before = state.borrow().ops_tracker.remaining();
        let result: Result<(), _> = engine.eval("camera_pop_override(1)");
        assert!(result.is_ok());
        let after = state.borrow().ops_tracker.remaining();
        assert_eq!(before - after, 3, "camera_pop_override 應扣 3 ops");
    }

    // ── camera_clear_overrides ──

    #[test]
    fn clear_overrides_valid_call() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> = engine.eval("camera_clear_overrides(0)");
        assert!(
            result.is_ok(),
            "broadcast clear_overrides 應成功：{:?}",
            result
        );

        let queue_len = state.borrow().camera_op_queue.len();
        assert_eq!(queue_len, 1);

        let ops = state.borrow_mut().camera_op_queue.drain();
        match &ops[0] {
            CameraBridgeOp::ClearOverrides { handle } => {
                assert_eq!(*handle, 0u32); // broadcast sentinel
            }
            _ => panic!("期望 CameraBridgeOp::ClearOverrides"),
        }
    }

    #[test]
    fn clear_overrides_deducts_ops_cost_five() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let before = state.borrow().ops_tracker.remaining();
        let result: Result<(), _> = engine.eval("camera_clear_overrides(0)");
        assert!(result.is_ok());
        let after = state.borrow().ops_tracker.remaining();
        assert_eq!(before - after, 5, "camera_clear_overrides 應扣 5 ops");
    }

    // ── camera_clear_shakes ──

    #[test]
    fn clear_shakes_valid_call() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        let result: Result<(), _> = engine.eval("camera_clear_shakes(2)");
        assert!(
            result.is_ok(),
            "camera_clear_shakes 呼叫應成功：{:?}",
            result
        );

        let queue_len = state.borrow().camera_op_queue.len();
        assert_eq!(queue_len, 1);

        let ops = state.borrow_mut().camera_op_queue.drain();
        match &ops[0] {
            CameraBridgeOp::ClearShakes { handle } => {
                assert_eq!(*handle, 2u32);
            }
            _ => panic!("期望 CameraBridgeOp::ClearShakes"),
        }
    }

    #[test]
    fn clear_shakes_deducts_ops_cost_five() {
        let state = make_state();
        let initial = state.borrow().ops_tracker.remaining();
        let engine = make_engine_with_camera(state.clone());
        engine.run("camera_clear_shakes(1);").unwrap();
        assert_eq!(state.borrow().ops_tracker.remaining(), initial - 5);
    }

    // ── handle 範圍檢查（pop + clear 共用）──

    #[test]
    fn pop_and_clear_handle_range_check() {
        let state = make_state();
        let engine = make_engine_with_camera(state.clone());

        // 負值 handle → 所有單參數 API 皆應 reject
        let r1: Result<(), _> = engine.eval("camera_pop_override(-1)");
        assert!(r1.is_err(), "pop_override 負值 handle 應 reject");

        let r2: Result<(), _> = engine.eval("camera_clear_overrides(-1)");
        assert!(r2.is_err(), "clear_overrides 負值 handle 應 reject");

        let r3: Result<(), _> = engine.eval("camera_clear_shakes(-1)");
        assert!(r3.is_err(), "clear_shakes 負值 handle 應 reject");

        // 超出 u32::MAX（4294967296）也應 reject
        let r4: Result<(), _> = engine.eval("camera_pop_override(4294967296)");
        assert!(r4.is_err(), "pop_override 超出 u32::MAX 應 reject");
    }
}
