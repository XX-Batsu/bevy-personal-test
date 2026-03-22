//! AES-256-GCM 對稱加解密模組
//!
//! 提供兩層 API：
//! - **底層 API**：`encrypt` / `decrypt`（4 參數，顯式 nonce + AAD）
//! - **便利 API**：`aes_encrypt` / `aes_decrypt`（2 參數，自動 nonce + prepend，無 AAD）
//!
//! 密文格式（便利 API）：`nonce (12 bytes) || ciphertext || auth_tag (16 bytes)`

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng, Payload},
    AeadCore, Aes256Gcm, Nonce,
};

use crate::CryptoError;

// ── 底層 API（對齊 crypto-spec.md）──────────────────────────

/// AES-256-GCM 加密（底層 API）。
///
/// 對齊 crypto-spec.md 4 參數簽名，呼叫端自行管理 nonce 與 AAD。
///
/// # 參數
/// - `key`: 32 bytes AES-256 密鑰（來自 HKDF 衍生）
/// - `nonce`: 12 bytes nonce（應為隨機生成，不可重用）
/// - `plaintext`: 待加密資料
/// - `aad`: Additional Authenticated Data（傳 `&[]` 表示無 AAD）
///
/// # 回傳
/// - `Ok(Vec<u8>)`: ciphertext + 16 bytes auth tag（追加在末尾）
pub fn encrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 金鑰長度由 &[u8; 32] 保證正確");
    let nonce = Nonce::from_slice(nonce);
    let payload = Payload {
        msg: plaintext,
        aad,
    };
    cipher
        .encrypt(nonce, payload)
        .map_err(|_| CryptoError::AuthTagMismatch)
}

/// AES-256-GCM 解密與認證（底層 API）。
///
/// 對齊 crypto-spec.md 4 參數簽名，呼叫端自行管理 nonce 與 AAD。
///
/// # 參數
/// - `key`: 32 bytes AES-256 密鑰
/// - `nonce`: 12 bytes nonce（必須與加密時相同）
/// - `ciphertext_with_tag`: ciphertext + 16 bytes auth tag
/// - `aad`: Additional Authenticated Data（必須與加密時相同）
///
/// # 回傳
/// - `Ok(Vec<u8>)`: 解密後的 plaintext
/// - `Err(CryptoError::AuthTagMismatch)`: 認證失敗（資料被竄改或密鑰錯誤）
pub fn decrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext_with_tag: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 金鑰長度由 &[u8; 32] 保證正確");
    let nonce = Nonce::from_slice(nonce);
    let payload = Payload {
        msg: ciphertext_with_tag,
        aad,
    };
    cipher
        .decrypt(nonce, payload)
        .map_err(|_| CryptoError::AuthTagMismatch)
}

// ── 欄位分離 API（供 bytecode file format 使用）─────────────

/// AES-256-GCM 加密結果（拆分欄位，供 bytecode file format 使用）
///
/// 與 `aes_encrypt` 回傳的 `Vec<u8>`（nonce || ciphertext || tag 拼接）不同，
/// 此結構將三者分離，讓呼叫端可獨立控制各欄位的寫入位置。
#[derive(Debug, Clone, PartialEq)]
pub struct AesEncryptedParts {
    /// 12-byte nonce（由 OsRng 隨機生成，build tool 層非 game logic）
    pub nonce: [u8; 12],
    /// 密文（不含 tag）
    pub ciphertext: Vec<u8>,
    /// 16-byte authentication tag
    pub tag: [u8; 16],
}

