//! key_exchange 錯誤型別

/// ECDH 握手與 session key 相關錯誤
#[derive(Debug, thiserror::Error)]
pub enum KeyExchangeError {
    #[error("無效的 client 公鑰")]
    InvalidClientKey,

    #[error("加解密錯誤：{0}")]
    CryptoError(#[from] crypto::CryptoError),

    #[error("解碼錯誤：{0}")]
    DecodeError(String),

    #[error("序列化錯誤：{0}")]
    SerializationError(String),
}
