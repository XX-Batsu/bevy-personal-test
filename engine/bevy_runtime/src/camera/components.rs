//! 攝影機系統 Component 定義與預設常數。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md` §3, §8

use bevy::prelude::*;

// ── 預設常數 ─────────────────────────────────────────────────────────────────

pub const DEFAULT_FOLLOW_SPEED: f32 = 8.0;
pub const DEFAULT_MOUSE_WEIGHT: f32 = 0.3;
pub const DEFAULT_VELOCITY_WEIGHT: f32 = 0.5;
pub const DEFAULT_LOOK_AHEAD_MAX_OFFSET: f32 = 120.0;
pub const DEFAULT_ZOOM_MIN: f32 = 360.0;
pub const DEFAULT_ZOOM_MAX: f32 = 1440.0;
pub const DEFAULT_ZOOM_SCROLL_SPEED: f32 = 60.0;
pub const DEFAULT_ZOOM_SPEED: f32 = 8.0;
pub const DEFAULT_CINEMATIC_SPEED: f32 = 5.0;
/// 攝影機正交投影 viewport 高度（world units）的框架預設值。
///
/// 此為 framework 中性 default，**不**綁定特定螢幕解析度；遊戲層可透過
/// `Projection::Orthographic` 的 `ScalingMode::FixedVertical { viewport_height }`
/// 或 `CameraZoom { current, target, .. }` 在 spawn 時直接覆蓋。
///
/// 建議區間：300–2000（典型 2D 動作遊戲）；過小會造成可視範圍不足，過大則 zoom 響應遲緩。
pub const DEFAULT_VIEWPORT_HEIGHT: f32 = 720.0;

// ── Shake 常數（framework 中性，無遊戲語意） ──────────────────────────────

/// Perlin noise 採樣頻率（Hz）。每秒採樣此次數以產生 offset。演算法內部參數，不代表遊戲語意。
pub const SHAKE_FREQUENCY: f32 = 25.0;

/// `ShakeParams.decay_rate` 的 `Default` 值（trauma 每秒 linear 衰減量）。
pub const DEFAULT_SHAKE_DECAY_RATE: f32 = 2.0;

/// `ShakeEntry.direction_bias` 的 `Default` 值（沿 direction 軸乘數）。
pub const DEFAULT_SHAKE_DIRECTION_BIAS: f32 = 1.5;

/// `ShakeEntry.perpendicular_damping` 的 `Default` 值（垂直 direction 軸乘數）。
pub const DEFAULT_SHAKE_PERPENDICULAR_DAMPING: f32 = 0.3;

/// `max_strength = 0.0` 時的視口相對 fallback — viewport_height 的比例。
pub const DEFAULT_SHAKE_MAX_STRENGTH_RATIO: f32 = 0.035;

// ── CameraTarget ─────────────────────────────────────────────────────────────

/// 標記攝影機追蹤的目標 entity。
/// 場上應僅有一個 entity 帶此 marker。若有多個，系統取 `iter().next()` 並發出 `warn_once!`。
#[derive(Component, Debug, Clone)]
pub struct CameraTarget;

// ── CameraFollow ─────────────────────────────────────────────────────────────

/// 平滑追蹤行為 — 掛在攝影機 entity 上。
/// 每幀使用 frame-rate-independent 指數衰減追蹤 `CameraTarget` entity 位置。
/// 自動插入 `PreviousTargetPosition`（Required Component）。
#[derive(Component, Debug, Clone)]
#[require(PreviousTargetPosition)]
pub struct CameraFollow {
    /// 衰減速度（越大越快收斂）。0.0 = 不動，正無窮 = 瞬移。
    /// 內部公式：factor = 1.0 - exp(-speed * dt)
    pub speed: f32,
}

impl Default for CameraFollow {
    fn default() -> Self {
        Self {
            speed: DEFAULT_FOLLOW_SPEED,
        }
    }
}

// ── CameraLookAhead ──────────────────────────────────────────────────────────

/// Look-ahead 預看偏移 — 掛在攝影機 entity 上。
/// 基於滑鼠位置 + 目標移動方向的加權混合。
/// 需與 `CameraFollow` 搭配使用（依賴 `PreviousTargetPosition`）。
#[derive(Component, Debug, Clone)]
pub struct CameraLookAhead {
    /// 滑鼠偏移權重。計算：(滑鼠世界座標 - 目標位置) × 權重。
    pub mouse_weight: f32,
    /// 移動方向偏移權重。計算：(目標本幀位移向量) × 權重。
    pub velocity_weight: f32,
    /// 最大偏移量（世界單位）。混合後的偏移向量長度超過此值時 clamp。
    pub max_offset: f32,
}

impl Default for CameraLookAhead {
    fn default() -> Self {
        Self {
            mouse_weight: DEFAULT_MOUSE_WEIGHT,
            velocity_weight: DEFAULT_VELOCITY_WEIGHT,
            max_offset: DEFAULT_LOOK_AHEAD_MAX_OFFSET,
        }
    }
}

// ── CameraZoom ───────────────────────────────────────────────────────────────

