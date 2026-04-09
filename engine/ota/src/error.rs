//! OTA 錯誤型別。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OtaError {
    #[error("Manifest 解析失敗：{0}")]
    ManifestParse(#[from] asset_manifest::ManifestError),

    #[error("簽名驗證失敗：{asset_id}")]
    SignatureInvalid { asset_id: String },

    #[error("下載失敗：{asset_id}（{reason}）")]
    DownloadFailed { asset_id: String, reason: String },

    #[error("OTA 記憶體預算超限：進行中 {current_bytes} bytes，上限 {budget_bytes} bytes")]
    MemoryBudgetExceeded {
        current_bytes: u64,
        budget_bytes: u64,
    },
}
