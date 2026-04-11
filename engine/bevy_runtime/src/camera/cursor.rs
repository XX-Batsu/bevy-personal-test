//! 滑鼠世界座標快取與座標轉換。
//!
//! 設計規格：`docs/design/2026-04-11-game-camera-design.md` §4, §5, §6.6

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::components::CameraFollow;

/// 每幀快取的滑鼠世界座標。由 `cursor_position_system` 在 Update 中寫入。
///
/// **渲染層限定**：僅供渲染層系統（攝影機、UI）使用。
/// 遊戲邏輯應透過 `RawPlayerInput` 取得滑鼠座標以確保確定性。
#[derive(Resource, Default)]
pub struct CursorWorldPosition {
    /// None = 滑鼠不在視窗內或無攝影機。
    pub position: Option<Vec2>,
}

/// 將視窗像素座標轉換為世界座標。
/// 供不想依賴 `CursorWorldPosition` Resource 的特殊場景使用。
pub fn viewport_to_world(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    viewport_pos: Vec2,
) -> Option<Vec2> {
    camera
        .viewport_to_world_2d(camera_transform, viewport_pos)
        .ok()
}

/// Update 系統：讀取視窗 cursor position，轉換為世界座標，寫入 `CursorWorldPosition`。
/// 使用 `With<CameraFollow>` filter 確保只處理遊戲攝影機。
pub fn cursor_position_system(
    camera_query: Query<(&Camera, &GlobalTransform), With<CameraFollow>>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    mut cursor_world: ResMut<CursorWorldPosition>,
) {
    let Ok(window) = window_query.get_single() else {
        cursor_world.position = None;
        return;
    };

    let Some(cursor_pos) = window.cursor_position() else {
        cursor_world.position = None;
        return;
    };

    let mut iter = camera_query.iter();
    let Some((camera, transform)) = iter.next() else {
        cursor_world.position = None;
        return;
    };

    if iter.next().is_some() {
        bevy::utils::warn_once!("偵測到多個帶 CameraFollow 的 Camera entity，取第一個");
    }

    cursor_world.position = viewport_to_world(camera, transform, cursor_pos);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_world_position_default_為_none() {
        let c = CursorWorldPosition::default();
        assert!(c.position.is_none());
    }

    #[test]
    fn viewport_to_world_呼叫不_panic() {
        let camera = Camera::default();
        let transform = GlobalTransform::default();
        let result = viewport_to_world(&camera, &transform, Vec2::new(100.0, 200.0));
        assert!(result.is_none());
    }

    #[test]
    fn cursor_position_system_無視窗時_position_為_none() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<CursorWorldPosition>();
        app.add_systems(Update, cursor_position_system);

        app.update();

        let cursor = app.world().resource::<CursorWorldPosition>();
        assert!(cursor.position.is_none(), "無視窗時應為 None");
    }
}