/// 加密並回傳各部件（nonce、ciphertext、tag 分離）
///
/// 與 `aes_encrypt` 不同之處：回傳結構化的 `AesEncryptedParts`
/// 而非 `nonce || ciphertext || tag` 拼接的 `Vec<u8>`，
/// 讓 bytecode file format 可將各欄位寫入指定的 offset。
///
/// Nonce 由 `Aes256Gcm::generate_nonce(&mut OsRng)` 隨機生成
/// （build tool 層，不受 Determinism Rules 約束）。
/// WASM 環境下透過 `getrandom` 的 `js` feature 取得隨機數。
pub fn aes_encrypt_parts(
    key: &[u8; 32],
    plaintext: &[u8],
) -> Result<AesEncryptedParts, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 金鑰長度由 &[u8; 32] 保證正確");
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let encrypted = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| CryptoError::AuthTagMismatch)?;
    // aes-gcm encrypt 回傳 ciphertext || tag(16)
    let split_point = encrypted.len() - 16;
    let ciphertext = encrypted[..split_point].to_vec();
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&encrypted[split_point..]);
    let nonce_arr: [u8; 12] = nonce.into();
    Ok(AesEncryptedParts {
        nonce: nonce_arr,
        ciphertext,
        tag,
    })
}

/// 從各部件解密
///
/// 內部重組 `ciphertext || tag` 後呼叫 AES-GCM decrypt。
///
/// **簽名設計決策**：接受 `&AesEncryptedParts` struct 而非 crypto-spec.md
/// 定義的 3 參數 `(key, nonce, ciphertext_with_tag)`。兩者為不同抽象層次：
/// crypto-spec.md 描述密碼學原語介面，本函式為面向 bytecode format 的便利包裝，
/// 內部將 struct 欄位重組為 `ciphertext || tag` 後調用 AES-GCM decrypt。
/// 見設計決策表 D-1。
pub fn aes_decrypt_parts(
    key: &[u8; 32],
    parts: &AesEncryptedParts,
) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 金鑰長度由 &[u8; 32] 保證正確");
    let mut combined = Vec::with_capacity(parts.ciphertext.len() + 16);
    combined.extend_from_slice(&parts.ciphertext);
    combined.extend_from_slice(&parts.tag);
    let nonce = Nonce::from_slice(&parts.nonce);
    cipher
        .decrypt(nonce, combined.as_ref())
        .map_err(|_| CryptoError::AuthTagMismatch)
}

// ── 便利 API（自動 nonce + prepend）─────────────────────────

/// AES-256-GCM 加密（便利 API）。
///
/// 自動生成隨機 nonce 並 prepend 到密文前方，無 AAD。
/// 內部呼叫底層 `encrypt()` 實作。
///
/// # 密文格式
/// ```text
/// [nonce: 12 bytes] [ciphertext + auth_tag]
/// ```
///
/// # 參數
/// - `key`: AES-256 密鑰（32 bytes），來自 HKDF 衍生
/// - `plaintext`: 待加密的原文
///
/// # 回傳
/// - `Ok(Vec<u8>)`: nonce || ciphertext || auth_tag
pub fn aes_encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let nonce_arr: [u8; 12] = nonce.into();
    let ciphertext_with_tag = encrypt(key, &nonce_arr, plaintext, &[])?;
    let mut result = Vec::with_capacity(12 + ciphertext_with_tag.len());
    result.extend_from_slice(&nonce_arr);
    result.extend_from_slice(&ciphertext_with_tag);
    Ok(result)
}

