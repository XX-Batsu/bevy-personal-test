//! Phase B 端對端整合測試：Rhai 腳本 → BridgeState queue → flush_camera_ops
//! → Phase A event → CameraOverrideStack / CameraShake apply。
//!
//! 測試邊界說明：
//! - 使用 plain `rhai::Engine::new()`，不啟用 SandboxedEngine 沙箱機制。
//!   SandboxedEngine 整合驗證留給 vm_runtime crate 既有測試。
//! - 用 temp_state + 手動 transfer ops 模擬 Rhai → Bevy bridge：
//!   production 端是同一 BridgeState 實例由 `Rc<RefCell>` 與 `Arc<Mutex>` 雙包裝，
//!   那是 GamePlugin 工作（Phase B 範圍之外）。
//!
//! Test 11 決策說明（flush_order_regression_bridge_event_and_camera_op_coexist）：
//! flush_bridge_events 系統需要 BridgeEventQueue、BridgeEntityMap、BridgeDiagnostics、
//! ClockResource 等資源，BridgePlugin 也需要這些資源。為避免過重設置，
//! Test 11 採取選項 (b)：只驗證 camera_op_queue drain（flush_camera_ops 正確執行），
//! 並在測試說明中記錄此範圍邊界決策。BridgeEvent 路徑的 flush 驗證
//! 由 vm_bevy_bridge crate 既有測試覆蓋。

use bevy::prelude::*;
use bevy_runtime::camera::camera_registry::BridgeCamera;
use bevy_runtime::camera::{CameraOverrideStack, CameraPlugin, CameraRegistry, CameraShake};
use bridge_types::{BridgeEvent, CameraBridgeOp};
use std::sync::{Arc, Mutex};
use vm_bevy_bridge::SharedBridgeState;
use vm_runtime::{register_bridge_api, BridgeState};

/// 建立端對端測試用 App（含 SharedBridgeState、GameFixedSet configure、ManualDuration、CameraPlugin）。
fn build_e2e_app() -> (App, SharedBridgeState) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // S4: FixedUpdate 需 ManualDuration 才會穩定 fire
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f32(1.0 / 60.0),
    ));
    // GameFixedSet chain（一般由 GamePlugin 配置）
    vm_bevy_bridge::configure_game_fixed_set_for_tests(&mut app);
    // SharedBridgeState（CameraPlugin 的 flush_camera_ops 需要）
    let bs = SharedBridgeState(Arc::new(Mutex::new(BridgeState::new())));
    app.insert_resource(bs.clone());
    // CameraPlugin 註冊 Phase A events、Phase B resources / systems
    app.add_plugins(CameraPlugin);

    app.update(); // 初始化

    (app, bs)
}

/// Helper：執行 Rhai 腳本並把產出的 ops 移轉至 SharedBridgeState。
///
/// **已知限制（Phase B 範圍邊界）**：
/// - 使用 plain `rhai::Engine::new()`，不啟用 SandboxedEngine 沙箱機制
///   （50K ops / 2ms timeout / eval block）。SandboxedEngine 整合驗證留給
///   vm_runtime crate 既有測試。
/// - 用 temp_state + 手動 transfer ops 模擬 Rhai → Bevy bridge：
///   production 端是同一 BridgeState 實例由 `Rc<RefCell>` 與 `Arc<Mutex>` 雙包裝，
///   那是 GamePlugin 工作（Phase B 範圍之外）。
fn run_rhai(bs: &SharedBridgeState, script: &str) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let temp_state: Rc<RefCell<BridgeState>> = Rc::new(RefCell::new(BridgeState::new()));
    let mut engine = rhai::Engine::new();
    register_bridge_api(&mut engine, temp_state.clone());
    engine.run(script).unwrap();

    let ops = temp_state.borrow_mut().camera_op_queue.drain();
    let mut shared = bs.0.lock().unwrap();
    for op in ops {
        shared.camera_op_queue.push(op);
    }
}

