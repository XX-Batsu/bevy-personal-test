// engine/bevy_runtime — 歡迎畫面：AppState、WelcomePlugin、start_welcome_app

use bevy::prelude::*;
use bevy::render::camera::ScalingMode;

/// 遊戲主狀態機。Welcome 為初始狀態（#[default]），InGame 保留供後續使用。
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Welcome,
    InGame, // 保留，現在不使用
}

/// Marker component：所有歡迎畫面 entity 攜帶此元件，供 despawn 系統清除。
#[derive(Component)]
pub struct WelcomeScreen;

/// 歡迎畫面插件。
///
/// 排程：
/// - `Startup` → `spawn_welcome_screen`：生成 Camera2d + World-space Text2d
/// - `OnExit(Welcome)` → `despawn_welcome_screen`：清除所有 WelcomeScreen entity
/// - `Update`（run_if Welcome）→ `on_any_key`：偵測按鍵，僅 log
pub struct WelcomePlugin;

impl Plugin for WelcomePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            // Startup：確保在第一幀無條件執行，避免 OnEnter 在 WASM DefaultPlugins
            // 下因初始 state 無 transition 而不觸發的問題。
            .add_systems(Startup, spawn_welcome_screen)
            .add_systems(OnExit(AppState::Welcome), despawn_welcome_screen)
            .add_systems(Update, on_any_key.run_if(in_state(AppState::Welcome)));
    }
}

fn spawn_welcome_screen(
    mut commands: Commands,
    asset_server: Option<Res<AssetServer>>,
    mut font_assets: Option<ResMut<Assets<Font>>>,
) {
    tracing::info!("歡迎畫面：生成 World-space Text2d entity");

    // WASM：字型 include_bytes! 編入 binary，零 HTTP 請求。
    // Native/測試：AssetServer 從檔案系統載入（MinimalPlugins 下 fallback default handle）。
    let cjk_font = load_cjk_font(asset_server.as_deref(), font_assets.as_deref_mut());

    // Camera2d 固定 720 world units 高（對應 16:9 → 1280 wide）。
    // ScalingMode::FixedVertical 確保：在任何視窗尺寸下，文字都佔畫面相同比例。
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 720.0,
            },
            ..OrthographicProjection::default_2d()
        }),
        WelcomeScreen,
    ));

    // 標題（ASCII，使用內嵌 FiraMono）
    // y=+160：上半部約 44% 處（160/360 ≈ 0.44），在任何 16:9 尺寸下可見
    commands.spawn((
        Text2d::new("BEVY GAME"),
        TextFont {
            font_size: 80.0,
            ..default()
        },
        TextColor(Color::WHITE),
        Transform::from_xyz(0.0, 160.0, 0.0),
        WelcomeScreen,
    ));

    // 提示（CJK，使用 Noto Sans TC）
    // y=-80：下半部約 22% 處（80/360 ≈ 0.22）
    commands.spawn((
        Text2d::new("按任意鍵開始"),
        TextFont {
            font: cjk_font,
            font_size: 36.0,
            ..default()
        },
        TextColor(Color::srgb(0.75, 0.75, 0.75)),
        Transform::from_xyz(0.0, -80.0, 0.0),
        WelcomeScreen,
    ));
}

/// CJK 字型載入策略：
/// - wasm32：`include_bytes!` 編入 binary（零 HTTP，首次開啟即可用）
/// - native/測試：AssetServer 從 `assets/fonts/` 載入（`Assets<Font>` 不存在時 fallback default）
fn load_cjk_font(
    _asset_server: Option<&AssetServer>,
    #[allow(unused_variables)] font_assets: Option<&mut Assets<Font>>,
) -> Handle<Font> {
    #[cfg(target_arch = "wasm32")]
    {
        // 需先執行 `just download-fonts`，字型在 WASM build 時編入 binary。
        static BYTES: &[u8] =
            include_bytes!("../../../client/js/assets/fonts/NotoSansTC-Regular.ttf");
        if let Some(fa) = font_assets {
            match Font::try_from_bytes(BYTES.to_vec()) {
                Ok(font) => return fa.add(font),
                Err(e) => tracing::error!("內嵌 CJK 字型解析失敗：{e:?}"),
            }
        }
        return Handle::default();
    }
    // native / 測試：AssetServer 載入
    #[allow(unreachable_code)]
    _asset_server
        .map(|s| s.load("fonts/NotoSansTC-Regular.ttf"))
        .unwrap_or_default()
}

fn despawn_welcome_screen(mut commands: Commands, query: Query<Entity, With<WelcomeScreen>>) {
    for entity in &query {
        commands.entity(entity).despawn_recursive();
    }
}

fn on_any_key(keyboard: Option<Res<ButtonInput<KeyCode>>>) {
    let Some(keyboard) = keyboard else { return };
    if keyboard.get_just_pressed().next().is_some() {
        tracing::info!("偵測到按鍵輸入（Welcome 狀態）");
    }
}

/// 建立並執行 Bevy 歡迎畫面 App。
///
/// - WASM：WinitPlugin 接管 rAF，此函式非阻塞返回。
/// - Native：開啟視窗（開發用途），`canvas` 參數被忽略。
///
/// 須透過 `wasm_start_game()` 呼叫，且只能呼叫一次。
pub fn start_welcome_app(canvas: &str) {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                canvas: Some(canvas.to_owned()),
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.102, 0.102, 0.180)))
        .add_plugins(super::FixedUpdatePlugin)
        .add_plugins(super::InputPlugin)
        .add_plugins(WelcomePlugin)
        .run();
}

// ── 測試 ──────────────────────────────────────────────────────────────────
//
// 使用 MinimalPlugins 驗證系統邏輯，不測渲染結果。
// WelcomeScreen entity 的存在與消滅是驗重點。

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin));
        app.add_plugins(WelcomePlugin);
        app
    }

    #[test]
    fn test_app_state_default_is_welcome() {
        assert_eq!(AppState::default(), AppState::Welcome);
    }

    #[test]
    fn test_welcome_plugin_builds_without_panic() {
        let _app = build_test_app();
    }

    /// Startup 應生成 3 個帶 WelcomeScreen 的 entity：
    /// Camera2d、標題 Text2d、提示 Text2d。
    #[test]
    fn test_spawn_welcome_screen_creates_three_entities() {
        let mut app = build_test_app();
        app.update(); // Startup → spawn_welcome_screen

        let mut q = app.world_mut().query::<&WelcomeScreen>();
        let count = q.iter(app.world()).count();
        assert_eq!(
            count, 3,
            "應生成 Camera2d + 標題 + 提示共 3 個 WelcomeScreen entity"
        );
    }

    /// OnExit(Welcome) 應清除所有 WelcomeScreen entity。
    #[test]
    fn test_despawn_welcome_screen_clears_all_entities() {
        let mut app = build_test_app();
        app.update(); // Startup → spawn

        {
            let mut next_state = app.world_mut().resource_mut::<NextState<AppState>>();
            next_state.set(AppState::InGame);
        }
        app.update(); // OnExit(Welcome) → despawn_welcome_screen
        app.update(); // Commands apply

        let mut q = app.world_mut().query::<&WelcomeScreen>();
        let count = q.iter(app.world()).count();
        assert_eq!(count, 0, "despawn 後應無 WelcomeScreen entity");
    }
}
