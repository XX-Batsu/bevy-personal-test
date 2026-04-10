//! FixedUpdate 60Hz 排程與累加器上限。
//!
//! - [`FixedUpdatePlugin`] — 設定 60Hz 固定步進、系統鏈順序、累加器上限
//! - [`FixedTickCounter`] — 追蹤 FixedUpdate 觸發次數
//! - [`GameFixedSet`] — 六階段系統排序標籤

use bevy::prelude::*;
use std::time::Duration;

/// 管理 FixedUpdate 60Hz 排程與累加器上限的 Plugin。
pub struct FixedUpdatePlugin;

impl Plugin for FixedUpdatePlugin {
    fn build(&self, app: &mut App) {
        // 1. 設定 60Hz 固定步進
        app.insert_resource(Time::<Fixed>::from_hz(60.0));

        // 2. 初始化 tick 計數器
        app.init_resource::<FixedTickCounter>();

        // 3. 設定系統集合執行順序（.chain() 強制依序）
        app.configure_sets(
            FixedUpdate,
            (
                GameFixedSet::ProcessInputs,
                GameFixedSet::RecognizeGestures,
                GameFixedSet::RunScripts,
                GameFixedSet::FlushBridgeEvents,
                GameFixedSet::UpdateEcsMirror,
                GameFixedSet::ComputeStateHash,
            )
                .chain(),
        );

        // 4. 累加器上限：限制 Virtual Time 每幀最大 delta 為 50ms
        //    防止 Spiral of Death（50ms ≈ 3 × 16.67ms，最多追趕 3 個 tick）
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_max_delta(Duration::from_millis(50));

        // 5. tick counter + 空佔位系統（ProcessInputs 由 InputPlugin 接管）
        app.add_systems(
            FixedUpdate,
            (
                increment_tick_counter.in_set(GameFixedSet::ProcessInputs),
                run_scripts_stub.in_set(GameFixedSet::RunScripts),
                flush_bridge_events_stub.in_set(GameFixedSet::FlushBridgeEvents),
                update_ecs_mirror_stub.in_set(GameFixedSet::UpdateEcsMirror),
                compute_state_hash_stub.in_set(GameFixedSet::ComputeStateHash),
            ),
        );
    }
}

/// 每次 FixedUpdate 遞增 tick 計數器。
pub(crate) fn increment_tick_counter(mut counter: ResMut<FixedTickCounter>) {
    counter.count += 1;
}

// 空佔位系統（後續 Task 替換；ProcessInputs 已由 InputPlugin 接管）
fn run_scripts_stub() {}
fn flush_bridge_events_stub() {}
fn update_ecs_mirror_stub() {}
fn compute_state_hash_stub() {}

/// FixedUpdate tick 計數器，每次 FixedUpdate 遞增。
#[derive(Resource, Debug, Clone, Default)]
pub struct FixedTickCounter {
    /// 累計觸發次數（從 0 開始）。
    pub count: u64,
}

