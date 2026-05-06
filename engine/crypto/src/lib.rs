pub mod aes;
pub mod ecdh;
pub mod error;
pub mod frame;
pub mod kdf;
pub mod signing;

pub use aes::{
    aes_decrypt, aes_decrypt_parts, aes_encrypt, aes_encrypt_parts, decrypt, encrypt,
    AesEncryptedParts,
};
pub use ecdh::EcdhKeyPair;
pub use error::CryptoError;
pub use frame::{decrypt_frame, decrypt_frame_aad, encrypt_frame, encrypt_frame_aad, FrameError};
pub use kdf::{build_hkdf_salt, derive_session_key, HKDF_INFO};
pub use signing::{verify_signature, SigningKeyPair};

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::Zeroize;

    /// 驗證所有 re-export 項皆可直接存取（編譯期保證）
    ///
    /// **設計決策**：刻意以 fn pointer type assertion 形式撰寫，除驗證 re-export
    /// 可達性外，亦保證每個函式的**簽名與預期一致**。對 crypto 這類安全敏感模組，
    /// 簽名變更若意外發生（例如改參數順序）此測試會編譯失敗，提供額外防護。
    /// 因此為此測試 opt-out clippy::type_complexity（簽名複雜度為設計意圖，非疏失）。
    #[test]
    #[allow(clippy::type_complexity)]
    fn test_reexport_availability() {
        // 若任一 re-export 遺漏，此測試無法編譯
        let _: fn(&[u8; 32], &[u8]) -> Result<Vec<u8>, CryptoError> = aes_encrypt;
        let _: fn(&[u8; 32], &[u8]) -> Result<Vec<u8>, CryptoError> = aes_decrypt;
        let _: fn(&[u8; 32], &[u8], &[u8]) -> Result<[u8; 32], CryptoError> = derive_session_key;
        let _: fn(&[u8; 32], &[u8; 32]) -> [u8; 64] = build_hkdf_salt;
        let _: fn(&[u8; 32], &[u8], &[u8; 64]) -> Result<(), CryptoError> = verify_signature;
        let _: &[u8] = HKDF_INFO;
        // Phase 5 新增 re-export
        let _: fn(&[u8; 32], &[u8]) -> Result<AesEncryptedParts, CryptoError> = aes_encrypt_parts;
        let _: fn(&[u8; 32], &AesEncryptedParts) -> Result<Vec<u8>, CryptoError> =
            aes_decrypt_parts;
        // EcdhKeyPair 與 SigningKeyPair 為 struct，透過建構驗證
        let _kp = EcdhKeyPair::generate();
        let _sp = SigningKeyPair::generate();
        let _sp_seed = SigningKeyPair::from_seed(&[0u8; 32]);
    }

    /// ECDH → HKDF → AES 完整加密流程端對端驗證
    #[test]
    fn test_ecdh_hkdf_aes_full_flow() {
        // Alice 與 Bob 各生成 ECDH 金鑰對
        let alice = EcdhKeyPair::generate();
        let bob = EcdhKeyPair::generate();
        let alice_pk = alice.public_key();
        let bob_pk = bob.public_key();

        // 雙方計算 shared secret
        let alice_ss = alice
            .derive_shared_secret(&bob_pk)
            .expect("Alice 計算 shared secret 失敗");
        let bob_ss = bob
            .derive_shared_secret(&alice_pk)
            .expect("Bob 計算 shared secret 失敗");
        assert_eq!(alice_ss, bob_ss, "雙方 shared secret 應相同");

        // 使用 build_hkdf_salt 組裝 salt（client=alice, server=bob）
        let salt = build_hkdf_salt(&alice_pk, &bob_pk);

        // 衍生 session key
        let alice_key =
            derive_session_key(&alice_ss, &salt, HKDF_INFO).expect("Alice 衍生 session key 失敗");
        let bob_key =
            derive_session_key(&bob_ss, &salt, HKDF_INFO).expect("Bob 衍生 session key 失敗");
        assert_eq!(alice_key, bob_key, "雙方 session key 應相同");

        // 加密 → 解密 round-trip
        let plaintext = b"Phase 4 Task 06 full flow test";
        let ciphertext = aes_encrypt(&alice_key, plaintext).expect("加密失敗");
        let decrypted = aes_decrypt(&bob_key, &ciphertext).expect("解密失敗");
        assert_eq!(decrypted, plaintext, "完整流程 round-trip 解密後原文應一致");
    }

    /// shared secret zeroize 後不影響已衍生的 session key
    #[test]
    fn test_full_flow_shared_secret_zeroize() {
        let alice = EcdhKeyPair::generate();
        let bob = EcdhKeyPair::generate();
        let alice_pk = alice.public_key();
        let bob_pk = bob.public_key();

        let mut shared_secret = alice
            .derive_shared_secret(&bob_pk)
            .expect("計算 shared secret 失敗");

        let salt = build_hkdf_salt(&alice_pk, &bob_pk);
        let session_key =
            derive_session_key(&shared_secret, &salt, HKDF_INFO).expect("衍生 session key 失敗");

        // 清零 shared secret（呼叫者責任，見 crypto-spec.md 不變式 #2）
        shared_secret.zeroize();
        assert_eq!(shared_secret, [0u8; 32], "shared secret 應已被清零");

        // session key 仍可正常使用
        let plaintext = b"zeroize test";
        let ciphertext = aes_encrypt(&session_key, plaintext).expect("加密失敗");
        let decrypted = aes_decrypt(&session_key, &ciphertext).expect("解密失敗");
        assert_eq!(
            decrypted, plaintext,
            "清零 shared secret 後 session key 仍可用"
        );

        // Bob 側同樣可解密
        let bob_ss = bob
            .derive_shared_secret(&alice_pk)
            .expect("Bob 計算 shared secret 失敗");
        let bob_key =
            derive_session_key(&bob_ss, &salt, HKDF_INFO).expect("Bob 衍生 session key 失敗");
        let bob_decrypted = aes_decrypt(&bob_key, &ciphertext).expect("Bob 解密失敗");
        assert_eq!(bob_decrypted, plaintext, "Bob 應能解密 Alice 加密的資料");
    }

    /// re-export 型別與模組內型別一致性驗證
    #[test]
    fn test_reexport_type_identity() {
        // 確認 re-export 的 CryptoError 與 aes 模組回傳的型別相同
        let key = [0u8; 32];
        let err: CryptoError = aes_decrypt(&key, b"short").unwrap_err();
        // 若型別不一致（alias 衝突），此 match 無法編譯
        match err {
            CryptoError::CiphertextTooShort => {}
            _ => panic!("預期 CiphertextTooShort"),
        }
    }

    /// 完整流程中使用錯誤的 session key 解密 → Err(DecryptionFailed)
    #[test]
    fn test_full_flow_wrong_key_decrypt() {
        let alice = EcdhKeyPair::generate();
        let bob = EcdhKeyPair::generate();
        let alice_pk = alice.public_key();
        let bob_pk = bob.public_key();

        let alice_ss = alice
            .derive_shared_secret(&bob_pk)
            .expect("計算 shared secret 失敗");
        let salt = build_hkdf_salt(&alice_pk, &bob_pk);
        let session_key =
            derive_session_key(&alice_ss, &salt, HKDF_INFO).expect("衍生 session key 失敗");

        let plaintext = b"secret message";
        let ciphertext = aes_encrypt(&session_key, plaintext).expect("加密失敗");

        // 第三方使用不同的 session key
        let eve = EcdhKeyPair::generate();
        let eve_pk = eve.public_key();
        let eve_ss = eve
            .derive_shared_secret(&bob_pk)
            .expect("Eve 計算 shared secret 失敗");
        let eve_salt = build_hkdf_salt(&eve_pk, &bob_pk);
        let eve_key =
            derive_session_key(&eve_ss, &eve_salt, HKDF_INFO).expect("Eve 衍生 session key 失敗");

        let result = aes_decrypt(&eve_key, &ciphertext);
        assert_eq!(
            result,
            Err(CryptoError::AuthTagMismatch),
            "錯誤 session key 解密應回傳 AuthTagMismatch"
        );
    }

    /// 完整流程中使用 all-zero 公鑰 → Err(InvalidPublicKey)
    #[test]
    fn test_full_flow_invalid_public_key() {
        let alice = EcdhKeyPair::generate();
        let zero_pk = [0u8; 32];
        let result = alice.derive_shared_secret(&zero_pk);
        assert_eq!(
            result,
            Err(CryptoError::InvalidPublicKey),
            "all-zero 公鑰應回傳 InvalidPublicKey"
        );
    }

    /// 驗證 frame 模組 re-export 可直接存取（編譯期保證）
    ///
    /// **設計決策**：刻意以 fn pointer type assertion 形式撰寫，除驗證 re-export
    /// 可達性外，亦保證每個函式的**簽名與預期一致**。對 crypto 這類安全敏感模組，
    /// 簽名變更若意外發生（例如改參數順序）此測試會編譯失敗，提供額外防護。
    /// 因此為此測試 opt-out clippy::type_complexity（簽名複雜度為設計意圖，非疏失）。
    #[test]
    #[allow(clippy::type_complexity)]
    fn test_frame_reexport_availability() {
        let _: fn(&[u8], &[u8; 32]) -> Result<Vec<u8>, FrameError> = encrypt_frame;
        let _: fn(&[u8], &[u8; 32]) -> Result<Vec<u8>, FrameError> = decrypt_frame;
        let _: fn(&[u8], &[u8; 32], u64) -> Result<Vec<u8>, FrameError> = encrypt_frame_aad;
        let _: fn(&[u8], &[u8; 32], u64) -> Result<Vec<u8>, FrameError> = decrypt_frame_aad;
    }
}
