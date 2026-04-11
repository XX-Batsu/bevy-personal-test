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
pub const DEFAULT_VIEWPORT_HEIGHT: f32 = 720.0;

// ── CameraTarget ─────────────────────────────────────────────────────────────

/// 標記攝影機追蹤的目標 entity。
/// 場上應僅有一個 entity 帶此 marker。若有多個，系統取 `iter().next()` 並發出 `warn_once!`。
#[derive(Component)]
pub struct CameraTarget;

// ── CameraFollow ─────────────────────────────────────────────────────────────

/// 平滑追蹤行為 — 掛在攝影機 entity 上。
/// 每幀使用 frame-rate-independent 指數衰減追蹤 `CameraTarget` entity 位置。
/// 自動插入 `PreviousTargetPosition`（Required Component）。
#[derive(Component)]
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
#[derive(Component)]
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
#[derive(Component)]
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
#[derive(Component)]
pub struct CameraBounds {
    /// 左下角世界座標。
    pub min: Vec2,
    /// 右上角世界座標。
    pub max: Vec2,
}

// ── CameraCinematic ──────────────────────────────────────────────────────────

/// Cinematic 演出模式 — 掛上啟用，移除恢復正常追蹤。
/// 存在時，`CameraFollow` 和 `CameraLookAhead` 系統自動跳過。
#[derive(Component)]
pub struct CameraCinematic {
    /// 演出目標世界位置。
    pub target_position: Vec2,
    /// 演出目標 viewport_height。
    pub target_zoom: f32,
    /// 過渡衰減速度。內部公式：factor = 1.0 - exp(-speed * dt)
    pub speed: f32,
}

// ── PreviousTargetPosition ───────────────────────────────────────────────────

/// 上一幀目標位置 — 掛在攝影機 entity 上。
/// 由 `update_previous_target_system` 每幀更新（排在 look-ahead 之後）。
/// 首幀時 position 為 None，look-ahead 將 velocity 部分視為零向量。
#[derive(Component, Default)]
pub struct PreviousTargetPosition {
    pub position: Option<Vec2>,
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
}
