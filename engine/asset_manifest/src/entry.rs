//! AssetEntry 與 AssetFormat — 單一素材的 manifest entry。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 素材格式，依副檔名推斷。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetFormat {
    Png,
    WebP,
    Glb,
    Gltf,
    Ogg,
    Wav,
    Ttf,
    Otf,
    Ron,
    Ftl,
    WebM,
    Mp4,
    /// 未知副檔名的 fallback。
    Other(String),
}

impl AssetFormat {
    /// 從副檔名推斷格式（小寫比對）。
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "png" => Self::Png,
            "webp" => Self::WebP,
            "glb" => Self::Glb,
            "gltf" => Self::Gltf,
            "ogg" => Self::Ogg,
            "wav" => Self::Wav,
            "ttf" => Self::Ttf,
            "otf" => Self::Otf,
            "ron" => Self::Ron,
            "ftl" => Self::Ftl,
            "webm" => Self::WebM,
            "mp4" => Self::Mp4,
            other => {
                tracing::warn!("未知素材格式：{other}");
                Self::Other(other.to_string())
            }
        }
    }

    /// 判斷是否為主檔格式（圖片/音訊/模型/影片）。
    pub fn is_primary(&self) -> bool {
        matches!(
            self,
            Self::Png
                | Self::WebP
                | Self::Glb
                | Self::Gltf
                | Self::Ogg
                | Self::Wav
                | Self::Ttf
                | Self::Otf
                | Self::WebM
                | Self::Mp4
        )
    }
}

/// 單一素材的 manifest entry。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetEntry {
    pub path: String,
    pub format: AssetFormat,
    pub size_bytes: u64,
    pub blake3: String,
    /// blake3 hash 的位元組快取，由 [`AssetManifest::parse()`] 填入。
    ///
    /// ⚠ 直接建構（manifest-gen）時此欄位為全零 `[0u8; 32]`，勿用於 diff。
    #[serde(skip)]
    pub blake3_bytes: [u8; 32],
    pub encrypted: bool,
    pub sidecars: Vec<String>,
    pub tags: BTreeSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_from_known_extensions() {
        assert_eq!(AssetFormat::from_extension("png"), AssetFormat::Png);
        assert_eq!(AssetFormat::from_extension("PNG"), AssetFormat::Png);
        assert_eq!(AssetFormat::from_extension("ogg"), AssetFormat::Ogg);
        assert_eq!(AssetFormat::from_extension("glb"), AssetFormat::Glb);
        assert_eq!(AssetFormat::from_extension("ron"), AssetFormat::Ron);
    }

    #[test]
    fn test_format_from_unknown_extension() {
        let fmt = AssetFormat::from_extension("xyz");
        assert_eq!(fmt, AssetFormat::Other("xyz".to_string()));
    }

    #[test]
    fn test_is_primary_true_for_media() {
        assert!(AssetFormat::Png.is_primary());
        assert!(AssetFormat::Glb.is_primary());
        assert!(AssetFormat::Ogg.is_primary());
        assert!(AssetFormat::Mp4.is_primary());
    }

    #[test]
    fn test_is_primary_false_for_definition() {
        assert!(!AssetFormat::Ron.is_primary());
        assert!(!AssetFormat::Ftl.is_primary());
        assert!(!AssetFormat::Other("json".to_string()).is_primary());
    }
}
