//! IdResolver — 將 Asset ID 轉為實際檔案路徑。

use asset_manifest::{AssetEntry, AssetId, AssetManifest};

/// 解析 Asset ID 到實際路徑與 metadata。
pub struct IdResolver<'a> {
    manifest: &'a AssetManifest,
}

/// 解析結果。
pub struct ResolvedAsset<'a> {
    pub entry: &'a AssetEntry,
}

impl<'a> IdResolver<'a> {
    pub fn new(manifest: &'a AssetManifest) -> Self {
        Self { manifest }
    }

    /// 從 Bevy asset path（如 `sprites/player/idle`）解析 manifest entry。
    pub fn resolve(&self, asset_path: &str) -> Option<ResolvedAsset<'a>> {
        let id = AssetId::new(asset_path);
        self.manifest.get(&id).map(|entry| ResolvedAsset { entry })
    }
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
                    tags: ["player"],
                ),
            },
        )"#;
        AssetManifest::parse(ron).unwrap()
    }

    #[test]
    fn test_resolve_existing_asset() {
        let manifest = sample_manifest();
        let resolver = IdResolver::new(&manifest);
        let resolved = resolver.resolve("sprites/player/idle").unwrap();
        assert_eq!(resolved.entry.path, "sprites/player/idle.png");
    }

    #[test]
    fn test_resolve_missing_asset() {
        let manifest = sample_manifest();
        let resolver = IdResolver::new(&manifest);
        assert!(resolver.resolve("nonexistent").is_none());
    }
}
