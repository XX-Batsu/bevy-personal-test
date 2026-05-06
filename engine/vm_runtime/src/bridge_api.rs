//! Bridge API 事件佇列與 Rhai 函數註冊
//!
//! 提供 [`EventQueue`]、[`EntityIdAllocator`]、[`BridgeState`] 與
//! [`register_bridge_api`] — 將全部 Bridge API 函數註冊至 Rhai 引擎。
//!
//! # 設計依據
//! - [03-bridge-api/README.md](../../../docs/design/script-engine/03-bridge-api/README.md)
//! - [03-bridge-api/bridge-events.md](../../../docs/design/script-engine/03-bridge-api/bridge-events.md)
//! - [06-performance/performance-costs.md](../../../docs/design/script-engine/06-performance/performance-costs.md)

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use bridge_types::{
    BridgeError, BridgeEvent, CameraBridgeOp, DynamicValue, EcsMirror, EffectHandle, EntityId,
    SoundHandle,
};
use deterministic::{SoftF32, SoftVec3};
use rhai::{Dynamic, EvalAltResult, Position};

use crate::bridge_helpers::{
    ops_err, validate_finite, validate_non_negative_id, validate_positive_id,
};
use crate::dynamic_convert::to_rhai_dynamic;
use crate::handle_registry::HandleRegistry;
use crate::ops_cost::{OpsCostTable, OpsTracker};

/// 共享狀態類型別名（WASM 單執行緒環境）
pub type SharedState<T> = Rc<RefCell<T>>;

/// 攝影機 op 佇列。Rhai → camera_module 註冊閉包 push，
/// 在 FixedUpdate.FlushBridgeEvents stage 由 bevy_runtime::camera::flush_camera_ops drain。
#[derive(Default)]
pub struct CameraOpQueue {
    ops: Vec<CameraBridgeOp>,
}

impl CameraOpQueue {
    pub fn new() -> Self {
        Self {
            ops: Vec::with_capacity(16),
        }
    }

    pub fn push(&mut self, op: CameraBridgeOp) {
        self.ops.push(op);
    }

    pub fn drain(&mut self) -> Vec<CameraBridgeOp> {
        self.ops.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// 事件佇列：收集單幀內所有 Bridge API 產生的 BridgeEvent
///
/// 本層不做容量限制，由 Phase 9 BridgeEventQueue 加入 4,096 上限。
pub struct EventQueue {
    events: Vec<BridgeEvent>,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl EventQueue {
    pub fn new() -> Self {
        Self {
            events: Vec::with_capacity(64),
        }
    }

    pub fn push(&mut self, event: BridgeEvent) {
        self.events.push(event);
    }

    /// 取出所有事件並清空佇列（FIFO 順序保證）
    pub fn drain(&mut self) -> Vec<BridgeEvent> {
        self.events.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Entity ID 預分配器（同步回傳 pre-allocated ID）
/// counter 從 1 開始，EntityId(0) 保留為無效值
pub struct EntityIdAllocator {
    counter: u64,
}

impl Default for EntityIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityIdAllocator {
    pub fn new() -> Self {
        Self { counter: 1 }
    }

    /// 分配下一個 EntityId（從 1 開始遞增）
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> EntityId {
        let id = EntityId(self.counter);
        self.counter += 1;
        id
    }
}

/// Bridge API 統一共享狀態
pub struct BridgeState {
    pub event_queue: EventQueue,
    pub camera_op_queue: CameraOpQueue, // 新增（B3 / B4 將使用）
    pub ecs_mirror: EcsMirror,
    pub ops_tracker: OpsTracker,
    pub handle_registry: HandleRegistry,
    pub entity_id_allocator: EntityIdAllocator,
}

impl Default for BridgeState {
    fn default() -> Self {
        Self::new()
    }
}

impl BridgeState {
    pub fn new() -> Self {
        Self {
            event_queue: EventQueue::new(),
            camera_op_queue: CameraOpQueue::new(), // 新增
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 0,
                delta_time: SoftF32::ZERO,
            },
            ops_tracker: OpsTracker::new(),
            handle_registry: HandleRegistry::new(),
            entity_id_allocator: EntityIdAllocator::new(),
        }
    }
}

// ── 內部輔助 ──

/// rhai::Dynamic → DynamicValue 轉換（渲染層用途，保留原生 f64）
fn to_dynamic_value(value: &Dynamic) -> Result<DynamicValue, BridgeError> {
    if value.is::<i64>() {
        Ok(DynamicValue::Int(value.clone_cast::<i64>()))
    } else if value.is::<f64>() {
        Ok(DynamicValue::Float(value.clone_cast::<f64>()))
    } else if value.is::<bool>() {
        Ok(DynamicValue::Bool(value.clone_cast::<bool>()))
    } else if value.is::<rhai::ImmutableString>() {
        Ok(DynamicValue::String(
            value.clone_cast::<rhai::ImmutableString>().to_string(),
        ))
    } else {
        Err(BridgeError::InvalidParameter(format!(
            "不支援的 Dynamic 型別: {}",
            value.type_name()
        )))
    }
}

/// BridgeError → Box<EvalAltResult>
fn bridge_err(e: BridgeError) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(format!("{:?}", e).into(), Position::NONE).into()
}

/// f64 三元組 → SoftVec3
fn to_soft_vec3(x: f64, y: f64, z: f64) -> SoftVec3 {
    SoftVec3 {
        x: SoftF32::from_f64(x),
        y: SoftF32::from_f64(y),
        z: SoftF32::from_f64(z),
    }
}

// ── 參數驗證（三策略：reject / clamp / truncate） ──

/// 座標有效範圍
const COORDINATE_MIN: f64 = -1_000_000.0;
const COORDINATE_MAX: f64 = 1_000_000.0;

/// Scale 有效範圍
const SCALE_MIN_EXCLUSIVE: f64 = 0.0;
const SCALE_MAX_INCLUSIVE: f64 = 1_000.0;

/// Blend weight 有效範圍（clamp 策略，NaN/Inf 仍 reject）
const BLEND_WEIGHT_MIN: f64 = 0.0;
const BLEND_WEIGHT_MAX: f64 = 1.0;

/// 字串長度上限（truncate 策略）
const MAX_STRING_BYTES: usize = 4_096;

/// 驗證座標值（reject 策略：有限數值且在 [-1e6, 1e6]）
fn validate_coordinate(value: f64, param_name: &str) -> Result<(), Box<EvalAltResult>> {
    if !value.is_finite() {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!(
                "InvalidParameter: {} 必須為有限數值，收到 {}",
                param_name, value
            )
            .into(),
            Position::NONE,
        )));
    }
    if !(COORDINATE_MIN..=COORDINATE_MAX).contains(&value) {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!(
                "InvalidParameter: {} 超出範圍 [{}, {}]，收到 {}",
                param_name, COORDINATE_MIN, COORDINATE_MAX, value
            )
            .into(),
            Position::NONE,
        )));
    }
    Ok(())
}

