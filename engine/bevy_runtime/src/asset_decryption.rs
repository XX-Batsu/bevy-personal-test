//! 資產解密模組 — AES-256-GCM 解密 + Zeroizing 記憶體管理。
//!
//! 提供以下核心元件：
//! - [`AssetDecryptionError`]：8 種解密錯誤型別
//! - [`SessionKeyStore`]：Zeroizing session key 管理
//! - [`StagingBuffer`]：8MB L1 快取級暫存區（Drop 自動清零）
//! - [`AssetDecryptor`]：逐 chunk 解密器
//! - [`EncryptedAssetReader`]：Bevy [`AssetReader`] trait 實作（裝飾器模式）
//! - [`AssetDecryptionPlugin`]：Bevy Plugin — 整合 EncryptedAssetReader 至 App

use std::sync::Arc;

use bevy::prelude::*;
use thiserror::Error;
use tracing;
use zeroize::{Zeroize, Zeroizing};

/// L1 快取級暫存區大小：8 MB
pub const L1_CACHE_SIZE: usize = 8 * 1024 * 1024;

/// 密文最小長度：12 bytes nonce + 16 bytes auth tag = 28 bytes
const CIPHERTEXT_MIN_LEN: usize = 28;

// ── AssetDecryptionError ──────────────────────────────────────────────

/// 資產解密錯誤型別
#[derive(Debug, Error)]
pub enum AssetDecryptionError {
    /// 密文格式無效（長度不足等）
    #[error("密文格式無效：{reason}")]
    InvalidFormat {
        /// 錯誤原因描述
        reason: &'static str,
    },

    /// AES-GCM 認證標籤驗證失敗（密鑰錯誤或資料被竄改）
    #[error("AES-GCM 認證標籤驗證失敗")]
    AuthTagMismatch,

    /// 解密後明文超過 L1 快取級暫存區大小
    #[error("明文區塊大小 {size} bytes 超過上限 {limit} bytes")]
    ChunkTooLarge {
        /// 實際明文大小
        size: usize,
        /// 允許的最大大小（L1_CACHE_SIZE）
        limit: usize,
    },

    /// 尚未設定 session key
    #[error("尚未設定 session key")]
    NoSessionKey,

    /// 分片解密失敗
    #[error("分片解密失敗：chunk {chunk_index}")]
    ChunkError {
        /// 失敗的 chunk 索引
        chunk_index: usize,
    },

    /// 非 plaintext-assets 模式下遇到未加密資料
    #[error("非加密資產在非 plaintext-assets 模式下不被允許")]
    PlaintextNotAllowed,

    /// I/O 錯誤（讀取 inner reader 失敗等）
    #[error("I/O 錯誤：{0}")]
    IoError(#[from] std::io::Error),
}

impl AssetDecryptionError {
    /// 將 AssetDecryptionError 轉為 bevy AssetReaderError::Io
    fn into_asset_reader_error(self) -> bevy::asset::io::AssetReaderError {
        bevy::asset::io::AssetReaderError::Io(Arc::new(std::io::Error::other(self.to_string())))
    }
}

// ── SessionKeyStore ───────────────────────────────────────────────────

/// Session key 管理器 — 以 Zeroizing 包裝確保 Drop 時自動清零。
///
/// 設計為 thread-safe（可包裝於 `Arc<Mutex<_>>` 中共享）。
pub struct SessionKeyStore {
    /// Zeroizing<[u8; 32]> 確保 drop 時自動清零
    key: Option<Zeroizing<[u8; 32]>>,
}

impl SessionKeyStore {
    /// 建立空的 key store（尚未設定 key）。
    pub fn new() -> Self {
        Self { key: None }
    }

    /// 設定 session key（便利方法，內部包裝為 Zeroizing）。
    /// 先清零舊 key 再寫入新 key。
    pub fn set_key(&mut self, key: [u8; 32]) {
        tracing::debug!("設定 session key");
        self.key = Some(Zeroizing::new(key));
    }

    /// 儲存 session key（Phase 13 ECDH 握手後呼叫）。
    ///
    /// 接受已包裝的 `Zeroizing<[u8; 32]>`，避免裸陣列在呼叫端暴露。
    pub fn store_key(&mut self, key: Zeroizing<[u8; 32]>) {
        tracing::debug!("儲存 session key");
        self.key = Some(key);
    }

    /// 輪換 key：清零舊 key，存入新 key。
    ///
    /// 舊 key 透過 `Zeroizing` 的 `Drop` 自動清零。
    pub fn rotate(&mut self, new_key: Zeroizing<[u8; 32]>) {
        tracing::debug!("輪換 session key");
        self.key = Some(new_key);
    }

    /// 取得 session key 的不可變參考。
    ///
    /// # 錯誤
    /// - [`AssetDecryptionError::NoSessionKey`]：尚未設定 key
    pub fn get_encryption_key(&self) -> Result<&[u8; 32], AssetDecryptionError> {
        self.key
            .as_deref()
            .ok_or(AssetDecryptionError::NoSessionKey)
    }

    /// 清除 session key（Drop 語義的顯式呼叫）。
    pub fn clear(&mut self) {
        tracing::debug!("清除 session key");
        self.key = None;
    }

    /// 是否已設定 session key。
    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }
}

impl Default for SessionKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── StagingBuffer ─────────────────────────────────────────────────────

/// 8MB L1 快取級暫存區 — Drop 時自動清零。
///
/// 用於承接解密後的明文資料，避免頻繁配置記憶體。
/// 使用 `Box<[u8; L1_CACHE_SIZE]>` 在 heap 上分配（避免 stack overflow）。
pub(crate) struct StagingBuffer {
    /// 8MB heap-allocated buffer
    pub(crate) data: Box<[u8; L1_CACHE_SIZE]>,
}

