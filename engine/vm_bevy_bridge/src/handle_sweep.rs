//! HandleRegistry sweep 排程 — 每 60 frames 清理過期 handle。
//!
//! # 設計依據
//! - `docs/design/architecture/04-vm-bridge/README.md`
//! - `engine/vm_runtime/src/handle_registry.rs`（sweep_expired 語義）
//!
//! # 排程策略
//! - 排程位置：Last schedule（所有 FixedUpdate 邏輯完成後）
//! - 頻率：每 60 frames 一次（非秒，不使用 std::time）
//! - 過期條件：`current_frame - created_frame >= max_lifetime_frames`（300 frames = 5 秒 @ 60fps）

use bevy::prelude::*;
use vm_runtime::HandleRegistry;

/// 模擬時鐘 Bevy Resource — 追蹤當前幀號。
///
/// 由 FixedUpdate 排程推進，handle_sweep 與 mirror_sync 共用。
/// 此定義為 Phase 9 Task 12 暫定版本；若 mirror_sync（Task 06+07）
/// 已定義相同 Resource，應統一至單一來源。
#[derive(Resource, Default, Debug, Clone)]
pub struct SimulationClock {
    /// 當前幀號，從 0 開始遞增。
    pub frame: u64,
}

/// Bevy Resource：包裝 Phase 7 的 HandleRegistry。
///
/// 透過 Bevy Resource 系統注入，供 sweep system 與其他系統存取。
/// 內部使用 BTreeMap 確保確定性（遵守 Determinism Rules）。
#[derive(Resource)]
pub struct HandleRegistryResource(pub HandleRegistry);

/// 每 60 frames 執行一次 HandleRegistry sweep。
///
/// 排程位置：Last schedule（所有 FixedUpdate 邏輯完成後）。
/// 觸發條件：`sim_clock.frame % 60 == 0`。
///
/// sweep_expired 保留條件：`valid && (current_frame - created_frame) < max_lifetime_frames`
/// - 已 invalidate 的 handle 一律移除
/// - 超過 300 frames 的 handle 自動過期
pub fn sweep_handle_registry(
    mut handle_registry: ResMut<HandleRegistryResource>,
    sim_clock: Res<SimulationClock>,
) {
    if sim_clock.frame.is_multiple_of(60) {
        handle_registry.0.sweep_expired(sim_clock.frame);
        tracing::debug!("HandleRegistry sweep 完成：frame={}", sim_clock.frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vm_runtime::HandleRegistry;

    /// 建構 sweep 測試用 Bevy App
    fn build_sweep_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimulationClock>();
        app.insert_resource(HandleRegistryResource(HandleRegistry::new()));
        app.add_systems(Last, sweep_handle_registry);
        app
    }

    // ─── 1. frame=0 時 sweep 觸發但無清理 ────────────────────────

    #[test]
    fn test_sweep_at_frame_zero() {
        let mut app = build_sweep_test_app();

        // frame=0，0 % 60 == 0 → sweep 觸發
        // 但 registry 為空，無 handle 可清理
        app.update();

        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.effect_count(), 0);
        assert_eq!(registry.0.sound_count(), 0);
    }

    // ─── 2. frame=59 不觸發 sweep ────────────────────────────────

    #[test]
    fn test_sweep_frequency_skip_59() {
        let mut app = build_sweep_test_app();

        // 建立一個 effect handle（frame=0）並立即 invalidate
        {
            let mut registry = app.world_mut().resource_mut::<HandleRegistryResource>();
            let handle = registry.0.create_effect(0);
            registry.0.invalidate_effect(handle);
        }

        // 設定 frame=59（59 % 60 != 0 → sweep 不觸發）
        app.world_mut().resource_mut::<SimulationClock>().frame = 59;
        app.update();

        // invalidated handle 應仍存在（sweep 未觸發）
        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.effect_count(), 1);
    }

    // ─── 3. frame=60 sweep，handle 未過期保留 ────────────────────

    #[test]
    fn test_sweep_preserves_unexpired_handles() {
        let mut app = build_sweep_test_app();

        // 建立 effect handle（frame=0）
        {
            let mut registry = app.world_mut().resource_mut::<HandleRegistryResource>();
            registry.0.create_effect(0);
        }

        // frame=60 → 觸發 sweep
        // 60 - 0 = 60 < 300 → 保留
        app.world_mut().resource_mut::<SimulationClock>().frame = 60;
        app.update();

        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.effect_count(), 1);
    }

    // ─── 4. frame=360，handle created at 0 過期 ──────────────────

    #[test]
    fn test_sweep_clears_expired() {
        let mut app = build_sweep_test_app();

        // 建立 effect handle（frame=0）
        {
            let mut registry = app.world_mut().resource_mut::<HandleRegistryResource>();
            registry.0.create_effect(0);
        }

        // frame=360 → 觸發 sweep
        // 360 - 0 = 360 >= 300 → 過期移除
        app.world_mut().resource_mut::<SimulationClock>().frame = 360;
        app.update();

        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.effect_count(), 0);
    }

    // ─── 5. 混合過期與未過期 ─────────────────────────────────────

    #[test]
    fn test_sweep_mixed_expired_and_valid() {
        let mut app = build_sweep_test_app();

        // A: frame=0 建立，B: frame=200 建立
        {
            let mut registry = app.world_mut().resource_mut::<HandleRegistryResource>();
            registry.0.create_effect(0); // A
            registry.0.create_effect(200); // B
        }

        // frame=360 → 觸發 sweep
        // A: 360 - 0 = 360 >= 300 → 過期
        // B: 360 - 200 = 160 < 300 → 保留
        app.world_mut().resource_mut::<SimulationClock>().frame = 360;
        app.update();

        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.effect_count(), 1);
    }

    // ─── 6. 空 registry sweep 不 panic ───────────────────────────

    #[test]
    fn test_sweep_empty_registry() {
        let mut app = build_sweep_test_app();

        // frame=60 → 觸發 sweep，但 registry 為空
        app.world_mut().resource_mut::<SimulationClock>().frame = 60;
        app.update();

        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.effect_count(), 0);
        assert_eq!(registry.0.sound_count(), 0);
    }

    // ─── 7. sound handle 也被 sweep ─────────────────────────────

    #[test]
    fn test_sweep_sound_handle() {
        let mut app = build_sweep_test_app();

        // 建立 sound handle（frame=0）
        {
            let mut registry = app.world_mut().resource_mut::<HandleRegistryResource>();
            registry.0.create_sound(0);
        }

        // frame=300 → 觸發 sweep（300 % 60 == 0）
        // 300 - 0 = 300 >= 300 → 過期移除
        app.world_mut().resource_mut::<SimulationClock>().frame = 300;
        app.update();

        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.sound_count(), 0);
    }

    // ─── 8. frame=120 觸發 sweep（120 % 60 == 0）────────────────

    #[test]
    fn test_sweep_at_frame_120() {
        let mut app = build_sweep_test_app();

        // 建立 effect handle（frame=0）
        {
            let mut registry = app.world_mut().resource_mut::<HandleRegistryResource>();
            registry.0.create_effect(0);
        }

        // frame=120 → 觸發 sweep（120 % 60 == 0）
        // 120 - 0 = 120 < 300 → 保留
        app.world_mut().resource_mut::<SimulationClock>().frame = 120;
        app.update();

        let registry = app.world().resource::<HandleRegistryResource>();
        assert_eq!(registry.0.effect_count(), 1);
    }
}