/// 縮放控制 — 掛在攝影機 entity 上。
/// 支援滾輪（含 macOS 觸控板）和程式控制。
#[derive(Component, Debug, Clone)]
pub struct CameraZoom {
    /// 當前 viewport_height。
    pub current: f32,
    /// zoom in 上限（最小 viewport_height）。
    pub min: f32,
    /// zoom out 上限（最大 viewport_height）。
    pub max: f32,
    /// 每單位滾輪 delta 的 zoom 變化量。
    pub scroll_speed: f32,
    /// zoom 衰減速度。內部公式：factor = 1.0 - exp(-speed * dt)
    pub speed: f32,
    /// zoom 目標值。滾輪或程式碼設定此值，系統 lerp current → target。
    pub target: f32,
}

impl Default for CameraZoom {
    fn default() -> Self {
        Self {
            current: DEFAULT_VIEWPORT_HEIGHT,
            min: DEFAULT_ZOOM_MIN,
            max: DEFAULT_ZOOM_MAX,
            scroll_speed: DEFAULT_ZOOM_SCROLL_SPEED,
            speed: DEFAULT_ZOOM_SPEED,
            target: DEFAULT_VIEWPORT_HEIGHT,
        }
    }
}

// ── CameraBounds ─────────────────────────────────────────────────────────────

/// 邊界限制 — 掛在攝影機 entity 上。
/// 攝影機位置被 clamp 在 min..max 範圍內。
/// 不掛此 component 時無邊界限制。
#[derive(Component, Debug, Clone)]
pub struct CameraBounds {
    /// 左下角世界座標。
    pub min: Vec2,
    /// 右上角世界座標。
    pub max: Vec2,
    /// `true` = shake offset 也受 bounds clamp（嚴格邊界模式）。
    /// `false` = shake 可暫時越界（業界標準、預設）。
    pub clamp_shake: bool,
}

impl Default for CameraBounds {
    fn default() -> Self {
        Self {
            min: Vec2::ZERO,
            max: Vec2::ZERO,
            clamp_shake: false,
        }
    }
}

// ── PreviousTargetPosition ───────────────────────────────────────────────────

/// 上一幀目標位置 — 掛在攝影機 entity 上。
/// 由 `update_previous_target_system` 每幀更新（排在 look-ahead 之後）。
/// 首幀時 position 為 None，look-ahead 將 velocity 部分視為零向量。
#[derive(Component, Debug, Clone, Default)]
pub struct PreviousTargetPosition {
    pub position: Option<Vec2>,
}

// ── 工具函式 ─────────────────────────────────────────────────────────────────

/// 計算 frame-rate-independent 指數衰減因子。
///
/// 公式：`1.0 - exp(-speed * dt)`
///
/// - `speed`：衰減速度（越大越快收斂）。負值視為 0.0（不移動）。
/// - `dt`：delta time（呼叫者應先 cap 至 0.1）。負值視為 0.0。
///
/// 回傳值域 \[0.0, 1.0)，可直接用於 `lerp` 的 `t` 參數。
#[inline]
pub fn decay_factor(speed: f32, dt: f32) -> f32 {
    let speed = speed.max(0.0);
    let dt = dt.max(0.0);
    1.0 - (-speed * dt).exp()
}

// ── 測試 ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_follow_default_使用預設常數() {
        let f = CameraFollow::default();
        assert_eq!(f.speed, DEFAULT_FOLLOW_SPEED);
    }

    #[test]
    fn camera_look_ahead_default_使用預設常數() {
        let la = CameraLookAhead::default();
        assert_eq!(la.mouse_weight, DEFAULT_MOUSE_WEIGHT);
        assert_eq!(la.velocity_weight, DEFAULT_VELOCITY_WEIGHT);
        assert_eq!(la.max_offset, DEFAULT_LOOK_AHEAD_MAX_OFFSET);
    }

    #[test]
    fn camera_zoom_default_使用預設常數() {
        let z = CameraZoom::default();
        assert_eq!(z.current, DEFAULT_VIEWPORT_HEIGHT);
        assert_eq!(z.target, DEFAULT_VIEWPORT_HEIGHT);
        assert_eq!(z.min, DEFAULT_ZOOM_MIN);
        assert_eq!(z.max, DEFAULT_ZOOM_MAX);
        assert_eq!(z.scroll_speed, DEFAULT_ZOOM_SCROLL_SPEED);
        assert_eq!(z.speed, DEFAULT_ZOOM_SPEED);
    }

    #[test]
    fn previous_target_position_default_為_none() {
        let p = PreviousTargetPosition::default();
        assert!(p.position.is_none());
    }

    #[test]
    fn camera_follow_require_自動插入_previous_target_position() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.update(); // 初始化

        let entity = app
            .world_mut()
            .spawn((CameraFollow::default(), Transform::default()))
            .id();
        app.update(); // Required component 插入

        assert!(
            app.world().get::<PreviousTargetPosition>(entity).is_some(),
            "#[require] 應自動插入 PreviousTargetPosition"
        );
    }

    #[test]
    fn decay_factor_負值_speed_回傳零() {
        let factor = decay_factor(-5.0, 1.0 / 60.0);
        assert!(
            factor.abs() < f32::EPSILON,
            "負值 speed 應被視為 0.0，factor 應為 0，實際: {factor}"
        );
    }

    #[test]
    fn decay_factor_負值_dt_回傳零() {
        let factor = decay_factor(8.0, -0.1);
        assert!(
            factor.abs() < f32::EPSILON,
            "負值 dt 應被視為 0.0，factor 應為 0，實際: {factor}"
        );
    }
}
