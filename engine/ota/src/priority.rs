//! OTA 優先級與下載項目定義。

use asset_manifest::AssetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical = 0,
    Normal = 1,
    Background = 2,
}

#[derive(Debug, Clone)]
pub enum OtaItem {
    Asset {
        id: AssetId,
        blake3: [u8; 32],
        size_bytes: u64,
    },
    /// Script OTA 項目（預留給未來 ScriptManifest 實作）。
    Script {
        id: AssetId, // 暫時用 AssetId，未來替換為 ScriptId
        blake3: [u8; 32],
    },
}

impl OtaItem {
    pub fn priority(&self) -> Priority {
        match self {
            OtaItem::Script { .. } => Priority::Critical,
            OtaItem::Asset { size_bytes, .. } => {
                if *size_bytes >= 1_048_576 {
                    Priority::Background
                } else {
                    Priority::Normal
                }
            }
        }
    }
}
