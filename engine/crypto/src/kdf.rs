/// HKDF-SHA256 金鑰衍生模組
///
/// 從 ECDH shared secret 衍生 AES-256 session key。
/// 設計對齊 RFC 5869 與 crypto-spec.md 不變式 #4。
use crate::CryptoError;
use hkdf::Hkdf;
use sha2::Sha256;

/// HKDF info 固定常數（用於 session key 衍生的 domain separation）。
/// 此值為跨版本契約，變更將導致既有 session key 不相容。
///
/// 注意：bytecode 加密使用不同的 info（`b"bevy-game-bytecode-v1"`），
/// 見 script-engine/05-bytecode/crypto-spec.md。
pub const HKDF_INFO: &[u8] = b"aes-key";

/// 組裝 HKDF salt：`client_public_key ‖ server_public_key`（64 bytes）。
///
/// 動態 salt 將 session key 綁定至特定握手的公鑰對，防止跨 session 的 key 混用。
/// 客戶端公鑰在前（index 0..32），伺服器公鑰在後（index 32..64）。
///
/// # 參數
/// - `client_public_key`: 客戶端 X25519 公鑰（32 bytes）
/// - `server_public_key`: 伺服器 X25519 公鑰（32 bytes）
///
/// # 回傳
/// - `[u8; 64]`: 組裝後的 salt（client ‖ server）
pub fn build_hkdf_salt(client_public_key: &[u8; 32], server_public_key: &[u8; 32]) -> [u8; 64] {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(client_public_key);
    salt[32..].copy_from_slice(server_public_key);
    salt
}

