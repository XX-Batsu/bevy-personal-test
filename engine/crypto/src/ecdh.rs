//! X25519 ECDH 金鑰交換模組。
//!
//! 提供 `EcdhKeyPair` 型別，封裝 X25519 ephemeral key pair。
//! `derive_shared_secret` consume self，編譯期保證金鑰僅使用一次。
//! RNG 使用 `OsRng`（WASM 透過 `getrandom` js feature 取得隨機數）。

use crate::CryptoError;
use rand_core::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey};

/// X25519 ECDH 金鑰對。
///
/// 不可 Clone、不可 Serialize。
/// `derive_shared_secret` consume self，編譯期保證金鑰僅使用一次。
/// RNG 使用 `OsRng`（WASM 透過 `getrandom` js feature 取得隨機數）。
pub struct EcdhKeyPair {
    secret: EphemeralSecret,
    public: PublicKey,
}

impl EcdhKeyPair {
    /// 生成新的 ephemeral 金鑰對。
    /// 每次連線應重新生成，不存入 localStorage 或任何持久化存儲。
    /// 參考 §11.7 Session Key 生命週期。
    pub fn generate() -> Self {
        let secret = EphemeralSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// 取得公鑰（32 bytes），用於傳送給對方。
    /// 對應 §11.5 Step 2: client_public_key (32 bytes)。
    pub fn public_key(&self) -> [u8; 32] {
        *self.public.as_bytes()
    }

    /// 計算 shared secret 並消耗自身。
    ///
    /// # 安全性
    /// - consume self 保證 EphemeralSecret 僅使用一次
    /// - all-zero 公鑰 → `Err(CryptoError::InvalidPublicKey)`
    /// - 回傳的 `[u8; 32]` 為裸陣列，**呼叫者負責在 HKDF 衍生完成後立即 zeroize**
    ///   （見 crypto-spec.md 不變式 #2）
    ///
    /// # 參數
    /// - `peer_public`: 對方的 X25519 公鑰（32 bytes）
    ///
    /// # 回傳
    /// - `Ok([u8; 32])`: shared secret（應立即傳給 task-03 `derive_session_key`，使用後 zeroize）
    /// - `Err(CryptoError::InvalidPublicKey)`: 公鑰無效
    pub fn derive_shared_secret(self, peer_public: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
        // 拒絕 all-zero 公鑰（low-order point）
        if peer_public.iter().all(|&b| b == 0) {
            return Err(CryptoError::InvalidPublicKey);
        }
        let peer_pk = PublicKey::from(*peer_public);
        let shared = self.secret.diffie_hellman(&peer_pk);
        Ok(*shared.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alice/Bob 雙方計算出相同 shared secret
    #[test]
    fn test_ecdh_shared_secret_agreement() {
        let alice = EcdhKeyPair::generate();
        let bob = EcdhKeyPair::generate();

        let alice_pk = alice.public_key();
        let bob_pk = bob.public_key();

        let alice_secret = alice
            .derive_shared_secret(&bob_pk)
            .expect("Alice 計算 shared secret 失敗");
        let bob_secret = bob
            .derive_shared_secret(&alice_pk)
            .expect("Bob 計算 shared secret 失敗");

        assert_eq!(
            alice_secret, bob_secret,
            "Alice 與 Bob 的 shared secret 應相同"
        );
    }

    /// 不同金鑰對產生不同 shared secret
    #[test]
    fn test_ecdh_different_pairs_different_secrets() {
        let bob1 = EcdhKeyPair::generate();
        let bob2 = EcdhKeyPair::generate();

        let bob1_pk = bob1.public_key();
        let bob2_pk = bob2.public_key();

        // 因為 derive_shared_secret 會消耗 self，需要各自獨立的 key pair
        let alice1 = EcdhKeyPair::generate();
        let alice2 = EcdhKeyPair::generate();
        let alice1_pk = alice1.public_key();
        let alice2_pk = alice2.public_key();

        let secret1 = alice1
            .derive_shared_secret(&bob1_pk)
            .expect("計算 shared secret 1 失敗");
        let _bob1_secret = bob1
            .derive_shared_secret(&alice1_pk)
            .expect("Bob1 計算失敗");

        let secret2 = alice2
            .derive_shared_secret(&bob2_pk)
            .expect("計算 shared secret 2 失敗");
        let _bob2_secret = bob2
            .derive_shared_secret(&alice2_pk)
            .expect("Bob2 計算失敗");

        assert_ne!(
            secret1, secret2,
            "不同金鑰對的 shared secret 應不同（機率性，collision 幾乎不可能）"
        );
    }

    // test_ecdh_consume_self：
    // `derive_shared_secret` consume self，第二次呼叫會產生編譯錯誤（use of moved value）。
    // 此為編譯期保證，非 runtime test，故以註解方式記錄。
    // 以下程式碼若取消註解，應無法編譯：
    // ```
    // let pair = EcdhKeyPair::generate();
    // let pk = [1u8; 32];
    // let _ = pair.derive_shared_secret(&pk);
    // let _ = pair.derive_shared_secret(&pk); // 編譯錯誤：use of moved value
    // ```

    /// all-zero 公鑰 → Err(CryptoError::InvalidPublicKey)
    #[test]
    fn test_ecdh_all_zero_public_key() {
        let pair = EcdhKeyPair::generate();
        let zero_pk = [0u8; 32];

        let result = pair.derive_shared_secret(&zero_pk);
        assert_eq!(
            result,
            Err(CryptoError::InvalidPublicKey),
            "all-zero 公鑰應回傳 InvalidPublicKey 錯誤"
        );
    }

    /// 公鑰長度為 32 bytes（型別系統保證）
    #[test]
    fn test_ecdh_public_key_is_32_bytes() {
        let pair = EcdhKeyPair::generate();
        let pk = pair.public_key();
        assert_eq!(pk.len(), 32, "公鑰長度應為 32 bytes");
    }

    /// 連續兩次 generate() 產生不同公鑰（OsRng 隨機性驗證）
    #[test]
    fn test_ecdh_repeated_generate_produces_different_keys() {
        let pair_a = EcdhKeyPair::generate();
        let pair_b = EcdhKeyPair::generate();

        assert_ne!(
            pair_a.public_key(),
            pair_b.public_key(),
            "連續生成的金鑰對應產生不同公鑰（collision 幾乎不可能）"
        );
    }
}
