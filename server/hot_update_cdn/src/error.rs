//! hot_update_cdn 錯誤型別

/// OTA 分片與廣播相關錯誤
#[derive(Debug, thiserror::Error)]
pub enum HotUpdateError {
    #[error("bytecode 不可為空")]
    EmptyBytecode,

    #[error("分片不完整：預期 {expected}，收到 {got}")]
    MissingChunks { expected: u16, got: u16 },

    #[error("Ed25519 簽章驗證失敗")]
    SignatureVerificationFailed,

    #[error("重組後 bytecode 超過上限：{size} bytes（上限 32 MB）")]
    BytecodeTooLarge { size: usize },
}
