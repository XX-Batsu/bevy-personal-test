//! Bytecode 載入器 — Phase 5 bytecode_compiler 的薄包裝
//!
//! 提供兩條載入路徑：
//! - [`BytecodeLoader::load`]：Release 路徑，載入加密並簽章的 .rhai.bc，回傳 [`ScriptInstance`]
//! - [`BytecodeLoader::load_debug`]：Debug 路徑，直接編譯 Rhai 原始碼，回傳 [`ScriptInstance`]
//!
//! # 設計依據
//! - docs/design/script-engine/05-bytecode/README.md

use crate::lifecycle::ScriptInstance;
use bytecode_compiler::{FormatError, LoadError as BcLoadError};

/// BytecodeLoader 載入錯誤
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// 格式解析失敗（Magic/截斷/Metadata 反序列化/MetadataLen 越界）
    #[error("格式解析失敗：{0}")]
    Format(#[from] FormatError),

    /// 版本不相容
    #[error("版本不相容：執行期 major={expected}，bytecode major={got}")]
    IncompatibleVersion { expected: u8, got: u8 },

    /// AES-GCM 解密失敗
    #[error("AES-GCM 解密失敗：{0}")]
    DecryptionFailed(String),

    /// Ed25519 簽章驗證失敗
    #[error("Ed25519 簽章驗證失敗：bytecode 可能被篡改")]
    SignatureVerificationFailed,

    /// AST 反序列化失敗（UTF-8 解碼或 Rhai 編譯）
    #[error("AST 反序列化失敗")]
    AstDeserializationFailed,

    /// Debug format bytecode 不允許在 release build 載入
    #[error("Debug format bytecode 不允許在 release build 載入")]
    DebugBuildBytecode,

    /// Rhai 編譯失敗（load_debug 路徑）
    #[error("Rhai 編譯失敗：{0}")]
    CompileError(String),

    /// script_id 不可為空
    #[error("script_id 不可為空")]
    EmptyScriptId,
}

/// Bytecode 載入器 — Phase 5 bytecode_compiler 的薄包裝
///
/// 零大小結構體，所有方法為關聯函式（無 &self）。
pub struct BytecodeLoader;

impl BytecodeLoader {
    /// 載入加密並簽章的 .rhai.bc bytecode（Release 路徑）
    ///
    /// 流程：parse_header → version check → AES-GCM 解密 → Ed25519 驗簽 → Rhai compile → ScriptInstance
    ///
    /// # 參數
    /// - `data`：.rhai.bc 二進位資料
    /// - `verifying_key`：Ed25519 驗簽公鑰（32 bytes）
    /// - `decryption_key`：AES-256-GCM 解密金鑰（32 bytes，HKDF 衍生）
    pub fn load(
        data: &[u8],
        verifying_key: &[u8; 32],
        decryption_key: &[u8; 32],
    ) -> Result<ScriptInstance, LoadError> {
        let (metadata, ast) = bytecode_compiler::load(data, verifying_key, decryption_key)
            .map_err(|e| match e {
                BcLoadError::Format(fmt_err) => LoadError::Format(fmt_err),
                BcLoadError::IncompatibleVersion { expected, got } => {
                    LoadError::IncompatibleVersion { expected, got }
                }
                BcLoadError::DecryptionFailed(crypto_err) => {
                    LoadError::DecryptionFailed(crypto_err.to_string())
                }
                BcLoadError::SignatureVerificationFailed => LoadError::SignatureVerificationFailed,
                BcLoadError::SourceDecodeFailed => LoadError::AstDeserializationFailed,
                BcLoadError::RhaiCompileFailed(_) => LoadError::AstDeserializationFailed,
                BcLoadError::DebugBuildBytecode => LoadError::DebugBuildBytecode,
            })?;

        Ok(ScriptInstance::new(
            metadata.script_id,
            ast,
            metadata.priority,
        ))
    }

