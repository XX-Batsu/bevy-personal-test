//! Fuzz target: bytecode 解碼器
//! 測試 bincode 反序列化對任意輸入的魯棒性

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 嘗試反序列化為 ReplayFrame — 不應 panic
    let _ = bincode::deserialize::<bridge_types::ReplayFrame>(data);
});
