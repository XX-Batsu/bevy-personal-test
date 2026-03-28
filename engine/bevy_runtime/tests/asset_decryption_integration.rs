//! 資產解密整合測試 — EncryptedAssetReader decrypt_all 等價邏輯 + SessionKeyStore 生命週期。
//!
//! 由於 `EncryptedAssetReader::decrypt_all` 為 private 方法，
//! 此整合測試透過 `AssetDecryptor`（pub API）+ `SessionKeyStore` 組合測試完整流程。

use bevy_runtime::asset_decryption::{
    AssetDecryptionError, AssetDecryptor, SessionKeyStore, L1_CACHE_SIZE,
};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

const TEST_KEY: [u8; 32] = [0x42u8; 32];
const WRONG_KEY: [u8; 32] = [0x43u8; 32];

/// 加密明文（單 chunk，使用 TEST_KEY）。
fn encrypt(plaintext: &[u8], key: &[u8; 32]) -> Vec<u8> {
    crypto::aes_encrypt(key, plaintext).unwrap()
}

/// 將明文按 chunk_size 分片，每片獨立加密，組裝為分片格式密文。
///
/// 分片格式：
/// `[chunk_count: u32 LE] || ([chunk_len: u32 LE] || [chunk_data]) × N`
fn encrypt_chunked(plaintext: &[u8], key: &[u8; 32], chunk_size: usize) -> Vec<u8> {
    let chunks: Vec<&[u8]> = plaintext.chunks(chunk_size).collect();
    let chunk_count = chunks.len() as u32;
    let mut result = chunk_count.to_le_bytes().to_vec();
    for chunk in chunks {
        let encrypted = crypto::aes_encrypt(key, chunk).unwrap();
        result.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
        result.extend_from_slice(&encrypted);
    }
    result
}

/// 建立已設定 key 的 SessionKeyStore（Arc<Mutex<_>> 包裝）。
fn create_key_store(key: [u8; 32]) -> Arc<Mutex<SessionKeyStore>> {
    let mut store = SessionKeyStore::new();
    store.store_key(Zeroizing::new(key));
    Arc::new(Mutex::new(store))
}

/// 模擬 EncryptedAssetReader::decrypt_all 的單 chunk 公開等價邏輯。
///
/// 由於 decrypt_all 為 private，此函式使用 pub API（SessionKeyStore + AssetDecryptor）
/// 重現單 chunk 解密流程（密文為 aes_encrypt 直接輸出）。
fn decrypt_single_via_public_api(
    key_store: &Arc<Mutex<SessionKeyStore>>,
    ciphertext: &[u8],
) -> Result<Vec<u8>, AssetDecryptionError> {
    let store = key_store
        .lock()
        .expect("SessionKeyStore mutex 不應被 poisoned");
    let key = store.get_encryption_key()?;
    let mut decryptor = AssetDecryptor::new(key);
    let plaintext = decryptor.decrypt_chunk(ciphertext)?;
    Ok(plaintext.to_vec())
}

