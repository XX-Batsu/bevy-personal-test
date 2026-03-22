//! bytecode-compiler CLI — 將 .rhai 原始碼編譯為 .rhai.bc 加密 bytecode
//!
//! 支援三個子命令：
//! - `compile`：編譯 .rhai → .rhai.bc（加密 + 簽章）
//! - `compile-debug`：編譯 .rhai → .rhai.bc（debug 模式，無加密）（僅 debug-mode feature）
//! - `verify`：驗證 .rhai.bc 檔案（解密 + 簽章驗證）

use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

#[cfg(feature = "debug-mode")]
use bytecode_compiler::compile_debug;
use bytecode_compiler::{compile, load, CompileError, LoadError};
use crypto::signing::SigningKeyPair;

#[derive(Parser)]
#[command(name = "bytecode-compiler")]
#[command(about = "將 .rhai 原始碼編譯為 .rhai.bc 加密 bytecode")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 編譯 .rhai → .rhai.bc（加密 + 簽章）
    Compile {
        /// 輸入 .rhai 檔案路徑
        input: PathBuf,
        /// 輸出 .rhai.bc 檔案路徑
        output: PathBuf,
        /// Ed25519 signing key 檔案（raw 32 bytes）
        #[arg(long)]
        signing_key: PathBuf,
        /// AES-256 encryption key 檔案（raw 32 bytes）
        #[arg(long)]
        encryption_key: PathBuf,
        /// 腳本唯一識別碼
        #[arg(long)]
        script_id: String,
        /// 執行優先序（0-255，0 = 最高）
        #[arg(long, default_value = "128")]
        priority: u8,
    },
    /// 編譯 .rhai → .rhai.bc（debug 模式，無加密）
    ///
    /// 僅在 `#[cfg(feature = "debug-mode")]` 下可用。
    /// Release build 不包含此子命令。
    #[cfg(feature = "debug-mode")]
    CompileDebug {
        /// 輸入 .rhai 檔案路徑
        input: PathBuf,
        /// 輸出 .rhai.bc 檔案路徑
        output: PathBuf,
        /// 腳本唯一識別碼
        #[arg(long)]
        script_id: String,
        /// 執行優先序（0-255，0 = 最高）
        #[arg(long, default_value = "128")]
        priority: u8,
    },
    /// 驗證 .rhai.bc 檔案（解密 + 簽章驗證）
    Verify {
        /// .rhai.bc 檔案路徑
        input: PathBuf,
        /// Ed25519 public key 檔案（raw 32 bytes）
        #[arg(long)]
        public_key: PathBuf,
        /// AES-256 decryption key 檔案（raw 32 bytes）
        #[arg(long)]
        decryption_key: PathBuf,
    },
}

