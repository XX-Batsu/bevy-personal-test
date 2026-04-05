//! Fuzz target: 資產解密器
//!
//! 測試 AES-256-GCM 解密對任意密文的魯棒性（不應 panic）。
//! 覆蓋：CiphertextTooShort（< 12 bytes）、AuthTagMismatch 兩條錯誤路徑。

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 使用全零 key：fuzz 目的是驗證不 panic，非驗證解密正確性
    let dummy_key = [0u8; 32];
    // 便利 API：自動從 data 前 12 bytes 提取 nonce
    // 所有失敗路徑應傳回 Err，不應 panic
    let _ = crypto::aes_decrypt(&dummy_key, data);
});
