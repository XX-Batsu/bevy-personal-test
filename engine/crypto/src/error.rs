use std::fmt;

/// 所有 crypto 操作的統一錯誤型別
///
/// 對齊 crypto-spec.md 定義，涵蓋四類加密原語的失敗模式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// 公鑰格式無效（如 all-zero、low-order point）
    InvalidPublicKey,
    /// AES-GCM 認證標籤驗證失敗（資料被竄改或金鑰錯誤）
    AuthTagMismatch,
    /// HKDF 衍生失敗
    HkdfError(String),
    /// 加密資料長度不足（需要至少 nonce + auth tag）
    CiphertextTooShort,
    /// 簽章驗證失敗
    SignatureVerificationFailed,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPublicKey => write!(f, "公鑰無效"),
            Self::AuthTagMismatch => write!(f, "AES-GCM 認證標籤驗證失敗"),
            Self::HkdfError(msg) => write!(f, "HKDF 衍生失敗：{msg}"),
            Self::CiphertextTooShort => write!(f, "加密資料長度不足"),
            Self::SignatureVerificationFailed => write!(f, "簽章驗證失敗"),
        }
    }
}

impl std::error::Error for CryptoError {}

impl From<aes_gcm::Error> for CryptoError {
    fn from(_: aes_gcm::Error) -> Self {
        Self::AuthTagMismatch
    }
}

impl From<hkdf::InvalidLength> for CryptoError {
    fn from(e: hkdf::InvalidLength) -> Self {
        Self::HkdfError(e.to_string())
    }
}

impl From<ed25519_dalek::SignatureError> for CryptoError {
    fn from(_: ed25519_dalek::SignatureError) -> Self {
        Self::SignatureVerificationFailed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_all_variants() {
        // 驗證所有 variant 的 Display 輸出皆非空
        let variants: Vec<CryptoError> = vec![
            CryptoError::InvalidPublicKey,
            CryptoError::AuthTagMismatch,
            CryptoError::HkdfError("test".into()),
            CryptoError::CiphertextTooShort,
            CryptoError::SignatureVerificationFailed,
        ];
        for v in &variants {
            let msg = format!("{v}");
            assert!(!msg.is_empty(), "Display 輸出不應為空：{v:?}");
        }
    }

    #[test]
    fn partial_eq_same_variant() {
        assert_eq!(
            CryptoError::CiphertextTooShort,
            CryptoError::CiphertextTooShort
        );
        assert_eq!(CryptoError::InvalidPublicKey, CryptoError::InvalidPublicKey);
        assert_eq!(CryptoError::AuthTagMismatch, CryptoError::AuthTagMismatch);
        assert_eq!(
            CryptoError::SignatureVerificationFailed,
            CryptoError::SignatureVerificationFailed
        );
        assert_eq!(
            CryptoError::HkdfError("x".into()),
            CryptoError::HkdfError("x".into())
        );
    }

    #[test]
    fn partial_eq_different_variants() {
        assert_ne!(CryptoError::AuthTagMismatch, CryptoError::InvalidPublicKey);
        assert_ne!(
            CryptoError::CiphertextTooShort,
            CryptoError::AuthTagMismatch
        );
        assert_ne!(
            CryptoError::HkdfError("a".into()),
            CryptoError::SignatureVerificationFailed
        );
    }

    #[test]
    fn clone_preserves_equality() {
        let original = CryptoError::HkdfError("test msg".into());
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn error_trait_impl() {
        let e: &dyn std::error::Error = &CryptoError::AuthTagMismatch;
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn from_aes_gcm_error() {
        // aes_gcm::Error 為 unit struct，可透過解密無效資料觸發
        let err = CryptoError::from(aes_gcm::Error);
        assert_eq!(err, CryptoError::AuthTagMismatch);
    }

    #[test]
    fn from_hkdf_invalid_length() {
        // 建構 hkdf::InvalidLength：嘗試 expand 至超長輸出
        use hkdf::Hkdf;
        use sha2::Sha256;

        let hk = Hkdf::<Sha256>::new(None, b"ikm");
        // HKDF-SHA256 最大輸出 = 255 * 32 = 8160 bytes，請求超過此值觸發 InvalidLength
        let mut too_long = vec![0u8; 8161];
        let hkdf_err = hk.expand(b"info", &mut too_long).unwrap_err();
        let crypto_err = CryptoError::from(hkdf_err);
        assert!(matches!(crypto_err, CryptoError::HkdfError(_)));
    }
}
