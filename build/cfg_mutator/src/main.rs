mod seed;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "cfg_mutator",
    about = "WASM CFG 混淆工具（wasm-mutate 包裝器）"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 生成 seed 並輸出至 stdout（供 build script 讀取）
    GenSeed {
        /// Git commit hash
        #[arg(long)]
        commit_hash: String,
        /// Build timestamp（Unix 秒）
        #[arg(long)]
        timestamp: u64,
    },
    /// 執行 wasm-mutate CFG 混淆
    Mutate {
        /// 輸入 WASM 檔案路徑（wasm-opt 優化後）
        input: PathBuf,
        /// 輸出 WASM 檔案路徑
        output: PathBuf,
        /// Build timestamp（Unix 秒）
        #[arg(long)]
        timestamp: u64,
        /// Git commit hash
        #[arg(long)]
        commit: String,
    },
}

fn main() -> anyhow::Result<()> {
    // no-cfg-mutation feature 啟用時，跳過所有 CFG mutation 邏輯
    #[cfg(feature = "no-cfg-mutation")]
    {
        eprintln!("資訊：no-cfg-mutation feature 啟用，跳過 CFG mutation");
        return Ok(());
    }

    // tracing 初始化（build tool 使用 stderr 輸出日誌）
    #[allow(unreachable_code)]
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::GenSeed {
            commit_hash,
            timestamp,
        } => {
            handle_gen_seed(&commit_hash, timestamp);
            Ok(())
        }
        Commands::Mutate {
            input,
            output,
            timestamp,
            commit,
        } => handle_mutate(&input, &output, timestamp, &commit),
    }
}

/// gen-seed 子命令：輸出 seed 至 stdout（十進位），供 build script $(...)  捕獲
fn handle_gen_seed(commit_hash: &str, timestamp: u64) {
    let seed = seed::resolve_seed(commit_hash, timestamp);
    println!("{seed}");
}

/// mutate 子命令：完整 wasm-mutate 流程，失敗時 fallback 至複製 input
fn handle_mutate(
    input: &PathBuf,
    output: &PathBuf,
    timestamp: u64,
    commit: &str,
) -> anyhow::Result<()> {
    // 場景 F8：輸入檔案不存在 → 唯一中止 build 的場景
    if !input.exists() {
        anyhow::bail!("輸入檔案不存在：{}", input.display());
    }

    // 生成 seed
    let seed = seed::resolve_seed(commit, timestamp);
    tracing::info!("CFG mutation seed = {seed}");

    // 嘗試呼叫 wasm-mutate（失敗則 fallback）
    match run_wasm_mutate(input, output, seed) {
        Ok(()) => {
            tracing::info!("CFG mutation 完成：{}", output.display());
        }
        Err(e) => {
            tracing::warn!("wasm-mutate 失敗（{e}），fallback 至 optimized.wasm");
            std::fs::copy(input, output)?;
            tracing::warn!(
                "已複製 {} → {}（未混淆）",
                input.display(),
                output.display()
            );
        }
    }

    Ok(())
}

/// 呼叫 wasm-mutate CLI subprocess
/// 對齊 pipeline-integration.md §wasm-mutate 詳細呼叫規格
fn run_wasm_mutate(input: &PathBuf, output: &PathBuf, seed: u64) -> anyhow::Result<()> {
    let result = Command::new("wasm-mutate")
        .arg("--seed")
        .arg(seed.to_string())
        .arg("--fuel")
        .arg("1000")
        .arg("--preserve-semantics")
        .arg("-o")
        .arg(output)
        .arg(input)
        .output();

    match result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("wasm-mutate 非零退出 code={code}, stderr={stderr}")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("wasm-mutate 工具不在 PATH：{e}")
        }
        Err(e) => {
            anyhow::bail!("wasm-mutate 行程啟動失敗：{e}")
        }
    }
}