/// 使用 HKDF-SHA256 從 input key material 衍生 32-byte session key。
///
/// # 參數
/// - `input_key_material`: ECDH 計算得到的 shared secret（32 bytes）
///   （語義等同於 shared secret，design doc 使用 `input_key_material` 命名
///   以對齊 RFC 5869 術語）
/// - `salt`: HKDF salt（動態值，由 `build_hkdf_salt()` 組裝為 64 bytes；
///   空 slice 時 HKDF 內部使用全零 salt，但正式用途不應為空）
/// - `info`: HKDF info context 字串（用於 domain separation，
///   session key 使用 `HKDF_INFO = b"aes-key"`）
///
/// # 回傳
/// - `Ok([u8; 32])`: 衍生的 AES-256 密鑰
/// - `Err(CryptoError::HkdfError)`: HKDF expand 失敗
///
/// # 設計決策
/// - salt 和 info 參數化，不硬編於函數內部
///   → 允許不同 domain（session key vs bytecode key）使用不同參數
///   → 典型用法：`derive_session_key(&ss, &build_hkdf_salt(&c, &s), HKDF_INFO)?`
/// - HKDF extract + expand 兩階段，符合 RFC 5869
/// - 回傳 Result 而非 panic，對齊 crypto crate 統一錯誤處理慣例
pub fn derive_session_key(
    input_key_material: &[u8; 32],
    salt: &[u8],
    info: &[u8],
) -> Result<[u8; 32], CryptoError> {
    let hk = Hkdf::<Sha256>::new(Some(salt), input_key_material);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)?;
    Ok(okm)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 相同輸入 → 相同衍生 key（確定性驗證）
    #[test]
    fn test_hkdf_deterministic() {
        let ikm = [0x42u8; 32];
        let salt = b"test";
        let info = b"key";

        let key1 = derive_session_key(&ikm, salt, info).expect("衍生應成功");
        let key2 = derive_session_key(&ikm, salt, info).expect("衍生應成功");

        assert_eq!(key1, key2, "相同輸入應產生相同衍生 key");
    }

    /// 不同 input_key_material → 不同 key
    #[test]
    fn test_hkdf_different_secrets() {
        let ikm_a = [0x01u8; 32];
        let ikm_b = [0x02u8; 32];
        let salt = b"same-salt";
        let info = b"same-info";

        let key_a = derive_session_key(&ikm_a, salt, info).expect("衍生應成功");
        let key_b = derive_session_key(&ikm_b, salt, info).expect("衍生應成功");

        assert_ne!(key_a, key_b, "不同 IKM 應產生不同 key");
    }

    /// 不同 salt → 不同 key
    #[test]
    fn test_hkdf_different_salt() {
        let ikm = [0xABu8; 32];
        let salt_a = b"salt-a";
        let salt_b = b"salt-b";
        let info = b"info";

        let key_a = derive_session_key(&ikm, salt_a, info).expect("衍生應成功");
        let key_b = derive_session_key(&ikm, salt_b, info).expect("衍生應成功");

        assert_ne!(key_a, key_b, "不同 salt 應產生不同 key");
    }

    /// 不同 info → 不同 key（domain separation 驗證）
    #[test]
    fn test_hkdf_different_info() {
        let ikm = [0xCDu8; 32];
        let salt = b"same-salt";
        let info_a = b"info-a";
        let info_b = b"info-b";

        let key_a = derive_session_key(&ikm, salt, info_a).expect("衍生應成功");
        let key_b = derive_session_key(&ikm, salt, info_b).expect("衍生應成功");

        assert_ne!(
            key_a, key_b,
            "不同 info 應產生不同 key（domain separation）"
        );
    }

    /// 空 salt → Ok，不 panic（HKDF 內部使用全零 salt）
    #[test]
    fn test_hkdf_empty_salt() {
        let ikm = [0xEFu8; 32];
        let info = b"key";

        let result = derive_session_key(&ikm, b"", info);

        assert!(result.is_ok(), "空 salt 不應導致錯誤");
    }

    /// 輸出恰好 32 bytes（型別系統保證，此處為語義確認）
    #[test]
    fn test_hkdf_output_length() {
        let ikm = [0x11u8; 32];
        let salt = b"salt";
        let info = b"info";

        let key = derive_session_key(&ikm, salt, info).expect("衍生應成功");

        assert_eq!(key.len(), 32, "衍生 key 長度應為 32 bytes");
    }

    /// 驗證 salt 組裝為 client ‖ server（拼接正確性）
    #[test]
    fn test_hkdf_build_salt_concatenation() {
        let client = [0xAAu8; 32];
        let server = [0xBBu8; 32];

        let salt = build_hkdf_salt(&client, &server);

        assert_eq!(&salt[..32], &[0xAAu8; 32], "前 32 bytes 應為 client 公鑰");
        assert_eq!(&salt[32..], &[0xBBu8; 32], "後 32 bytes 應為 server 公鑰");
        assert_eq!(salt.len(), 64, "salt 總長度應為 64 bytes");
    }

    /// client/server 互換 → 不同 salt → 不同 key（順序敏感性）
    #[test]
    fn test_hkdf_build_salt_order_matters() {
        let client = [0xAAu8; 32];
        let server = [0xBBu8; 32];
        let ikm = [0x33u8; 32];
        let info = HKDF_INFO;

        let salt_cs = build_hkdf_salt(&client, &server);
        let salt_sc = build_hkdf_salt(&server, &client);

        assert_ne!(salt_cs, salt_sc, "client/server 順序互換應產生不同 salt");

        let key_cs = derive_session_key(&ikm, &salt_cs, info).expect("衍生應成功");
        let key_sc = derive_session_key(&ikm, &salt_sc, info).expect("衍生應成功");

        assert_ne!(key_cs, key_sc, "不同 salt 順序應產生不同衍生 key");
    }

    /// HKDF_INFO 常數值驗證
    #[test]
    fn test_hkdf_info_constant() {
        assert_eq!(HKDF_INFO, b"aes-key", "HKDF_INFO 應為 b\"aes-key\"");
        assert_eq!(HKDF_INFO.len(), 7, "HKDF_INFO 長度應為 7 bytes");
    }
}
