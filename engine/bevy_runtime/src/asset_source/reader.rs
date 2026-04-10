//! ManagedAssetReader — Bevy AssetReader 實作，依 manifest 切換 Plain/Encrypted 讀取。
//!
//! 注意：目前只有 native 實作。WASM 的 PlainHttpReader（HTTP fetch）將在後續迭代加入。

use asset_manifest::AssetManifest;
use bevy::asset::io::{AssetReader, AssetReaderError, PathStream, VecReader};
use std::path::Path;
use std::sync::Arc;

// ── Native 實作 ──────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
use super::id_resolver::IdResolver;

#[cfg(not(target_arch = "wasm32"))]
/// Managed asset reader — 根據 manifest 的 encrypted flag 選擇讀取方式（Native）。
pub struct ManagedAssetReader {
    manifest: Arc<AssetManifest>,
    assets_root: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl ManagedAssetReader {
    pub fn new(manifest: Arc<AssetManifest>, assets_root: String) -> Self {
        Self {
            manifest,
            assets_root,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AssetReader for ManagedAssetReader {
    async fn read<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, AssetReaderError> {
        let asset_path = path.to_string_lossy();
        let resolver = IdResolver::new(&self.manifest);

        let resolved = resolver.resolve(&asset_path).ok_or_else(|| {
            tracing::warn!("素材不在 manifest 中：{asset_path}");
            AssetReaderError::NotFound(path.to_path_buf())
        })?;

        if resolved.entry.encrypted {
            // TODO: 整合 EncryptedAssetReader（後續迭代）
            tracing::warn!("加密素材載入尚未實作：{asset_path}");
            Err(AssetReaderError::NotFound(path.to_path_buf()))
        } else {
            let full_path = std::path::PathBuf::from(&self.assets_root).join(&resolved.entry.path);

            // Path traversal 防禦
            if let Ok(canonical) = full_path.canonicalize() {
                let root_canonical = std::path::PathBuf::from(&self.assets_root)
                    .canonicalize()
                    .unwrap_or_else(|_| std::path::PathBuf::from(&self.assets_root));
                if !canonical.starts_with(&root_canonical) {
                    tracing::error!(
                        "Path traversal 偵測：{} 超出 assets root",
                        full_path.display()
                    );
                    return Err(AssetReaderError::NotFound(path.to_path_buf()));
                }
            }

            let data = std::fs::read(&full_path).map_err(|e| {
                AssetReaderError::Io(std::sync::Arc::new(std::io::Error::other(format!(
                    "讀取素材失敗 {}: {e}",
                    full_path.display()
                ))))
            })?;
            Ok(VecReader::new(data))
        }
    }

    async fn read_meta<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, AssetReaderError> {
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, _path: &'a Path) -> Result<bool, AssetReaderError> {
        Ok(false)
    }
}

// ── WASM stub ────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
/// Managed asset reader — WASM stub。PlainHttpReader（HTTP fetch）將在後續迭代加入。
pub struct ManagedAssetReader {
    #[allow(dead_code)]
    manifest: Arc<AssetManifest>,
    #[allow(dead_code)]
    assets_root: String,
}

#[cfg(target_arch = "wasm32")]
impl ManagedAssetReader {
    pub fn new(manifest: Arc<AssetManifest>, assets_root: String) -> Self {
        Self {
            manifest,
            assets_root,
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl AssetReader for ManagedAssetReader {
    async fn read<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, AssetReaderError> {
        tracing::warn!("WASM PlainHttpReader 尚未實作");
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_meta<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, AssetReaderError> {
        tracing::warn!("WASM PlainHttpReader 尚未實作");
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        tracing::warn!("WASM PlainHttpReader 尚未實作");
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, _path: &'a Path) -> Result<bool, AssetReaderError> {
        tracing::warn!("WASM PlainHttpReader 尚未實作");
        Ok(false)
    }
}