/// 驗證二軸座標（reject 策略：show_damage_number 的 x/y 世界座標）
fn validate_coordinates_2d(x: f64, y: f64) -> Result<(), Box<EvalAltResult>> {
    validate_coordinate(x, "x")?;
    validate_coordinate(y, "y")?;
    Ok(())
}

/// 驗證三軸座標（reject 策略：set_transform/set_rotation/play_vfx/play_sound_at）
fn validate_coordinates(x: f64, y: f64, z: f64) -> Result<(), Box<EvalAltResult>> {
    validate_coordinate(x, "x")?;
    validate_coordinate(y, "y")?;
    validate_coordinate(z, "z")?;
    Ok(())
}

/// 驗證 scale 值（reject 策略：> 0 且 <= 1000）
fn validate_scale(value: f64, param_name: &str) -> Result<(), Box<EvalAltResult>> {
    if !value.is_finite() || value <= SCALE_MIN_EXCLUSIVE || value > SCALE_MAX_INCLUSIVE {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!(
                "InvalidParameter: {} 必須在 (0, {}] 範圍內，收到 {}",
                param_name, SCALE_MAX_INCLUSIVE, value
            )
            .into(),
            Position::NONE,
        )));
    }
    Ok(())
}

/// 驗證三軸 scale（reject 策略）
fn validate_scales(sx: f64, sy: f64, sz: f64) -> Result<(), Box<EvalAltResult>> {
    validate_scale(sx, "sx")?;
    validate_scale(sy, "sy")?;
    validate_scale(sz, "sz")?;
    Ok(())
}

/// 驗證 health bar 參數（reject 策略：max > 0, current ∈ [0, max]）
fn validate_health_bar(current: f64, max: f64) -> Result<(), Box<EvalAltResult>> {
    if !max.is_finite() {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            "InvalidParameter: health bar max 必須為有限數值"
                .to_string()
                .into(),
            Position::NONE,
        )));
    }
    if !current.is_finite() {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            "InvalidParameter: health bar current 必須為有限數值"
                .to_string()
                .into(),
            Position::NONE,
        )));
    }
    if max <= 0.0 {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!("InvalidParameter: health bar max 必須 > 0，收到 {}", max).into(),
            Position::NONE,
        )));
    }
    if current < 0.0 || current > max {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!(
                "InvalidParameter: health bar current 必須在 [0, max] 範圍內，收到 current={}, max={}",
                current, max
            )
            .into(),
            Position::NONE,
        )));
    }
    Ok(())
}

/// 夾限 blend weight（clamp 策略：有限值超界時夾限 + warn）
fn clamp_blend_weight(weight: f64) -> f64 {
    if !(BLEND_WEIGHT_MIN..=BLEND_WEIGHT_MAX).contains(&weight) {
        tracing::warn!(
            "blend_animation weight 超出 [{}, {}]，已夾限：{} → {}",
            BLEND_WEIGHT_MIN,
            BLEND_WEIGHT_MAX,
            weight,
            weight.clamp(BLEND_WEIGHT_MIN, BLEND_WEIGHT_MAX)
        );
    }
    weight.clamp(BLEND_WEIGHT_MIN, BLEND_WEIGHT_MAX)
}

/// 截斷字串（truncate 策略：超 4096 bytes 在 UTF-8 邊界截斷 + warn）
fn truncate_string(value: &str, param_name: &str) -> String {
    if value.len() > MAX_STRING_BYTES {
        tracing::warn!(
            "{} 長度超過 {} bytes（實際 {} bytes），已截斷",
            param_name,
            MAX_STRING_BYTES,
            value.len()
        );
        let truncated = &value[..value.floor_char_boundary(MAX_STRING_BYTES)];
        truncated.to_string()
    } else {
        value.to_string()
    }
}

/// EntityNotFound 處理：存在回傳 Some(())，不存在回傳 None + warn
fn check_entity_exists(eid: EntityId, ecs_mirror: &EcsMirror) -> Option<()> {
    if ecs_mirror.entities.contains_key(&eid) {
        Some(())
    } else {
        tracing::warn!("EntityNotFound: entity {:?} 不存在或已 despawn", eid);
        None
    }
}

// ── Bridge API 註冊 ──

/// 將所有 Bridge API 函數註冊至 Rhai Engine
///
/// 共 28 個 Rhai native function（21 mutation + 2 network + 5 query）。
/// 每個閉包：clone SharedState → borrow_mut → OpsTracker::deduct → 業務邏輯 → 回傳。
pub fn register_bridge_api(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    // §3.1 Entity（6）
    register_spawn_entity(engine, state.clone());
    register_despawn_entity(engine, state.clone());
    register_set_transform(engine, state.clone());
    register_set_rotation(engine, state.clone());
    register_set_scale(engine, state.clone());
    register_set_visibility(engine, state.clone());

    // §3.2 VFX / Audio（5）
    register_play_vfx(engine, state.clone());
    register_stop_vfx(engine, state.clone());
    register_play_sound(engine, state.clone());
    register_play_sound_at(engine, state.clone());
    register_stop_sound(engine, state.clone());

    // §3.3 UI（6）
    register_show_dialog(engine, state.clone());
    register_hide_dialog(engine, state.clone());
    register_update_hud(engine, state.clone());
    register_set_health_bar(engine, state.clone());
    register_show_damage_number(engine, state.clone());
    register_show_toast(engine, state.clone());

    // §3.4 Animation（4）
    register_play_animation(engine, state.clone());
    register_play_animation_once(engine, state.clone());
    register_stop_animation(engine, state.clone());
    register_blend_animation(engine, state.clone());

    // §3.5 Query（5）
    register_get_position(engine, state.clone());
    register_get_entity_state(engine, state.clone());
    register_get_local_player_id(engine, state.clone());
    register_get_frame_number(engine, state.clone());
    register_get_delta_time(engine, state.clone());

    // §3.6 Network（2）
    register_send_prediction(engine, state.clone());
    register_request_state(engine, state.clone());

    // §3.8 Camera Bridge（Phase B）
    crate::camera_module::register_camera_module(engine, state);
}

