//! Rapier 物理引擎整合 — PhysicsPlugin、碰撞事件、SoftVec3 邊界轉換。
//!
//! # 確定性架構例外
//!
//! Rapier 內部使用 native f32。WASM IEEE 754 保證 same-platform same-binary 位元級確定性。
//! SoftF32 ↔ native f32 僅在 Rapier 邊界轉換，遊戲邏輯仍使用 SoftF32。
//!
//! # 碰撞事件
//!
//! [`CollisionEvents`] 使用 `BTreeMap` 確保確定性迭代順序（README.md Determinism Rules）。
//! [`update_collision_events`] 排程在 `RunScripts` 之後、`FlushBridgeEvents` 之前。

use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use deterministic::{SoftF32, SoftVec3};
use std::collections::BTreeMap;
use vm_bevy_bridge::GameFixedSet;

// ── SoftVec3 ↔ Rapier Vec3 邊界轉換 ─────────────────────────────────────

/// 將確定性 SoftVec3 轉換為 Rapier 使用的 native Vec3。
///
/// 僅在 Rapier 邊界使用，遊戲邏輯禁止使用 native float。
pub fn soft_to_rapier_vec(v: SoftVec3) -> Vec3 {
    Vec3::new(v.x.to_native(), v.y.to_native(), v.z.to_native())
}

/// 將 Rapier native Vec3 轉換回確定性 SoftVec3。
///
/// 從 Rapier 讀取結果後立即轉回 SoftF32，保留 bit pattern。
pub fn rapier_to_soft_vec(v: Vec3) -> SoftVec3 {
    SoftVec3::new(
        SoftF32::from_f32(v.x),
        SoftF32::from_f32(v.y),
        SoftF32::from_f32(v.z),
    )
}

// ── 碰撞狀態 ─────────────────────────────────────────────────────────────

/// 碰撞狀態：追蹤兩個 Entity 之間的碰撞變化。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollisionState {
    /// 碰撞開始（本幀新發生）
    pub started: bool,
    /// 碰撞結束（本幀新分離）
    pub stopped: bool,
}

/// 碰撞事件集合（Bevy Resource）。
///
/// 使用 BTreeMap 確保確定性迭代順序（README.md Determinism Rules）。
/// Key 為 `(u32, u32)`，Entity::index() 較小的在前（[`ordered_entity_pair`]）。
#[derive(Resource, Default, Debug)]
pub struct CollisionEvents {
    pub events: BTreeMap<(u32, u32), CollisionState>,
}

/// 將兩個 Entity 的 index 排序為 (較小, 較大) 的確定性 pair。
fn ordered_entity_pair(a: Entity, b: Entity) -> (u32, u32) {
    let a_idx = a.index();
    let b_idx = b.index();
    if a_idx <= b_idx {
        (a_idx, b_idx)
    } else {
        (b_idx, a_idx)
    }
}

// ── PhysicsPlugin ────────────────────────────────────────────────────────

/// Rapier 3D 物理引擎 Plugin。
///
/// - 加入 `RapierPhysicsPlugin::<NoUserData>::default()`
/// - Startup 系統設定 `TimestepMode::Fixed { dt: 1.0/60.0, substeps: 1 }`
/// - 初始化 [`CollisionEvents`] Resource
/// - 排程 [`update_collision_events`] 在 `RunScripts` 之後、`FlushBridgeEvents` 之前
pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        // 1. 加入 Rapier 物理 Plugin（無自訂 user data）
        app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default());

        // 2. 設定固定時間步進模式
        app.insert_resource(TimestepMode::Fixed {
            dt: 1.0 / 60.0,
            substeps: 1,
        });

        // 3. 初始化碰撞事件 Resource
        app.init_resource::<CollisionEvents>();

        // 4. Startup 系統：設定 Rapier 組態
        app.add_systems(Startup, configure_rapier);

        // 5. 排程 update_collision_events：RunScripts 之後、FlushBridgeEvents 之前
        app.add_systems(
            FixedUpdate,
            update_collision_events
                .after(GameFixedSet::RunScripts)
                .before(GameFixedSet::FlushBridgeEvents),
        );
    }
}

/// Startup 系統：設定 Rapier 物理引擎組態。
///
/// - 設定重力為 (0, -9.81, 0)
/// - 啟用物理模擬管線
fn configure_rapier(mut rapier_config: Query<&mut RapierConfiguration>) {
    for mut config in rapier_config.iter_mut() {
        config.gravity = Vec3::new(0.0, -9.81, 0.0);
        config.physics_pipeline_active = true;
        config.query_pipeline_active = true;
        tracing::info!("Rapier 物理引擎組態完成：gravity={:?}", config.gravity);
    }
}

