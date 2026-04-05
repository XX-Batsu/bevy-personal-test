//! Fuzz target: bytecode 解碼器
//!
//! 測試 bytecode_compiler 的 parse_header / load / load_debug 對任意輸入的魯棒性。
//! 三路 fuzz 策略覆蓋 LoadError 全部 6 variant + FormatError 全部 4 variant。

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 路徑 1：純格式解析（覆蓋所有 FormatError variant）
    let _ = bytecode_compiler::parse_header(data);

    // 路徑 2：release 載入路徑（dummy key → 解密必定失敗）
    let dummy_verifying_key = [0u8; 32];
    let dummy_decryption_key = [0u8; 32];
    let _ = bytecode_compiler::load(data, &dummy_verifying_key, &dummy_decryption_key);

    // 路徑 3：debug 明文載入路徑（可完整走到 AST 反序列化）
    let _ = bytecode_compiler::load_debug(data);
});
