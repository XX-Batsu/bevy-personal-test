//! Asset ID — 薄包裝，cheap clone。
//!
//! # ⚠ Hash derive 使用限制
//! Hash 是 content-based（Arc<str> 委託給 str::hash()），非 pointer-based。
//! **禁止**在遊戲邏輯中使用 `HashMap<AssetId, _>`——遊戲邏輯必須用 `BTreeMap` 確保確定性。

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::sync::Arc;

/// 素材 ID，由相對於 `assets/` 的路徑去掉副檔名推導。
/// 例如：`sprites/player/idle.png` → `"sprites/player/idle"`
#[derive(Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct AssetId(Arc<str>);

impl Serialize for AssetId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}

impl AssetId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetId(\"{}\")", self.0)
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for AssetId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_asset_id_equality() {
        let a = AssetId::new("sprites/player/idle");
        let b = AssetId::new("sprites/player/idle");
        assert_eq!(a, b);
    }

    #[test]
    fn test_asset_id_ordering() {
        let a = AssetId::new("audio/bgm");
        let b = AssetId::new("sprites/player");
        assert!(a < b, "audio < sprites 按字典序");
    }

    #[test]
    fn test_asset_id_btreemap_key() {
        let mut map = BTreeMap::new();
        map.insert(AssetId::new("a"), 1);
        map.insert(AssetId::new("b"), 2);
        assert_eq!(map.get(&AssetId::new("a")), Some(&1));
    }

    #[test]
    fn test_asset_id_clone_is_cheap() {
        let a = AssetId::new("sprites/player/idle");
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn test_asset_id_display() {
        let id = AssetId::new("sprites/player/idle");
        assert_eq!(format!("{id}"), "sprites/player/idle");
    }

    #[test]
    fn test_asset_id_from_str() {
        let id: AssetId = "audio/bgm".into();
        assert_eq!(id.as_str(), "audio/bgm");
    }
}
