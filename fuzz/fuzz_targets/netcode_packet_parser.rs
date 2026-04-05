//! Fuzz target: 網路封包解碼
//!
//! 測試 protocol::codec::decode 對任意 byte 輸入的魯棒性。
//! Wire format: [4 bytes LE u32 length] | [bincode(NetMessage)]
//! 覆蓋：InsufficientData、MessageTooLarge、Deserialize 三條錯誤路徑。

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 對任意位元序列解碼：不應 panic，應回傳 Ok 或 Err
    let _ = protocol::codec::decode(data);
});