// ── §3.1 Entity ──

fn register_spawn_entity(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "spawn_entity",
        move |type_id: i64| -> Result<i64, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("spawn_entity"))
                .map_err(ops_err)?;
            let entity_id = st.entity_id_allocator.next();
            st.event_queue.push(BridgeEvent::SpawnEntity { type_id });
            Ok(entity_id.0 as i64)
        },
    );
}

fn register_despawn_entity(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "despawn_entity",
        move |eid: i64| -> Result<bool, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("despawn_entity"))
                .map_err(ops_err)?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(true);
            }
            st.event_queue
                .push(BridgeEvent::DespawnEntity { eid: entity_id });
            Ok(true)
        },
    );
}

fn register_set_transform(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "set_transform",
        move |eid: i64, x: f64, y: f64, z: f64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("set_transform"))
                .map_err(ops_err)?;
            validate_coordinates(x, y, z)?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::SetTransform {
                eid: entity_id,
                pos: to_soft_vec3(x, y, z),
            });
            Ok(())
        },
    );
}

fn register_set_rotation(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "set_rotation",
        move |eid: i64, rx: f64, ry: f64, rz: f64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("set_rotation"))
                .map_err(ops_err)?;
            validate_coordinates(rx, ry, rz)?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::SetRotation {
                eid: entity_id,
                euler: to_soft_vec3(rx, ry, rz),
            });
            Ok(())
        },
    );
}

fn register_set_scale(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "set_scale",
        move |eid: i64, sx: f64, sy: f64, sz: f64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("set_scale"))
                .map_err(ops_err)?;
            validate_scales(sx, sy, sz)?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::SetScale {
                eid: entity_id,
                scale: to_soft_vec3(sx, sy, sz),
            });
            Ok(())
        },
    );
}

fn register_set_visibility(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "set_visibility",
        move |eid: i64, visible: bool| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("set_visibility"))
                .map_err(ops_err)?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::SetVisibility {
                eid: entity_id,
                visible,
            });
            Ok(())
        },
    );
}

// ── §3.2 VFX / Audio ──

fn register_play_vfx(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "play_vfx",
        move |vfx_id: i64, x: f64, y: f64, z: f64| -> Result<i64, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("play_vfx"))
                .map_err(ops_err)?;
            validate_positive_id(vfx_id, "vfx_id")?;
            validate_coordinates(x, y, z)?;
            let handle = st.handle_registry.create_effect(0);
            st.event_queue.push(BridgeEvent::PlayVfx {
                vfx_id,
                pos: to_soft_vec3(x, y, z),
            });
            Ok(handle.0 as i64)
        },
    );
}

fn register_stop_vfx(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "stop_vfx",
        move |handle: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("stop_vfx"))
                .map_err(ops_err)?;
            let h = EffectHandle(handle as u32);
            if st.handle_registry.is_valid_effect(h) {
                st.event_queue.push(BridgeEvent::StopVfx { handle: h });
            }
            // 無效 handle → 靜默忽略（fire-and-forget）
            Ok(())
        },
    );
}

fn register_play_sound(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "play_sound",
        move |sound_id: i64| -> Result<i64, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("play_sound"))
                .map_err(ops_err)?;
            validate_positive_id(sound_id, "sound_id")?;
            let handle = st.handle_registry.create_sound(0);
            st.event_queue.push(BridgeEvent::PlaySound { sound_id });
            Ok(handle.0 as i64)
        },
    );
}

fn register_play_sound_at(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "play_sound_at",
        move |sound_id: i64, x: f64, y: f64, z: f64| -> Result<i64, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("play_sound_at"))
                .map_err(ops_err)?;
            validate_positive_id(sound_id, "sound_id")?;
            validate_coordinates(x, y, z)?;
            let handle = st.handle_registry.create_sound(0);
            st.event_queue.push(BridgeEvent::PlaySoundAt {
                sound_id,
                pos: to_soft_vec3(x, y, z),
            });
            Ok(handle.0 as i64)
        },
    );
}

fn register_stop_sound(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "stop_sound",
        move |handle: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("stop_sound"))
                .map_err(ops_err)?;
            let h = SoundHandle(handle as u32);
            if st.handle_registry.is_valid_sound(h) {
                st.event_queue.push(BridgeEvent::StopSound { handle: h });
            }
            Ok(())
        },
    );
}

// ── §3.3 UI ──

fn register_show_dialog(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "show_dialog",
        move |dialog_id: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("show_dialog"))
                .map_err(ops_err)?;
            validate_non_negative_id(dialog_id, "dialog_id")?;
            st.event_queue.push(BridgeEvent::ShowDialog { dialog_id });
            Ok(())
        },
    );
}

fn register_hide_dialog(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "hide_dialog",
        move |dialog_id: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("hide_dialog"))
                .map_err(ops_err)?;
            validate_non_negative_id(dialog_id, "dialog_id")?;
            st.event_queue.push(BridgeEvent::HideDialog { dialog_id });
            Ok(())
        },
    );
}

fn register_update_hud(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "update_hud",
        move |key: String, value: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("update_hud"))
                .map_err(ops_err)?;
            let key = truncate_string(&key, "key");
            let dv = to_dynamic_value(&value).map_err(bridge_err)?;
            st.event_queue
                .push(BridgeEvent::UpdateHud { key, value: dv });
            Ok(())
        },
    );
}

