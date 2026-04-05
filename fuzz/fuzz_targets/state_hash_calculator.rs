//! Fuzz target: state hash 計算
//!
//! 測試 bincode 反序列化 EcsMirror + compute_state_hash 對任意輸入的魯棒性。
//! 使用 bincode::Options::with_limit(1 MB) 防止 OOM。

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 限制反序列化大小防止 OOM（上限 1 MB）
    let options = bincode::Options::with_limit(bincode::DefaultOptions::new(), 1024 * 1024);

    // 嘗試從任意 bytes 反序列化 EcsMirror
    if let Ok(mirror) =
        bincode::Options::deserialize::<bridge_types::EcsMirror>(options, data)
    {
        // 萃取欄位呼叫 compute_state_hash（3 參數簽名）
        let rng_state: [u8; 16] = [0u8; 16];
        let _hash = state_hash::compute_state_hash(
            &mirror.entities,
            &rng_state,
            mirror.frame_number,
        );
    }
    // 反序列化失敗時直接 return，不 panic
});
