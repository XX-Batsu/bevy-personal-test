//! OTA Session Key 橋接層 — SessionKeyStore 定義與 HKDF 衍生介面。
//!
//! 提供 [`SessionKeyStore`] 作為 Bevy Resource，儲存透過 ECDH 握手衍生的
//! OTA encryption key。Drop 時自動 zeroize 金鑰材料。
//!
//! # 域分離
//! OTA HKDF info 為 [`OTA_HKDF_INFO`]（`b"ota-encryption-key"`），
//! 與 session key（`b"aes-key"`）和 bytecode（`b"bevy-game-bytecode-v1"`）各自獨立。

use crypto;
use zeroize::Zeroize;

/// Session key 相關錯誤
///
/// `CryptoError`（Phase 4）不含 `KeyNotInitialized` variant，
/// 因此定義獨立 enum。`DerivationFailed` 對應上游
/// `CryptoError::HkdfError(String)` 的映射。
#[derive(Debug)]
pub enum SessionKeyError {
    /// Session key 尚未透過 `store_from_ecdh()` 初始化
    KeyNotInitialized,
    /// HKDF 衍生失敗，內含上游 `CryptoError::HkdfError` 的訊息
    DerivationFailed(String),
}

impl std::fmt::Display for SessionKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyNotInitialized => write!(f, "Session key 尚未初始化"),
            Self::DerivationFailed(msg) => write!(f, "Session key 衍生失敗：{}", msg),
        }
    }
}

impl std::error::Error for SessionKeyError {}

/// OTA encryption key 衍生的 HKDF info 常數（域分離標籤）。
///
/// 與 session key 的 `b"aes-key"`、bytecode 的
/// `b"bevy-game-bytecode-v1"` 三者各自獨立，不可混用。
pub const OTA_HKDF_INFO: &[u8] = b"ota-encryption-key";

/// OTA Session key 儲存庫，作為 Bevy Resource 插入。
///
/// 在 Drop 時自動 zeroize 金鑰材料。內部使用
/// `crypto::derive_session_key()` 衍生 encryption key。
///
/// # 設計決策
/// - `shared_secret` 型別 `&[u8; 32]`：對齊上游 `crypto::derive_session_key(ikm: &[u8; 32], ...)`
/// - salt = `build_hkdf_salt(client_public_key, server_public_key)`（64 bytes，動態）
/// - info = `OTA_HKDF_INFO`（`b"ota-encryption-key"`，靜態域分離標籤）
pub struct SessionKeyStore {
    encryption_key: [u8; 32],
    initialized: bool,
}

impl SessionKeyStore {
    /// 建立未初始化的 `SessionKeyStore`
    pub fn new() -> Self {
        Self {
            encryption_key: [0u8; 32],
            initialized: false,
        }
    }

    /// 從 ECDH shared_secret 衍生並儲存 encryption key。
    ///
    /// 內部呼叫 `crypto::derive_session_key(shared_secret, &salt, OTA_HKDF_INFO)`，
    /// 其中 `salt = build_hkdf_salt(client_public_key, server_public_key)`（64 bytes）。
    ///
    /// # 錯誤
    /// - [`SessionKeyError::DerivationFailed`]：上游 `CryptoError::HkdfError` 映射
    pub fn store_from_ecdh(
        &mut self,
        shared_secret: &[u8; 32],
        client_public_key: &[u8; 32],
        server_public_key: &[u8; 32],
    ) -> Result<(), SessionKeyError> {
        let salt = crypto::build_hkdf_salt(client_public_key, server_public_key);
        self.encryption_key = crypto::derive_session_key(shared_secret, &salt, OTA_HKDF_INFO)
            .map_err(|e| SessionKeyError::DerivationFailed(e.to_string()))?;
        self.initialized = true;
        tracing::info!("Session key 已從 ECDH shared_secret 衍生並儲存");
        Ok(())
    }

    /// 取得目前的 encryption key（若未初始化回傳 Err）。
    ///
    /// # 錯誤
    /// - [`SessionKeyError::KeyNotInitialized`]：尚未呼叫 `store_from_ecdh()`
    pub fn get_encryption_key(&self) -> Result<&[u8; 32], SessionKeyError> {
        if !self.initialized {
            return Err(SessionKeyError::KeyNotInitialized);
        }
        Ok(&self.encryption_key)
    }

