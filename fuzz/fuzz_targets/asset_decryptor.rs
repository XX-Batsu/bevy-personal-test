//! Fuzz target: 資產解密器
//! 測試 AES-GCM 解密對任意密文的魯棒性（不應 panic）

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let key = [0x42u8; 32];
    // 嘗試解密任意輸入 — 應回傳 Err，不應 panic
    let _ = crypto::aes_decrypt(&key, data);
});
