mod encrypt;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "asset_encryptor", about = "資產加密與簽名工具")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 加密單一檔案
    Encrypt {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        key_file: PathBuf,
    },
    /// 解密單一檔案
    Decrypt {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        key_file: PathBuf,
    },
    /// 批次加密目錄內的 .glb/.png 檔案
    EncryptDir {
        dir: PathBuf,
        #[arg(long)]
        key_file: PathBuf,
    },
    /// Ed25519 簽名
    Sign {
        file: PathBuf,
        #[arg(long)]
        key_file: PathBuf,
    },
    /// Ed25519 驗簽
    Verify {
        file: PathBuf,
        #[arg(long)]
        key_file: PathBuf,
        #[arg(long)]
        sig_file: PathBuf,
    },
    /// 生成金鑰組：AES-256 key + Ed25519 key pair
    /// 輸出：<out>.aes.key（32 bytes）、<out>.ed25519.key（seed, 32 bytes）、<out>.ed25519.pub（verifying key, 32 bytes）
    GenKey {
        /// 輸出路徑前綴，例如 keys/game → keys/game.aes.key 等
        #[arg(long, default_value = "keys/game")]
        out: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Encrypt {
            input,
            output,
            key_file,
        } => {
            let key = encrypt::load_key_file(&key_file)?;
            encrypt::encrypt_file(&input, &output, &key)?;
            eprintln!("完成：加密 {} → {}", input.display(), output.display());
        }
        Commands::Decrypt {
            input,
            output,
            key_file,
        } => {
            let key = encrypt::load_key_file(&key_file)?;
            encrypt::decrypt_file(&input, &output, &key)?;
            eprintln!("完成：解密 {} → {}", input.display(), output.display());
        }
        Commands::EncryptDir { dir, key_file } => {
            let key = encrypt::load_key_file(&key_file)?;
            let count = encrypt::encrypt_dir(&dir, &key)?;
            eprintln!("完成：加密 {count} 個檔案");
        }
        Commands::Sign { file, key_file } => {
            encrypt::sign_file(&file, &key_file)?;
        }
        Commands::Verify {
            file,
            key_file,
            sig_file,
        } => {
            encrypt::verify_file(&file, &key_file, &sig_file)?;
        }
        Commands::GenKey { out } => {
            encrypt::gen_key(&out)?;
        }
    }

    Ok(())
}
