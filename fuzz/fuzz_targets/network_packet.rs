//! Fuzz target: 網路封包解碼
//! 測試 PlayerInput 反序列化對任意輸入的魯棒性

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = bincode::deserialize::<bridge_types::PlayerInput>(data);
});