/// AES-256-GCM 解密（便利 API）。
///
/// 從密文前方提取 nonce，無 AAD。
/// 內部呼叫底層 `decrypt()` 實作。
///
/// # 參數
/// - `key`: AES-256 密鑰（32 bytes）
/// - `ciphertext_with_nonce`: `[nonce: 12 bytes] [ciphertext + auth_tag]`
///
/// # 回傳
/// - `Ok(Vec<u8>)`: 解密後的原文
/// - `Err(CryptoError::CiphertextTooShort)`: 密文長度不足（< 12 bytes）
/// - `Err(CryptoError::AuthTagMismatch)`: 密鑰錯誤或密文被篡改
pub fn aes_decrypt(key: &[u8; 32], ciphertext_with_nonce: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if ciphertext_with_nonce.len() < 12 {
        return Err(CryptoError::CiphertextTooShort);
    }
    let (nonce_bytes, ciphertext_with_tag) = ciphertext_with_nonce.split_at(12);
    let nonce: &[u8; 12] = nonce_bytes.try_into().unwrap();
    decrypt(key, nonce, ciphertext_with_tag, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 底層 API 測試 ──────────────────────────────────────

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        // 4 參數 encrypt → decrypt，原文一致
        let key = [0x42u8; 32];
        let nonce = [0x01u8; 12];
        let plaintext = b"Hello";

        let ciphertext = encrypt(&key, &nonce, plaintext, &[]).expect("加密應成功");
        let decrypted = decrypt(&key, &nonce, &ciphertext, &[]).expect("解密應成功");
        assert_eq!(decrypted, plaintext, "解密後原文應一致");
    }

    #[test]
    fn test_encrypt_decrypt_with_aad() {
        // 含 AAD 加解密 round-trip 成功
        let key = [0x42u8; 32];
        let nonce = [0x01u8; 12];
        let plaintext = b"Hello";
        let aad = b"metadata";

        let ciphertext = encrypt(&key, &nonce, plaintext, aad).expect("加密應成功");
        let decrypted = decrypt(&key, &nonce, &ciphertext, aad).expect("含 AAD 解密應成功");
        assert_eq!(decrypted, plaintext, "含 AAD 解密後原文應一致");
    }

    #[test]
    fn test_decrypt_wrong_aad() {
        // 解密時使用不同 AAD → Err(AuthTagMismatch)
        let key = [0x42u8; 32];
        let nonce = [0x01u8; 12];
        let plaintext = b"Hello";

        let ciphertext = encrypt(&key, &nonce, plaintext, b"correct").expect("加密應成功");
        let result = decrypt(&key, &nonce, &ciphertext, b"wrong");
        assert_eq!(
            result,
            Err(CryptoError::AuthTagMismatch),
            "AAD 不一致應回傳 AuthTagMismatch"
        );
    }

    #[test]
    fn test_decrypt_wrong_key() {
        // 4 參數 API 使用錯誤密鑰解密 → Err(AuthTagMismatch)
        let key_a = [0x42u8; 32];
        let key_b = [0x99u8; 32];
        let nonce = [0x01u8; 12];
        let plaintext = b"Hello";

        let ciphertext = encrypt(&key_a, &nonce, plaintext, &[]).expect("加密應成功");
        let result = decrypt(&key_b, &nonce, &ciphertext, &[]);
        assert_eq!(
            result,
            Err(CryptoError::AuthTagMismatch),
            "錯誤密鑰應回傳 AuthTagMismatch"
        );
    }

    #[test]
    fn test_encrypt_output_length() {
        // 密文長度 = plaintext.len() + 16（auth tag）
        let key = [0x42u8; 32];
        let nonce = [0x01u8; 12];

        for len in [0, 1, 16, 100, 1024] {
            let plaintext = vec![0xABu8; len];
            let ciphertext = encrypt(&key, &nonce, &plaintext, &[]).expect("加密應成功");
            assert_eq!(
                ciphertext.len(),
                len + 16,
                "密文長度應為 plaintext({len}) + 16(auth tag)"
            );
        }
    }

    // ── 便利 API 測試 ──────────────────────────────────────

    #[test]
    fn test_aes_encrypt_decrypt_roundtrip() {
        // 加密後解密，原文一致
        let key = [0x42u8; 32];
        let plaintext = b"Hello, crypto!";

        let ciphertext = aes_encrypt(&key, plaintext).expect("便利加密應成功");
        let decrypted = aes_decrypt(&key, &ciphertext).expect("便利解密應成功");
        assert_eq!(decrypted, plaintext, "便利 API 解密後原文應一致");
    }

    #[test]
    fn test_aes_decrypt_wrong_key() {
        // 使用錯誤密鑰解密 → Err(AuthTagMismatch)
        let key_a = [0x42u8; 32];
        let key_b = [0x99u8; 32];
        let plaintext = b"Hello, crypto!";

        let ciphertext = aes_encrypt(&key_a, plaintext).expect("加密應成功");
        let result = aes_decrypt(&key_b, &ciphertext);
        assert_eq!(
            result,
            Err(CryptoError::AuthTagMismatch),
            "便利 API 錯誤密鑰應回傳 AuthTagMismatch"
        );
    }

    #[test]
    fn test_aes_decrypt_tampered_ciphertext() {
        // 竄改密文 → Err(AuthTagMismatch)
        let key = [0x42u8; 32];
        let plaintext = b"Hello, crypto!";

        let mut ciphertext = aes_encrypt(&key, plaintext).expect("加密應成功");
        // 竄改 nonce 之後的第一個 byte（ciphertext 部分）
        if ciphertext.len() > 12 {
            ciphertext[12] ^= 0xFF;
        }
        let result = aes_decrypt(&key, &ciphertext);
        assert_eq!(
            result,
            Err(CryptoError::AuthTagMismatch),
            "竄改密文應回傳 AuthTagMismatch"
        );
    }

    #[test]
    fn test_aes_decrypt_tampered_nonce() {
        // 竄改 nonce 部分 → Err(AuthTagMismatch)
        let key = [0x42u8; 32];
        let plaintext = b"Hello, crypto!";

        let mut ciphertext = aes_encrypt(&key, plaintext).expect("加密應成功");
        // 竄改前 12 bytes（nonce）的某個 byte
        ciphertext[0] ^= 0xFF;
        let result = aes_decrypt(&key, &ciphertext);
        assert_eq!(
            result,
            Err(CryptoError::AuthTagMismatch),
            "竄改 nonce 應回傳 AuthTagMismatch"
        );
    }

    #[test]
    fn test_aes_different_plaintexts_different_ciphertexts() {
        // 不同原文 → 不同密文（忽略 nonce 差異，只比較 ciphertext 部分）
        let key = [0x42u8; 32];
        let ct_a = aes_encrypt(&key, b"plaintext_a").expect("加密 a 應成功");
        let ct_b = aes_encrypt(&key, b"plaintext_b").expect("加密 b 應成功");
        assert_ne!(ct_a, ct_b, "不同原文應產生不同密文");
    }

    #[test]
    fn test_aes_nonce_uniqueness() {
        // 同一原文加密兩次 → 密文不同（隨機 nonce 保證）
        let key = [0x42u8; 32];
        let plaintext = b"same plaintext";
        let ct_1 = aes_encrypt(&key, plaintext).expect("第一次加密應成功");
        let ct_2 = aes_encrypt(&key, plaintext).expect("第二次加密應成功");
        assert_ne!(ct_1, ct_2, "隨機 nonce 應使同一原文產生不同密文");
    }

    #[test]
    fn test_aes_decrypt_too_short() {
        // 密文長度 < 12 bytes → Err(CiphertextTooShort)
        let key = [0x42u8; 32];
        let short = b"short"; // 5 bytes
        let result = aes_decrypt(&key, short);
        assert_eq!(
            result,
            Err(CryptoError::CiphertextTooShort),
            "長度不足 12 bytes 應回傳 CiphertextTooShort"
        );
    }

    #[test]
    fn test_aes_decrypt_exactly_12_bytes() {
        // 密文恰好 12 bytes（僅 nonce，無 ciphertext/tag）→ Err(AuthTagMismatch)
        let key = [0x42u8; 32];
        let exactly_12 = [0u8; 12];
        let result = aes_decrypt(&key, &exactly_12);
        assert_eq!(
            result,
            Err(CryptoError::AuthTagMismatch),
            "恰好 12 bytes 應回傳 AuthTagMismatch（無 ciphertext/tag）"
        );
    }

    #[test]
    fn test_aes_empty_plaintext() {
        // 空原文加解密 round-trip 成功
        let key = [0x42u8; 32];
        let plaintext = b"";

        let ciphertext = aes_encrypt(&key, plaintext).expect("空原文加密應成功");
        // 密文長度 = 12 (nonce) + 0 (plaintext) + 16 (tag) = 28
        assert_eq!(ciphertext.len(), 28, "空原文密文長度應為 28 bytes");
        let decrypted = aes_decrypt(&key, &ciphertext).expect("空原文解密應成功");
        assert_eq!(decrypted, plaintext.to_vec(), "空原文解密後應為空");
    }

    #[test]
    fn test_aes_large_plaintext() {
        // 1 MB 原文加解密 round-trip 成功
        let key = [0x42u8; 32];
        let plaintext = vec![0xABu8; 1_048_576];

        let ciphertext = aes_encrypt(&key, &plaintext).expect("大量資料加密應成功");
        let decrypted = aes_decrypt(&key, &ciphertext).expect("大量資料解密應成功");
        assert_eq!(decrypted, plaintext, "1 MB 原文解密後應一致");
    }

    #[test]
    fn test_aes_ciphertext_length() {
        // 加密後長度 = 12（nonce）+ plaintext.len() + 16（tag）
        let key = [0x42u8; 32];

        for len in [0, 1, 16, 100, 1024] {
            let plaintext = vec![0xABu8; len];
            let ciphertext = aes_encrypt(&key, &plaintext).expect("加密應成功");
            assert_eq!(
                ciphertext.len(),
                12 + len + 16,
                "便利 API 密文長度應為 12(nonce) + {len}(plaintext) + 16(tag)"
            );
        }
    }

    // ── 欄位分離 API 測試（Phase 5 Task 02）──────────────────

    #[test]
    fn encrypt_parts_decrypt_parts_round_trip() {
        let key = [42u8; 32];
        let plaintext = b"hello world bytecode payload";
        let parts = aes_encrypt_parts(&key, plaintext).unwrap();
        assert_eq!(parts.nonce.len(), 12);
        assert_eq!(parts.tag.len(), 16);
        // 驗證密文不等於明文（加密確實發生）
        assert_ne!(&parts.ciphertext[..], &plaintext[..]);
        let decrypted = aes_decrypt_parts(&key, &parts).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_parts_empty_plaintext() {
        let key = [42u8; 32];
        let plaintext = b"";
        let parts = aes_encrypt_parts(&key, plaintext).unwrap();
        assert!(parts.ciphertext.is_empty());
        assert_eq!(parts.nonce.len(), 12);
        assert_eq!(parts.tag.len(), 16);
        let decrypted = aes_decrypt_parts(&key, &parts).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_parts_large_plaintext() {
        let key = [42u8; 32];
        let plaintext = vec![0xAB_u8; 1_048_576]; // 1 MB
        let parts = aes_encrypt_parts(&key, &plaintext).unwrap();
        let decrypted = aes_decrypt_parts(&key, &parts).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails_parts() {
        let key = [42u8; 32];
        let parts = aes_encrypt_parts(&key, b"hello").unwrap();
        let wrong_key = [99u8; 32];
        let result = aes_decrypt_parts(&wrong_key, &parts);
        assert!(matches!(result, Err(CryptoError::AuthTagMismatch)));
    }

    #[test]
    fn tampered_ciphertext_fails_parts() {
        let key = [42u8; 32];
        let mut parts = aes_encrypt_parts(&key, b"hello world").unwrap();
        if !parts.ciphertext.is_empty() {
            parts.ciphertext[0] ^= 0xFF;
        }
        let result = aes_decrypt_parts(&key, &parts);
        assert!(matches!(result, Err(CryptoError::AuthTagMismatch)));
    }

    #[test]
    fn tampered_tag_fails_parts() {
        let key = [42u8; 32];
        let mut parts = aes_encrypt_parts(&key, b"hello world").unwrap();
        parts.tag[0] ^= 0xFF;
        let result = aes_decrypt_parts(&key, &parts);
        assert!(matches!(result, Err(CryptoError::AuthTagMismatch)));
    }

    #[test]
    fn encrypt_parts_nonce_uniqueness() {
        let key = [42u8; 32];
        let plaintext = b"same plaintext";
        let parts1 = aes_encrypt_parts(&key, plaintext).unwrap();
        let parts2 = aes_encrypt_parts(&key, plaintext).unwrap();
        // OsRng 產生的 nonce 應不同（碰撞機率 < 2^-96）
        assert_ne!(parts1.nonce, parts2.nonce);
    }
}