// ══════════════════════════════════════════════════════════════════════════
// 基本契約（5 個測試）
// ══════════════════════════════════════════════════════════════════════════

/// 測試 1：Rhai shake handle=1 → 單台 camera 收到 CameraShakeRequest event，
/// event_handler 套用後 CameraShake 有 entry。
#[test]
fn rhai_shake_handle_one_single_camera_receives_event() {
    let (mut app, bs) = build_e2e_app();

    // 生成一個帶 BridgeCamera（自動分配 handle=1）與 CameraShake 的 camera
    let cam = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();

    // PreUpdate 跑 register_system
    app.update();

    // 確認已註冊
    let handle = {
        let r = app.world().resource::<CameraRegistry>();
        r.resolve(1)
    };
    assert_eq!(handle, Some(cam), "camera 應已以 handle=1 註冊");

    // 透過 Rhai 發出 shake
    run_rhai(&bs, "camera_shake(1, 0.8, 0.0, 0.0, 25.0, 2.0, 1.5, 0.3);");

    // 雙 app.update() 為防範 schedule 變動的安全網（Test 6 已證明單次足夠
    // 滿足當前 schedule；保留 redundancy 是 belt-and-suspenders）。
    app.update();
    app.update();

    let shake = app.world().get::<CameraShake>(cam).unwrap();
    assert_eq!(
        shake.len(),
        1,
        "每個 shake 應產生 1 個 entry，shake.len()={}",
        shake.len()
    );
}

/// 測試 2：Rhai shake handle=0（broadcast）→ 所有已註冊 cameras 都收到震動。
#[test]
fn rhai_shake_handle_zero_broadcasts_to_all_cameras() {
    let (mut app, bs) = build_e2e_app();

    let cam1 = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();
    let cam2 = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();

    app.update(); // 讓兩台 camera 都完成 register

    run_rhai(&bs, "camera_shake(0, 0.6, 0.0, 0.0, 25.0, 2.0, 1.5, 0.3);");

    app.update();
    app.update();

    let shake1 = app.world().get::<CameraShake>(cam1).unwrap();
    let shake2 = app.world().get::<CameraShake>(cam2).unwrap();
    assert_eq!(
        shake1.len(),
        1,
        "broadcast 每台 camera 應產生 1 個 entry，shake1.len()={}",
        shake1.len()
    );
    assert_eq!(
        shake2.len(),
        1,
        "broadcast 每台 camera 應產生 1 個 entry，shake2.len()={}",
        shake2.len()
    );
}