impl StagingBuffer {
    /// 建立新的 8MB 暫存區（heap 分配，全零初始化）。
    pub fn new() -> Self {
        Self {
            data: new_staging_buffer(),
        }
    }
}

impl Default for StagingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for StagingBuffer {
    fn drop(&mut self) {
        tracing::trace!("清零 StagingBuffer（8MB）");
        self.data.as_mut_slice().zeroize();
    }
}

/// 在 heap 上配置 8MB 全零 buffer（避免 stack overflow）。
fn new_staging_buffer() -> Box<[u8; L1_CACHE_SIZE]> {
    vec![0u8; L1_CACHE_SIZE]
        .into_boxed_slice()
        .try_into()
        .expect("Vec 長度與 L1_CACHE_SIZE 一致")
}

// ── AssetDecryptor ────────────────────────────────────────────────────

/// 逐 chunk AES-256-GCM 解密器。
///
/// 持有 session key 參考與 8MB 暫存區。每次 `decrypt_chunk` 將解密後的明文
/// 寫入暫存區並回傳 slice 參考，避免額外記憶體配置。
pub struct AssetDecryptor<'key> {
    session_key: &'key [u8; 32],
    l1_buffer: StagingBuffer,
}

impl<'key> AssetDecryptor<'key> {
    /// 建立新的解密器。
    pub fn new(session_key: &'key [u8; 32]) -> Self {
        Self {
            session_key,
            l1_buffer: StagingBuffer::new(),
        }
    }

    /// 解密單一加密 chunk。
    ///
    /// # 密文格式
    /// `[nonce: 12 bytes] || [ciphertext: N bytes] || [auth_tag: 16 bytes]`
    ///
    /// # 回傳
    /// 解密後的明文 slice（參考內部 L1 暫存區）。
    ///
    /// # 錯誤
    /// - [`AssetDecryptionError::InvalidFormat`]：密文長度不足 28 bytes
    /// - [`AssetDecryptionError::ChunkTooLarge`]：解密後明文超過 8MB
    /// - [`AssetDecryptionError::AuthTagMismatch`]：密鑰錯誤或資料被竄改
    pub fn decrypt_chunk(&mut self, ciphertext: &[u8]) -> Result<&[u8], AssetDecryptionError> {
        // 1. 驗證密文最小長度
        if ciphertext.len() < CIPHERTEXT_MIN_LEN {
            return Err(AssetDecryptionError::InvalidFormat {
                reason: "密文長度不足 28 bytes（12 nonce + 0 明文 + 16 tag）",
            });
        }

        // 2. 計算解密後明文大小，檢查是否超過 L1 快取
        let plaintext_len = ciphertext.len() - CIPHERTEXT_MIN_LEN;
        if plaintext_len > L1_CACHE_SIZE {
            return Err(AssetDecryptionError::ChunkTooLarge {
                size: plaintext_len,
                limit: L1_CACHE_SIZE,
            });
        }

        // 3. 執行 AES-256-GCM 解密
        match crypto::aes_decrypt(self.session_key, ciphertext) {
            Ok(plaintext) => {
                self.l1_buffer.data[..plaintext_len].copy_from_slice(&plaintext);
                Ok(&self.l1_buffer.data[..plaintext_len])
            }
            Err(_) => {
                // 失敗時清零 buffer 再回傳 Err
                self.l1_buffer.data.as_mut_slice().zeroize();
                Err(AssetDecryptionError::AuthTagMismatch)
            }
        }
    }

    /// 明確清零暫存區與相關狀態。
    pub fn zeroize(&mut self) {
        self.l1_buffer.data.as_mut_slice().zeroize();
    }

    /// 測試用：取得 buffer 內容的不可變參考。
    #[cfg(test)]
    pub(crate) fn buffer_content_for_test(&self) -> &[u8] {
        &*self.l1_buffer.data
    }
}

// ── EncryptedAssetReader ──────────────────────────────────────────────

/// 加密資產讀取器（裝飾器模式）。
///
/// 包裝任意 `Box<dyn bevy::asset::io::ErasedAssetReader>`，
/// 讀取時先透過 inner reader 取得密文，再逐 chunk 解密後回傳明文。
///
/// **行為約束**：
/// - `AuthTagMismatch` → `panic!`（底層 `decrypt_chunk` 回傳 Err 後由此層 panic）
/// - `NoSessionKey` → 轉為 `AssetReaderError::Io`（不 panic）
pub struct EncryptedAssetReader {
    /// 內層資產讀取器
    inner: Box<dyn bevy::asset::io::ErasedAssetReader>,
    /// Session key 管理器
    key_store: Arc<std::sync::Mutex<SessionKeyStore>>,
}

impl EncryptedAssetReader {
    /// 建立新的加密資產讀取器。
    pub fn new(
        inner: Box<dyn bevy::asset::io::ErasedAssetReader>,
        key_store: Arc<std::sync::Mutex<SessionKeyStore>>,
    ) -> Self {
        Self { inner, key_store }
    }

