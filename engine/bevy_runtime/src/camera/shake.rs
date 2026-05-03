//! 攝影機震動（Camera Shake） — trauma² + Perlin noise，多 entry offset 累加。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.2

use bevy::prelude::*;

use super::bounds::{clamp_to_bounds, viewport_half_size};
use super::components::{
    CameraBounds, CameraZoom, DEFAULT_SHAKE_DECAY_RATE, DEFAULT_SHAKE_DIRECTION_BIAS,
    DEFAULT_SHAKE_MAX_STRENGTH_RATIO, DEFAULT_SHAKE_PERPENDICULAR_DAMPING, DEFAULT_VIEWPORT_HEIGHT,
    SHAKE_FREQUENCY,
};
use super::events::{CameraShakeOp, CameraShakeRequest};
use noise::{NoiseFn, Perlin};

pub type ShakeId = u64;

/// 單個 shake 條目。
#[derive(Debug, Clone)]
pub struct ShakeEntry {
    pub id: ShakeId,
    pub trauma: f32,
    pub decay_rate: f32,
    pub direction: Option<Vec2>,
    pub max_strength: f32,
    pub direction_bias: f32,
    pub perpendicular_damping: f32,
    pub seed: u64,
    /// internal：累積時間，用於 Perlin 採樣位置
    pub(crate) elapsed: f32,
}

/// `add()` 方法的參數（不含 `id`、`elapsed`）。
#[derive(Debug, Clone)]
pub struct ShakeParams {
    pub trauma: f32,
    pub decay_rate: f32,
    pub direction: Option<Vec2>,
    pub max_strength: f32,
    pub direction_bias: f32,
    pub perpendicular_damping: f32,
    pub seed: Option<u64>,
}

impl Default for ShakeParams {
    fn default() -> Self {
        Self {
            trauma: 0.5,
            decay_rate: DEFAULT_SHAKE_DECAY_RATE,
            direction: None,
            max_strength: 0.0, // 0 = fallback 至視口相對
            direction_bias: DEFAULT_SHAKE_DIRECTION_BIAS,
            perpendicular_damping: DEFAULT_SHAKE_PERPENDICULAR_DAMPING,
            seed: None,
        }
    }
}

/// Per-camera shake 容器。多個 shake 並存，offset 累加，trauma <= 0 自動移除。
#[derive(Component, Default, Debug)]
pub struct CameraShake {
    entries: Vec<ShakeEntry>,
    next_id: u64,
    /// internal：上一次套用的總 offset（供 `current_offset()` debug 讀取）
    last_applied_offset: Vec2,
}

impl CameraShake {
    /// 新增一個 shake entry。
    /// 呼叫者須提供已決定的 `seed`（由 `shake_event_handler_system` 從 `ShakeSeedCounter` 取得，或使用者自填）。
    /// 欄位 clamp 規則：
    /// - `trauma` → clamp 至 [0.0, 1.0]
    /// - `max_strength`、`decay_rate`、`direction_bias`、`perpendicular_damping` → clamp 至 `>= 0.0`
    /// - `direction = Some(Vec2::ZERO)` → 視為 `None`（warn_once）
    pub fn add(&mut self, params: ShakeParams, seed: u64) -> ShakeId {
        let id = self.next_id;
        self.next_id += 1;

        let direction = match params.direction {
            Some(d) if d.length_squared() < f32::EPSILON => {
                bevy::utils::warn_once!("CameraShake::add 收到 direction = Vec2::ZERO，視為 None");
                None
            }
            other => other,
        };

        let entry = ShakeEntry {
            id,
            trauma: params.trauma.clamp(0.0, 1.0),
            decay_rate: params.decay_rate.max(0.0),
            direction,
            max_strength: params.max_strength.max(0.0),
            direction_bias: params.direction_bias.max(0.0),
            perpendicular_damping: params.perpendicular_damping.max(0.0),
            seed,
            elapsed: 0.0,
        };

        self.entries.push(entry);

        if self.entries.len() > 64 {
            bevy::utils::warn_once!("CameraShake 累積超過 64 個 entries（可能有洩漏）");
        }

        id
    }

