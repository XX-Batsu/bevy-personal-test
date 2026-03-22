//! bytecode_compiler 錯誤型別定義
//!
//! 三層錯誤結構：
//! - [`FormatError`] — 格式解析錯誤（parse_header / deserialize_metadata）
//! - [`CompileError`] — 編譯 .rhai → .rhai.bc 錯誤
//! - [`LoadError`] — 載入 .rhai.bc 錯誤（包裝 FormatError + 業務錯誤）

use crypto::CryptoError;

/// 格式解析錯誤（§5.2）— 對應 parse_header() 與 deserialize_metadata() 的失敗原因
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// Magic Number 不是 "RHBC"
    #[error("無效的 Magic Number：預期 RHBC，得到 {0:?}")]
    InvalidMagic([u8; 4]),

    /// 資料長度不足以包含必要欄位
    #[error("資料截斷：至少需要 {expected} bytes，僅有 {got}")]
    TruncatedData { expected: usize, got: usize },

    /// bincode 反序列化 ScriptMetadata 失敗
    #[error("Metadata 反序列化失敗：{0}")]
    MetadataDeserializationFailed(#[from] bincode::Error),

    /// Metadata Length 欄位指向超出資料範圍
    #[error("Metadata Length 超出資料範圍")]
    MetadataLengthOutOfRange,
}

/// 編譯 .rhai → .rhai.bc 的錯誤（§5.1）
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// Rhai 語法解析失敗
    #[error("Rhai 編譯失敗：{0}")]
    RhaiParseFailed(#[from] rhai::ParseError),

    /// 加密或簽章操作失敗
    #[error("加密失敗：{0}")]
    EncryptionFailed(#[from] CryptoError),

    /// 原始碼為空字串
    #[error("source 為空字串")]
    EmptySource,
}

/// 載入 .rhai.bc 的錯誤（§5.3）— 對應各載入步驟的失敗原因
///
/// 格式解析錯誤統一由 FormatError 承接（見 file-format.md），
/// 避免 LoadError 與 FormatError 維護同名 variant（如 InvalidMagic）。
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// 格式解析失敗（Magic/截斷/Metadata 反序列化/MetadataLen 越界）
    #[error("格式解析失敗：{0}")]
    Format(#[from] FormatError),

    /// Version major 不匹配
    #[error("版本不相容：執行期 major={expected}，bytecode major={got}")]
    IncompatibleVersion { expected: u8, got: u8 },

    /// 解密失敗（密鑰錯誤或密文被篡改）
    ///
    /// 不使用 `#[from] CryptoError`：手動轉換以避免非解密相關的
    /// CryptoError variant 被錯誤分類。實作端以 `.map_err(LoadError::DecryptionFailed)` 轉換。
    #[error("AES-GCM 解密失敗")]
    DecryptionFailed(CryptoError),

    /// Ed25519 簽章驗證失敗
    #[error("Ed25519 簽章驗證失敗：bytecode 可能被篡改")]
    SignatureVerificationFailed,

    /// 解密後的 payload 無法以 UTF-8 解碼為原始碼
    #[error("原始碼 UTF-8 解碼失敗")]
    SourceDecodeFailed,

    /// 解碼後的原始碼無法通過 Rhai 編譯（語法錯誤或版本不相容）
    #[error("Rhai 編譯失敗：{0}")]
    RhaiCompileFailed(rhai::ParseError),

    /// Debug format bytecode 不允許在 release build 載入
    #[error("Debug format bytecode 不允許在 release build 載入")]
    DebugBuildBytecode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_error_from_rhai_parse_error() {
        // 透過 Engine::compile 觸發 ParseError
        let engine = rhai::Engine::new();
        let parse_err = engine.compile("let x = ;").unwrap_err();
        let compile_err = CompileError::from(parse_err);
        assert!(matches!(compile_err, CompileError::RhaiParseFailed(_)));
        // Display 應包含原始錯誤訊息
        let msg = format!("{compile_err}");
        assert!(msg.contains("Rhai 編譯失敗"));
    }

    #[test]
    fn load_error_from_format_error() {
        let fmt_err = FormatError::MetadataLengthOutOfRange;
        let load_err = LoadError::from(fmt_err);
        assert!(matches!(load_err, LoadError::Format(_)));
        let msg = format!("{load_err}");
        assert!(msg.contains("格式解析失敗"));
    }
}
