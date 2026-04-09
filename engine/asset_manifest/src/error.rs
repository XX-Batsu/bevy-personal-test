//! Manifest 錯誤型別。

use thiserror::Error;

/// Manifest 操作錯誤。
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("RON 解析失敗：{0}")]
    ParseError(#[from] ron::error::SpannedError),

    #[error("Manifest 版本不相容：期望 {expected}，實際 {actual}")]
    VersionMismatch { expected: String, actual: String },

    #[error("blake3 hash 格式錯誤（asset: {asset_id}）：{reason}")]
    InvalidBlake3 { asset_id: String, reason: String },

    #[error("重複的 Asset ID：{0}")]
    DuplicateAssetId(String),

    #[error("Asset 路徑指向不存在的檔案：{0}")]
    MissingFile(String),

    #[error("IO 錯誤：{0}")]
    IoError(#[from] std::io::Error),
}