    /// 輪替 key：zeroize 舊 key，從新 secret 衍生新 key。
    ///
    /// 流程：先 `self.encryption_key.zeroize()`，再呼叫 `store_from_ecdh()`。
    /// 若衍生失敗，舊 key 已被清零，`initialized` 為 `false`。
    ///
    /// # 錯誤
    /// - [`SessionKeyError::DerivationFailed`]：同 `store_from_ecdh()`
    pub fn rotate(
        &mut self,
        new_shared_secret: &[u8; 32],
        client_public_key: &[u8; 32],
        server_public_key: &[u8; 32],
    ) -> Result<(), SessionKeyError> {
        self.encryption_key.zeroize();
        self.initialized = false;
        self.store_from_ecdh(new_shared_secret, client_public_key, server_public_key)
    }

    /// 查詢是否已透過 `store_from_ecdh()` 完成初始化
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

impl Default for SessionKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SessionKeyStore {
    fn drop(&mut self) {
        self.encryption_key.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock 測試資料（32 bytes，模擬 ECDH shared_secret 與公鑰）
    const MOCK_SECRET_1: [u8; 32] = [0x11u8; 32];
    const MOCK_SECRET_2: [u8; 32] = [0x22u8; 32];
    const MOCK_CLIENT_PK: [u8; 32] = [0xAAu8; 32];
    const MOCK_SERVER_PK: [u8; 32] = [0xBBu8; 32];

    #[test]
    fn test_store_and_get_key() {
        let mut store = SessionKeyStore::new();
        store
            .store_from_ecdh(&MOCK_SECRET_1, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        let key = store.get_encryption_key().unwrap();
        assert_eq!(key.len(), 32);
        // HKDF 衍生結果不應等於原始 secret
        assert_ne!(key, &MOCK_SECRET_1);
    }

    #[test]
    fn test_get_key_before_init() {
        let store = SessionKeyStore::new();
        let result = store.get_encryption_key();
        assert!(matches!(result, Err(SessionKeyError::KeyNotInitialized)));
    }

    #[test]
    fn test_rotate_updates_key() {
        let mut store = SessionKeyStore::new();
        store
            .store_from_ecdh(&MOCK_SECRET_1, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        let key_before = *store.get_encryption_key().unwrap();
        store
            .rotate(&MOCK_SECRET_2, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        let key_after = *store.get_encryption_key().unwrap();
        // 兩個不同的 secret 應衍生出不同的 key
        assert_ne!(key_before, key_after);
    }

    #[test]
    fn test_rotate_preserves_initialized() {
        let mut store = SessionKeyStore::new();
        store
            .store_from_ecdh(&MOCK_SECRET_1, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        store
            .rotate(&MOCK_SECRET_2, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        assert!(store.is_initialized());
    }

    #[test]
    fn test_is_initialized_lifecycle() {
        let mut store = SessionKeyStore::new();
        assert!(!store.is_initialized());
        store
            .store_from_ecdh(&MOCK_SECRET_1, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        assert!(store.is_initialized());
    }

    #[test]
    fn test_hkdf_deterministic() {
        let mut store1 = SessionKeyStore::new();
        let mut store2 = SessionKeyStore::new();
        store1
            .store_from_ecdh(&MOCK_SECRET_1, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        store2
            .store_from_ecdh(&MOCK_SECRET_1, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        let key1 = *store1.get_encryption_key().unwrap();
        let key2 = *store2.get_encryption_key().unwrap();
        // 相同 secret + 相同 salt/info → 相同 key
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_derived_key_differs_from_secret() {
        let mut store = SessionKeyStore::new();
        store
            .store_from_ecdh(&MOCK_SECRET_1, &MOCK_CLIENT_PK, &MOCK_SERVER_PK)
            .unwrap();
        let key = store.get_encryption_key().unwrap();
        // HKDF 衍生結果不應為恆等映射
        assert_ne!(key, &MOCK_SECRET_1);
    }

    #[test]
    fn test_is_initialized_false_before_store() {
        let store = SessionKeyStore::new();
        assert!(!store.is_initialized());
    }
}
