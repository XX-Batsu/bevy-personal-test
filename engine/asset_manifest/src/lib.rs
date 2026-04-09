//! 素材 Manifest 模組 — 零 Bevy 依賴，負責 manifest 解析、查詢與 OTA diff 計算。

pub mod asset_id;
pub mod diff;
pub mod entry;
pub mod error;
pub mod manifest;

pub use asset_id::AssetId;
pub use diff::{ManifestDiff, ManifestDiffable};
pub use entry::{AssetEntry, AssetFormat};
pub use error::ManifestError;
pub use manifest::AssetManifest;
