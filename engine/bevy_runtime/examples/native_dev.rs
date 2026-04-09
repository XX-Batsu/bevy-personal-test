//! Native 開發模式入口 — 桌面視窗 + 素材匯入 + 熱載入。
//!
//! 用法：
//!   cargo run --package bevy_runtime --example native_dev --features "hot-reload-native,debug-mode"
//!   或
//!   just dev-native

use asset_manifest::AssetManifest;
use bevy_runtime::{BevyAssetImportPlugin, WelcomePlugin};

use bevy::prelude::*;
use std::sync::Arc;

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

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "遊戲開發模式（Native）".to_string(),
                resolution: (1280.0, 720.0).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.102, 0.102, 0.180)))
        .add_plugins(WelcomePlugin)
        .add_plugins(BevyAssetImportPlugin {
            manifest,
            assets_root: "assets".to_string(),
        })
        .run();
}
