//! bytecode_compiler — .rhai.bc 加密 bytecode 編譯器
//!
//! 對齊 docs/design/script-engine/05-bytecode/README.md §5.1–§5.6。
//! 提供 ScriptMetadata、CompileError、LoadError、FormatError 型別定義，
//! 以及 .rhai.bc 二進位格式常數與組裝函式。

pub mod error;
pub mod format;
pub mod metadata;

// Error types
pub use error::{CompileError, FormatError, LoadError};

// Core API
#[cfg(feature = "debug-mode")]
pub use format::compile_debug;
pub use format::{compile, load, load_debug};

// Metadata types and constants
pub use format::{
    assemble_bytecode, deserialize_metadata, parse_header, BytecodeHeader, AUTH_TAG_SIZE,
    DEBUG_VERSION_MINOR, FORMAT_VERSION_MAJOR, FORMAT_VERSION_MINOR, HEADER_FIXED_SIZE, MAGIC,
    NONCE_SIZE, SIGNATURE_SIZE,
};
pub use metadata::ScriptMetadata;
