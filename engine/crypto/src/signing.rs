// engine/crypto/src/signing.rs

use crate::CryptoError;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;

/// Ed25519 簽章金鑰對。
///
/// 內部 `SigningKey` 在 drop 時自動清零（`ed25519_dalek::SigningKey`
/// 已實作 `Zeroize` + `Drop`，無需手動實作）。
/// RNG 使用 `OsRng`（基礎設施層，非 game logic，不受 Determinism Rules 約束）。
pub struct SigningKeyPair {
    signing_key: SigningKey,
}

// 注意：不需手動 impl Drop。`ed25519_dalek::SigningKey`（v2）內部已實作
// Zeroize + Drop，在 `SigningKeyPair` drop 時自動清零 signing_key 欄位。

impl SigningKeyPair {
    /// 生成新的 Ed25519 簽章金鑰對。
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self { signing_key }
    }

    /// 從已知的 secret key bytes（32-byte seed）建立金鑰對。
    /// Phase 5 (bytecode) 需要此方法載入預設的簽章金鑰。
    ///
    /// `ed25519_dalek::SigningKey::from_bytes()` 接受 `&[u8; 32]`，
    /// 任何 32 bytes 都是合法的 Ed25519 seed（不會失敗），因此此方法
    /// 不回傳 `Result`。型別系統已保證長度為 32 bytes，無需額外驗證。
    pub fn from_bytes(secret_key: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(secret_key);
        Self { signing_key }
    }

    /// 取得公鑰（32 bytes），用於分發給驗證方。
    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// 簽署資料，回傳 64-byte 簽章。
    ///
    /// Ed25519 簽章為**確定性**（RFC 8032）：相同金鑰 + 相同資料 = 相同簽章。
    /// 不需要 RNG 參與簽章過程（僅 `generate()` 需要 OsRng）。
    ///
    /// # 參數
    /// - `data`: 待簽署的資料（任意長度，含空資料）
    ///
    /// # 回傳
    /// - `[u8; 64]`: Ed25519 簽章
    pub fn sign(&self, data: &[u8]) -> [u8; 64] {
        let signature = self.signing_key.sign(data);
        signature.to_bytes()
    }
}

/// 驗證 Ed25519 簽章。
///
/// # 參數
/// - `public_key`: 簽署方的公鑰（32 bytes）
/// - `data`: 被簽署的原始資料
/// - `signature`: 64-byte 簽章
///
/// # 回傳
/// - `Ok(())`: 簽章有效
/// - `Err(CryptoError::SignatureVerificationFailed)`: 簽章無效
/// - `Err(CryptoError::InvalidPublicKey)`: 公鑰格式無效
pub fn verify_signature(
    public_key: &[u8; 32],
    data: &[u8],
    signature: &[u8; 64],
) -> Result<(), CryptoError> {
    let verifying_key =
        VerifyingKey::from_bytes(public_key).map_err(|_| CryptoError::InvalidPublicKey)?;
    let sig = ed25519_dalek::Signature::from_bytes(signature);
    verifying_key
        .verify(data, &sig)
        .map_err(|_| CryptoError::SignatureVerificationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 簽署後驗證成功
    #[test]
    fn test_sign_verify_roundtrip() {
        let key_pair = SigningKeyPair::generate();
        let data = b"hello";
        let signature = key_pair.sign(data);
        let public_key = key_pair.public_key();

        let result = verify_signature(&public_key, data, &signature);
        assert_eq!(result, Ok(()));
    }

    /// 錯誤公鑰驗證 → SignatureVerificationFailed
    #[test]
    fn test_verify_wrong_public_key() {
        let key_a = SigningKeyPair::generate();
        let key_b = SigningKeyPair::generate();
        let data = b"hello";
        let signature = key_a.sign(data);

        let result = verify_signature(&key_b.public_key(), data, &signature);
        assert_eq!(result, Err(CryptoError::SignatureVerificationFailed));
    }

    /// 竄改資料後驗證 → SignatureVerificationFailed
    #[test]
    fn test_verify_tampered_data() {
        let key_pair = SigningKeyPair::generate();
        let data = b"hello";
        let signature = key_pair.sign(data);

        let result = verify_signature(&key_pair.public_key(), b"world", &signature);
        assert_eq!(result, Err(CryptoError::SignatureVerificationFailed));
    }

    /// 空資料簽署 → 有效簽章，驗證成功
    #[test]
    fn test_sign_empty_data() {
        let key_pair = SigningKeyPair::generate();
        let data = b"";
        let signature = key_pair.sign(data);

        let result = verify_signature(&key_pair.public_key(), data, &signature);
        assert_eq!(result, Ok(()));
    }

    /// from_bytes 建立的金鑰對與原始金鑰對產生相同簽章（Ed25519 確定性特性）
    #[test]
    fn test_from_bytes_roundtrip() {
        let key_pair = SigningKeyPair::generate();
        let secret_bytes = key_pair.signing_key.to_bytes();
        let restored = SigningKeyPair::from_bytes(&secret_bytes);

        let data = b"deterministic signing test";
        let sig_original = key_pair.sign(data);
        let sig_restored = restored.sign(data);

        assert_eq!(sig_original, sig_restored);
        assert_eq!(key_pair.public_key(), restored.public_key());
    }

    /// 不同金鑰簽署相同資料 → 簽章不同
    #[test]
    fn test_sign_different_keys_different_signatures() {
        let key_a = SigningKeyPair::generate();
        let key_b = SigningKeyPair::generate();
        let data = b"hello";

        let sig_a = key_a.sign(data);
        let sig_b = key_b.sign(data);

        assert_ne!(sig_a, sig_b);
    }

    /// 公鑰長度為 32 bytes（型別保證，編譯期檢查）
    #[test]
    fn test_public_key_length() {
        let key_pair = SigningKeyPair::generate();
        let pk: [u8; 32] = key_pair.public_key();
        assert_eq!(pk.len(), 32);
    }

    /// 簽章長度為 64 bytes（型別保證，編譯期檢查）
    #[test]
    fn test_signature_length() {
        let key_pair = SigningKeyPair::generate();
        let sig: [u8; 64] = key_pair.sign(b"data");
        assert_eq!(sig.len(), 64);
    }
}