// GameFixedSet 權威定義移至 vm_bevy_bridge::schedule，此處 re-export
pub use vm_bevy_bridge::schedule::GameFixedSet;

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// 建立測試 App，使用 ManualDuration 控制時間推進。
    fn build_test_app_with_delta(delta: Duration) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(FixedUpdatePlugin);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(delta));
        app
    }

    fn build_test_app() -> App {
        // 不推進時間的 app
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(FixedUpdatePlugin);
        app
    }

    #[test]
    fn test_fixed_update_60hz_configured() {
        let app = build_test_app();
        let fixed_time = app.world().resource::<Time<Fixed>>();
        let hz = 1.0 / fixed_time.timestep().as_secs_f64();
        assert!((hz - 60.0).abs() < 0.01, "期望 60Hz，實際: {hz}");
    }

    #[test]
    fn test_tick_counter_resource_exists() {
        let app = build_test_app();
        let counter = app.world().get_resource::<FixedTickCounter>();
        assert!(counter.is_some(), "FixedTickCounter 應由 Plugin 初始化");
        assert_eq!(counter.unwrap().count, 0, "初始值應為 0");
    }

    #[test]
    fn test_tick_counter_increments() {
        // 每幀推進 20ms（> 16.67ms），一次 update 應觸發 1 次 FixedUpdate
        let mut app = build_test_app_with_delta(Duration::from_millis(20));
        // Frame 0: Bevy 初始化，不觸發 FixedUpdate
        app.update();
        // Frame 1: 累積 20ms > 16.67ms，觸發一次
        app.update();
        let count = app.world().resource::<FixedTickCounter>().count;
        assert!(
            count >= 1,
            "推進 20ms 應至少觸發一次 FixedUpdate，實際: {count}"
        );
    }

    #[test]
    fn test_tick_counter_no_advance_without_time() {
        let mut app = build_test_app_with_delta(Duration::ZERO);
        app.update();
        app.update();
        let count = app.world().resource::<FixedTickCounter>().count;
        assert_eq!(count, 0, "零 delta 不應觸發 FixedUpdate");
    }

    #[test]
    fn test_accumulator_cap_200ms() {
        // 單幀推進 200ms
        let mut app = build_test_app_with_delta(Duration::from_millis(200));
        app.update(); // Frame 0
        app.update(); // Frame 1: 累積 200ms，被截斷為 ≤50ms
        let count = app.world().resource::<FixedTickCounter>().count;
        assert!(count <= 3, "累加器上限 50ms，最多 3 tick，實際: {count}");
    }

    #[test]
    fn test_accumulator_cap_exact_50ms() {
        let mut app = build_test_app_with_delta(Duration::from_millis(50));
        app.update(); // Frame 0
        app.update(); // Frame 1: 累積 50ms
        let count = app.world().resource::<FixedTickCounter>().count;
        assert!(
            count >= 2 && count <= 3,
            "50ms 邊界應觸發 2-3 tick，實際: {count}"
        );
    }

    #[test]
    fn test_accumulator_normal_no_cap() {
        let mut app = build_test_app_with_delta(Duration::from_millis(30));
        app.update(); // Frame 0
        app.update(); // Frame 1: 累積 30ms < 50ms，不截斷
        let count = app.world().resource::<FixedTickCounter>().count;
        assert!(
            count >= 1,
            "30ms < 50ms 不觸發截斷，應至少 1 tick，實際: {count}"
        );
    }

    #[test]
    fn test_system_chain_order() {
        #[derive(Resource, Default)]
        struct OrderTracker {
            order: Vec<u32>,
        }

        fn make_order_system(id: u32) -> impl FnMut(ResMut<OrderTracker>) {
            move |mut tracker: ResMut<OrderTracker>| {
                tracker.order.push(id);
            }
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(FixedUpdatePlugin);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            20,
        )));
        app.init_resource::<OrderTracker>();
        app.add_systems(
            FixedUpdate,
            (
                make_order_system(0).in_set(GameFixedSet::ProcessInputs),
                make_order_system(1).in_set(GameFixedSet::RecognizeGestures),
                make_order_system(2).in_set(GameFixedSet::RunScripts),
                make_order_system(3).in_set(GameFixedSet::FlushBridgeEvents),
                make_order_system(4).in_set(GameFixedSet::UpdateEcsMirror),
                make_order_system(5).in_set(GameFixedSet::ComputeStateHash),
            ),
        );

        app.update(); // Frame 0
        app.update(); // Frame 1: 觸發 FixedUpdate
        let tracker = app.world().resource::<OrderTracker>();
        assert_eq!(
            tracker.order,
            vec![0, 1, 2, 3, 4, 5],
            "系統鏈順序應為 ProcessInputs→RecognizeGestures→RunScripts→FlushBridgeEvents→UpdateEcsMirror→ComputeStateHash，實際: {:?}",
            tracker.order
        );
    }

    #[test]
    fn test_multiple_updates_accumulate() {
        let mut app = build_test_app_with_delta(Duration::from_millis(20));
        app.update(); // Frame 0
        for _ in 0..3 {
            app.update(); // 每幀推進 20ms，各觸發一次
        }
        let count = app.world().resource::<FixedTickCounter>().count;
        assert_eq!(count, 3, "連續三幀推進 20ms 應各觸發一次，實際: {count}");
    }

    #[test]
    fn test_game_fixed_set_variants() {
        let variants = [
            GameFixedSet::ProcessInputs,
            GameFixedSet::RecognizeGestures,
            GameFixedSet::RunScripts,
            GameFixedSet::FlushBridgeEvents,
            GameFixedSet::UpdateEcsMirror,
            GameFixedSet::ComputeStateHash,
        ];
        for v in &variants {
            let _ = format!("{v:?}");
            let cloned = v.clone();
            assert_eq!(v, &cloned);
        }
    }
}
