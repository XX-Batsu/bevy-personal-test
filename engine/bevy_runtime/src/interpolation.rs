//! 渲染插值 — save_previous_transform + interpolate_rendering。
//!
//! - [`PreviousTransform`]: 儲存上一個 FixedUpdate tick 的 Transform
//! - [`save_previous_transform`]: FixedUpdate 系統，保存當前 Transform
//! - [`interpolate_rendering`]: Update 系統，使用 overstep alpha 插值 Transform

use bevy::prelude::*;

/// 儲存上一個 FixedUpdate tick 的 Transform，供渲染插值使用。
#[derive(Component, Clone, Copy, Debug)]
pub struct PreviousTransform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for PreviousTransform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

impl From<&Transform> for PreviousTransform {
    fn from(t: &Transform) -> Self {
        Self {
            translation: t.translation,
            rotation: t.rotation,
            scale: t.scale,
        }
    }
}

/// FixedUpdate 系統：在每個 FixedUpdate 開始時保存當前 Transform。
/// 排程位置：FixedUpdate → GameFixedSet::ProcessInputs（最前端）
pub fn save_previous_transform(mut query: Query<(&Transform, &mut PreviousTransform)>) {
    for (transform, mut previous) in query.iter_mut() {
        previous.translation = transform.translation;
        previous.rotation = transform.rotation;
        previous.scale = transform.scale;
    }
}

