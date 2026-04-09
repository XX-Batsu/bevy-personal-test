//! 素材匯入系統 — ManagedAssetSource + BevyAssetImportPlugin。

pub mod id_resolver;
#[cfg(feature = "hot-reload-native")]
pub mod native_watcher;
pub mod reader;
#[cfg(all(target_arch = "wasm32", feature = "hot-reload-wasm"))]
pub mod wasm_watcher;

use asset_manifest::AssetManifest;
use bevy::asset::io::AssetSource;
use bevy::prelude::*;
use std::sync::Arc;

use asset_manifest::AssetId;

/// 素材變更監聽器共通介面。
/// Native 和 WASM 各自實作，ManagedAssetSource 透過此 trait 統一消費。
#[cfg(not(target_arch = "wasm32"))]
pub trait AssetWatcher: Send {
    /// 輪詢自上次呼叫以來的變更清單。
    fn poll_changes(&self) -> Vec<AssetId>;
    /// 停止監聽。
    fn stop(&mut self);
}

/// 素材變更監聯器共通介面（WASM 版本，不要求 Send + Sync）。
#[cfg(target_arch = "wasm32")]
pub trait AssetWatcher {
    /// 輪詢自上次呼叫以來的變更清單。
    fn poll_changes(&self) -> Vec<AssetId>;
    /// 停止監聽。
    fn stop(&mut self);
}

/// AssetManifest 的 Bevy Resource 包裝。
#[derive(Resource, Clone)]
pub struct AssetManifestRes(pub Arc<AssetManifest>);

/// 素材匯入 Plugin — 註冊 `managed://` AssetSource。
pub struct BevyAssetImportPlugin {
    pub manifest: Arc<AssetManifest>,
    pub assets_root: String,
}

impl Plugin for BevyAssetImportPlugin {
    fn build(&self, app: &mut App) {
        let manifest = self.manifest.clone();
        let assets_root = self.assets_root.clone();

        app.insert_resource(AssetManifestRes(self.manifest.clone()));

        app.register_asset_source(
            "managed",
            AssetSource::build().with_reader(move || {
                Box::new(reader::ManagedAssetReader::new(
                    manifest.clone(),
                    assets_root.clone(),
                ))
            }),
        );

        tracing::info!(
            "BevyAssetImportPlugin 已註冊：{} 筆素材",
            self.manifest.assets.len()
        );
    }
}
