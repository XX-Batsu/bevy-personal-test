//! Rhai Bridge — 素材調用 API（asset_load, asset_ready, asset_info, asset_load_tag）。
//!
//! Rhai script 透過 opaque u64 token 操作素材 Handle，不暴露 Bevy 內部。
//! 遵循現有 bridge error convention：不拋 Rhai runtime error，回傳特殊值 + tracing::warn!。

use asset_manifest::{AssetId, AssetManifest};
use bevy::prelude::*;
use std::collections::BTreeMap;

/// Rhai token ↔ Bevy UntypedHandle 映射表。
///
/// Token counter 為普通 u64 field（非全域 AtomicU64），
/// 確保 deterministic replay 時 token 分配一致。
#[derive(Resource)]
pub struct AssetHandleMap {
    token_to_handle: BTreeMap<u64, UntypedHandle>,
    id_to_token: BTreeMap<AssetId, u64>,
    next_token: u64,
}

impl Default for AssetHandleMap {
    fn default() -> Self {
        Self {
            token_to_handle: BTreeMap::new(),
            id_to_token: BTreeMap::new(),
            next_token: 1,
        }
    }
}

impl AssetHandleMap {
    /// 載入素材並回傳 token。若已載入則回傳既有 token。
    pub fn load_or_get(&mut self, asset_id: &AssetId, asset_server: &AssetServer) -> u64 {
        if let Some(&token) = self.id_to_token.get(asset_id) {
            return token;
        }

        let path = format!("managed://{}", asset_id.as_str());
        let handle = asset_server.load_untyped(&path);
        let token = self.next_token;
        self.next_token += 1;

        self.token_to_handle.insert(token, handle.into());
        self.id_to_token.insert(asset_id.clone(), token);
        token
    }

    /// 查詢 token 對應的 handle。
    pub fn get_handle(&self, token: u64) -> Option<&UntypedHandle> {
        self.token_to_handle.get(&token)
    }

    /// 重置映射表與 token counter（場景載入時呼叫，確保 deterministic replay 一致）。
    pub fn reset(&mut self) {
        self.token_to_handle.clear();
        self.id_to_token.clear();
        self.next_token = 1;
    }
}

/// Rhai `asset_load("sprites/player/idle")` → u64 token。
/// 無效 ID 回傳 0。
pub fn rhai_asset_load(
    asset_id_str: &str,
    handle_map: &mut AssetHandleMap,
    manifest: &AssetManifest,
    asset_server: &AssetServer,
) -> u64 {
    let asset_id = AssetId::new(asset_id_str);
    if manifest.get(&asset_id).is_none() {
        tracing::warn!("asset_load：素材不在 manifest 中：{asset_id_str}");
        return 0;
    }
    handle_map.load_or_get(&asset_id, asset_server)
}

/// Rhai `asset_ready(token)` → bool。
/// 無效 token 回傳 false。
pub fn rhai_asset_ready(
    token: u64,
    handle_map: &AssetHandleMap,
    asset_server: &AssetServer,
) -> bool {
    if token == 0 {
        return false;
    }
    match handle_map.get_handle(token) {
        Some(handle) => asset_server.is_loaded_with_dependencies(handle.id()),
        None => {
            tracing::warn!("asset_ready：無效 token {token}");
            false
        }
    }
}

/// Rhai `asset_info("sprites/player/idle")` → Rhai Dynamic map。
/// 不存在的 ID 回傳空 map。
pub fn rhai_asset_info(asset_id_str: &str, manifest: &AssetManifest) -> rhai::Map {
    let asset_id = AssetId::new(asset_id_str);
    match manifest.get(&asset_id) {
        Some(entry) => {
            let mut map = rhai::Map::new();
            map.insert("format".into(), format!("{:?}", entry.format).into());
            map.insert("size_bytes".into(), (entry.size_bytes as i64).into());
            map.insert("encrypted".into(), entry.encrypted.into());
            let tags: rhai::Array = entry
                .tags
                .iter()
                .map(|t| rhai::Dynamic::from(t.clone()))
                .collect();
            map.insert("tags".into(), tags.into());
            map
        }
        None => {
            tracing::warn!("asset_info：素材不在 manifest 中：{asset_id_str}");
            rhai::Map::new()
        }
    }
}

/// Rhai `asset_load_tag("player")` → [token, ...]。
pub fn rhai_asset_load_tag(
    tag: &str,
    handle_map: &mut AssetHandleMap,
    manifest: &AssetManifest,
    asset_server: &AssetServer,
) -> Vec<u64> {
    manifest
        .query_by_tag(tag)
        .map(|id| handle_map.load_or_get(id, asset_server))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> AssetManifest {
        let ron = r#"AssetManifest(
            version: "1.0.0",
            generated_at: "2026-04-09T12:00:00Z",
            assets: {
                "sprites/player/idle": AssetEntry(
                    path: "sprites/player/idle.png",
                    format: Png,
                    size_bytes: 24576,
                    blake3: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
                    encrypted: false,
                    sidecars: [],
                    tags: ["player", "animation"],
                ),
                "audio/bgm/main_theme": AssetEntry(
                    path: "audio/bgm/main_theme.ogg",
                    format: Ogg,
                    size_bytes: 1048576,
                    blake3: "d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5",
                    encrypted: false,
                    sidecars: [],
                    tags: ["bgm"],
                ),
            },
        )"#;
        AssetManifest::parse(ron).unwrap()
    }

    #[test]
    fn test_rhai_asset_info_existing() {
        let manifest = sample_manifest();
        let info = rhai_asset_info("sprites/player/idle", &manifest);
        assert_eq!(
            info.get("format").unwrap().clone().into_string().unwrap(),
            "Png"
        );
        assert_eq!(
            info.get("size_bytes").unwrap().clone().as_int().unwrap(),
            24576
        );
    }

    #[test]
    fn test_rhai_asset_info_missing() {
        let manifest = sample_manifest();
        let info = rhai_asset_info("nonexistent", &manifest);
        assert!(info.is_empty());
    }

    #[test]
    fn test_token_counter_increments() {
        let mut map = AssetHandleMap::default();
        let t1 = map.next_token;
        map.next_token += 1;
        let t2 = map.next_token;
        map.next_token += 1;
        assert_eq!(t1, 1);
        assert_eq!(t2, 2);
    }

    #[test]
    fn test_reset_token_counter() {
        let mut map = AssetHandleMap::default();
        map.next_token += 1;
        map.next_token += 1;
        map.reset();
        assert_eq!(map.next_token, 1);
    }
}
