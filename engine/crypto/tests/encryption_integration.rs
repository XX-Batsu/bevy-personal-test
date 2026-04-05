//! 加密 pipeline 整合測試
//!
//! 驗證跨模組 round-trip：AES-256-GCM、Ed25519、ECDH → HKDF → AES-GCM、
//! 金鑰輪換、空資料與截斷密文邊界案例。
//!
//! 使用 `cargo test -p crypto --features integration` 執行。

#![cfg(feature = "integration")]

use crypto::{
    aes_decrypt, aes_encrypt, build_hkdf_salt, derive_session_key, verify_signature, CryptoError,
    EcdhKeyPair, SigningKeyPair, HKDF_INFO,
};

// ── AES-256-GCM Round-Trip ──

#[test]
fn aes_gcm_round_trip() {
    let key = [0x42u8; 32];
    let plaintext = b"Hello, World! This is test data.";
    let encrypted = aes_encrypt(&key, plaintext).unwrap();
    let decrypted = aes_decrypt(&key, &encrypted).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn aes_gcm_empty_plaintext() {
    let key = [0xAAu8; 32];
    let plaintext = b"";
    let encrypted = aes_encrypt(&key, plaintext).unwrap();
    let decrypted = aes_decrypt(&key, &encrypted).unwrap();
    assert_eq!(decrypted, plaintext.to_vec());
}

#[test]
fn aes_gcm_wrong_key_rejected() {
    let key_a = [0x42u8; 32];
    let key_b = [0x99u8; 32];
    let plaintext = b"secret data";
    let encrypted = aes_encrypt(&key_a, plaintext).unwrap();
    let result = aes_decrypt(&key_b, &encrypted);
    assert!(
        matches!(result, Err(CryptoError::AuthTagMismatch)),
        "錯誤金鑰解密應回傳 AuthTagMismatch"
    );
}

#[test]
fn aes_gcm_truncated_ciphertext_rejected() {
    let key = [0x42u8; 32];
    // 密文格式：nonce(12) + ciphertext + auth_tag(16)
    // 截斷至少於 12 bytes 應觸發 CiphertextTooShort
    let short = vec![0u8; 5];
    let result = aes_decrypt(&key, &short);
    assert!(
        matches!(result, Err(CryptoError::CiphertextTooShort)),
        "截斷密文應回傳 CiphertextTooShort"
    );
}

#[test]
fn aes_gcm_tampered_ciphertext_rejected() {
    let key = [0x42u8; 32];
    let plaintext = b"important data";
    let mut encrypted = aes_encrypt(&key, plaintext).unwrap();
    // 篡改密文（修改 nonce 後的第一個 byte）
    if encrypted.len() > 12 {
        encrypted[12] ^= 0xFF;
    }
    let result = aes_decrypt(&key, &encrypted);
    assert!(result.is_err(), "篡改密文應解密失敗");
}

#[test]
fn aes_gcm_large_payload() {
    let key = [0x42u8; 32];
    let plaintext = vec![0xBBu8; 1024 * 1024]; // 1 MB
    let encrypted = aes_encrypt(&key, &plaintext).unwrap();
    let decrypted = aes_decrypt(&key, &encrypted).unwrap();
    assert_eq!(decrypted, plaintext);
}

// ── Ed25519 簽章 ──

#[test]
fn ed25519_sign_and_verify() {
    let key_pair = SigningKeyPair::generate();
    let public_key = key_pair.public_key();
    let message = b"bytecode content";
    let signature = key_pair.sign(message);
    assert!(verify_signature(&public_key, message, &signature).is_ok());
}

#[test]
fn ed25519_forged_signature_rejected() {
    let key_pair = SigningKeyPair::generate();
    let public_key = key_pair.public_key();
    let message = b"bytecode content";
    let forged: [u8; 64] = [0xAB; 64];
    assert!(matches!(
        verify_signature(&public_key, message, &forged),
        Err(CryptoError::SignatureVerificationFailed)
    ));
}

#[test]
fn ed25519_wrong_public_key_rejected() {
    let key_pair_a = SigningKeyPair::generate();
    let key_pair_b = SigningKeyPair::generate();
    let message = b"data";
    let signature = key_pair_a.sign(message);
    let result = verify_signature(&key_pair_b.public_key(), message, &signature);
    assert!(result.is_err(), "不同公鑰驗證應失敗");
}

#[test]
fn ed25519_deterministic_signature() {
    // Ed25519 (RFC 8032) 簽章為確定性
    let key_pair = SigningKeyPair::from_bytes(&[0x42u8; 32]);
    let message = b"deterministic test";
    let sig_a = key_pair.sign(message);
    let sig_b = key_pair.sign(message);
    assert_eq!(sig_a, sig_b, "Ed25519 簽章應為確定性");
}

#[test]
fn ed25519_empty_message() {
    let key_pair = SigningKeyPair::generate();
    let public_key = key_pair.public_key();
    let signature = key_pair.sign(b"");
    assert!(verify_signature(&public_key, b"", &signature).is_ok());
}

// ── ECDH → HKDF → AES-GCM 完整流程 ──

#[test]
fn ecdh_full_pipeline_encrypt_decrypt() {
    // Step 1: 雙方各自生成 ephemeral 金鑰對
    let client_pair = EcdhKeyPair::generate();
    let server_pair = EcdhKeyPair::generate();
    let client_pub = client_pair.public_key();
    let server_pub = server_pair.public_key();

    // Step 2: ECDH derive shared secret
    let client_ss = client_pair.derive_shared_secret(&server_pub).unwrap();
    let server_ss = server_pair.derive_shared_secret(&client_pub).unwrap();
    assert_eq!(
        client_ss, server_ss,
        "ECDH 對稱性：雙方 shared secret 應相同"
    );

    // Step 3: HKDF 衍生 session key
    let salt = build_hkdf_salt(&client_pub, &server_pub);
    let client_key = derive_session_key(&client_ss, &salt, HKDF_INFO).unwrap();
    let server_key = derive_session_key(&server_ss, &salt, HKDF_INFO).unwrap();
    assert_eq!(client_key, server_key, "雙方衍生的 session key 應相同");

    // Step 4: 使用 session key 加解密
    let plaintext = b"session data";
    let encrypted = aes_encrypt(&client_key, plaintext).unwrap();
    let decrypted = aes_decrypt(&server_key, &encrypted).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn ecdh_all_zero_public_key_rejected() {
    let pair = EcdhKeyPair::generate();
    let zero_pub = [0u8; 32];
    let result = pair.derive_shared_secret(&zero_pub);
    assert!(
        matches!(result, Err(CryptoError::InvalidPublicKey)),
        "all-zero 公鑰應被拒絕"
    );
}

// ── 金鑰輪換 ──

#[test]
fn key_rotation_old_key_cannot_decrypt_new() {
    let old_key = [0x11u8; 32];
    let new_key = [0x22u8; 32];
    let plaintext = b"rotated data";

    // 使用新金鑰加密
    let encrypted = aes_encrypt(&new_key, plaintext).unwrap();

    // 舊金鑰解密應失敗
    let result = aes_decrypt(&old_key, &encrypted);
    assert!(result.is_err(), "舊金鑰不應能解密新金鑰加密的資料");

    // 新金鑰解密應成功
    let decrypted = aes_decrypt(&new_key, &encrypted).unwrap();
    assert_eq!(decrypted, plaintext);
}

// ── Sign-then-Encrypt Pipeline（模擬 asset pipeline） ──

#[test]
fn sign_then_encrypt_asset_round_trip() {
    let signing_kp = SigningKeyPair::generate();
    let verifying_key = signing_kp.public_key();
    let aes_key = [0x42u8; 32];

    let asset_data = b"game asset content: sprite sheet data";

    // 簽章
    let signature = signing_kp.sign(asset_data);

    // 加密（asset_data + signature）
    let mut payload = asset_data.to_vec();
    payload.extend_from_slice(&signature);
    let encrypted = aes_encrypt(&aes_key, &payload).unwrap();

    // 解密
    let decrypted = aes_decrypt(&aes_key, &encrypted).unwrap();

    // 拆分 asset_data 和 signature
    let (data, sig) = decrypted.split_at(decrypted.len() - 64);
    let sig: [u8; 64] = sig.try_into().unwrap();

    // 驗證簽章
    assert!(verify_signature(&verifying_key, data, &sig).is_ok());
    assert_eq!(data, asset_data);
}