fn register_set_health_bar(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "set_health_bar",
        move |eid: i64, current: f64, max: f64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("set_health_bar"))
                .map_err(ops_err)?;
            validate_health_bar(current, max)?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::SetHealthBar {
                eid: entity_id,
                current,
                max,
            });
            Ok(())
        },
    );
}

fn register_show_damage_number(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "show_damage_number",
        move |x: f64, y: f64, value: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("show_damage_number"))
                .map_err(ops_err)?;
            validate_coordinates_2d(x, y)?;
            st.event_queue
                .push(BridgeEvent::ShowDamageNumber { x, y, value });
            Ok(())
        },
    );
}

fn register_show_toast(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "show_toast",
        move |msg: String, duration_ms: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("show_toast"))
                .map_err(ops_err)?;
            validate_positive_id(duration_ms, "duration_ms")?;
            let msg = truncate_string(&msg, "msg");
            st.event_queue
                .push(BridgeEvent::ShowToast { msg, duration_ms });
            Ok(())
        },
    );
}

// ── §3.4 Animation ──

fn register_play_animation(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "play_animation",
        move |eid: i64, anim_id: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("play_animation"))
                .map_err(ops_err)?;
            validate_non_negative_id(anim_id, "anim_id")?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::PlayAnimation {
                eid: entity_id,
                anim_id,
            });
            Ok(())
        },
    );
}

fn register_play_animation_once(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "play_animation_once",
        move |eid: i64, anim_id: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("play_animation_once"))
                .map_err(ops_err)?;
            validate_non_negative_id(anim_id, "anim_id")?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::PlayAnimationOnce {
                eid: entity_id,
                anim_id,
            });
            Ok(())
        },
    );
}

fn register_stop_animation(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "stop_animation",
        move |eid: i64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("stop_animation"))
                .map_err(ops_err)?;
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue
                .push(BridgeEvent::StopAnimation { eid: entity_id });
            Ok(())
        },
    );
}

fn register_blend_animation(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "blend_animation",
        move |eid: i64, anim_a: i64, anim_b: i64, weight: f64| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("blend_animation"))
                .map_err(ops_err)?;
            validate_finite(weight, "weight")?;
            let weight = clamp_blend_weight(weight);
            let entity_id = EntityId(eid as u64);
            if check_entity_exists(entity_id, &st.ecs_mirror).is_none() {
                return Ok(());
            }
            st.event_queue.push(BridgeEvent::BlendAnimation {
                eid: entity_id,
                anim_a,
                anim_b,
                weight,
            });
            Ok(())
        },
    );
}

// ── §3.5 Query（唯讀，不產生 event） ──

fn register_get_position(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "get_position",
        move |eid: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("get_position"))
                .map_err(ops_err)?;
            let entity_id = EntityId(eid as u64);
            match st.ecs_mirror.entities.get(&entity_id) {
                Some(entity) => {
                    let arr: rhai::Array = vec![
                        Dynamic::from(entity.position.x.to_f64()),
                        Dynamic::from(entity.position.y.to_f64()),
                        Dynamic::from(entity.position.z.to_f64()),
                    ];
                    Ok(Dynamic::from(arr))
                }
                None => {
                    tracing::warn!("EntityNotFound: {}", eid);
                    Ok(Dynamic::UNIT)
                }
            }
        },
    );
}

fn register_get_entity_state(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "get_entity_state",
        move |eid: i64, key: String| -> Result<Dynamic, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("get_entity_state"))
                .map_err(ops_err)?;
            let entity_id = EntityId(eid as u64);
            match st.ecs_mirror.entities.get(&entity_id) {
                Some(entity) => {
                    // 預定義 key → 結構欄位
                    match key.as_str() {
                        "hp" => Ok(Dynamic::from(entity.hp)),
                        "max_hp" => Ok(Dynamic::from(entity.max_hp)),
                        "state" => Ok(Dynamic::from(entity.state.0 as i64)),
                        "animation_id" => match entity.animation_id {
                            Some(id) => Ok(Dynamic::from(id as i64)),
                            None => Ok(Dynamic::UNIT),
                        },
                        _ => {
                            // 自定義 key → custom BTreeMap
                            match entity.custom.get(&key) {
                                Some(dv) => Ok(to_rhai_dynamic(dv)),
                                None => Ok(Dynamic::UNIT),
                            }
                        }
                    }
                }
                None => {
                    tracing::warn!("EntityNotFound: {}", eid);
                    Ok(Dynamic::UNIT)
                }
            }
        },
    );
}

fn register_get_local_player_id(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "get_local_player_id",
        move || -> Result<i64, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("get_local_player_id"))
                .map_err(ops_err)?;
            Ok(st.ecs_mirror.local_player_id.0 as i64)
        },
    );
}

fn register_get_frame_number(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "get_frame_number",
        move || -> Result<i64, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("get_frame_number"))
                .map_err(ops_err)?;
            Ok(st.ecs_mirror.frame_number as i64)
        },
    );
}

fn register_get_delta_time(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "get_delta_time",
        move || -> Result<f64, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("get_delta_time"))
                .map_err(ops_err)?;
            Ok(st.ecs_mirror.delta_time.to_f64())
        },
    );
}

// ── §3.6 Network ──

fn register_send_prediction(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "send_prediction",
        move |input_type: i64, data: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("send_prediction"))
                .map_err(ops_err)?;
            let dv = to_dynamic_value(&data).map_err(bridge_err)?;
            st.event_queue.push(BridgeEvent::SendPrediction {
                input_type,
                data: dv,
            });
            Ok(())
        },
    );
}