/// Update 系統（每渲染幀）：使用 overstep alpha 插值 Transform。
/// 直接修改 Transform（非額外的 RenderTransform component）。
///
/// alpha = Time::<Fixed>::overstep_fraction()（Bevy 0.15 穩定 API，回傳 f32）
/// - translation: Vec3::lerp
/// - rotation: Quat::slerp（最短路徑球面插值）
/// - scale: Vec3::lerp
pub fn interpolate_rendering(
    fixed_time: Res<Time<Fixed>>,
    mut query: Query<(&PreviousTransform, &mut Transform)>,
) {
    let alpha = fixed_time.overstep_fraction();

    for (previous, mut transform) in query.iter_mut() {
        // 暫存當前 FixedUpdate 的目標值（transform 是 FixedUpdate 後的結果）
        let target_translation = transform.translation;
        let target_rotation = transform.rotation;
        let target_scale = transform.scale;

        transform.translation = previous.translation.lerp(target_translation, alpha);
        transform.rotation = previous.rotation.slerp(target_rotation, alpha);
        transform.scale = previous.scale.lerp(target_scale, alpha);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    // ── 輔助 ──────────────────────────────────────────────

    fn build_interp_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app
    }

    // ── Translation lerp 測試 ─────────────────────────────

    #[test]
    fn test_translation_lerp_midpoint() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(10.0, 20.0, 30.0);
        let result = a.lerp(b, 0.5);
        let expected = Vec3::new(5.0, 10.0, 15.0);
        assert!(
            result.abs_diff_eq(expected, 1e-5),
            "midpoint 插值失敗: {result:?} != {expected:?}"
        );
    }

    #[test]
    fn test_translation_lerp_alpha_zero() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(10.0, 20.0, 30.0);
        let result = a.lerp(b, 0.0);
        assert!(
            result.abs_diff_eq(a, 1e-5),
            "alpha=0 應回傳起點: {result:?} != {a:?}"
        );
    }

    #[test]
    fn test_translation_lerp_alpha_one() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(10.0, 20.0, 30.0);
        let result = a.lerp(b, 1.0);
        assert!(
            result.abs_diff_eq(b, 1e-5),
            "alpha=1 應回傳終點: {result:?} != {b:?}"
        );
    }

    #[test]
    fn test_translation_lerp_near_zero() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(100.0, 200.0, 300.0);
        let alpha = 0.01;
        let result = a.lerp(b, alpha);
        let expected = Vec3::new(1.0, 2.0, 3.0);
        assert!(
            result.abs_diff_eq(expected, 1e-3),
            "near-zero alpha 插值失敗: {result:?} != {expected:?}"
        );
    }

    #[test]
    fn test_translation_lerp_near_one() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(100.0, 200.0, 300.0);
        let alpha = 0.99;
        let result = a.lerp(b, alpha);
        let expected = Vec3::new(99.0, 198.0, 297.0);
        assert!(
            result.abs_diff_eq(expected, 1e-3),
            "near-one alpha 插值失敗: {result:?} != {expected:?}"
        );
    }

    // ── Rotation slerp 測試 ───────────────────────────────

    #[test]
    fn test_rotation_slerp_midpoint_45deg() {
        let a = Quat::IDENTITY;
        let b = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2); // 90°
        let result = a.slerp(b, 0.5);
        let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4); // 45°
        assert!(
            result.abs_diff_eq(expected, 1e-5),
            "slerp midpoint 應為 45°: {result:?} != {expected:?}"
        );
    }

    // ── Scale lerp 測試 ───────────────────────────────────

    #[test]
    fn test_scale_lerp_midpoint() {
        let a = Vec3::ONE;
        let b = Vec3::new(3.0, 3.0, 3.0);
        let result = a.lerp(b, 0.5);
        let expected = Vec3::new(2.0, 2.0, 2.0);
        assert!(
            result.abs_diff_eq(expected, 1e-5),
            "scale midpoint 插值失敗: {result:?} != {expected:?}"
        );
    }

    // ── save_previous_transform 系統測試 ──────────────────

    #[test]
    fn test_save_copies_translation() {
        let mut app = build_interp_test_app();
        let entity = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(5.0, 10.0, 15.0)),
                PreviousTransform::default(),
            ))
            .id();

        // 執行 save
        app.world_mut()
            .run_system_once(save_previous_transform)
            .unwrap();

        let prev = app.world().get::<PreviousTransform>(entity).unwrap();
        assert!(
            prev.translation
                .abs_diff_eq(Vec3::new(5.0, 10.0, 15.0), 1e-5),
            "save 應複製 translation: {:?}",
            prev.translation
        );
    }

    #[test]
    fn test_save_copies_rotation() {
        let mut app = build_interp_test_app();
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let entity = app
            .world_mut()
            .spawn((Transform::from_rotation(rot), PreviousTransform::default()))
            .id();

        app.world_mut()
            .run_system_once(save_previous_transform)
            .unwrap();

        let prev = app.world().get::<PreviousTransform>(entity).unwrap();
        assert!(
            prev.rotation.abs_diff_eq(rot, 1e-5),
            "save 應複製 rotation: {:?}",
            prev.rotation
        );
    }

    #[test]
    fn test_save_copies_scale() {
        let mut app = build_interp_test_app();
        let entity = app
            .world_mut()
            .spawn((
                Transform::from_scale(Vec3::new(2.0, 3.0, 4.0)),
                PreviousTransform::default(),
            ))
            .id();

        app.world_mut()
            .run_system_once(save_previous_transform)
            .unwrap();

        let prev = app.world().get::<PreviousTransform>(entity).unwrap();
        assert!(
            prev.scale.abs_diff_eq(Vec3::new(2.0, 3.0, 4.0), 1e-5),
            "save 應複製 scale: {:?}",
            prev.scale
        );
    }

    // ── 邊界測試 ──────────────────────────────────────────

    #[test]
    fn test_identity_when_same() {
        // 當 previous 與 current 相同時，插值結果不變
        let prev = PreviousTransform {
            translation: Vec3::new(5.0, 5.0, 5.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let current_translation = Vec3::new(5.0, 5.0, 5.0);
        let current_rotation = Quat::IDENTITY;
        let current_scale = Vec3::ONE;

        // 任意 alpha 都應得到相同值
        for alpha in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let t = prev.translation.lerp(current_translation, alpha);
            let r = prev.rotation.slerp(current_rotation, alpha);
            let s = prev.scale.lerp(current_scale, alpha);
            assert!(
                t.abs_diff_eq(current_translation, 1e-5),
                "相同值插值 translation 應不變 (alpha={alpha}): {t:?}"
            );
            assert!(
                r.abs_diff_eq(current_rotation, 1e-5),
                "相同值插值 rotation 應不變 (alpha={alpha}): {r:?}"
            );
            assert!(
                s.abs_diff_eq(current_scale, 1e-5),
                "相同值插值 scale 應不變 (alpha={alpha}): {s:?}"
            );
        }
    }

    #[test]
    fn test_multiple_entities() {
        let mut app = build_interp_test_app();

        let e1 = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
                PreviousTransform::default(),
            ))
            .id();

        let e2 = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(0.0, 20.0, 0.0)),
                PreviousTransform::default(),
            ))
            .id();

        // save_previous_transform 應同時處理多個 entity
        app.world_mut()
            .run_system_once(save_previous_transform)
            .unwrap();

        let prev1 = app.world().get::<PreviousTransform>(e1).unwrap();
        let prev2 = app.world().get::<PreviousTransform>(e2).unwrap();

        assert!(
            prev1
                .translation
                .abs_diff_eq(Vec3::new(10.0, 0.0, 0.0), 1e-5),
            "entity 1 translation: {:?}",
            prev1.translation
        );
        assert!(
            prev2
                .translation
                .abs_diff_eq(Vec3::new(0.0, 20.0, 0.0), 1e-5),
            "entity 2 translation: {:?}",
            prev2.translation
        );
    }
}
