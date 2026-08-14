//! 環境變數讀取（build-time 專用）

/// 從環境變數取得 debug 用加密金鑰
///
/// 優先順序：
/// 1. `BEVY_GAME_DEV_KEY` 環境變數（hex 編碼的 32 bytes = 64 hex chars）
/// 2. 預設全零 debug key（僅用於開發環境）
///
/// 錯誤處理：hex 格式錯誤或長度不符時 log warning 並 fallback 至全零
/// （對齊 env-vars.md 不變式 #4：Fallback 必須 log warning）。
///
/// 此函式放在 build 工具層，避免污染 crypto crate 核心職責。
/// **不應**在 crypto crate 或 game logic 中呼叫此函式。
pub fn get_debug_encryption_key() -> [u8; 32] {
    match std::env::var("BEVY_GAME_DEV_KEY") {
        Ok(hex_key) if hex_key.len() == 64 => match hex::decode(&hex_key) {
            Ok(bytes) => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                tracing::debug!("使用 BEVY_GAME_DEV_KEY 環境變數作為 debug key");
                key
            }
            Err(e) => {
                tracing::warn!("BEVY_GAME_DEV_KEY hex 解碼失敗：{e}，使用全零預設值");
                [0u8; 32]
            }
        },
        Ok(hex_key) => {
            tracing::warn!(
                "BEVY_GAME_DEV_KEY 長度錯誤（預期 64 hex 字元，實際 {} 字元），使用全零預設值",
                hex_key.len()
            );
            [0u8; 32]
        }
        Err(_) => {
            tracing::debug!("BEVY_GAME_DEV_KEY 未設定，使用全零預設 debug key");
            [0u8; 32]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// 四個測試共用同一個行程層級環境變數，預設的平行執行會互相覆寫，
    /// 造成非決定性失敗；以此鎖序列化。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 取得環境變數鎖；忽略毒化狀態（前一個測試 panic 不應連累其餘測試）。
    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_no_env_returns_zero_key() {
        let _guard = env_guard();
        // 清除環境變數
        std::env::remove_var("BEVY_GAME_DEV_KEY");
        let key = get_debug_encryption_key();
        assert_eq!(key, [0u8; 32]);
    }

    #[test]
    fn test_valid_hex_key() {
        let _guard = env_guard();
        let hex_key = "aa".repeat(32); // 64 hex chars = 32 bytes of 0xAA
        std::env::set_var("BEVY_GAME_DEV_KEY", &hex_key);
        let key = get_debug_encryption_key();
        assert_eq!(key, [0xAAu8; 32]);
        std::env::remove_var("BEVY_GAME_DEV_KEY");
    }

    #[test]
    fn test_wrong_length_falls_back() {
        let _guard = env_guard();
        std::env::set_var("BEVY_GAME_DEV_KEY", "aabb"); // 太短
        let key = get_debug_encryption_key();
        assert_eq!(key, [0u8; 32]);
        std::env::remove_var("BEVY_GAME_DEV_KEY");
    }

    #[test]
    fn test_invalid_hex_falls_back() {
        let _guard = env_guard();
        let bad_hex = "gg".repeat(32); // 非法 hex
        std::env::set_var("BEVY_GAME_DEV_KEY", &bad_hex);
        let key = get_debug_encryption_key();
        assert_eq!(key, [0u8; 32]);
        std::env::remove_var("BEVY_GAME_DEV_KEY");
    }
}