fn register_request_state(engine: &mut rhai::Engine, state: SharedState<BridgeState>) {
    let s = state;
    engine.register_fn(
        "request_state",
        move |key: String| -> Result<Dynamic, Box<EvalAltResult>> {
            let mut st = s.borrow_mut();
            st.ops_tracker
                .deduct(OpsCostTable::cost_for("request_state"))
                .map_err(ops_err)?;
            // 推入事件（Phase 9 處理）+ 同步從 EcsMirror 查詢
            st.event_queue
                .push(BridgeEvent::RequestState { key: key.clone() });
            // 此處從 ecs_mirror 查不到特定 key，回傳 unit
            // Phase 9 會擴展 EcsMirror 支援 key-based query
            Ok(Dynamic::UNIT)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, EntityState, MirroredEntity};
    use std::collections::BTreeMap;

    // ── 測試輔助 ──

    fn test_ecs_mirror() -> EcsMirror {
        let mut entities = BTreeMap::new();
        let mut custom = BTreeMap::new();
        custom.insert("speed".to_string(), DeterministicValue::Int(10));

        entities.insert(
            EntityId(1),
            MirroredEntity {
                position: SoftVec3::new(
                    SoftF32::from_f64(1.0),
                    SoftF32::from_f64(2.0),
                    SoftF32::from_f64(3.0),
                ),
                rotation: SoftVec3::zero(),
                scale: SoftVec3::new(SoftF32::ONE, SoftF32::ONE, SoftF32::ONE),
                hp: 100,
                max_hp: 100,
                state: EntityState::IDLE,
                animation_id: None,
                custom,
            },
        );

        EcsMirror {
            entities,
            local_player_id: EntityId(1),
            frame_number: 42,
            delta_time: SoftF32::from_f64(1.0 / 60.0),
        }
    }

    fn make_state() -> SharedState<BridgeState> {
        Rc::new(RefCell::new(BridgeState::new()))
    }

    fn make_state_with_mirror() -> SharedState<BridgeState> {
        let state = make_state();
        state.borrow_mut().ecs_mirror = test_ecs_mirror();
        state
    }

    fn make_engine_with_api(state: SharedState<BridgeState>) -> rhai::Engine {
        let mut engine = rhai::Engine::new();
        register_bridge_api(&mut engine, state);
        engine
    }

    // ═══════════════════════════════════════════════════════════════
    // EventQueue
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_event_queue_new_is_empty() {
        let eq = EventQueue::new();
        assert_eq!(eq.len(), 0);
        assert!(eq.is_empty());
    }

    #[test]
    fn test_event_queue_push_and_len() {
        let mut eq = EventQueue::new();
        for i in 0..3 {
            eq.push(BridgeEvent::ShowDialog { dialog_id: i });
        }
        assert_eq!(eq.len(), 3);
        assert!(!eq.is_empty());
    }

    #[test]
    fn test_event_queue_drain_returns_all_fifo() {
        let mut eq = EventQueue::new();
        eq.push(BridgeEvent::ShowDialog { dialog_id: 1 });
        eq.push(BridgeEvent::ShowDialog { dialog_id: 2 });
        eq.push(BridgeEvent::ShowDialog { dialog_id: 3 });
        let drained = eq.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0], BridgeEvent::ShowDialog { dialog_id: 1 });
        assert_eq!(drained[2], BridgeEvent::ShowDialog { dialog_id: 3 });
        assert!(eq.is_empty());
    }

    #[test]
    fn test_event_queue_drain_empty() {
        let mut eq = EventQueue::new();
        let drained = eq.drain();
        assert!(drained.is_empty());
    }

    #[test]
    fn test_event_queue_reuse_after_drain() {
        let mut eq = EventQueue::new();
        eq.push(BridgeEvent::ShowDialog { dialog_id: 1 });
        let _ = eq.drain();
        eq.push(BridgeEvent::ShowDialog { dialog_id: 2 });
        let drained = eq.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0], BridgeEvent::ShowDialog { dialog_id: 2 });
    }

    // ═══════════════════════════════════════════════════════════════
    // EntityIdAllocator
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_entity_id_allocator_sequential() {
        let mut alloc = EntityIdAllocator::new();
        assert_eq!(alloc.next(), EntityId(1));
        assert_eq!(alloc.next(), EntityId(2));
        assert_eq!(alloc.next(), EntityId(3));
    }

    #[test]
    fn test_entity_id_allocator_never_zero() {
        let mut alloc = EntityIdAllocator::new();
        assert_eq!(alloc.next(), EntityId(1));
    }

    // ═══════════════════════════════════════════════════════════════
    // BridgeState
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_bridge_state_new_defaults() {
        let bs = BridgeState::new();
        assert!(bs.event_queue.is_empty());
        assert_eq!(bs.ops_tracker.remaining(), 50_000);
    }

    // ═══════════════════════════════════════════════════════════════
    // EcsMirror fixture
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_ecs_mirror_fixture() {
        let mirror = test_ecs_mirror();
        assert_eq!(mirror.frame_number, 42);
        let e = mirror.entities.get(&EntityId(1)).unwrap();
        assert_eq!(e.hp, 100);
        assert_eq!(e.animation_id, None);
        assert_eq!(e.state, EntityState::IDLE);
    }

    // ═══════════════════════════════════════════════════════════════
    // Bridge API 透過 Rhai 呼叫測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_spawn_entity_produces_event() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result: i64 = engine.eval("spawn_entity(100)").unwrap();
        assert_eq!(result, 1); // EntityId(1)
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], BridgeEvent::SpawnEntity { type_id: 100 });
    }

    #[test]
    fn test_set_transform_produces_event() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("set_transform(1, 1.0, 2.0, 3.0)")
            .unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            BridgeEvent::SetTransform {
                eid: EntityId(1),
                pos: to_soft_vec3(1.0, 2.0, 3.0),
            }
        );
    }

    #[test]
    fn test_play_vfx_produces_event() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let handle: i64 = engine.eval("play_vfx(10, 0.0, 1.0, 0.0)").unwrap();
        assert!(handle >= 0);
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BridgeEvent::PlayVfx { vfx_id: 10, .. }));
    }

    #[test]
    fn test_play_sound_produces_event() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let handle: i64 = engine.eval("play_sound(5)").unwrap();
        assert!(handle >= 0);
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], BridgeEvent::PlaySound { sound_id: 5 });
    }

    #[test]
    fn test_get_position_reads_mirror() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: rhai::Array = engine.eval("get_position(1)").unwrap();
        assert_eq!(result.len(), 3);
        let x = result[0].clone_cast::<f64>();
        let y = result[1].clone_cast::<f64>();
        let z = result[2].clone_cast::<f64>();
        assert!((x - 1.0).abs() < 0.01);
        assert!((y - 2.0).abs() < 0.01);
        assert!((z - 3.0).abs() < 0.01);
        // Query 不產生 event
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_get_frame_number() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: i64 = engine.eval("get_frame_number()").unwrap();
        assert_eq!(result, 42);
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_ops_deducted_on_bridge_call() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<i64>("spawn_entity(1)").unwrap();
        let remaining = state.borrow().ops_tracker.remaining();
        let expected_cost = OpsCostTable::cost_for("spawn_entity");
        assert_eq!(remaining, 50_000 - expected_cost);
    }

    #[test]
    fn test_ops_exceeded_interrupts_script() {
        let state = Rc::new(RefCell::new(BridgeState {
            event_queue: EventQueue::new(),
            camera_op_queue: CameraOpQueue::new(),
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 0,
                delta_time: SoftF32::ZERO,
            },
            ops_tracker: OpsTracker::with_limit(40),
            handle_registry: HandleRegistry::new(),
            entity_id_allocator: EntityIdAllocator::new(),
        }));
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<i64>("spawn_entity(1)");
        assert!(result.is_err());
        // 超限時不產生 BridgeEvent
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_ops_multiple_calls_accumulate() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<i64>("spawn_entity(1)").unwrap();
        engine
            .eval::<()>("set_transform(1, 0.0, 0.0, 0.0)")
            .unwrap();
        let used = 50_000 - state.borrow().ops_tracker.remaining();
        let expected =
            OpsCostTable::cost_for("spawn_entity") + OpsCostTable::cost_for("set_transform");
        assert_eq!(used, expected);
    }

    #[test]
    fn test_register_bridge_api_skeleton_exists() {
        let state = make_state();
        let _engine = make_engine_with_api(state);
        // 不 panic 即成功
    }

    #[test]
    #[should_panic]
    fn test_entity_id_allocator_overflow() {
        let mut alloc = EntityIdAllocator { counter: u64::MAX };
        alloc.next(); // overflow panic in debug mode
    }

    // ═══════════════════════════════════════════════════════════════
    // Task 06 額外測試
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_despawn_entity_produces_event() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: bool = engine.eval("despawn_entity(1)").unwrap();
        assert!(result);
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events[0], BridgeEvent::DespawnEntity { eid: EntityId(1) });
    }

    #[test]
    fn test_set_rotation_softf32_conversion() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let pi_str = std::f64::consts::PI.to_string();
        engine
            .eval::<()>(&format!("set_rotation(1, 1.5, 0.0, {pi_str})"))
            .unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::SetRotation {
                eid: EntityId(1),
                euler: to_soft_vec3(1.5, 0.0, std::f64::consts::PI),
            }
        );
    }

    #[test]
    fn test_play_sound_at_produces_event() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let handle: i64 = engine.eval("play_sound_at(5, 1.0, 2.0, 3.0)").unwrap();
        assert!(handle >= 0);
        let events = state.borrow_mut().event_queue.drain();
        assert!(matches!(
            events[0],
            BridgeEvent::PlaySoundAt { sound_id: 5, .. }
        ));
    }

    #[test]
    fn test_stop_vfx_invalid_handle_silent() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("stop_vfx(9999)").unwrap();
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_update_hud_int_value() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>(r#"update_hud("score", 42)"#).unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::UpdateHud {
                key: "score".to_string(),
                value: DynamicValue::Int(42),
            }
        );
    }

    #[test]
    fn test_update_hud_unsupported_type() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>(r#"update_hud("data", [1,2,3])"#);
        assert!(result.is_err());
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_show_damage_number_world_coords() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("show_damage_number(10.5, 20.3, 999)")
            .unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::ShowDamageNumber {
                x: 10.5,
                y: 20.3,
                value: 999,
            }
        );
    }

    #[test]
    fn test_blend_animation_weight_f64() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("blend_animation(1, 2, 3, 0.75)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::BlendAnimation {
                eid: EntityId(1),
                anim_a: 2,
                anim_b: 3,
                weight: 0.75,
            }
        );
    }

    #[test]
    fn test_get_position_returns_array() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: rhai::Array = engine.eval("get_position(1)").unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_get_entity_state_predefined_hp() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: i64 = engine.eval(r#"get_entity_state(1, "hp")"#).unwrap();
        assert_eq!(result, 100);
    }

    #[test]
    fn test_get_entity_state_custom_key() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: i64 = engine.eval(r#"get_entity_state(1, "speed")"#).unwrap();
        assert_eq!(result, 10);
    }

    #[test]
    fn test_get_entity_state_not_found() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: Dynamic = engine.eval(r#"get_entity_state(999, "hp")"#).unwrap();
        assert!(result.is_unit());
    }

    #[test]
    fn test_get_delta_time_native_f64() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result: f64 = engine.eval("get_delta_time()").unwrap();
        assert!((result - 1.0 / 60.0).abs() < 0.001);
    }

    #[test]
    fn test_send_prediction_dynamic_value() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("send_prediction(1, 42)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::SendPrediction {
                input_type: 1,
                data: DynamicValue::Int(42),
            }
        );
    }

    #[test]
    fn test_send_prediction_invalid_type() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("send_prediction(1, [1,2,3])");
        assert!(result.is_err());
    }

    #[test]
    fn test_query_no_event_produced() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let _ = engine.eval::<Dynamic>("get_position(1)").unwrap();
        let _ = engine
            .eval::<Dynamic>(r#"get_entity_state(1, "hp")"#)
            .unwrap();
        engine.eval::<i64>("get_local_player_id()").unwrap();
        engine.eval::<i64>("get_frame_number()").unwrap();
        engine.eval::<f64>("get_delta_time()").unwrap();
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_multiple_spawn_entity_unique_ids() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let id1: i64 = engine.eval("spawn_entity(1)").unwrap();
        let id2: i64 = engine.eval("spawn_entity(2)").unwrap();
        let id3: i64 = engine.eval("spawn_entity(3)").unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_ops_error_message_format() {
        let state = Rc::new(RefCell::new(BridgeState {
            event_queue: EventQueue::new(),
            camera_op_queue: CameraOpQueue::new(),
            ecs_mirror: EcsMirror {
                entities: BTreeMap::new(),
                local_player_id: EntityId(0),
                frame_number: 0,
                delta_time: SoftF32::ZERO,
            },
            ops_tracker: OpsTracker::with_limit(10),
            handle_registry: HandleRegistry::new(),
            entity_id_allocator: EntityIdAllocator::new(),
        }));
        let engine = make_engine_with_api(state);
        let err = engine.eval::<i64>("spawn_entity(1)").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ops budget exceeded"));
    }

    // ═══════════════════════════════════════════════════════════════
    // Task 07+08: 參數驗證測試
    // ═══════════════════════════════════════════════════════════════

    // ── 一、Reject 策略（InvalidParameter error，不 push event） ──

    #[test]
    fn test_set_transform_nan_coordinate() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_transform(1, 0.0/0.0, 0.0, 0.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_set_transform_inf_coordinate() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_transform(1, 1.0/0.0, 0.0, 0.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_set_transform_coordinate_too_large() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_transform(1, 1000001.0, 0.0, 0.0)");
        assert!(result.is_err());
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_set_transform_coordinate_too_small() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_transform(1, -1000001.0, 0.0, 0.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_set_rotation_nan_coordinate() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_rotation(1, 0.0/0.0, 0.0, 0.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_set_scale_negative() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_scale(1, -1.0, 1.0, 1.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_set_scale_zero() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_scale(1, 0.0, 1.0, 1.0)");
        assert!(result.is_err());
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_set_scale_too_large() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_scale(1, 1001.0, 1.0, 1.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_show_dialog_negative_id() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("show_dialog(-1)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_play_animation_negative_id() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("play_animation(1, -1)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_play_vfx_zero_id() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<i64>("play_vfx(0, 0.0, 0.0, 0.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_play_sound_zero_id() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<i64>("play_sound(0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_play_sound_at_zero_id() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<i64>("play_sound_at(0, 0.0, 0.0, 0.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_show_toast_zero_duration() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>(r#"show_toast("hi", 0)"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_show_toast_negative_duration() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>(r#"show_toast("hi", -1)"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_health_bar_current_exceeds_max() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_health_bar(1, 200.0, 100.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_health_bar_max_zero() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_health_bar(1, 0.0, 0.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_health_bar_max_negative() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_health_bar(1, 0.0, -10.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_health_bar_max_nan() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_health_bar(1, 50.0, 0.0/0.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_health_bar_current_nan() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_health_bar(1, 0.0/0.0, 100.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_health_bar_current_negative() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("set_health_bar(1, -1.0, 100.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_play_vfx_nan_coordinate() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<i64>("play_vfx(1, 0.0/0.0, 0.0, 0.0)");
        assert!(result.is_err());
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_play_vfx_coordinate_too_large() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<i64>("play_vfx(1, 1000001.0, 0.0, 0.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_play_sound_at_inf_coordinate() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<i64>("play_sound_at(1, 1.0/0.0, 0.0, 0.0)");
        assert!(result.is_err());
    }

    #[test]
    fn test_show_damage_number_nan_coordinate() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("show_damage_number(0.0/0.0, 0.0, 100)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_show_damage_number_coordinate_too_large() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("show_damage_number(1000001.0, 0.0, 100)");
        assert!(result.is_err());
    }

    #[test]
    fn test_blend_animation_weight_nan() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("blend_animation(1, 0, 1, 0.0/0.0)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("InvalidParameter"));
    }

    #[test]
    fn test_blend_animation_weight_inf() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let result = engine.eval::<()>("blend_animation(1, 0, 1, 1.0/0.0)");
        assert!(result.is_err());
    }

    // ── 二、Clamp 策略（正常 push event，weight 已夾限） ──

    #[test]
    fn test_blend_animation_weight_clamp_above() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("blend_animation(1, 0, 1, 1.5)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
        match &events[0] {
            BridgeEvent::BlendAnimation { weight, .. } => assert_eq!(*weight, 1.0),
            _ => panic!("預期 BlendAnimation 事件"),
        }
    }

    #[test]
    fn test_blend_animation_weight_clamp_below() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("blend_animation(1, 0, 1, -0.1)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::BlendAnimation { weight, .. } => assert_eq!(*weight, 0.0),
            _ => panic!("預期 BlendAnimation 事件"),
        }
    }

    #[test]
    fn test_blend_animation_weight_clamp_slight_overshoot() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("blend_animation(1, 0, 1, 1.0000001)")
            .unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::BlendAnimation { weight, .. } => assert_eq!(*weight, 1.0),
            _ => panic!("預期 BlendAnimation 事件"),
        }
    }

    // ── 三、Truncate 策略（正常 push event，字串已截斷） ──

    #[test]
    fn test_update_hud_key_truncate() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        // 產生 5000 bytes 的 key
        let script = r#"let k = ""; for i in 0..5000 { k += "x"; } update_hud(k, 42)"#;
        engine.eval::<()>(script).unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
        match &events[0] {
            BridgeEvent::UpdateHud { key, .. } => assert!(key.len() <= 4096),
            _ => panic!("預期 UpdateHud 事件"),
        }
    }

    #[test]
    fn test_show_toast_msg_truncate() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let script = r#"let m = ""; for i in 0..5000 { m += "x"; } show_toast(m, 100)"#;
        engine.eval::<()>(script).unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
        match &events[0] {
            BridgeEvent::ShowToast { msg, .. } => assert!(msg.len() <= 4096),
            _ => panic!("預期 ShowToast 事件"),
        }
    }

    #[test]
    fn test_update_hud_string_at_limit() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        // 恰好 4096 bytes → 不截斷
        let script = r#"let k = ""; for i in 0..4096 { k += "x"; } update_hud(k, 42)"#;
        engine.eval::<()>(script).unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::UpdateHud { key, .. } => assert_eq!(key.len(), 4096),
            _ => panic!("預期 UpdateHud 事件"),
        }
    }

    #[test]
    fn test_show_toast_multibyte_truncate() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        // "你" = 3 bytes，2000 個 = 6000 bytes → 截斷至 floor_char_boundary(4096) = 4095
        let script = r#"let m = ""; for i in 0..2000 { m += "你"; } show_toast(m, 100)"#;
        engine.eval::<()>(script).unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::ShowToast { msg, .. } => {
                assert!(msg.len() <= 4096);
                // UTF-8 安全：截斷後應為有效 UTF-8
                assert!(std::str::from_utf8(msg.as_bytes()).is_ok());
            }
            _ => panic!("預期 ShowToast 事件"),
        }
    }

    // ── 四、有效參數測試 ──

    #[test]
    fn test_set_transform_valid_params() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("set_transform(1, 100.0, -200.0, 300.0)")
            .unwrap();
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_set_transform_boundary_max() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("set_transform(1, 1000000.0, -1000000.0, 0.0)")
            .unwrap();
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_set_scale_valid_min() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("set_scale(1, 0.001, 1.0, 1.0)").unwrap();
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_set_scale_valid_max() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("set_scale(1, 1000.0, 1000.0, 1000.0)")
            .unwrap();
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_blend_animation_boundary_zero() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("blend_animation(1, 0, 1, 0.0)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::BlendAnimation { weight, .. } => assert_eq!(*weight, 0.0),
            _ => panic!("預期 BlendAnimation 事件"),
        }
    }

    #[test]
    fn test_blend_animation_boundary_one() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("blend_animation(1, 0, 1, 1.0)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::BlendAnimation { weight, .. } => assert_eq!(*weight, 1.0),
            _ => panic!("預期 BlendAnimation 事件"),
        }
    }

    #[test]
    fn test_show_dialog_zero_id() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("show_dialog(0)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events[0], BridgeEvent::ShowDialog { dialog_id: 0 });
    }

    #[test]
    fn test_play_vfx_valid_params() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let handle: i64 = engine.eval("play_vfx(1, 100.0, -200.0, 300.0)").unwrap();
        assert!(handle >= 0);
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_play_sound_valid() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        let handle: i64 = engine.eval("play_sound(1)").unwrap();
        assert!(handle >= 0);
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_play_sound_at_valid() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<i64>("play_sound_at(1, 0.0, 0.0, 0.0)")
            .unwrap();
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_show_damage_number_valid() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("show_damage_number(10.5, 20.3, 999)")
            .unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::ShowDamageNumber {
                x: 10.5,
                y: 20.3,
                value: 999,
            }
        );
    }

    #[test]
    fn test_show_toast_valid() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>(r#"show_toast("hello", 1000)"#).unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::ShowToast {
                msg: "hello".to_string(),
                duration_ms: 1000,
            }
        );
    }

    #[test]
    fn test_health_bar_valid() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("set_health_bar(1, 50.0, 100.0)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::SetHealthBar {
                eid: EntityId(1),
                current: 50.0,
                max: 100.0,
            }
        );
    }

    #[test]
    fn test_health_bar_zero_current() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("set_health_bar(1, 0.0, 100.0)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_health_bar_full() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("set_health_bar(1, 100.0, 100.0)")
            .unwrap();
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    #[test]
    fn test_play_animation_zero_anim_id() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("play_animation(1, 0)").unwrap();
        let events = state.borrow_mut().event_queue.drain();
        assert_eq!(
            events[0],
            BridgeEvent::PlayAnimation {
                eid: EntityId(1),
                anim_id: 0,
            }
        );
    }

    #[test]
    fn test_set_transform_subnormal_coordinate() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        // subnormal 極小值，通過 is_finite() 且在 [-1e6, 1e6] 內
        engine
            .eval::<()>("set_transform(1, 5e-324, 0.0, 0.0)")
            .unwrap();
        assert_eq!(state.borrow_mut().event_queue.drain().len(), 1);
    }

    // ── 四b、不變式驗證 ──

    #[test]
    fn test_play_vfx_pos_stored_as_soft_vec3() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<i64>("play_vfx(1, 100.5, -200.3, 0.0)")
            .unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::PlayVfx { pos, .. } => {
                assert_eq!(pos.x, SoftF32::from_f64(100.5));
                assert_eq!(pos.y, SoftF32::from_f64(-200.3));
                assert_eq!(pos.z, SoftF32::from_f64(0.0));
            }
            _ => panic!("預期 PlayVfx 事件"),
        }
    }

    #[test]
    fn test_play_sound_at_pos_stored_as_soft_vec3() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<i64>("play_sound_at(1, 50.0, 60.0, 70.0)")
            .unwrap();
        let events = state.borrow_mut().event_queue.drain();
        match &events[0] {
            BridgeEvent::PlaySoundAt { pos, .. } => {
                assert_eq!(pos.x, SoftF32::from_f64(50.0));
                assert_eq!(pos.y, SoftF32::from_f64(60.0));
                assert_eq!(pos.z, SoftF32::from_f64(70.0));
            }
            _ => panic!("預期 PlaySoundAt 事件"),
        }
    }

    #[test]
    fn test_set_transform_nan_no_event_pushed() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let _ = engine.eval::<()>("set_transform(1, 0.0/0.0, 0.0, 0.0)");
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_set_scale_zero_no_event_pushed() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        let _ = engine.eval::<()>("set_scale(1, 0.0, 1.0, 1.0)");
        assert!(state.borrow().event_queue.is_empty());
    }

    // ── 五、EntityNotFound / InvalidHandle 特殊處理 ──

    #[test]
    fn test_set_transform_entity_not_found() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        // entity 999 不存在 → Ok(()), 不 push event
        engine
            .eval::<()>("set_transform(999, 0.0, 0.0, 0.0)")
            .unwrap();
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_play_animation_entity_not_found() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("play_animation(999, 1)").unwrap();
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_blend_animation_entity_not_found() {
        let state = make_state_with_mirror();
        let engine = make_engine_with_api(state.clone());
        engine
            .eval::<()>("blend_animation(999, 0, 1, 0.5)")
            .unwrap();
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_stop_vfx_invalid_handle_no_event() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("stop_vfx(99999)").unwrap();
        assert!(state.borrow().event_queue.is_empty());
    }

    #[test]
    fn test_stop_sound_invalid_handle_no_event() {
        let state = make_state();
        let engine = make_engine_with_api(state.clone());
        engine.eval::<()>("stop_sound(99999)").unwrap();
        assert!(state.borrow().event_queue.is_empty());
    }
}