/// 模擬 EncryptedAssetReader::decrypt_all 的多 chunk streaming 公開等價邏輯。
///
/// 解析 encrypt_chunked 產出的分片格式，逐 chunk 解密並合併。
fn decrypt_chunked_via_public_api(
    key_store: &Arc<Mutex<SessionKeyStore>>,
    chunked_bytes: &[u8],
) -> Result<Vec<u8>, AssetDecryptionError> {
    let store = key_store
        .lock()
        .expect("SessionKeyStore mutex 不應被 poisoned");
    let key = store.get_encryption_key()?;
    let mut decryptor = AssetDecryptor::new(key);

    if chunked_bytes.len() < 4 {
        return Err(AssetDecryptionError::InvalidFormat {
            reason: "分片格式：資料不足以包含 chunk_count",
        });
    }
    let chunk_count = u32::from_le_bytes(chunked_bytes[..4].try_into().unwrap()) as usize;

    let mut offset = 4;
    let mut output = Vec::new();

    for i in 0..chunk_count {
        if offset + 4 > chunked_bytes.len() {
            output.as_mut_slice().fill(0);
            return Err(AssetDecryptionError::ChunkError { chunk_index: i });
        }
        let chunk_len =
            u32::from_le_bytes(chunked_bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;

        if offset + chunk_len > chunked_bytes.len() {
            output.as_mut_slice().fill(0);
            return Err(AssetDecryptionError::ChunkError { chunk_index: i });
        }
        let chunk_data = &chunked_bytes[offset..offset + chunk_len];
        offset += chunk_len;

        match decryptor.decrypt_chunk(chunk_data) {
            Ok(plaintext) => {
                output.extend_from_slice(plaintext);
                decryptor.zeroize();
            }
            Err(AssetDecryptionError::AuthTagMismatch) => {
                output.as_mut_slice().fill(0);
                return Err(AssetDecryptionError::AuthTagMismatch);
            }
            Err(_) => {
                output.as_mut_slice().fill(0);
                return Err(AssetDecryptionError::ChunkError { chunk_index: i });
            }
        }
    }

    Ok(output)
}

// ── 1. 加密→解密 roundtrip ─────────────────────────────────────────────

#[test]
fn test_integration_encrypt_decrypt_roundtrip() {
    let plaintext = b"test asset content for bevy integration";
    let ciphertext = encrypt(plaintext, &TEST_KEY);

    let key_store = create_key_store(TEST_KEY);
    let result = decrypt_single_via_public_api(&key_store, &ciphertext).unwrap();

    assert_eq!(
        result.as_slice(),
        plaintext,
        "解密後明文應 bit-for-byte 相符"
    );
}

// ── 2. 錯誤 key → AuthTagMismatch ─────────────────────────────────────

#[test]
fn test_integration_wrong_key_auth_tag_mismatch() {
    let plaintext = b"encrypted with correct key";
    let ciphertext = encrypt(plaintext, &TEST_KEY);

    // 使用 WRONG_KEY 建構 key_store
    let key_store = create_key_store(WRONG_KEY);
    let result = decrypt_single_via_public_api(&key_store, &ciphertext);

    assert!(
        matches!(result, Err(AssetDecryptionError::AuthTagMismatch)),
        "錯誤密鑰應回傳 AuthTagMismatch，實際：{result:?}"
    );
}

// ── 3. 空 key store → NoSessionKey ─────────────────────────────────────

#[test]
fn test_integration_no_session_key() {
    let plaintext = b"should fail without key";
    let ciphertext = encrypt(plaintext, &TEST_KEY);

    // 建構空的 key_store（未呼叫 store_key）
    let key_store = Arc::new(Mutex::new(SessionKeyStore::new()));
    let result = decrypt_single_via_public_api(&key_store, &ciphertext);

    assert!(
        matches!(result, Err(AssetDecryptionError::NoSessionKey)),
        "未設定 session key 應回傳 NoSessionKey，實際：{result:?}"
    );
}

// ── 4. 1MB 分片 streaming 解密 ────────────────────────────────────────

#[test]
fn test_integration_large_asset_streaming() {
    // 1MB 明文，分片加密（chunk_size=500KB）→ 2 片
    let plaintext = vec![0xABu8; 1_048_576]; // 1 MB
    let chunked = encrypt_chunked(&plaintext, &TEST_KEY, 500_000);

    let key_store = create_key_store(TEST_KEY);
    let result = decrypt_chunked_via_public_api(&key_store, &chunked).unwrap();

    assert_eq!(result.len(), plaintext.len(), "解密後長度應一致");
    assert_eq!(result, plaintext, "1MB 分片解密後明文應完整還原");
}

// ── 5. SessionKeyStore 生命週期：store → decrypt → rotate → decrypt → 舊 key 失敗 ──

#[test]
fn test_integration_session_key_lifecycle() {
    let new_key: [u8; 32] = [0x99u8; 32];
    let plaintext = b"lifecycle test data";

    // 階段 1：store_key → 解密成功
    let key_store = create_key_store(TEST_KEY);
    let ciphertext_old = encrypt(plaintext, &TEST_KEY);
    let result = decrypt_single_via_public_api(&key_store, &ciphertext_old).unwrap();
    assert_eq!(result.as_slice(), plaintext, "初始 key 解密應成功");

    // 階段 2：rotate 新 key → 用新 key 加密 → 解密成功
    {
        let mut store = key_store.lock().unwrap();
        store.rotate(Zeroizing::new(new_key));
    }
    let ciphertext_new = encrypt(plaintext, &new_key);
    let result = decrypt_single_via_public_api(&key_store, &ciphertext_new).unwrap();
    assert_eq!(result.as_slice(), plaintext, "rotate 後用新 key 解密應成功");

    // 階段 3：用舊 key 加密的密文 → 新 key 解密 → Err(AuthTagMismatch)
    let result = decrypt_single_via_public_api(&key_store, &ciphertext_old);
    assert!(
        matches!(result, Err(AssetDecryptionError::AuthTagMismatch)),
        "rotate 後用舊 key 密文應回傳 AuthTagMismatch，實際：{result:?}"
    );
}

// ── 6. 恰好 8MB 單 chunk 解密 ─────────────────────────────────────────

#[test]
#[ignore] // 8MB 測試較慢，CI 可標記獨立執行
fn test_integration_exactly_8mb_single_chunk() {
    let plaintext = vec![0xEFu8; L1_CACHE_SIZE]; // 恰好 8MB
    let ciphertext = encrypt(&plaintext, &TEST_KEY);

    let key_store = create_key_store(TEST_KEY);
    let result = decrypt_single_via_public_api(&key_store, &ciphertext).unwrap();

    assert_eq!(result.len(), L1_CACHE_SIZE, "解密後長度應為 8MB");
    assert_eq!(result[0], 0xEF, "首 byte 應正確");
    assert_eq!(result[L1_CACHE_SIZE / 2], 0xEF, "中間 byte 應正確");
    assert_eq!(result[L1_CACHE_SIZE - 1], 0xEF, "末 byte 應正確");
    assert_eq!(result, plaintext, "8MB 單 chunk 解密後明文應完整還原");
}
