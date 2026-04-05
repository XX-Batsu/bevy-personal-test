/// 【L1 純函式】生成 CFG mutation seed。
///
/// blake3(timestamp_LE || commit_hash_utf8) 前 8 bytes 轉為 u64 LE。
/// 不讀取環境變數，不呼叫 tracing。
///
/// 設計規格：12-cfg-mutation/seed-management.md
pub fn generate_seed(timestamp: u64, commit_hash: &str) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&timestamp.to_le_bytes()); // u64 LE 8 bytes
    hasher.update(commit_hash.as_bytes()); // raw UTF-8
    let hash = hasher.finalize();

    u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
}

/// 【L2 環境感知】從環境變數讀取覆蓋 seed（CI 重現用）。
///
/// 優先順序：BEVY_GAME_BUILD_SEED（若設定且合法 u64）→ 直接使用。
/// 否則呼叫 generate_seed() 計算 blake3 值。
/// parse 失敗時 tracing::warn!() + fallback（不 panic）。
///
/// 設計規格：12-cfg-mutation/seed-management.md §resolve_seed
pub fn resolve_seed(commit_hash: &str, timestamp_secs: u64) -> u64 {
    if let Ok(seed_str) = std::env::var("BEVY_GAME_BUILD_SEED") {
        match seed_str.trim().parse::<u64>() {
            Ok(seed) => {
                tracing::debug!("使用 BEVY_GAME_BUILD_SEED 環境變數：{seed}");
                return seed;
            }
            Err(e) => {
                tracing::warn!("BEVY_GAME_BUILD_SEED 無法解析為 u64：{e}，改用 blake3 計算值");
            }
        }
    }

    generate_seed(timestamp_secs, commit_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ════════════════════════════════════════════════════════════════
    // generate_seed() 測試群組 —— 純 blake3 計算，無環境變數
    // ════════════════════════════════════════════════════════════════

    /// 相同 timestamp + commit → 相同 seed
    #[test]
    fn test_generate_seed_deterministic() {
        let seed1 = generate_seed(1_700_000_000, "abc123");
        let seed2 = generate_seed(1_700_000_000, "abc123");
        assert_eq!(seed1, seed2, "相同輸入應產生相同 seed");
    }

    /// 不同 timestamp → 不同 seed（高度機率）
    #[test]
    fn test_generate_different_timestamp_different_seed() {
        let seed1 = generate_seed(1_700_000_000, "abc123");
        let seed2 = generate_seed(1_700_000_001, "abc123");
        assert_ne!(seed1, seed2, "不同 timestamp 應產生不同 seed");
    }

    /// 不同 commit hash → 不同 seed
    #[test]
    fn test_generate_different_commit_different_seed() {
        let seed1 = generate_seed(1_700_000_000, "abc123");
        let seed2 = generate_seed(1_700_000_000, "def456");
        assert_ne!(seed1, seed2, "不同 commit hash 應產生不同 seed");
    }

    /// 手動計算 blake3 hash 驗證 seed 計算正確性
    #[test]
    fn test_generate_seed_matches_blake3() {
        let timestamp: u64 = 42;
        let commit = "deadbeef";

        let mut hasher = blake3::Hasher::new();
        hasher.update(&timestamp.to_le_bytes());
        hasher.update(commit.as_bytes());
        let hash = hasher.finalize();
        let expected = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());

        let actual = generate_seed(timestamp, commit);
        assert_eq!(actual, expected, "seed 必須符合 blake3 二進位計算結果");
    }

    /// timestamp 為 0（最小值）
    #[test]
    fn test_generate_seed_timestamp_zero() {
        let seed = generate_seed(0, "abc123");
        let _ = seed; // 主要驗證不 panic
    }

    /// timestamp 為 u64::MAX（最大值）
    #[test]
    fn test_generate_seed_timestamp_max() {
        let seed = generate_seed(u64::MAX, "abc123");
        let _ = seed;
    }

    /// commit hash 為空字串
    #[test]
    fn test_generate_seed_empty_commit() {
        let seed = generate_seed(1_700_000_000, "");
        let _ = seed;
    }

    /// commit hash 超長（256 chars）
    #[test]
    fn test_generate_seed_long_commit() {
        let long_hash = "a".repeat(256);
        let seed = generate_seed(1_700_000_000, &long_hash);
        let _ = seed;
    }

    /// commit hash 含非 ASCII（中文字元）
    #[test]
    fn test_generate_seed_non_ascii_commit() {
        let seed = generate_seed(1_700_000_000, "提交哈希值");
        let _ = seed;
    }

    /// commit hash 含前後空白
    #[test]
    fn test_generate_seed_commit_with_whitespace() {
        let seed_trimmed = generate_seed(1_700_000_000, "abc123");
        let seed_with_space = generate_seed(1_700_000_000, " abc123 ");
        assert_ne!(
            seed_trimmed, seed_with_space,
            "含空白的 commit hash 應產生不同 seed（blake3 不 trim）"
        );
    }

    // ════════════════════════════════════════════════════════════════
    // resolve_seed() 測試群組 —— 環境變數覆蓋 + fallback
    // ════════════════════════════════════════════════════════════════

    /// 設定 BEVY_GAME_BUILD_SEED → 直接使用該值，跳過 blake3 計算
    #[test]
    #[serial]
    fn test_resolve_seed_env_override() {
        let expected_seed: u64 = 12345678;
        unsafe { std::env::set_var("BEVY_GAME_BUILD_SEED", expected_seed.to_string()) };

        let actual = resolve_seed("任意值", 0);
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };

        assert_eq!(actual, expected_seed, "環境變數覆蓋時應直接使用該 seed");
    }

    /// BEVY_GAME_BUILD_SEED=0 → 合法 u64，直接使用（不視為未設定）
    #[test]
    #[serial]
    fn test_resolve_seed_env_zero() {
        unsafe { std::env::set_var("BEVY_GAME_BUILD_SEED", "0") };

        let actual = resolve_seed("abc123", 1_700_000_000);
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };

        assert_eq!(actual, 0, "BEVY_GAME_BUILD_SEED=0 應視為合法值直接使用");
    }

    /// BEVY_GAME_BUILD_SEED=u64::MAX → 最大合法值
    #[test]
    #[serial]
    fn test_resolve_seed_env_max_u64() {
        unsafe { std::env::set_var("BEVY_GAME_BUILD_SEED", u64::MAX.to_string()) };

        let actual = resolve_seed("abc123", 1_700_000_000);
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };

        assert_eq!(actual, u64::MAX, "BEVY_GAME_BUILD_SEED=u64::MAX 應為合法值");
    }

    /// 未設定 BEVY_GAME_BUILD_SEED → fallback 至 generate_seed() 的 blake3 計算
    #[test]
    #[serial]
    fn test_resolve_seed_no_env_uses_blake3() {
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };
        let actual = resolve_seed("cafebabe", 9999);
        assert_eq!(
            actual,
            expected_blake3_seed(9999, "cafebabe"),
            "未設定環境變數時應使用 blake3 計算"
        );
    }

    /// 輔助函式：計算預期 blake3 seed
    fn expected_blake3_seed(timestamp: u64, commit: &str) -> u64 {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&timestamp.to_le_bytes());
        hasher.update(commit.as_bytes());
        let hash = hasher.finalize();
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
    }

    /// BEVY_GAME_BUILD_SEED 設為非數字 → fallback 至 blake3（不 panic）
    #[test]
    #[serial]
    fn test_resolve_seed_invalid_env_fallback() {
        unsafe { std::env::set_var("BEVY_GAME_BUILD_SEED", "not_a_number") };
        let actual = resolve_seed("test", 1);
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };
        assert_eq!(
            actual,
            expected_blake3_seed(1, "test"),
            "parse 失敗時應 fallback 至 blake3 計算值（不 panic）"
        );
    }

    /// BEVY_GAME_BUILD_SEED 設為負數 → u64 parse 失敗 → fallback
    #[test]
    #[serial]
    fn test_resolve_seed_negative_env_fallback() {
        unsafe { std::env::set_var("BEVY_GAME_BUILD_SEED", "-1") };
        let actual = resolve_seed("neg_test", 42);
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };
        assert_eq!(
            actual,
            expected_blake3_seed(42, "neg_test"),
            "負數 parse 為 u64 失敗時應 fallback"
        );
    }

    /// BEVY_GAME_BUILD_SEED="" → std::env::var 回傳 Ok("")，parse 失敗 → fallback
    #[test]
    #[serial]
    fn test_resolve_seed_empty_env_fallback() {
        unsafe { std::env::set_var("BEVY_GAME_BUILD_SEED", "") };
        let actual = resolve_seed("empty_env", 7);
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };
        assert_eq!(
            actual,
            expected_blake3_seed(7, "empty_env"),
            "空字串 parse 失敗時應 fallback"
        );
    }

    /// BEVY_GAME_BUILD_SEED 含前後空白 → trim 後 parse
    #[test]
    #[serial]
    fn test_resolve_seed_env_with_whitespace() {
        unsafe { std::env::set_var("BEVY_GAME_BUILD_SEED", "  42  ") };

        let actual = resolve_seed("abc", 0);
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };

        assert_eq!(actual, 42, "BEVY_GAME_BUILD_SEED 前後空白應被 trim");
    }

    // ════════════════════════════════════════════════════════════════
    // resolve_seed() 與 generate_seed() 一致性測試
    // ════════════════════════════════════════════════════════════════

    /// 未設定 env var 時，resolve_seed 回傳值必須等於 generate_seed
    #[test]
    #[serial]
    fn test_resolve_seed_equals_generate_seed_without_env() {
        unsafe { std::env::remove_var("BEVY_GAME_BUILD_SEED") };

        let timestamp: u64 = 1_700_000_000;
        let commit = "abc123";

        let gen = generate_seed(timestamp, commit);
        let res = resolve_seed(commit, timestamp);
        assert_eq!(gen, res, "無 env var 時 resolve_seed 應等同 generate_seed");
    }
}