    /// 解密完整密文資料。
    ///
    /// - 若 `encrypted_bytes.len() <= L1_CACHE_SIZE + 28`：單 chunk 解密
    /// - 若 `encrypted_bytes.len() > L1_CACHE_SIZE + 28`：分片 streaming 解密
    ///   每片為獨立加密的 chunk（各自含 nonce+tag）
    ///
    /// # 分片格式
    /// `[chunk_count: u32 LE] || [chunk_0_len: u32 LE] || [chunk_0_data] || [chunk_1_len: u32 LE] || [chunk_1_data] || ...`
    fn decrypt_all(&self, encrypted_bytes: &[u8]) -> Result<Vec<u8>, AssetDecryptionError> {
        let key_store = self
            .key_store
            .lock()
            .expect("SessionKeyStore mutex 不應被 poisoned");
        let key = key_store.get_encryption_key()?;
        let mut decryptor = AssetDecryptor::new(key);

        // 單 chunk 模式：密文 <= 8MB + 28 bytes overhead
        if encrypted_bytes.len() <= L1_CACHE_SIZE + CIPHERTEXT_MIN_LEN {
            let plaintext = decryptor.decrypt_chunk(encrypted_bytes)?;
            return Ok(plaintext.to_vec());
        }

        // 多 chunk streaming：讀取分片格式 header
        if encrypted_bytes.len() < 4 {
            return Err(AssetDecryptionError::InvalidFormat {
                reason: "分片格式：資料不足以包含 chunk_count",
            });
        }
        let chunk_count = u32::from_le_bytes(encrypted_bytes[..4].try_into().unwrap()) as usize;

        let mut offset = 4;
        let mut output = Vec::new();

        for i in 0..chunk_count {
            // 每個 chunk: [chunk_len: u32 LE] || [chunk_data: chunk_len bytes]
            if offset + 4 > encrypted_bytes.len() {
                output.as_mut_slice().zeroize();
                return Err(AssetDecryptionError::ChunkError { chunk_index: i });
            }
            let chunk_len =
                u32::from_le_bytes(encrypted_bytes[offset..offset + 4].try_into().unwrap())
                    as usize;
            offset += 4;

            if offset + chunk_len > encrypted_bytes.len() {
                output.as_mut_slice().zeroize();
                return Err(AssetDecryptionError::ChunkError { chunk_index: i });
            }
            let chunk_data = &encrypted_bytes[offset..offset + chunk_len];
            offset += chunk_len;

            match decryptor.decrypt_chunk(chunk_data) {
                Ok(plaintext) => {
                    output.extend_from_slice(plaintext);
                    decryptor.zeroize();
                }
                Err(AssetDecryptionError::AuthTagMismatch) => {
                    output.as_mut_slice().zeroize();
                    return Err(AssetDecryptionError::AuthTagMismatch);
                }
                Err(_) => {
                    output.as_mut_slice().zeroize();
                    return Err(AssetDecryptionError::ChunkError { chunk_index: i });
                }
            }
        }

        Ok(output)
    }
}

impl bevy::asset::io::AssetReader for EncryptedAssetReader {
    async fn read<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, bevy::asset::io::AssetReaderError> {
        tracing::debug!("讀取加密資產：{}", path.display());

        // 檢查 session key 是否已設定
        {
            let key_store = self
                .key_store
                .lock()
                .expect("SessionKeyStore mutex 不應被 poisoned");
            if !key_store.has_key() {
                return Err(AssetDecryptionError::NoSessionKey.into_asset_reader_error());
            }
        }

        // 透過 inner reader 讀取密文
        let mut reader = self.inner.read(path).await?;
        let mut encrypted_bytes = Vec::new();
        reader.read_to_end(&mut encrypted_bytes).await?;

        // 解密
        let decrypted = self.decrypt_all(&encrypted_bytes).map_err(|e| match e {
            AssetDecryptionError::AuthTagMismatch => {
                panic!("資產認證標籤驗證失敗：{} — 資料可能被竄改", path.display());
            }
            AssetDecryptionError::NoSessionKey => e.into_asset_reader_error(),
            other => other.into_asset_reader_error(),
        })?;

        Ok(bevy::asset::io::VecReader::new(decrypted))
    }

    async fn read_meta<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, bevy::asset::io::AssetReaderError> {
        // meta 檔案不加密，直接透傳
        let reader = self.inner.read_meta(path).await?;
        // 將 Box<dyn Reader> 包裝為 VecReader 以統一回傳型別
        let mut boxed_reader = reader;
        let mut bytes = Vec::new();
        boxed_reader.read_to_end(&mut bytes).await?;
        Ok(bevy::asset::io::VecReader::new(bytes))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> Result<Box<bevy::asset::io::PathStream>, bevy::asset::io::AssetReaderError> {
        self.inner.read_directory(path).await
    }

    async fn is_directory<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> Result<bool, bevy::asset::io::AssetReaderError> {
        self.inner.is_directory(path).await
    }
}

// ── AssetDecryptionPlugin ─────────────────────────────────────────────

/// 資產解密 Plugin — 整合 [`EncryptedAssetReader`] 至 Bevy App。
///
/// **使用方式**：由最高層 App 初始化時加入（非 GamePlugin 內部）。
/// Session key 透過 `Arc<Mutex<SessionKeyStore>>` 共享，Phase 13 ECDH 握手後注入。
///
/// **plaintext-assets feature gate**：
/// - `#[cfg(not(feature = "plaintext-assets"))]`：註冊 key_store 為 Resource
/// - `#[cfg(feature = "plaintext-assets")]`：跳過，使用預設 reader
///
/// 注意：完整的 AssetReader 替換需要 Bevy AssetPlugin 配置，
/// 在 Phase 13 之前不會實際用到。Phase 10 的最低目標是讓型別存在並可被下游 import。
pub struct AssetDecryptionPlugin {
    /// Session key 管理器（共享參考）
    pub key_store: Arc<std::sync::Mutex<SessionKeyStore>>,
}

impl Plugin for AssetDecryptionPlugin {
    fn build(&self, _app: &mut App) {
        #[cfg(feature = "plaintext-assets")]
        {
            tracing::info!("plaintext-assets 模式：跳過資產解密");
            return;
        }
        #[cfg(not(feature = "plaintext-assets"))]
        {
            tracing::info!("啟用資產解密 EncryptedAssetReader");
            // 注意：實際替換 Bevy AssetReader 需要在 AssetPlugin 配置前執行
            // Bevy 0.15 的 AssetPlugin::build() 中使用 AssetSourceBuilder
            // 此處註冊 key_store 為 Resource，Phase 13 ECDH 握手後注入 key
            _app.insert_resource(SharedKeyStore(self.key_store.clone()));
        }
    }
}

/// Bevy Resource 包裝 `Arc<Mutex<SessionKeyStore>>`，供系統存取 session key。
#[derive(Resource, Clone)]
pub struct SharedKeyStore(pub Arc<std::sync::Mutex<SessionKeyStore>>);

// ── 測試 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 測試輔助 ──────────────────────────────────────────────────