    pub fn remove_by_id(&mut self, id: ShakeId) -> Option<ShakeEntry> {
        match self.entries.iter().position(|e| e.id == id) {
            Some(idx) => Some(self.entries.remove(idx)),
            None => {
                bevy::utils::warn_once!("CameraShake::remove_by_id 找不到 id={}", id);
                None
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_applied_offset = Vec2::ZERO;
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 上一幀套用的總 offset（debug 用途）。
    pub fn current_offset(&self) -> Vec2 {
        self.last_applied_offset
    }

    pub(crate) fn entries_mut(&mut self) -> &mut Vec<ShakeEntry> {
        &mut self.entries
    }

    pub(crate) fn set_last_applied_offset(&mut self, offset: Vec2) {
        self.last_applied_offset = offset;
    }
}

/// 全域 shake 種子計數器。`ShakeParams.seed = None` 時由 `shake_event_handler_system` 遞增取用。
#[derive(Resource, Default)]
pub struct ShakeSeedCounter(pub u64);

impl ShakeSeedCounter {
    pub fn next_seed(&mut self) -> u64 {
        let s = self.0;
        self.0 = self.0.wrapping_add(1);
        s
    }
}

/// `Update` 系統：套用所有 shake entries 的 offset 至 transform；衰減 trauma；移除歸零 entry。
/// 若 `CameraBounds.clamp_shake = true`，套用後再 clamp 至 bounds。
///
/// **viewport 來源說明**：
/// - 計算 `effective_strength`（`max_strength = 0` 時 fallback 到視口比例）優先使用
///   `CameraZoom.current`；無 `CameraZoom` 則退到 `DEFAULT_VIEWPORT_HEIGHT`。
/// - 計算 `clamp_shake` bounds 必須與 `camera_bounds_system` 完全一致 —
///   一律從 `&Projection` 透過 `bounds::viewport_half_size` 取，避免兩處邏輯漂移。
///   無 `Projection` 的 camera 不執行 clamp_shake（half_w/half_h 不存在）。
#[allow(clippy::type_complexity)]
pub fn shake_apply_system(
    time: Res<Time>,
    mut perlin: Local<Option<Perlin>>,
    mut query: Query<(
        &mut CameraShake,
        &mut Transform,
        Option<&CameraZoom>,
        Option<&CameraBounds>,
        Option<&Projection>,
    )>,
) {
    let perlin = perlin.get_or_insert_with(|| Perlin::new(0));
    let dt = time.delta_secs().min(0.1);

    for (mut shake, mut transform, zoom_opt, bounds_opt, projection_opt) in query.iter_mut() {
        let strength_viewport_h = zoom_opt
            .map(|z| z.current)
            .unwrap_or(DEFAULT_VIEWPORT_HEIGHT);
        let mut total_offset = Vec2::ZERO;
        let mut to_remove: Vec<ShakeId> = Vec::new();

        for entry in shake.entries_mut().iter_mut() {
            entry.trauma = (entry.trauma - entry.decay_rate * dt).max(0.0);
            if entry.trauma <= 0.0 {
                to_remove.push(entry.id);
                continue;
            }
            entry.elapsed += dt;

            let effective_strength = if entry.max_strength > 0.0 {
                entry.max_strength
            } else {
                strength_viewport_h * DEFAULT_SHAKE_MAX_STRENGTH_RATIO
            };
            let shake_amount = entry.trauma.powi(2) * effective_strength;

            let t = (entry.elapsed * SHAKE_FREQUENCY) as f64;
            let seed_offset = entry.seed as f64 * 0.1;
            let noise_x = perlin.get([t, seed_offset]) as f32;
            let noise_y = perlin.get([t, seed_offset + 1000.0]) as f32;

            let mut offset = Vec2::new(noise_x, noise_y) * shake_amount;

            if let Some(dir) = entry.direction {
                let dn = dir.normalize();
                let along = offset.dot(dn) * dn;
                let perp = offset - along;
                offset = along * entry.direction_bias + perp * entry.perpendicular_damping;
            }

            total_offset += offset;
        }

        for id in to_remove {
            if let Some(idx) = shake.entries_mut().iter().position(|e| e.id == id) {
                shake.entries_mut().remove(idx);
            }
        }

        shake.set_last_applied_offset(total_offset);

        transform.translation.x += total_offset.x;
        transform.translation.y += total_offset.y;

        if let Some(bounds) = bounds_opt {
            if bounds.clamp_shake && projection_opt.is_none() {
                bevy::utils::warn_once!(
                    "CameraBounds.clamp_shake = true 但 camera 缺少 Projection — clamp_shake 會被跳過。\
                     請為 camera 加上 Projection component（如 Projection::Orthographic）。"
                );
            }
            if let Some(projection) = projection_opt {
                if bounds.clamp_shake
                    && bounds.min.x <= bounds.max.x
                    && bounds.min.y <= bounds.max.y
                {
                    let (half_w, half_h) = viewport_half_size(projection);
                    clamp_to_bounds(&mut transform.translation, bounds, half_w, half_h);
                }
            }
        }
    }
}

/// `Update` 系統：消費 `CameraShakeRequest` events，套用至對應 `CameraShake`。
/// `Add` 且 `ShakeParams.seed = None` 時，由 `ShakeSeedCounter` 提供遞增 seed。
pub fn shake_event_handler_system(
    mut events: EventReader<CameraShakeRequest>,
    mut seed_counter: ResMut<ShakeSeedCounter>,
    mut query: Query<(Entity, &mut CameraShake)>,
) {
    for ev in events.read() {
        let mut matched = false;
        for (entity, mut shake) in query.iter_mut() {
            if let Some(target) = ev.camera {
                if target != entity {
                    continue;
                }
            }
            matched = true;
            match ev.op.clone() {
                CameraShakeOp::Add(params) => {
                    let seed = params.seed.unwrap_or_else(|| seed_counter.next_seed());
                    shake.add(params, seed);
                }
                CameraShakeOp::RemoveById(id) => {
                    shake.remove_by_id(id);
                }
                CameraShakeOp::Clear => {
                    shake.clear();
                }
            }
        }
        if !matched {
            bevy::utils::warn_once!(
                "CameraShakeRequest camera={:?} 找不到符合的 CameraShake",
                ev.camera
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_後_len_為_1() {
        let mut shake = CameraShake::default();
        let id = shake.add(ShakeParams::default(), 0);
        assert_eq!(shake.len(), 1);
        assert_eq!(id, 0);
    }

    #[test]
    fn add_trauma_clamp_至_0_1() {
        let mut shake = CameraShake::default();
        shake.add(
            ShakeParams {
                trauma: 5.0,
                ..ShakeParams::default()
            },
            0,
        );
        assert_eq!(shake.entries_mut()[0].trauma, 1.0);

        shake.add(
            ShakeParams {
                trauma: -0.5,
                ..ShakeParams::default()
            },
            1,
        );
        assert_eq!(shake.entries_mut()[1].trauma, 0.0);
    }

    #[test]
    fn add_negative_decay_rate_clamp_零() {
        let mut shake = CameraShake::default();
        shake.add(
            ShakeParams {
                decay_rate: -1.0,
                ..ShakeParams::default()
            },
            0,
        );
        assert_eq!(shake.entries_mut()[0].decay_rate, 0.0);
    }

    #[test]
    fn add_direction_zero_視為_none() {
        let mut shake = CameraShake::default();
        shake.add(
            ShakeParams {
                direction: Some(Vec2::ZERO),
                ..ShakeParams::default()
            },
            0,
        );
        assert_eq!(shake.entries_mut()[0].direction, None);
    }

    #[test]
    fn remove_by_id_存在回傳_some() {
        let mut shake = CameraShake::default();
        let id = shake.add(ShakeParams::default(), 0);
        assert!(shake.remove_by_id(id).is_some());
        assert!(shake.is_empty());
    }

    #[test]
    fn remove_by_id_不存在回傳_none() {
        let mut shake = CameraShake::default();
        assert!(shake.remove_by_id(9999).is_none());
    }

    #[test]
    fn clear_清空且重置_offset() {
        let mut shake = CameraShake::default();
        shake.add(ShakeParams::default(), 0);
        shake.set_last_applied_offset(Vec2::new(5.0, 5.0));
        shake.clear();
        assert!(shake.is_empty());
        assert_eq!(shake.current_offset(), Vec2::ZERO);
    }

    #[test]
    fn current_offset_初始為_zero() {
        let shake = CameraShake::default();
        assert_eq!(shake.current_offset(), Vec2::ZERO);
    }

    #[test]
    fn id_單調遞增() {
        let mut shake = CameraShake::default();
        let a = shake.add(ShakeParams::default(), 0);
        let b = shake.add(ShakeParams::default(), 0);
        let c = shake.add(ShakeParams::default(), 0);
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2);
    }

    #[test]
    fn seed_counter_遞增() {
        let mut counter = ShakeSeedCounter::default();
        assert_eq!(counter.next_seed(), 0);
        assert_eq!(counter.next_seed(), 1);
        assert_eq!(counter.next_seed(), 2);
    }

    #[test]
    fn default_params_max_strength_為_零_代表_fallback() {
        let p = ShakeParams::default();
        assert_eq!(p.max_strength, 0.0);
        assert_eq!(p.direction_bias, DEFAULT_SHAKE_DIRECTION_BIAS);
        assert_eq!(p.perpendicular_damping, DEFAULT_SHAKE_PERPENDICULAR_DAMPING);
        assert_eq!(p.decay_rate, DEFAULT_SHAKE_DECAY_RATE);
    }
}

#[cfg(test)]
mod system_tests {
    use super::super::components::{
        CameraBounds, CameraZoom, DEFAULT_SHAKE_MAX_STRENGTH_RATIO, DEFAULT_VIEWPORT_HEIGHT,
    };
    use super::super::events::{CameraShakeOp, CameraShakeRequest};
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn build_app(secs: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            secs,
        )));
        app
    }

    #[test]
    fn apply_trauma_零_無_offset() {
        let mut app = build_app(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraShake::default(),
                Transform::from_xyz(100.0, 200.0, 0.0),
            ))
            .id();

        {
            let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
            shake.add(
                ShakeParams {
                    trauma: 0.0,
                    max_strength: 100.0,
                    ..ShakeParams::default()
                },
                42,
            );
        }

        app.update();
        app.world_mut().run_system_once(shake_apply_system).unwrap();

        let t = app.world().get::<Transform>(cam).unwrap();
        assert!((t.translation.x - 100.0).abs() < 0.01);
        assert!((t.translation.y - 200.0).abs() < 0.01);
    }

    #[test]
    fn apply_trauma_一_產生_offset() {
        let mut app = build_app(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((CameraShake::default(), Transform::from_xyz(0.0, 0.0, 0.0)))
            .id();

        {
            let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
            shake.add(
                ShakeParams {
                    trauma: 1.0,
                    max_strength: 25.0,
                    decay_rate: 0.0,
                    ..ShakeParams::default()
                },
                42,
            );
        }

        app.update();
        for _ in 0..3 {
            app.world_mut().run_system_once(shake_apply_system).unwrap();
            app.update();
        }

        let t = app.world().get::<Transform>(cam).unwrap();
        let dist = t.translation.truncate().length();
        assert!(
            dist > 0.0 && dist < 150.0,
            "offset 應 > 0 且 < 150（3 frame 累加上限），實際: {dist}"
        );
    }

    #[test]
    fn apply_trauma_衰減至零自動移除() {
        let mut app = build_app(0.1);
        let cam = app
            .world_mut()
            .spawn((CameraShake::default(), Transform::from_xyz(0.0, 0.0, 0.0)))
            .id();

        {
            let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
            shake.add(
                ShakeParams {
                    trauma: 1.0,
                    decay_rate: 5.0,
                    max_strength: 10.0,
                    ..ShakeParams::default()
                },
                0,
            );
        }

        app.update();
        for _ in 0..4 {
            app.world_mut().run_system_once(shake_apply_system).unwrap();
            app.update();
        }

        let shake = app.world().get::<CameraShake>(cam).unwrap();
        assert!(shake.is_empty(), "trauma 衰減至零應自動移除");
    }

    #[test]
    fn apply_max_strength_零_fallback_至_視口比例() {
        let mut app = build_app(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraShake::default(),
                CameraZoom::default(),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        {
            let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
            shake.add(
                ShakeParams {
                    trauma: 1.0,
                    max_strength: 0.0,
                    decay_rate: 0.0,
                    ..ShakeParams::default()
                },
                42,
            );
        }

        app.update();
        for _ in 0..3 {
            app.world_mut().run_system_once(shake_apply_system).unwrap();
            app.update();
        }

        let t = app.world().get::<Transform>(cam).unwrap();
        let expected_max = DEFAULT_VIEWPORT_HEIGHT * DEFAULT_SHAKE_MAX_STRENGTH_RATIO;
        let dist = t.translation.truncate().length();
        assert!(
            dist <= expected_max * 5.0,
            "fallback 應用 viewport_h × ratio（3 frame 累加上限 ~5×），dist={dist}, expected_max={expected_max}"
        );
    }

    #[test]
    fn apply_direction_某軸_offset_主要沿該軸() {
        let mut app = build_app(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((CameraShake::default(), Transform::from_xyz(0.0, 0.0, 0.0)))
            .id();

        {
            let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
            shake.add(
                ShakeParams {
                    trauma: 1.0,
                    max_strength: 100.0,
                    direction: Some(Vec2::new(1.0, 0.0)),
                    direction_bias: 3.0,
                    perpendicular_damping: 0.1,
                    decay_rate: 0.0,
                    ..ShakeParams::default()
                },
                42,
            );
        }

        let mut xs = Vec::new();
        let mut ys = Vec::new();
        app.update();
        for _ in 0..20 {
            app.world_mut().run_system_once(shake_apply_system).unwrap();
            let t = app.world().get::<Transform>(cam).unwrap();
            xs.push(t.translation.x.abs());
            ys.push(t.translation.y.abs());
            let mut t = app.world_mut().get_mut::<Transform>(cam).unwrap();
            t.translation = Vec3::ZERO;
            app.update();
        }
        let x_sum: f32 = xs.iter().sum();
        let y_sum: f32 = ys.iter().sum();
        assert!(
            x_sum > y_sum * 3.0,
            "direction=(1,0) 時 x 累積應 >> y，實際 x={x_sum}, y={y_sum}"
        );
    }

    #[test]
    fn apply_相同_seed_輸出相同() {
        fn run_once(seed: u64) -> Vec2 {
            let mut app = build_app(1.0 / 60.0);
            let cam = app
                .world_mut()
                .spawn((CameraShake::default(), Transform::from_xyz(0.0, 0.0, 0.0)))
                .id();
            {
                let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
                shake.add(
                    ShakeParams {
                        trauma: 1.0,
                        max_strength: 25.0,
                        decay_rate: 0.0,
                        ..ShakeParams::default()
                    },
                    seed,
                );
            }
            app.update();
            app.world_mut().run_system_once(shake_apply_system).unwrap();
            app.world()
                .get::<Transform>(cam)
                .unwrap()
                .translation
                .truncate()
        }

        let a = run_once(123);
        let b = run_once(123);
        assert_eq!(a, b, "相同 seed 應產生相同 offset");
    }

    #[test]
    fn apply_bounds_clamp_shake_true_震動受限() {
        let mut app = build_app(1.0 / 60.0);
        let cam = app
            .world_mut()
            .spawn((
                CameraShake::default(),
                CameraBounds {
                    min: Vec2::new(-1.0, -1.0),
                    max: Vec2::new(1.0, 1.0),
                    clamp_shake: true,
                },
                Projection::Orthographic(bevy::render::camera::OrthographicProjection {
                    scaling_mode: bevy::render::camera::ScalingMode::FixedVertical {
                        viewport_height: 0.5,
                    },
                    ..bevy::render::camera::OrthographicProjection::default_2d()
                }),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        {
            let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
            shake.add(
                ShakeParams {
                    trauma: 1.0,
                    max_strength: 100.0,
                    decay_rate: 0.0,
                    ..ShakeParams::default()
                },
                42,
            );
        }

        app.update();
        for _ in 0..5 {
            app.world_mut().run_system_once(shake_apply_system).unwrap();
            let t = app.world().get::<Transform>(cam).unwrap();
            assert!(
                t.translation.x.abs() <= 0.57,
                "x 應被 clamp 至 ≈0.556，實際: {}",
                t.translation.x
            );
            assert!(
                t.translation.y.abs() <= 0.76,
                "y 應被 clamp 至 0.75，實際: {}",
                t.translation.y
            );
            let mut t = app.world_mut().get_mut::<Transform>(cam).unwrap();
            t.translation = Vec3::ZERO;
            app.update();
        }
    }

    #[test]
    fn event_handler_add_插入_entry() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ShakeSeedCounter>();
        app.add_event::<CameraShakeRequest>();

        let cam = app.world_mut().spawn(CameraShake::default()).id();

        app.world_mut()
            .resource_mut::<Events<CameraShakeRequest>>()
            .send(CameraShakeRequest {
                camera: Some(cam),
                op: CameraShakeOp::Add(ShakeParams {
                    trauma: 0.5,
                    max_strength: 10.0,
                    ..ShakeParams::default()
                }),
            });

        app.world_mut()
            .run_system_once(shake_event_handler_system)
            .unwrap();

        let shake = app.world().get::<CameraShake>(cam).unwrap();
        assert_eq!(shake.len(), 1);
    }

    #[test]
    fn event_handler_seed_none_用_counter() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ShakeSeedCounter>();
        app.add_event::<CameraShakeRequest>();

        let cam = app.world_mut().spawn(CameraShake::default()).id();

        for _ in 0..3 {
            app.world_mut()
                .resource_mut::<Events<CameraShakeRequest>>()
                .send(CameraShakeRequest {
                    camera: Some(cam),
                    op: CameraShakeOp::Add(ShakeParams {
                        seed: None,
                        ..ShakeParams::default()
                    }),
                });
        }

        app.world_mut()
            .run_system_once(shake_event_handler_system)
            .unwrap();

        assert_eq!(app.world().resource::<ShakeSeedCounter>().0, 3);
    }

    #[test]
    fn event_handler_camera_none_廣播() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ShakeSeedCounter>();
        app.add_event::<CameraShakeRequest>();

        let a = app.world_mut().spawn(CameraShake::default()).id();
        let b = app.world_mut().spawn(CameraShake::default()).id();

        app.world_mut()
            .resource_mut::<Events<CameraShakeRequest>>()
            .send(CameraShakeRequest {
                camera: None,
                op: CameraShakeOp::Add(ShakeParams::default()),
            });

        app.world_mut()
            .run_system_once(shake_event_handler_system)
            .unwrap();

        assert_eq!(app.world().get::<CameraShake>(a).unwrap().len(), 1);
        assert_eq!(app.world().get::<CameraShake>(b).unwrap().len(), 1);
    }

    #[test]
    fn event_handler_clear_清空() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ShakeSeedCounter>();
        app.add_event::<CameraShakeRequest>();

        let cam = app.world_mut().spawn(CameraShake::default()).id();
        {
            let mut shake = app.world_mut().get_mut::<CameraShake>(cam).unwrap();
            shake.add(ShakeParams::default(), 0);
            shake.add(ShakeParams::default(), 1);
        }

        app.world_mut()
            .resource_mut::<Events<CameraShakeRequest>>()
            .send(CameraShakeRequest {
                camera: Some(cam),
                op: CameraShakeOp::Clear,
            });

        app.world_mut()
            .run_system_once(shake_event_handler_system)
            .unwrap();

        assert!(app.world().get::<CameraShake>(cam).unwrap().is_empty());
    }
}