    /// 直接編譯 Rhai 原始碼並回傳 ScriptInstance（Debug 路徑）
    ///
    /// 跳過所有加密與簽章驗證，僅做 Rhai 語法編譯。
    /// 供開發期快速迭代使用，不應出現在 release build。
    ///
    /// # 參數
    /// - `source`：Rhai 原始碼
    /// - `script_id`：腳本唯一識別碼（不可為空）
    /// - `priority`：執行優先順序（0 = 最高，255 = 最低）
    /// - `engine`：Rhai 引擎（供編譯用）
    pub fn load_debug(
        source: &str,
        script_id: &str,
        priority: u8,
        engine: &rhai::Engine,
    ) -> Result<ScriptInstance, LoadError> {
        // script_id 不可為空
        if script_id.is_empty() {
            return Err(LoadError::EmptyScriptId);
        }
        // Rhai 語法編譯
        let ast = engine
            .compile(source)
            .map_err(|e| LoadError::CompileError(e.to_string()))?;
        Ok(ScriptInstance::new(script_id.to_string(), ast, priority))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytecode_compiler::{compile, FormatError};
    use crypto::signing::SigningKeyPair;
    use rhai::Engine;

    /// 建立確定性測試金鑰：(signing_key, verifying_key, encryption_key)
    fn make_test_keys() -> (SigningKeyPair, [u8; 32], [u8; 32]) {
        let signing_key = SigningKeyPair::from_seed(&[42u8; 32]);
        let verifying_key: [u8; 32] = signing_key.public_key();
        let encryption_key = [1u8; 32];
        (signing_key, verifying_key, encryption_key)
    }

    /// 編譯測試用 bytecode
    fn compile_test_bytecode(
        source: &str,
        script_id: &str,
        priority: u8,
        signing_key: &SigningKeyPair,
        enc_key: &[u8; 32],
    ) -> Vec<u8> {
        compile(source, script_id, priority, signing_key, enc_key)
            .expect("測試用 bytecode 編譯不應失敗")
    }

    // === Debug 路徑（4 個）===

    /// 基本 load_debug 測試：script_id 與 priority 正確設置
    #[test]
    fn test_load_debug_basic() {
        let engine = Engine::new();
        let result = BytecodeLoader::load_debug("fn main() { 42 }", "test_script", 0, &engine);
        let instance = result.unwrap();
        assert_eq!(instance.script_id, "test_script");
        assert_eq!(instance.priority, 0);
    }

    /// load_debug priority 邊界值：255
    #[test]
    fn test_load_debug_priority() {
        let engine = Engine::new();
        let result = BytecodeLoader::load_debug("fn tick(dt) {}", "s", 100, &engine);
        assert_eq!(result.unwrap().priority, 100);
    }

    /// load_debug 遇到 Rhai 語法錯誤時回傳 CompileError
    #[test]
    fn test_load_debug_compile_error() {
        let engine = Engine::new();
        let result = BytecodeLoader::load_debug("fn main( {{{ invalid", "s", 0, &engine);
        assert!(
            matches!(result, Err(LoadError::CompileError(_))),
            "預期 CompileError"
        );
    }

    /// load_debug 遇到空 script_id 時回傳 EmptyScriptId
    #[test]
    fn test_load_debug_empty_script_id() {
        let engine = Engine::new();
        let result = BytecodeLoader::load_debug("fn tick(dt) {}", "", 0, &engine);
        assert!(
            matches!(result, Err(LoadError::EmptyScriptId)),
            "預期 EmptyScriptId"
        );
    }

    // === Release 路徑 — 正常載入（2 個）===

    /// 正常載入 bytecode：script_id 與 priority 正確設置
    #[test]
    fn test_load_valid_bytecode() {
        let (signing_key, verifying_key, enc_key) = make_test_keys();
        let data = compile_test_bytecode("fn main() { 42 }", "s1", 5, &signing_key, &enc_key);
        let instance = BytecodeLoader::load(&data, &verifying_key, &enc_key).unwrap();
        assert_eq!(instance.script_id, "s1");
        assert_eq!(instance.priority, 5);
    }

    /// priority 邊界值：0 與 255
    #[test]
    fn test_load_metadata_priority_boundary() {
        let (signing_key, verifying_key, enc_key) = make_test_keys();

        let data_0 =
            compile_test_bytecode("fn tick(dt) {}", "prio_zero", 0, &signing_key, &enc_key);
        let instance_0 = BytecodeLoader::load(&data_0, &verifying_key, &enc_key).unwrap();
        assert_eq!(instance_0.priority, 0);

        let data_255 =
            compile_test_bytecode("fn tick(dt) {}", "prio_max", 255, &signing_key, &enc_key);
        let instance_255 = BytecodeLoader::load(&data_255, &verifying_key, &enc_key).unwrap();
        assert_eq!(instance_255.priority, 255);
    }

    // === Release 路徑 — 格式錯誤（4 個）===

    /// 空 bytes 回傳 TruncatedData
    #[test]
    fn test_load_empty_bytes() {
        let key = [0u8; 32];
        let result = BytecodeLoader::load(&[], &key, &key);
        assert!(
            matches!(
                result,
                Err(LoadError::Format(FormatError::TruncatedData {
                    expected: 8,
                    got: 0
                }))
            ),
            "預期 TruncatedData {{ expected: 8, got: 0 }}"
        );
    }

    /// 無效 Magic Number 回傳 InvalidMagic
    #[test]
    fn test_load_invalid_magic() {
        let mut data = b"XXXX".to_vec();
        data.extend_from_slice(&[0u8; 100]);
        let result = BytecodeLoader::load(&data, &[0u8; 32], &[0u8; 32]);
        assert!(
            matches!(
                result,
                Err(LoadError::Format(FormatError::InvalidMagic([
                    88, 88, 88, 88
                ])))
            ),
            "預期 InvalidMagic([88,88,88,88])"
        );
    }

    /// 資料截斷（3 bytes）回傳 TruncatedData
    #[test]
    fn test_load_truncated_data() {
        let result = BytecodeLoader::load(&[0u8; 3], &[0u8; 32], &[0u8; 32]);
        assert!(
            matches!(
                result,
                Err(LoadError::Format(FormatError::TruncatedData {
                    expected: 8,
                    got: 3
                }))
            ),
            "預期 TruncatedData {{ expected: 8, got: 3 }}"
        );
    }

    /// 錯誤解密金鑰回傳 DecryptionFailed
    #[test]
    fn test_load_wrong_key_decryption_fail() {
        let (signing_key, verifying_key, enc_key) = make_test_keys();
        let data = compile_test_bytecode("fn main() { 1 }", "s", 0, &signing_key, &enc_key);
        // enc_key = [1u8;32]，使用明確不同的 key 觸發解密失敗
        let wrong_key = [99u8; 32];
        let result = BytecodeLoader::load(&data, &verifying_key, &wrong_key);
        assert!(
            matches!(result, Err(LoadError::DecryptionFailed(_))),
            "預期 DecryptionFailed"
        );
    }

    // === Release 路徑 — 版本與安全（2 個）===

    /// major version 不符回傳 IncompatibleVersion
    #[test]
    fn test_load_version_major_mismatch() {
        let (signing_key, verifying_key, enc_key) = make_test_keys();
        let mut data = compile_test_bytecode("fn main() { 1 }", "s", 0, &signing_key, &enc_key);
        // byte[4] = version_major
        data[4] = 99;
        let result = BytecodeLoader::load(&data, &verifying_key, &enc_key);
        assert!(
            matches!(
                result,
                Err(LoadError::IncompatibleVersion {
                    expected: 1,
                    got: 99
                })
            ),
            "預期 IncompatibleVersion {{ expected: 1, got: 99 }}"
        );
    }

    /// Debug bytecode 不允許被 release load 載入
    #[test]
    fn test_load_debug_bytecode_rejected() {
        let data = bytecode_compiler::compile_debug("fn main() { 1 }", "s", 0)
            .expect("compile_debug 不應失敗");
        let result = BytecodeLoader::load(&data, &[0u8; 32], &[0u8; 32]);
        assert!(
            matches!(result, Err(LoadError::DebugBuildBytecode)),
            "預期 DebugBuildBytecode"
        );
    }

    // === 排序驗證（1 個）===

    /// 多腳本依 priority 排序後順序正確
    #[test]
    fn test_multiple_scripts_priority_sort() {
        let engine = Engine::new();
        let mut instances = vec![
            BytecodeLoader::load_debug("fn tick(dt) {}", "a", 2, &engine).unwrap(),
            BytecodeLoader::load_debug("fn tick(dt) {}", "b", 0, &engine).unwrap(),
            BytecodeLoader::load_debug("fn tick(dt) {}", "c", 1, &engine).unwrap(),
        ];
        instances.sort_by_key(|s| s.priority);
        let priorities: Vec<u8> = instances.iter().map(|s| s.priority).collect();
        assert_eq!(priorities, vec![0, 1, 2]);
    }
}