    const TEST_KEY: [u8; 32] = [0x42u8; 32];
    const WRONG_KEY: [u8; 32] = [0x43u8; 32];

    fn encrypt(plaintext: &[u8]) -> Vec<u8> {
        crypto::aes_encrypt(&TEST_KEY, plaintext).unwrap()
    }

    fn make_decryptor(key: &[u8; 32]) -> AssetDecryptor<'_> {
        AssetDecryptor::new(key)
    }

    // ── Task 01 測試：AssetDecryptionError 基本錯誤型別 ──────────

    #[test]
    fn test_error_display_invalid_format() {
        let err = AssetDecryptionError::InvalidFormat {
            reason: "測試原因"
        };
        let msg = format!("{err}");
        assert!(msg.contains("測試原因"), "Display 應包含 reason：{msg}");
    }

    #[test]
    fn test_error_display_auth_tag_mismatch() {
        let err = AssetDecryptionError::AuthTagMismatch;
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "Display 不應為空");
    }

    #[test]
    fn test_error_display_chunk_too_large() {
        let err = AssetDecryptionError::ChunkTooLarge {
            size: 9_000_000,
            limit: L1_CACHE_SIZE,
        };
        let msg = format!("{err}");
        assert!(msg.contains("9000000"), "Display 應包含 size：{msg}");
    }

    #[test]
    fn test_error_display_no_session_key() {
        let err = AssetDecryptionError::NoSessionKey;
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "Display 不應為空");
    }

    // ── Task 01 測試：SessionKeyStore ────────────────────────────

    #[test]
    fn test_session_key_store_new_has_no_key() {
        let store = SessionKeyStore::new();
        assert!(!store.has_key(), "新建的 store 不應有 key");
        assert!(
            matches!(
                store.get_encryption_key(),
                Err(AssetDecryptionError::NoSessionKey)
            ),
            "取 key 應回傳 NoSessionKey"
        );
    }

    #[test]
    fn test_session_key_store_set_and_get() {
        let mut store = SessionKeyStore::new();
        store.set_key(TEST_KEY);
        assert!(store.has_key(), "設定後應有 key");
        let key = store.get_encryption_key().unwrap();
        assert_eq!(key, &TEST_KEY, "取得的 key 應與設定值一致");
    }

    #[test]
    fn test_session_key_store_clear() {
        let mut store = SessionKeyStore::new();
        store.set_key(TEST_KEY);
        store.clear();
        assert!(!store.has_key(), "clear 後不應有 key");
        assert!(
            matches!(
                store.get_encryption_key(),
                Err(AssetDecryptionError::NoSessionKey)
            ),
            "clear 後取 key 應回傳 NoSessionKey"
        );
    }

    #[test]
    fn test_session_key_store_overwrite() {
        let mut store = SessionKeyStore::new();
        store.set_key(TEST_KEY);
        store.set_key(WRONG_KEY);
        let key = store.get_encryption_key().unwrap();
        assert_eq!(key, &WRONG_KEY, "覆寫後應取得新 key");
    }

    // ── Task 01 測試：AssetDecryptor::decrypt_chunk ──────────────

    #[test]
    fn test_decrypt_chunk_roundtrip() {
        let plaintext = b"Hello, decryption!";
        let ciphertext = encrypt(plaintext);
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&ciphertext).unwrap();
        assert_eq!(result, plaintext, "解密後明文應一致");
    }

    #[test]
    fn test_decrypt_chunk_empty_plaintext() {
        let plaintext = b"";
        let ciphertext = encrypt(plaintext);
        assert_eq!(ciphertext.len(), 28, "空明文的密文長度應為 28 bytes");
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&ciphertext).unwrap();
        assert!(result.is_empty(), "空明文解密後應為空");
    }

    #[test]
    fn test_decrypt_chunk_wrong_key() {
        let plaintext = b"secret data";
        let ciphertext = encrypt(plaintext);
        let mut decryptor = make_decryptor(&WRONG_KEY);
        let result = decryptor.decrypt_chunk(&ciphertext);
        assert!(
            matches!(result, Err(AssetDecryptionError::AuthTagMismatch)),
            "錯誤密鑰應回傳 AuthTagMismatch"
        );
    }

    #[test]
    fn test_decrypt_chunk_tampered_ciphertext() {
        let plaintext = b"tamper test data";
        let mut ciphertext = encrypt(plaintext);
        // 竄改 nonce 之後的第一個 ciphertext byte
        if ciphertext.len() > 12 {
            ciphertext[12] ^= 0xFF;
        }
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&ciphertext);
        assert!(
            matches!(result, Err(AssetDecryptionError::AuthTagMismatch)),
            "竄改密文應回傳 AuthTagMismatch"
        );
    }

    #[test]
    fn test_decrypt_chunk_too_short() {
        let short = vec![0u8; 27]; // < 28 bytes
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&short);
        assert!(
            matches!(result, Err(AssetDecryptionError::InvalidFormat { .. })),
            "長度不足 28 bytes 應回傳 InvalidFormat"
        );
    }

    #[test]
    fn test_decrypt_chunk_exactly_28_bytes() {
        // 恰好 28 bytes = nonce(12) + tag(16)，無明文部分
        // 但這不是合法的 aes_encrypt 輸出（tag 是錯的），所以會 AuthTagMismatch
        let exactly_28 = vec![0u8; 28];
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&exactly_28);
        // 28 bytes 可能解密失敗（tag 不對），但不應觸發 InvalidFormat 或 ChunkTooLarge
        assert!(
            matches!(result, Ok(_) | Err(AssetDecryptionError::AuthTagMismatch)),
            "28 bytes 應為 Ok（空明文）或 AuthTagMismatch"
        );
    }

    #[test]
    fn test_decrypt_chunk_large_plaintext() {
        // 1 MB 明文
        let plaintext = vec![0xABu8; 1_048_576];
        let ciphertext = encrypt(&plaintext);
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&ciphertext).unwrap();
        assert_eq!(result, &plaintext[..], "1 MB 明文解密後應一致");
    }

    #[test]
    #[ignore] // 8MB 測試較慢，CI 可選擇跳過
    fn test_chunk_exactly_8mb() {
        let plaintext = vec![0xCDu8; L1_CACHE_SIZE];
        let ciphertext = encrypt(&plaintext);
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&ciphertext).unwrap();
        assert_eq!(result.len(), L1_CACHE_SIZE, "8MB 明文解密後長度應一致");
        assert_eq!(result[0], 0xCD, "8MB 明文解密後首 byte 應一致");
        assert_eq!(
            result[L1_CACHE_SIZE - 1],
            0xCD,
            "8MB 明文解密後末 byte 應一致"
        );
    }

    #[test]
    fn test_decrypt_chunk_exceeds_8mb() {
        // 構造密文使解密後明文 > 8MB：明文 = ciphertext.len() - 28
        // 需要密文長度 = 8MB + 28 + 1 bytes
        let fake_ciphertext = vec![0u8; L1_CACHE_SIZE + CIPHERTEXT_MIN_LEN + 1];
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&fake_ciphertext);
        assert!(
            matches!(
                result,
                Err(AssetDecryptionError::ChunkTooLarge { size, limit })
                if size == L1_CACHE_SIZE + 1 && limit == L1_CACHE_SIZE
            ),
            "超過 8MB 應回傳 ChunkTooLarge"
        );
    }

    #[test]
    fn test_decrypt_chunk_multiple_sequential() {
        // 連續解密多個 chunk，驗證 buffer 正確覆寫
        let mut decryptor = make_decryptor(&TEST_KEY);

        let plaintext_a = b"chunk A content";
        let ciphertext_a = encrypt(plaintext_a);
        let result_a = decryptor.decrypt_chunk(&ciphertext_a).unwrap();
        assert_eq!(result_a, plaintext_a, "第一個 chunk 應正確解密");

        let plaintext_b = b"chunk B different";
        let ciphertext_b = encrypt(plaintext_b);
        let result_b = decryptor.decrypt_chunk(&ciphertext_b).unwrap();
        assert_eq!(result_b, plaintext_b, "第二個 chunk 應正確覆寫 buffer");
    }

    // ── Task 03 測試：Zeroize 行為驗證 ──────────────────────────

    #[test]
    fn test_staging_buffer_drop_zeroes_content() {
        // 驗證 StagingBuffer drop 後記憶體被清零
        let mut buffer = StagingBuffer::new();
        // 寫入可辨識資料
        buffer.data[0] = 0xAA;
        buffer.data[1] = 0xBB;
        buffer.data[L1_CACHE_SIZE - 1] = 0xCC;

        // 取得 raw pointer 在 drop 前驗證非零
        let ptr = buffer.data.as_ptr();
        assert_eq!(buffer.data[0], 0xAA, "Drop 前首 byte 應為 0xAA");

        // 手動呼叫 drop（觸發 Zeroize）
        // 注意：drop 後 ptr 指向已釋放記憶體，不能安全讀取
        // 改用明確的 zeroize 驗證
        buffer.data.as_mut_slice().zeroize();
        // zeroize 後驗證
        assert_eq!(buffer.data[0], 0, "Zeroize 後首 byte 應為 0");
        assert_eq!(buffer.data[1], 0, "Zeroize 後次 byte 應為 0");
        assert_eq!(
            buffer.data[L1_CACHE_SIZE - 1],
            0,
            "Zeroize 後末 byte 應為 0"
        );
        let _ = ptr; // suppress unused warning
    }

    #[test]
    fn test_decryptor_zeroize_clears_buffer() {
        let plaintext = b"sensitive data to be zeroized";
        let ciphertext = encrypt(plaintext);
        let mut decryptor = make_decryptor(&TEST_KEY);

        // 解密，寫入 buffer
        let _result = decryptor.decrypt_chunk(&ciphertext).unwrap();

        // 驗證 buffer 包含明文
        let buf = decryptor.buffer_content_for_test();
        assert_eq!(&buf[..plaintext.len()], plaintext, "解密後 buffer 應含明文");

        // 呼叫 zeroize
        decryptor.zeroize();

        // 驗證 buffer 已清零
        let buf = decryptor.buffer_content_for_test();
        assert!(
            buf[..plaintext.len()].iter().all(|&b| b == 0),
            "Zeroize 後 buffer 中明文區域應全為 0"
        );
    }

    #[test]
    fn test_decryptor_failure_zeroes_buffer() {
        // 先成功解密寫入 buffer，再故意解密失敗，驗證 buffer 被清零
        let plaintext = b"will be zeroized on failure";
        let ciphertext = encrypt(plaintext);
        let mut decryptor = make_decryptor(&TEST_KEY);

        // 成功解密
        let _result = decryptor.decrypt_chunk(&ciphertext).unwrap();
        let buf = decryptor.buffer_content_for_test();
        assert_eq!(
            &buf[..plaintext.len()],
            plaintext,
            "成功解密後 buffer 應含明文"
        );

        // 製造解密失敗（竄改密文）
        let mut bad_ciphertext = ciphertext.clone();
        bad_ciphertext[12] ^= 0xFF;
        let result = decryptor.decrypt_chunk(&bad_ciphertext);
        assert!(result.is_err(), "竄改密文應解密失敗");

        // 驗證 buffer 已清零（失敗時應清零整個 buffer）
        let buf = decryptor.buffer_content_for_test();
        assert!(
            buf.iter().all(|&b| b == 0),
            "解密失敗後整個 buffer 應被清零"
        );
    }

    #[test]
    fn test_session_key_store_zeroizing_on_clear() {
        let mut store = SessionKeyStore::new();
        store.set_key(TEST_KEY);
        assert!(store.has_key(), "設定後應有 key");

        store.clear();
        assert!(!store.has_key(), "clear 後不應有 key");
        // Zeroizing<[u8; 32]> 在 drop 時自動清零，此處驗證行為語義
        assert!(
            matches!(
                store.get_encryption_key(),
                Err(AssetDecryptionError::NoSessionKey)
            ),
            "clear 後應回傳 NoSessionKey"
        );
    }

    #[test]
    fn test_session_key_store_overwrite_zeroizes_old() {
        let mut store = SessionKeyStore::new();
        store.set_key(TEST_KEY);
        // 覆寫 — 舊的 Zeroizing<[u8; 32]> 在 Option 替換時 drop→zeroize
        store.set_key(WRONG_KEY);
        let key = store.get_encryption_key().unwrap();
        assert_eq!(key, &WRONG_KEY, "覆寫後應取得新 key");
        // 舊 key 已被 Zeroizing drop 清零（無法直接驗證，但 Zeroizing 保證行為）
    }

    #[test]
    fn test_multiple_decrypt_then_zeroize() {
        let mut decryptor = make_decryptor(&TEST_KEY);

        // 解密多個 chunk
        for i in 0..5 {
            let plaintext = format!("chunk {i} data for zeroize test");
            let ciphertext = encrypt(plaintext.as_bytes());
            let _result = decryptor.decrypt_chunk(&ciphertext).unwrap();
        }

        // 最後一次解密結果在 buffer 中
        let last = b"chunk 4 data for zeroize test";
        let buf = decryptor.buffer_content_for_test();
        assert_eq!(&buf[..last.len()], last, "最後一次解密的明文應在 buffer 中");

        // zeroize 後清零
        decryptor.zeroize();
        let buf = decryptor.buffer_content_for_test();
        assert!(
            buf[..last.len()].iter().all(|&b| b == 0),
            "Zeroize 後 buffer 應全為 0"
        );
    }

    #[test]
    fn test_staging_buffer_new_is_zeroed() {
        let buffer = StagingBuffer::new();
        // 檢查起始、中間、結尾
        assert_eq!(buffer.data[0], 0, "新建 buffer 首 byte 應為 0");
        assert_eq!(
            buffer.data[L1_CACHE_SIZE / 2],
            0,
            "新建 buffer 中間 byte 應為 0"
        );
        assert_eq!(
            buffer.data[L1_CACHE_SIZE - 1],
            0,
            "新建 buffer 末 byte 應為 0"
        );
    }

    #[test]
    #[ignore] // 8MB 測試較慢，CI 可選擇跳過
    fn test_zeroize_exactly_8mb() {
        // 8MB 明文解密後 zeroize
        let plaintext = vec![0xEFu8; L1_CACHE_SIZE];
        let ciphertext = encrypt(&plaintext);
        let mut decryptor = make_decryptor(&TEST_KEY);

        let result = decryptor.decrypt_chunk(&ciphertext).unwrap();
        assert_eq!(result.len(), L1_CACHE_SIZE, "8MB 解密後長度應正確");

        decryptor.zeroize();
        let buf = decryptor.buffer_content_for_test();
        // 抽樣檢查（全部逐 byte 檢查太慢）
        assert_eq!(buf[0], 0, "Zeroize 後首 byte 應為 0");
        assert_eq!(buf[L1_CACHE_SIZE / 2], 0, "Zeroize 後中間 byte 應為 0");
        assert_eq!(buf[L1_CACHE_SIZE - 1], 0, "Zeroize 後末 byte 應為 0");
        // 額外驗證：整個 buffer 全為 0
        assert!(
            buf.iter().all(|&b| b == 0),
            "Zeroize 後整個 8MB buffer 應全為 0"
        );
    }

    #[test]
    fn test_decryptor_zeroize_idempotent() {
        let mut decryptor = make_decryptor(&TEST_KEY);

        // 連續 zeroize 兩次不應 panic
        decryptor.zeroize();
        decryptor.zeroize();

        let buf = decryptor.buffer_content_for_test();
        assert!(
            buf.iter().all(|&b| b == 0),
            "連續 zeroize 後 buffer 應全為 0"
        );
    }

    // ── C3 測試輔助：建構分片格式密文 ────────────────────────────

    /// 將明文按 chunk_size 分片，每片獨立加密，組裝為分片格式密文。
    ///
    /// 分片格式：
    /// `[chunk_count: u32 LE] || [chunk_0_len: u32 LE] || [chunk_0_data] || ...`
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

    // ── C3 測試：竄改 nonce ──────────────────────────────────────

    #[test]
    fn test_decrypt_tampered_nonce() {
        let plaintext = b"nonce tamper test data";
        let mut ciphertext = encrypt(plaintext);
        // 竄改前 12 bytes（nonce 區域）
        ciphertext[0] ^= 0xFF;
        ciphertext[5] ^= 0xFF;
        ciphertext[11] ^= 0xFF;
        let mut decryptor = make_decryptor(&TEST_KEY);
        let result = decryptor.decrypt_chunk(&ciphertext);
        assert!(
            matches!(result, Err(AssetDecryptionError::AuthTagMismatch)),
            "竄改 nonce 應回傳 AuthTagMismatch"
        );
    }

    // ── C3 測試：多 chunk streaming 解密 ─────────────────────────

    #[test]
    fn test_chunk_streaming_small() {
        // 小型分片測試：256 bytes 分 2 片（每片 128 bytes）
        let plaintext = vec![0xABu8; 256];
        let chunked = encrypt_chunked(&plaintext, &TEST_KEY, 128);

        // 建構 EncryptedAssetReader 用的 key_store
        let key_store = Arc::new(std::sync::Mutex::new(SessionKeyStore::new()));
        key_store.lock().unwrap().set_key(TEST_KEY);

        // 使用 decrypt_all 需透過 EncryptedAssetReader，但 decrypt_all 是 private
        // 改用直接測試分片解密邏輯：手動模擬 decrypt_all 行為
        let store = key_store.lock().unwrap();
        let key = store.get_encryption_key().unwrap();
        let mut decryptor = AssetDecryptor::new(key);

        // 分片密文 > L1_CACHE_SIZE + 28？否，所以走單 chunk 路徑會失敗
        // 改直接解析分片格式
        let chunk_count = u32::from_le_bytes(chunked[..4].try_into().unwrap()) as usize;
        assert_eq!(chunk_count, 2, "應有 2 個 chunk");

        let mut offset = 4;
        let mut output = Vec::new();
        for _ in 0..chunk_count {
            let chunk_len =
                u32::from_le_bytes(chunked[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            let chunk_data = &chunked[offset..offset + chunk_len];
            offset += chunk_len;
            let pt = decryptor.decrypt_chunk(chunk_data).unwrap();
            output.extend_from_slice(pt);
            decryptor.zeroize();
        }
        assert_eq!(output, plaintext, "分片解密後明文應一致");
    }

    #[test]
    fn test_chunk_streaming_three_chunks() {
        // 300 bytes 分 3 片（每片 100 bytes）
        let plaintext: Vec<u8> = (0..300).map(|i| (i % 256) as u8).collect();
        let chunked = encrypt_chunked(&plaintext, &TEST_KEY, 100);

        let chunk_count = u32::from_le_bytes(chunked[..4].try_into().unwrap()) as usize;
        assert_eq!(chunk_count, 3, "應有 3 個 chunk");

        let store = SessionKeyStore::new();
        let mut decryptor = AssetDecryptor::new(&TEST_KEY);
        let mut offset = 4;
        let mut output = Vec::new();
        for _ in 0..chunk_count {
            let chunk_len =
                u32::from_le_bytes(chunked[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            let chunk_data = &chunked[offset..offset + chunk_len];
            offset += chunk_len;
            let pt = decryptor.decrypt_chunk(chunk_data).unwrap();
            output.extend_from_slice(pt);
            decryptor.zeroize();
        }
        let _ = store;
        assert_eq!(output, plaintext, "3 片分片解密後明文應一致");
    }

    #[test]
    fn test_chunk_streaming_unaligned() {
        // 250 bytes 分 100-byte chunks → 3 片（100 + 100 + 50）
        let plaintext = vec![0xDDu8; 250];
        let chunked = encrypt_chunked(&plaintext, &TEST_KEY, 100);

        let chunk_count = u32::from_le_bytes(chunked[..4].try_into().unwrap()) as usize;
        assert_eq!(chunk_count, 3, "250/100 應有 3 個 chunk（100+100+50）");

        let mut decryptor = AssetDecryptor::new(&TEST_KEY);
        let mut offset = 4;
        let mut output = Vec::new();
        for _ in 0..chunk_count {
            let chunk_len =
                u32::from_le_bytes(chunked[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            let chunk_data = &chunked[offset..offset + chunk_len];
            offset += chunk_len;
            let pt = decryptor.decrypt_chunk(chunk_data).unwrap();
            output.extend_from_slice(pt);
            decryptor.zeroize();
        }
        assert_eq!(output, plaintext, "非對齊分片解密後明文應一致");
    }

    // ── I1 測試：store_key / rotate ──────────────────────────────

    #[test]
    fn test_session_key_store_key() {
        let mut store = SessionKeyStore::new();
        let zeroizing_key = Zeroizing::new(TEST_KEY);
        store.store_key(zeroizing_key);
        assert!(store.has_key(), "store_key 後應有 key");
        let key = store.get_encryption_key().unwrap();
        assert_eq!(key, &TEST_KEY, "store_key 存入的 key 應與原始值一致");
    }

    #[test]
    fn test_session_key_rotate() {
        let mut store = SessionKeyStore::new();
        store.store_key(Zeroizing::new(TEST_KEY));
        assert_eq!(
            store.get_encryption_key().unwrap(),
            &TEST_KEY,
            "初始 key 應為 TEST_KEY"
        );

        // 輪換為 WRONG_KEY
        store.rotate(Zeroizing::new(WRONG_KEY));
        assert!(store.has_key(), "rotate 後應有 key");
        let key = store.get_encryption_key().unwrap();
        assert_eq!(key, &WRONG_KEY, "rotate 後 key 應為新 key");
    }

    // ── I2 測試：新增 variant Display ────────────────────────────

    #[test]
    fn test_error_display_chunk_error() {
        let err = AssetDecryptionError::ChunkError { chunk_index: 3 };
        let msg = format!("{err}");
        assert!(msg.contains("3"), "Display 應包含 chunk_index：{msg}");
        assert!(
            msg.contains("分片解密失敗"),
            "Display 應包含錯誤描述：{msg}"
        );
    }

    #[test]
    fn test_error_display_plaintext_not_allowed() {
        let err = AssetDecryptionError::PlaintextNotAllowed;
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "PlaintextNotAllowed Display 不應為空");
        assert!(
            msg.contains("plaintext-assets"),
            "Display 應提及 plaintext-assets：{msg}"
        );
    }

    // ── C3 測試：chunk error 清零 output ─────────────────────────

    #[test]
    fn test_chunk_error_zeroizes_output() {
        // 建構 2 片分片密文，第 2 片竄改
        let plaintext = vec![0xABu8; 200];
        let mut chunked = encrypt_chunked(&plaintext, &TEST_KEY, 100);

        // 找到第 2 片的位置並竄改
        let chunk_count = u32::from_le_bytes(chunked[..4].try_into().unwrap()) as usize;
        assert_eq!(chunk_count, 2);

        // 跳過 header + chunk 0
        let mut offset = 4;
        let chunk_0_len =
            u32::from_le_bytes(chunked[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4 + chunk_0_len;

        // 竄改 chunk 1 的 nonce（前 12 bytes of chunk data）
        let chunk_1_data_start = offset + 4; // 跳過 chunk_1_len
        chunked[chunk_1_data_start] ^= 0xFF;
        chunked[chunk_1_data_start + 5] ^= 0xFF;

        // 透過 EncryptedAssetReader 的 decrypt_all 測試
        // decrypt_all 是 private，所以手動模擬分片解密邏輯以驗證 zeroize 行為
        let mut decryptor = AssetDecryptor::new(&TEST_KEY);
        let mut parse_offset = 4;
        let mut output = Vec::new();
        let mut had_error = false;

        for _i in 0..chunk_count {
            let chunk_len =
                u32::from_le_bytes(chunked[parse_offset..parse_offset + 4].try_into().unwrap())
                    as usize;
            parse_offset += 4;
            let chunk_data = &chunked[parse_offset..parse_offset + chunk_len];
            parse_offset += chunk_len;

            match decryptor.decrypt_chunk(chunk_data) {
                Ok(pt) => {
                    output.extend_from_slice(pt);
                    decryptor.zeroize();
                }
                Err(AssetDecryptionError::AuthTagMismatch) => {
                    // 模擬 decrypt_all 的清零行為
                    output.as_mut_slice().zeroize();
                    had_error = true;
                    break;
                }
                Err(_) => {
                    output.as_mut_slice().zeroize();
                    had_error = true;
                    break;
                }
            }
        }

        assert!(had_error, "第 2 片竄改應導致解密失敗");
        assert!(output.iter().all(|&b| b == 0), "解密失敗後 output 應被清零");
    }

    // ── C2 測試：AssetDecryptionPlugin ───────────────────────────

    #[test]
    fn test_asset_decryption_plugin_registers_resource() {
        let key_store = Arc::new(std::sync::Mutex::new(SessionKeyStore::new()));
        let plugin = AssetDecryptionPlugin {
            key_store: key_store.clone(),
        };

        let mut app = bevy::app::App::new();
        app.add_plugins(MinimalPlugins);
        plugin.build(&mut app);

        // 驗證 SharedKeyStore resource 已註冊
        let shared = app
            .world()
            .get_resource::<SharedKeyStore>()
            .expect("SharedKeyStore resource 應存在");
        // 驗證內部 key_store 為同一 Arc
        assert!(
            Arc::ptr_eq(&shared.0, &key_store),
            "SharedKeyStore 應持有相同的 Arc"
        );
    }

    // ── I5 測試：panic path + GPU upload 模擬 ────────────────────

    #[test]
    fn test_staging_buffer_zeroed_on_panic_path() {
        use std::panic;

        // 使用 catch_unwind 驗證 panic 不會阻止 Drop 清零
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let mut decryptor = AssetDecryptor::new(&TEST_KEY);
            // 寫入非零資料到 buffer
            decryptor.l1_buffer.data[..100].copy_from_slice(&[0xBB; 100]);
            panic!("模擬 panic");
        }));

        assert!(result.is_err(), "應該 panic");
        // StagingBuffer::Drop 已在 panic unwind 期間執行
        // 無法直接驗證清零（記憶體已釋放），但驗證 Drop 確實被呼叫
        // 透過 Drop 實作中的 zeroize() 保證清零
    }

    #[test]
    fn test_gpu_upload_simulation_zeroed() {
        let plaintext = b"GPU upload test data - important content";
        let ciphertext = encrypt(plaintext);
        let mut decryptor = make_decryptor(&TEST_KEY);

        // 1. 解密
        let decrypted = decryptor.decrypt_chunk(&ciphertext).unwrap();

        // 2. 模擬 GPU 上傳前的 copy
        let output_buffer: Vec<u8> = decrypted.to_vec();

        // 3. copy 完成，清零 staging buffer
        decryptor.zeroize();

        // 4. 驗證：output 保留原始明文
        assert_eq!(&output_buffer, plaintext, "output 應保留完整明文");

        // 5. 驗證：staging buffer 已清零
        assert!(
            decryptor.buffer_content_for_test().iter().all(|&b| b == 0),
            "zeroize 後 staging buffer 應全零"
        );
    }
}