/// FixedUpdate 系統：讀取 Rapier 碰撞事件並更新 [`CollisionEvents`]。
///
/// 每幀清空並重建碰撞事件 BTreeMap。
/// 排程在 `RunScripts` 之後、`FlushBridgeEvents` 之前。
fn update_collision_events(
    mut collision_events: ResMut<CollisionEvents>,
    mut rapier_events: EventReader<CollisionEvent>,
) {
    // 每幀重置
    collision_events.events.clear();

    for event in rapier_events.read() {
        match event {
            CollisionEvent::Started(e1, e2, _flags) => {
                let pair = ordered_entity_pair(*e1, *e2);
                collision_events.events.entry(pair).or_default().started = true;
                tracing::debug!(entity_a = pair.0, entity_b = pair.1, "碰撞開始");
            }
            CollisionEvent::Stopped(e1, e2, _flags) => {
                let pair = ordered_entity_pair(*e1, *e2);
                collision_events.events.entry(pair).or_default().stopped = true;
                tracing::debug!(entity_a = pair.0, entity_b = pair.1, "碰撞結束");
            }
        }
    }
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// 建構物理測試 App：MinimalPlugins + TransformPlugin + PhysicsPlugin。
    fn build_physics_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_plugins(PhysicsPlugin);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            17,
        )));
        app
    }

    // ── 轉換測試（5 個）──────────────────────────────────────────────────

    /// 測試 SoftVec3 → Vec3 → SoftVec3 來回轉換（bit-exact）。
    #[test]
    fn test_soft_to_rapier_vec_roundtrip() {
        let soft = SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(2.0),
            SoftF32::from_f32(3.0),
        );
        let vec3 = soft_to_rapier_vec(soft);
        let back = rapier_to_soft_vec(vec3);
        assert_eq!(soft, back, "精確表示的值來回轉換必須 bit-exact");
    }

    /// 測試 Vec3 → SoftVec3 → Vec3 來回轉換（bit-exact）。
    #[test]
    fn test_rapier_to_soft_vec_roundtrip() {
        let vec3 = Vec3::new(4.0, 5.0, 6.0);
        let soft = rapier_to_soft_vec(vec3);
        let back = soft_to_rapier_vec(soft);
        assert_eq!(vec3, back, "精確表示的值來回轉換必須 bit-exact");
    }

    /// 測試 NaN 邊界值的轉換保留 NaN 性質。
    #[test]
    fn test_soft_to_rapier_vec_boundary_nan() {
        let nan = SoftF32::from_f32(f32::NAN);
        let soft = SoftVec3::new(nan, SoftF32::from_f32(0.0), SoftF32::from_f32(0.0));
        let vec3 = soft_to_rapier_vec(soft);
        assert!(vec3.x.is_nan(), "NaN 轉換後仍應為 NaN");
        let back = rapier_to_soft_vec(vec3);
        assert!(back.x.is_nan(), "NaN 來回轉換後仍應為 NaN");
    }

    /// 測試 Infinity 邊界值的轉換保留 Inf 性質。
    #[test]
    fn test_soft_to_rapier_vec_boundary_inf() {
        let pos_inf = SoftF32::from_f32(f32::INFINITY);
        let neg_inf = SoftF32::from_f32(f32::NEG_INFINITY);
        let soft = SoftVec3::new(pos_inf, neg_inf, SoftF32::from_f32(0.0));
        let vec3 = soft_to_rapier_vec(soft);
        assert!(
            vec3.x.is_infinite() && vec3.x > 0.0,
            "+Inf 轉換後仍應為 +Inf"
        );
        assert!(
            vec3.y.is_infinite() && vec3.y < 0.0,
            "-Inf 轉換後仍應為 -Inf"
        );
        let back = rapier_to_soft_vec(vec3);
        assert!(back.x.is_inf(), "+Inf 來回轉換後仍應為 Inf");
        assert!(back.y.is_inf(), "-Inf 來回轉換後仍應為 Inf");
    }

    /// 測試 denormal（次正規化數）邊界值的轉換（bit-exact）。
    #[test]
    fn test_rapier_to_soft_vec_boundary_denormal() {
        // 最小的正 denormal number：0x0000_0001
        let denormal = f32::from_bits(0x0000_0001);
        let vec3 = Vec3::new(denormal, -denormal, 0.0);
        let soft = rapier_to_soft_vec(vec3);
        let back = soft_to_rapier_vec(soft);
        assert_eq!(
            vec3.x.to_bits(),
            back.x.to_bits(),
            "denormal 正值 bit-exact"
        );
        assert_eq!(
            vec3.y.to_bits(),
            back.y.to_bits(),
            "denormal 負值 bit-exact"
        );
    }

    // ── 確定性測試（2 個）────────────────────────────────────────────────

    /// 測試相同初始狀態的 Rapier 模擬產生相同輸出（10 步）。
    #[test]
    fn test_rapier_deterministic_same_state_same_output() {
        /// 建立場景並模擬 N 步，回傳最終位置的 SoftVec3。
        fn simulate(steps: usize) -> SoftVec3 {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins);
            app.add_plugins(bevy::transform::TransformPlugin);
            app.add_plugins(PhysicsPlugin);
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                17,
            )));

            // Frame 0: 初始化（含 Rapier Startup）
            app.update();

            // Spawn 動態剛體 + 球形碰撞體 + 初速度
            let entity = app
                .world_mut()
                .spawn((
                    Transform::from_xyz(0.0, 10.0, 0.0),
                    RigidBody::Dynamic,
                    Collider::ball(0.5),
                    Velocity::linear(Vec3::new(1.0, 0.0, 0.0)),
                ))
                .id();

            // 執行 N 步
            for _ in 0..steps {
                app.update();
            }

            // 讀取最終位置
            let transform = app.world().entity(entity).get::<Transform>().unwrap();
            rapier_to_soft_vec(transform.translation)
        }

        let result1 = simulate(10);
        let result2 = simulate(10);
        assert_eq!(
            result1, result2,
            "相同初始狀態 10 步模擬必須 bit-exact：\n  run1={result1:?}\n  run2={result2:?}"
        );
    }

    /// 測試相同初始狀態的 Rapier 模擬產生相同輸出（100 步，逐步比較）。
    #[test]
    fn test_rapier_100_step_determinism() {
        /// 建立場景並模擬，記錄每步位置。
        fn simulate_and_record(steps: usize) -> Vec<SoftVec3> {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins);
            app.add_plugins(bevy::transform::TransformPlugin);
            app.add_plugins(PhysicsPlugin);
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                17,
            )));

            // Frame 0: 初始化
            app.update();

            // Spawn 動態剛體
            let entity = app
                .world_mut()
                .spawn((
                    Transform::from_xyz(0.0, 50.0, 0.0),
                    RigidBody::Dynamic,
                    Collider::ball(0.5),
                    Velocity::linear(Vec3::new(2.0, 0.0, -1.0)),
                ))
                .id();

            let mut positions = Vec::with_capacity(steps);
            for _ in 0..steps {
                app.update();
                let transform = app.world().entity(entity).get::<Transform>().unwrap();
                positions.push(rapier_to_soft_vec(transform.translation));
            }
            positions
        }

        let run1 = simulate_and_record(100);
        let run2 = simulate_and_record(100);

        assert_eq!(run1.len(), run2.len(), "步數不一致");
        for (i, (p1, p2)) in run1.iter().zip(run2.iter()).enumerate() {
            assert_eq!(
                p1, p2,
                "第 {i} 步位置不一致（bit-exact）：\n  run1={p1:?}\n  run2={p2:?}"
            );
        }
    }

    // ── 碰撞事件測試（3 個）──────────────────────────────────────────────

    /// 測試碰撞事件產生：兩個相交的剛體應產生碰撞事件。
    #[test]
    fn test_collision_event_generated() {
        let mut app = build_physics_test_app();

        // Frame 0: 初始化
        app.update();

        // Spawn 靜態地板 + 動態剛體（在地板上方，給予初始下落速度加速碰撞）
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            Collider::cuboid(10.0, 0.5, 10.0),
            ActiveEvents::COLLISION_EVENTS,
        ));
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            RigidBody::Dynamic,
            Collider::ball(0.5),
            Velocity::linear(Vec3::new(0.0, -10.0, 0.0)),
            ActiveEvents::COLLISION_EVENTS,
        ));

        // 推進數幀讓球掉落並碰撞，逐幀檢查事件（因為每幀清空）
        let mut found_started = false;
        for _ in 0..60 {
            app.update();
            let events = app.world().resource::<CollisionEvents>();
            for (_pair, state) in &events.events {
                if state.started {
                    found_started = true;
                }
            }
            if found_started {
                break;
            }
        }

        assert!(
            found_started,
            "兩個靠近的剛體應產生碰撞開始事件（started == true）"
        );
    }

    /// 測試 CollisionEvents 使用 BTreeMap（編譯即通過）。
    #[test]
    fn test_collision_events_use_btreemap() {
        let events = CollisionEvents::default();
        // 型別驗證：如果 events.events 不是 BTreeMap<(u32, u32), CollisionState>，
        // 此行會編譯失敗。
        let _: &BTreeMap<(u32, u32), CollisionState> = &events.events;
    }

    /// 測試碰撞狀態轉換：Started → Stopped。
    #[test]
    fn test_collision_state_transition() {
        let mut app = build_physics_test_app();

        // Frame 0: 初始化
        app.update();

        // Spawn 靜態地板
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            Collider::cuboid(10.0, 0.5, 10.0),
            ActiveEvents::COLLISION_EVENTS,
        ));

        // Spawn 動態球體，從上方高速掉落
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 3.0, 0.0),
            RigidBody::Dynamic,
            Collider::ball(0.5),
            Velocity::linear(Vec3::new(0.0, -5.0, 0.0)),
            ActiveEvents::COLLISION_EVENTS,
            Restitution::coefficient(1.5), // 高彈性，碰後會反彈分離
        ));

        // 搜尋 started / stopped 事件
        let mut found_started = false;
        let mut found_stopped = false;

        for _ in 0..60 {
            app.update();
            let events = app.world().resource::<CollisionEvents>();
            for (_pair, state) in &events.events {
                if state.started {
                    found_started = true;
                }
                if state.stopped {
                    found_stopped = true;
                }
            }
        }

        assert!(found_started, "應偵測到碰撞開始（started == true）事件");
        assert!(
            found_stopped,
            "高彈性球體反彈後應偵測到碰撞結束（stopped == true）事件"
        );
    }
}