/// 測試 3：Rhai push_override handle=1 → camera 的 CameraOverrideStack 有正確 top。
#[test]
fn rhai_push_override_single_camera_applies() {
    let (mut app, bs) = build_e2e_app();

    let cam = app
        .world_mut()
        .spawn((
            CameraOverrideStack::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();

    app.update(); // register

    // camera_push_override(handle, x, y, zoom, speed, priority, duration)
    // zoom=-1.0 sentinel（不改 zoom）；duration=-1.0 sentinel（無期限）
    run_rhai(
        &bs,
        "camera_push_override(1, 1000.0, 0.0, -1.0, 5.0, 50, -1.0);",
    );

    app.update();
    app.update();

    let stack = app.world().get::<CameraOverrideStack>(cam).unwrap();
    assert!(
        !stack.is_empty(),
        "override stack 應有 entry（flush + event_handler 已執行）"
    );

    let top = stack.top().unwrap();
    assert_eq!(
        top.target_position,
        Vec2::new(1000.0, 0.0),
        "top override 目標位置應為 (1000, 0)，實際: {:?}",
        top.target_position
    );
    assert_eq!(
        top.priority, 50,
        "top override priority 應為 50，實際: {}",
        top.priority
    );
}

/// 測試 4：未註冊 handle → flush 層丟棄，不產生 CameraOverrideRequest event，不 panic。
#[test]
fn rhai_unregistered_handle_drops_no_panic() {
    use bevy_runtime::camera::events::CameraOverrideRequest;

    let (mut app, bs) = build_e2e_app();

    // 生成 handle=1 的 camera（不使用 handle=999）
    app.world_mut().spawn((
        CameraOverrideStack::default(),
        BridgeCamera::default(),
        Transform::default(),
    ));

    app.update(); // register

    // 對未註冊的 handle=999 發出 push_override
    run_rhai(
        &bs,
        "camera_push_override(999, 100.0, 0.0, -1.0, 5.0, 50, -1.0);",
    );

    // 不應 panic
    app.update();
    app.update();

    // flush 層應 drop 該 op，不產生 event
    let events = app
        .world_mut()
        .resource_mut::<Events<CameraOverrideRequest>>();
    let mut cursor = events.get_cursor();
    let evs: Vec<_> = cursor.read(&events).collect();
    assert!(
        evs.is_empty(),
        "未註冊 handle 應由 flush 層 drop，不產生 event，實際 event 數: {}",
        evs.len()
    );
}

/// 測試 5：camera despawn 後 handle 失效 → Rhai 發出的 camera_clear_shakes 被 flush 層丟棄。
#[test]
fn rhai_camera_despawn_invalidates_handle() {
    use bevy_runtime::camera::events::CameraShakeRequest;

    let (mut app, bs) = build_e2e_app();

    let cam = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();

    app.update(); // register（handle=1）

    // 驗證已註冊
    assert_eq!(
        app.world().resource::<CameraRegistry>().resolve(1),
        Some(cam),
        "handle=1 應已指向 cam"
    );

    // Despawn camera
    app.world_mut().entity_mut(cam).despawn();
    app.update(); // bridge_camera_unregister_system 在 PreUpdate 執行

    // 驗證已解除註冊
    assert_eq!(
        app.world().resource::<CameraRegistry>().resolve(1),
        None,
        "despawn 後 handle=1 應解除註冊"
    );

    // 發出針對（已消失的）handle=1 的操作
    run_rhai(&bs, "camera_clear_shakes(1);");

    app.update();

    // flush 層應 warn + drop，不產生 event
    let events = app.world_mut().resource_mut::<Events<CameraShakeRequest>>();
    let mut cursor = events.get_cursor();
    let evs: Vec<_> = cursor.read(&events).collect();
    assert!(
        evs.is_empty(),
        "despawn 後 handle 失效，flush 應 drop op，不產生 event，實際: {}",
        evs.len()
    );
}

// ══════════════════════════════════════════════════════════════════════════
// 補強驗證（6 個測試）
// ══════════════════════════════════════════════════════════════════════════

/// 測試 6：push op 直接至 SharedBridgeState 後，單次 app.update() 即可完成
/// FixedUpdate flush + Update event_handler（同 frame 傳播）。
#[test]
fn rhai_push_camera_receives_effect_in_single_update() {
    let (mut app, bs) = build_e2e_app();

    let cam = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();

    app.update(); // register

    // 直接 push op（繞過 Rhai，驗證同 frame 傳播）
    bs.0.lock()
        .unwrap()
        .camera_op_queue
        .push(CameraBridgeOp::Shake {
            handle: 1,
            trauma: 0.5,
            dir_x: 0.0,
            dir_y: 0.0,
            max_strength: 25.0,
            decay_rate: 2.0,
            direction_bias: 1.5,
            perpendicular_damping: 0.3,
        });

    // 單次 update：FixedUpdate（flush） + Update（event_handler）同 frame
    app.update();

    let shake = app.world().get::<CameraShake>(cam).unwrap();
    assert_eq!(
        shake.len(),
        1,
        "單次 app.update() 後每個 shake 應產生 1 個 entry，shake.len()={}",
        shake.len()
    );
}

/// 測試 7：多 camera 不同 handle — shake handle=2 只影響 cam_2，不影響 cam_1。
#[test]
fn rhai_shake_handle_two_only_affects_matching_camera() {
    let (mut app, bs) = build_e2e_app();

    // cam_1 自動分配 handle=1，cam_2 自動分配 handle=2
    let cam1 = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();
    let cam2 = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();

    app.update(); // register 兩台

    // 只對 handle=2 發 shake
    run_rhai(&bs, "camera_shake(2, 0.7, 0.0, 0.0, 25.0, 2.0, 1.5, 0.3);");

    app.update();
    app.update();

    let shake1 = app.world().get::<CameraShake>(cam1).unwrap();
    let shake2 = app.world().get::<CameraShake>(cam2).unwrap();

    assert_eq!(
        shake1.len(),
        0,
        "cam1（handle=1）不應受影響，shake1.len()={}",
        shake1.len()
    );
    assert_eq!(
        shake2.len(),
        1,
        "cam2（handle=2）應產生 1 個 shake entry，shake2.len()={}",
        shake2.len()
    );
}

/// 測試 8：NaN 參數 → Rhai 回傳 Err，queue 保持空。
///
/// 說明：camera_shake 先 deduct ops 再 reject NaN，deduct 成功但 push 不發生，
/// queue 仍空。Rhai engine.run() 回傳 Err（EvalAltResult::ErrorRuntime）。
#[test]
fn rhai_nan_parameter_reject_leaves_queue_empty() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let temp_state: Rc<RefCell<BridgeState>> = Rc::new(RefCell::new(BridgeState::new()));
    let mut engine = rhai::Engine::new();
    register_bridge_api(&mut engine, temp_state.clone());

    // NaN trauma（0.0 / 0.0 = NaN in Rhai）
    let result =
        engine.run("let x = 0.0 / 0.0; camera_shake(1, x, 0.0, 0.0, 25.0, 2.0, 1.5, 0.3);");
    assert!(result.is_err(), "NaN 參數應讓 engine.run 回傳 Err");

    // queue 應為空（NaN reject 阻止了 push）
    assert!(
        temp_state.borrow().camera_op_queue.is_empty(),
        "NaN reject 後 camera_op_queue 應為空"
    );

    // SharedBridgeState（未作任何 transfer）也應為空
    let (_app, bs) = build_e2e_app();
    // 未轉移任何 op，bs 的 queue 應為空
    assert!(
        bs.0.lock().unwrap().camera_op_queue.is_empty(),
        "SharedBridgeState queue 應為空（NaN reject 後無 transfer）"
    );
}

/// 測試 9：ops budget 耗盡時，第 6 次 camera_pop_override 應被 reject。
///
/// camera_pop_override cost = 3；OpsTracker::with_limit(15) → 15/3 = 5 次成功，第 6 次 Err。
#[test]
fn rhai_ops_cost_exhausted_rejects() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use vm_runtime::OpsTracker;

    let temp_state: Rc<RefCell<BridgeState>> = Rc::new(RefCell::new(BridgeState::new()));
    // 設定 15 ops 預算：camera_pop_override cost=3，5 次成功，第 6 次耗盡
    temp_state.borrow_mut().ops_tracker = OpsTracker::with_limit(15);

    let mut engine = rhai::Engine::new();
    register_bridge_api(&mut engine, temp_state.clone());

    // 前 5 次應成功
    for i in 1..=5 {
        engine
            .run("camera_pop_override(1);")
            .unwrap_or_else(|e| panic!("第 {i} 次應成功，實際 err: {e}"));
    }

    // 第 6 次應 reject（budget 耗盡）
    let err = engine.run("camera_pop_override(1);").unwrap_err();
    let err_str = format!("{err}");
    assert!(
        err_str.to_lowercase().contains("ops")
            || err_str.to_lowercase().contains("budget")
            || err_str.to_lowercase().contains("exceeded")
            || err_str.to_lowercase().contains("remaining"),
        "第 6 次應 reject（提示 ops/budget 耗盡），實際: {err_str}"
    );
}

