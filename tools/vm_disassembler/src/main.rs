//! vm_disassembler — .rhai.bc 反組譯工具
//!
//! 讀取 .rhai.bc 檔案，顯示 metadata 與 AST dump。
//! 支援 debug（明文）和 release（加密）兩種格式。

use clap::Parser;
use std::process;

/// .rhai.bc → 人類可讀 metadata + AST dump
#[derive(Parser)]
#[command(name = "vm_disassembler", version, about = ".rhai.bc 反組譯工具")]
struct Cli {
    /// .rhai.bc 檔案路徑
    input: String,

    /// 解密金鑰檔案（release bytecode 需要，raw 32 bytes）
    #[arg(long)]
    key_file: Option<String>,

    /// 簽章驗證公鑰檔案（release build 需要，raw 32 bytes）
    #[arg(long)]
    signing_key: Option<String>,

    /// 只顯示 metadata，不反組譯 AST
    #[arg(long)]
    metadata_only: bool,

    /// AST 輸出最大行數（預設 10,000，防止 OOM）
    #[arg(long, default_value = "10000")]
    max_ast_lines: usize,
}

/// 讀取 32 bytes 金鑰檔案
fn load_key_file(path: &str) -> Result<[u8; 32], String> {
    let bytes = std::fs::read(path).map_err(|e| format!("無法讀取金鑰檔案 {path}: {e}"))?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("金鑰檔案 {path} 必須恰好 32 bytes，實際 {} bytes", v.len()))
}

fn main() {
    let cli = Cli::parse();

    // 讀取 bytecode 檔案
    let data = match std::fs::read(&cli.input) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("錯誤：無法讀取 {}：{e}", cli.input);
            process::exit(1);
        }
    };

    // === Metadata 讀取（不需解密）===
    match bytecode_compiler::read_metadata(&data) {
        Ok(meta) => {
            println!("=== Metadata ===");
            println!("  script_id:       {}", meta.script_id);
            println!("  priority:        {}", meta.priority);
            println!("  source_hash:     {}", hex_encode(&meta.source_hash));
            println!("  build_timestamp: {}", meta.build_timestamp);
        }
        Err(e) => {
            eprintln!("錯誤：metadata 解析失敗：{e}");
            process::exit(1);
        }
    }

    if cli.metadata_only {
        return;
    }

    // === AST 反組譯 ===
    println!("\n=== AST ===");

    // 嘗試 debug 明文載入
    match bytecode_compiler::load_debug(&data) {
        Ok((_meta, ast)) => {
            let ast_str = format!("{ast:#?}");
            let lines: Vec<&str> = ast_str.lines().collect();
            let display_count = lines.len().min(cli.max_ast_lines);
            for line in &lines[..display_count] {
                println!("{line}");
            }
            if lines.len() > cli.max_ast_lines {
                println!(
                    "... ({} 行已截斷，共 {} 行。使用 --max-ast-lines 調整)",
                    lines.len() - cli.max_ast_lines,
                    lines.len()
                );
            }
            return;
        }
        Err(_) => {
            // debug 載入失敗，嘗試 release 載入
        }
    }

    // 嘗試 release 加密載入
    let (key_file, signing_key_file) = match (&cli.key_file, &cli.signing_key) {
        (Some(k), Some(s)) => (k.as_str(), s.as_str()),
        _ => {
            eprintln!("錯誤：非 debug bytecode，需提供 --key-file 和 --signing-key");
            process::exit(1);
        }
    };

    let decryption_key = match load_key_file(key_file) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("錯誤：{e}");
            process::exit(1);
        }
    };

    let verifying_key = match load_key_file(signing_key_file) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("錯誤：{e}");
            process::exit(1);
        }
    };

    match bytecode_compiler::load(&data, &verifying_key, &decryption_key) {
        Ok((_meta, ast)) => {
            let ast_str = format!("{ast:#?}");
            let lines: Vec<&str> = ast_str.lines().collect();
            let display_count = lines.len().min(cli.max_ast_lines);
            for line in &lines[..display_count] {
                println!("{line}");
            }
            if lines.len() > cli.max_ast_lines {
                println!(
                    "... ({} 行已截斷，共 {} 行)",
                    lines.len() - cli.max_ast_lines,
                    lines.len()
                );
            }
        }
        Err(e) => {
            eprintln!("錯誤：bytecode 載入失敗：{e}");
            process::exit(1);
        }
    }
}

/// Hex 編碼 byte 陣列
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
