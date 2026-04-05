//! Fuzz target: state hash 計算
//! 測試 compute_state_hash 對任意輸入的魯棒性

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    // 使用 fuzz 資料作為 rng_state（取前 16 bytes）
    if data.len() >= 16 {
        let mut rng_state = [0u8; 16];
        rng_state.copy_from_slice(&data[..16]);
        let tick = if data.len() >= 24 {
            u64::from_le_bytes(data[16..24].try_into().unwrap())
        } else {
            0
        };
        let entities = BTreeMap::new();
        let _ = state_hash::compute_state_hash(&entities, &rng_state, tick);
    }
});
