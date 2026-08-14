//! Native 開發模式入口 — 桌面視窗 + 素材匯入 + 熱載入。
//!
//! 用法：
//!   cargo run --package bevy_runtime --example native_dev --features "hot-reload-native,debug-mode"
//!   或
//!   just dev-native

use asset_manifest::AssetManifest;
use bevy_runtime::{BevyAssetImportPlugin, WelcomePlugin};

use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use std::sync::Arc;

/// Native dev 模式的字型/素材根目錄（絕對路徑，避免 CARGO_MANIFEST_DIR 干擾）。
/// AssetServer 在 dev build 時會把 file_path 拼在 CARGO_MANIFEST_DIR 之後；
/// 但 POSIX 下 join(absolute) 直接取代前綴，故使用絕對路徑可跨工作目錄。
const CLIENT_ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../client/js/assets");

/// 素材根目錄解析：
/// - Release 包（執行檔旁有 `assets/fonts/`）：回傳相對路徑 `assets`，
///   由 Bevy 以執行檔目錄為 asset root 解析，因此可在任何機器上執行。
/// - 開發樹：回傳編譯期絕對路徑 [`CLIENT_ASSETS_DIR`]。
fn assets_dir() -> String {
    let exe_assets = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.join("assets")));

    match exe_assets {
        Some(path) if path.join("fonts").is_dir() => "assets".to_string(),
        _ => CLIENT_ASSETS_DIR.to_string(),
    }
}

fn main() {
    // 載入 manifest（若不存在則使用空 manifest）
    let manifest = match std::fs::read_to_string("assets/manifest.ron") {
        Ok(ron_str) => match AssetManifest::parse(&ron_str) {
            Ok(m) => {
                tracing::info!("Manifest 載入成功：{} 筆素材", m.assets.len());
                Arc::new(m)
            }
            Err(e) => {
                eprintln!("Manifest 解析失敗：{e}，使用空 manifest");
                Arc::new(AssetManifest::empty())
            }
        },
        Err(_) => {
            eprintln!("assets/manifest.ron 不存在，請先執行 just manifest");
            eprintln!("使用空 manifest 啟動...");
            Arc::new(AssetManifest::empty())
        }
    };

    // BevyAssetImportPlugin 必須在 DefaultPlugins 之前加入，
    // 才能在 AssetPlugin 初始化前完成 "managed://" source 的註冊。
    App::new()
        .add_plugins(BevyAssetImportPlugin {
            manifest,
            assets_root: "assets".to_string(),
        })
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    // 指向含 fonts/NotoSansTC-Regular.ttf 的素材根目錄，
                    // 開發樹用編譯期絕對路徑，release 包用執行檔旁的 assets/。
                    file_path: assets_dir(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "遊戲開發模式（Native）".to_string(),
                        resolution: (1280.0_f32, 720.0_f32).into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .insert_resource(ClearColor(Color::srgb(0.102, 0.102, 0.180)))
        .add_plugins(WelcomePlugin)
        .run();
}