/// 讀取 32-byte raw key 檔案
///
/// 失敗時輸出錯誤訊息並以 exit(1) 終止程式。
/// 失敗原因：
/// - I/O 錯誤（檔案不存在、無權限等）
/// - 檔案長度 != 32 bytes
fn read_key_file(path: &Path) -> [u8; 32] {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("無法讀取 key 檔案 {}: {e}", path.display());
        process::exit(1);
    });
    if data.len() != 32 {
        eprintln!(
            "Key 檔案 {} 長度錯誤：預期 32 bytes，得到 {} bytes",
            path.display(),
            data.len()
        );
        process::exit(1);
    }
    data.try_into().unwrap()
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compile {
            input,
            output,
            signing_key,
            encryption_key,
            script_id,
            priority,
        } => {
            // 讀取原始碼
            let source = std::fs::read_to_string(&input).unwrap_or_else(|e| {
                eprintln!("無法讀取來源檔案 {}: {e}", input.display());
                process::exit(1);
            });

            // 讀取 key 檔案
            let sign_seed = read_key_file(&signing_key);
            let enc_key = read_key_file(&encryption_key);

            // 建立簽章金鑰對
            let kp = SigningKeyPair::from_seed(&sign_seed);

            // 編譯
            let bytecode = match compile(&source, &script_id, priority, &kp, &enc_key) {
                Ok(b) => b,
                Err(e) => {
                    match &e {
                        CompileError::EmptySource => {
                            eprintln!("編譯失敗：原始碼為空");
                        }
                        CompileError::RhaiParseFailed(parse_err) => {
                            eprintln!("Rhai 編譯失敗：{parse_err}");
                        }
                        CompileError::EncryptionFailed(crypto_err) => {
                            eprintln!("加密失敗：{crypto_err}");
                        }
                    }
                    process::exit(1);
                }
            };

            // 寫入輸出
            std::fs::write(&output, &bytecode).unwrap_or_else(|e| {
                eprintln!("無法寫入輸出檔案 {}: {e}", output.display());
                process::exit(1);
            });

            println!(
                "編譯成功：{} → {}（{} bytes）",
                input.display(),
                output.display(),
                bytecode.len()
            );
        }
        #[cfg(feature = "debug-mode")]
        Commands::CompileDebug {
            input,
            output,
            script_id,
            priority,
        } => {
            // 讀取原始碼
            let source = std::fs::read_to_string(&input).unwrap_or_else(|e| {
                eprintln!("無法讀取來源檔案 {}: {e}", input.display());
                process::exit(1);
            });

            // 編譯（debug 模式，無加密）
            let bytecode = match compile_debug(&source, &script_id, priority) {
                Ok(b) => b,
                Err(e) => {
                    match &e {
                        CompileError::EmptySource => {
                            eprintln!("編譯失敗：原始碼為空");
                        }
                        CompileError::RhaiParseFailed(parse_err) => {
                            eprintln!("Rhai 編譯失敗：{parse_err}");
                        }
                        CompileError::EncryptionFailed(crypto_err) => {
                            eprintln!("加密失敗：{crypto_err}");
                        }
                    }
                    process::exit(1);
                }
            };

            // 寫入輸出
            std::fs::write(&output, &bytecode).unwrap_or_else(|e| {
                eprintln!("無法寫入輸出檔案 {}: {e}", output.display());
                process::exit(1);
            });

            println!(
                "Debug 編譯成功：{} → {}（{} bytes）",
                input.display(),
                output.display(),
                bytecode.len()
            );
        }
        Commands::Verify {
            input,
            public_key,
            decryption_key,
        } => {
            // 讀取 bytecode 檔案
            let data = std::fs::read(&input).unwrap_or_else(|e| {
                eprintln!("無法讀取 bytecode 檔案 {}: {e}", input.display());
                process::exit(1);
            });

            // 讀取 key 檔案
            let pub_key = read_key_file(&public_key);
            let dec_key = read_key_file(&decryption_key);

            // 載入並驗證
            match load(&data, &pub_key, &dec_key) {
                Ok((metadata, _ast)) => {
                    let hash_hex: String = metadata
                        .source_hash
                        .iter()
                        .take(8)
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    println!("驗證成功：{}", input.display());
                    println!("  script_id:       {}", metadata.script_id);
                    println!("  priority:        {}", metadata.priority);
                    println!("  build_timestamp: {}", metadata.build_timestamp);
                    println!("  source_hash:     {hash_hex}...");
                }
                Err(e) => {
                    match &e {
                        LoadError::Format(fmt_err) => match fmt_err {
                            bytecode_compiler::FormatError::InvalidMagic(_) => {
                                eprintln!("驗證失敗：非合法 bytecode 格式");
                            }
                            bytecode_compiler::FormatError::TruncatedData { .. } => {
                                eprintln!("驗證失敗：檔案截斷或損壞");
                            }
                            bytecode_compiler::FormatError::MetadataDeserializationFailed(_) => {
                                eprintln!("驗證失敗：metadata 解碼失敗");
                            }
                            bytecode_compiler::FormatError::MetadataLengthOutOfRange => {
                                eprintln!("驗證失敗：metadata 長度越界");
                            }
                        },
                        LoadError::IncompatibleVersion { expected, got } => {
                            eprintln!(
                                "驗證失敗：版本不相容（預期 major={expected}，得到 major={got}）"
                            );
                        }
                        LoadError::DecryptionFailed(_) => {
                            eprintln!("驗證失敗：解密失敗（密鑰錯誤或密文被篡改）");
                        }
                        LoadError::SignatureVerificationFailed => {
                            eprintln!("驗證失敗：簽章驗證失敗");
                        }
                        LoadError::SourceDecodeFailed => {
                            eprintln!("驗證失敗：原始碼 UTF-8 解碼失敗");
                        }
                        LoadError::RhaiCompileFailed(parse_err) => {
                            eprintln!("驗證失敗：Rhai 編譯失敗：{parse_err}");
                        }
                        LoadError::DebugBuildBytecode => {
                            eprintln!("驗證失敗：debug bytecode 不可在 release 驗證");
                        }
                    }
                    process::exit(1);
                }
            }
        }
    }
}
