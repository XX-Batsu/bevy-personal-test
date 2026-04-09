//! AssetManifest — manifest.ron 解析、查詢。

use crate::{AssetEntry, AssetId, ManifestError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 支援的 manifest 版本。
const SUPPORTED_VERSION: &str = "1.0.0";

/// 素材 Manifest — build 時由 manifest-gen 自動產生。
#[derive(Debug, Serialize, Deserialize)]
pub struct AssetManifest {
    /// Manifest 格式版本。
    ///
    /// ⚠ 僅供 manifest-gen 直接建構，runtime 使用 [`AssetManifest::parse()`] 取得。
    pub version: String,
    /// 產生時間（ISO 8601）。
    ///
    /// ⚠ 僅供 manifest-gen 直接建構，runtime 使用 [`AssetManifest::parse()`] 取得。
    pub generated_at: String,
    /// 所有素材 entry，以 AssetId 為 key，確定性排序。
    ///
    /// ⚠ 僅供 manifest-gen 直接建構，runtime 使用 [`AssetManifest::parse()`] 取得。
    /// 直接建構的 [`AssetEntry::blake3_bytes`] 為全零 `[0u8; 32]`；
    /// 若需 diff 比對，必須先 serialize 為 RON 再透過 `parse()` 重建。
    pub assets: BTreeMap<AssetId, AssetEntry>,
}

impl AssetManifest {
    /// 從 RON 字串解析 manifest。
    ///
    /// 解析後驗證：
    /// 1. 版本相容性
    /// 2. 重複 Asset ID 偵測（RON 的 BTreeMap 對重複 key 靜默取最後一筆，需手動驗證）
    /// 3. 每個 entry 的 blake3 hex 字串長度為 64
    /// 4. 轉換 blake3 hex → `[u8; 32]` 快取
    pub fn parse(ron_str: &str) -> Result<Self, ManifestError> {
        let mut manifest: Self = ron::from_str(ron_str)?;

        // 版本檢查
        if manifest.version != SUPPORTED_VERSION {
            return Err(ManifestError::VersionMismatch {
                expected: SUPPORTED_VERSION.to_string(),
                actual: manifest.version,
            });
        }

        // 重複 Asset ID 偵測：
        // RON BTreeMap 反序列化對重複 key 靜默取最後一筆。
        // 計算 ": AssetEntry(" 出現次數（前面有冒號+空格，大幅降低 value 中誤中的機率）。
        {
            let entry_count = ron_str.matches(": AssetEntry(").count();
            if entry_count != manifest.assets.len() {
                return Err(ManifestError::DuplicateAssetId(format!(
                    "manifest 含重複的 Asset ID（RON 出現 {} 個 entry，BTreeMap 僅保留 {} 個）",
                    entry_count,
                    manifest.assets.len()
                )));
            }
        }

        // blake3 hex 驗證與快取
        for (id, entry) in manifest.assets.iter_mut() {
            if entry.blake3.len() != 64 {
                return Err(ManifestError::InvalidBlake3 {
                    asset_id: id.to_string(),
                    reason: format!("期望 64 字元 hex，實際 {} 字元", entry.blake3.len()),
                });
            }
            let bytes = hex_to_bytes(&entry.blake3).map_err(|e| ManifestError::InvalidBlake3 {
                asset_id: id.to_string(),
                reason: e,
            })?;
            entry.blake3_bytes = bytes;
        }

        Ok(manifest)
    }

    /// 建立空 manifest（無素材）。
    pub fn empty() -> Self {
        Self {
            version: "1.0.0".to_string(),
            generated_at: String::new(),
            assets: BTreeMap::new(),
        }
    }

    /// 用 Asset ID 查詢。
    pub fn get(&self, id: &AssetId) -> Option<&AssetEntry> {
        self.assets.get(id)
    }

    /// 依 tag 批次查詢（回傳 iterator 避免不必要的 Vec 分配）。
    pub fn query_by_tag<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a AssetId> {
        self.assets
            .iter()
            .filter(move |(_, entry)| entry.tags.contains(tag))
            .map(|(id, _)| id)
    }

    /// 列出所有已註冊 ID。
    pub fn all_ids(&self) -> impl Iterator<Item = &AssetId> {
        self.assets.keys()
    }
}

use crate::diff::{ManifestDiff, ManifestDiffable};

impl ManifestDiffable for AssetManifest {
    type Id = AssetId;

    fn diff(&self, newer: &Self) -> ManifestDiff<AssetId> {
        let mut added = Vec::new();
        let mut updated = Vec::new();
        let mut removed = Vec::new();

        for (id, new_entry) in &newer.assets {
            match self.assets.get(id) {
                None => added.push(id.clone()),
                Some(old_entry) => {
                    if old_entry.blake3_bytes != new_entry.blake3_bytes {
                        updated.push(id.clone());
                    }
                }
            }
        }

        for id in self.assets.keys() {
            if !newer.assets.contains_key(id) {
                removed.push(id.clone());
            }
        }

        ManifestDiff {
            added,
            updated,
            removed,
        }
    }

    fn version(&self) -> &str {
        &self.version
    }
}

/// 將 64 字元 hex 字串轉為 [u8; 32]。
fn hex_to_bytes(hex: &str) -> Result<[u8; 32], String> {
    if hex.len() != 64 {
        return Err(format!("hex 長度必須為 64，實際 {}", hex.len()));
    }
    let mut bytes = [0u8; 32];
    for i in 0..32 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("hex 解析失敗 at byte {i}: {e}"))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::AssetFormat;

    fn sample_manifest_ron() -> String {
        r#"AssetManifest(
    version: "1.0.0",
    generated_at: "2026-04-09T12:00:00Z",
    assets: {
        "sprites/player/idle": AssetEntry(
            path: "sprites/player/idle.png",
            format: Png,
            size_bytes: 24576,
            blake3: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
            encrypted: false,
            sidecars: ["sprites/player/idle.ron"],
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
)"#
        .to_string()
    }

    #[test]
    fn test_parse_valid_manifest() {
        let manifest = AssetManifest::parse(&sample_manifest_ron()).unwrap();
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.assets.len(), 2);

        let player = manifest.get(&AssetId::new("sprites/player/idle")).unwrap();
        assert_eq!(player.path, "sprites/player/idle.png");
        assert_eq!(player.format, AssetFormat::Png);
        assert_eq!(player.size_bytes, 24576);
        assert!(!player.encrypted);
        assert_eq!(player.sidecars, vec!["sprites/player/idle.ron"]);
        assert!(player.tags.contains("player"));
        assert_eq!(player.blake3_bytes[0], 0xa1);
        assert_eq!(player.blake3_bytes[1], 0xb2);
    }

    #[test]
    fn test_parse_invalid_ron() {
        let result = AssetManifest::parse("not valid ron {{{");
        assert!(matches!(result, Err(ManifestError::ParseError(_))));
    }

    #[test]
    fn test_parse_version_mismatch() {
        let ron = r#"AssetManifest(
    version: "99.0.0",
    generated_at: "2026-04-09T12:00:00Z",
    assets: {},
)"#;
        let result = AssetManifest::parse(ron);
        assert!(matches!(result, Err(ManifestError::VersionMismatch { .. })));
    }

    #[test]
    fn test_parse_invalid_blake3_length() {
        let ron = r#"AssetManifest(
    version: "1.0.0",
    generated_at: "2026-04-09T12:00:00Z",
    assets: {
        "bad": AssetEntry(
            path: "bad.png",
            format: Png,
            size_bytes: 100,
            blake3: "tooshort",
            encrypted: false,
            sidecars: [],
            tags: [],
        ),
    },
)"#;
        let result = AssetManifest::parse(ron);
        assert!(matches!(result, Err(ManifestError::InvalidBlake3 { .. })));
    }

    #[test]
    fn test_parse_duplicate_id() {
        let ron = r#"AssetManifest(
    version: "1.0.0",
    generated_at: "2026-04-09T12:00:00Z",
    assets: {
        "same_id": AssetEntry(
            path: "a.png",
            format: Png,
            size_bytes: 100,
            blake3: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
            encrypted: false,
            sidecars: [],
            tags: [],
        ),
        "same_id": AssetEntry(
            path: "b.png",
            format: Png,
            size_bytes: 200,
            blake3: "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
            encrypted: false,
            sidecars: [],
            tags: [],
        ),
    },
)"#;
        let result = AssetManifest::parse(ron);
        assert!(matches!(result, Err(ManifestError::DuplicateAssetId(_))));
    }

    #[test]
    fn test_get_existing_id() {
        let manifest = AssetManifest::parse(&sample_manifest_ron()).unwrap();
        assert!(manifest.get(&AssetId::new("sprites/player/idle")).is_some());
    }

    #[test]
    fn test_get_missing_id() {
        let manifest = AssetManifest::parse(&sample_manifest_ron()).unwrap();
        assert!(manifest.get(&AssetId::new("nonexistent")).is_none());
    }

    #[test]
    fn test_query_by_tag() {
        let manifest = AssetManifest::parse(&sample_manifest_ron()).unwrap();
        let player_assets: Vec<_> = manifest.query_by_tag("player").collect();
        assert_eq!(player_assets.len(), 1);
        assert_eq!(player_assets[0].as_str(), "sprites/player/idle");
    }

    #[test]
    fn test_query_by_tag_empty() {
        let manifest = AssetManifest::parse(&sample_manifest_ron()).unwrap();
        let result: Vec<_> = manifest.query_by_tag("nonexistent").collect();
        assert!(result.is_empty());
    }

    #[test]
    fn test_all_ids_deterministic_order() {
        let manifest = AssetManifest::parse(&sample_manifest_ron()).unwrap();
        let ids: Vec<_> = manifest.all_ids().map(|id| id.as_str()).collect();
        assert_eq!(ids, vec!["audio/bgm/main_theme", "sprites/player/idle"]);
    }
}