/// 測試 10：BridgeCamera handle=Some(0) → graceful fallback（自動分配 handle=1）。
///
/// resolve(0) 永遠回 None（0 為 broadcast sentinel）；
/// resolve(1) 應指向該 entity。
#[test]
fn bridge_camera_handle_some_0_graceful_fallback() {
    let (mut app, _bs) = build_e2e_app();

    let cam = app
        .world_mut()
        .spawn((BridgeCamera { handle: Some(0) }, Transform::default()))
        .id();

    app.update(); // bridge_camera_register_system（PreUpdate）

    let registry = app.world().resource::<CameraRegistry>();

    // Some(0) 應 graceful fallback 至自動分配 handle=1
    assert_eq!(
        registry.resolve(1),
        Some(cam),
        "Some(0) 應 fallback 至自動分配 handle=1"
    );

    // resolve(0) 永遠回 None（broadcast sentinel 保留）
    assert_eq!(
        registry.resolve(0),
        None,
        "resolve(0) 應永遠回 None（broadcast sentinel）"
    );
}

/// 測試 11：flush 順序 regression — BridgeEvent 與 CameraBridgeOp 同幀並存。
///
/// 決策說明（選項 b）：
/// flush_bridge_events 需要 BridgeEventQueue / BridgeEntityMap / BridgeDiagnostics /
/// ClockResource 等資源，加入 BridgePlugin 會使測試設置過重，且 BridgeEvent flush
/// 驗證由 vm_bevy_bridge crate 既有測試覆蓋。
/// 本測試聚焦在：BridgeEvent 入隊後 **CameraBridgeOp queue 仍正確 drain**（無排程衝突）；
/// BridgeEvent queue 的 flush 驗證留給 vm_bevy_bridge::flush 相關測試。
#[test]
fn flush_order_regression_bridge_event_and_camera_op_coexist() {
    let (mut app, bs) = build_e2e_app();

    let cam = app
        .world_mut()
        .spawn((
            CameraShake::default(),
            BridgeCamera::default(),
            Transform::default(),
        ))
        .id();

    app.update(); // register

    {
        let mut state = bs.0.lock().unwrap();
        // BridgeEvent 路徑（本測試環境中 flush_bridge_events 不運行，
        // 因為缺少 BridgeEventQueue 等資源；此 push 用於驗證不影響 camera_op 路徑）
        // 用 ShowDialog（plan 寫 LogScript，但該 variant 不存在於 BridgeEvent；
        // 任意 simple variant 皆可，重點是檢驗兩 queue 並存）
        state
            .event_queue
            .push(BridgeEvent::ShowDialog { dialog_id: 0 });
        // CameraBridgeOp 路徑（flush_camera_ops 在 FixedUpdate.FlushBridgeEvents 運行）
        state.camera_op_queue.push(CameraBridgeOp::Shake {
            handle: 1,
            trauma: 0.5,
            dir_x: 0.0,
            dir_y: 0.0,
            max_strength: 100.0,
            decay_rate: 0.0,
            direction_bias: 1.5,
            perpendicular_damping: 0.3,
        });
    }

    app.update();

    // camera_op_queue 應已被 flush_camera_ops drain
    let state = bs.0.lock().unwrap();
    assert!(
        state.camera_op_queue.is_empty(),
        "CameraBridgeOp queue 應已 drain（flush_camera_ops 正確執行）"
    );
    // option-(b) 明確化：event_queue 在本測試環境下不會被 drain（缺
    // BridgePlugin 提供的 BridgeEntityMap / BridgeDiagnostics / ClockResource，
    // flush_bridge_events 不會 run）。BridgeEvent flush 路徑由 vm_bevy_bridge
    // crate 既有測試覆蓋。
    assert!(
        !state.event_queue.is_empty(),
        "BridgeEvent queue 在本測試環境中保留（缺 BridgePlugin 必要 resources）"
    );
    drop(state);

    // CameraShake 應收到震動（camera_op 路徑正確執行）
    let shake = app.world().get::<CameraShake>(cam).unwrap();
    assert_eq!(
        shake.len(),
        1,
        "BridgeEvent 存在不影響 camera_op flush，shake 應產生 1 個 entry，shake.len()={}",
        shake.len()
    );
}
