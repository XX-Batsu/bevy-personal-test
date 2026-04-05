//! Replay Debugger CLI
//!
//! 載入 replay 檔案（bincode + gzip），逐幀顯示 state hash，
//! 可選比對 golden hash 檔案標記 desync 位置。
//!
//! 用法：
//!   replay_debugger --input replay.bin.gz [--golden golden.bin]

use std::io::Read;

use clap::Parser;
use flate2::read::GzDecoder;
use serde::Deserialize;

/// Replay 格式版本
const REPLAY_FORMAT_VERSION: u16 = 1;

/// Replay 檔案結構（對齊 server/replay_engine/src/recorder.rs）
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct ReplayFile {
    version: u16,
    session_id: u64,
    seed: u64,
    player_count: u8,
    frames: Vec<ReplayFrame>,
}

/// Replay 單幀
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct ReplayFrame {
    tick: u64,
    inputs: Vec<bridge_types::PlayerInput>,
    rng_state: [u8; 16],
    state_hash: [u8; 32],
}

#[derive(Parser)]
#[command(name = "replay_debugger", about = "Replay 檔案除錯工具")]
struct Cli {
    /// Replay 檔案路徑（bincode + gzip）
    #[arg(short, long)]
    input: String,

    /// Golden hash 檔案路徑（可選，bincode Vec<TickHash>）
    #[arg(short, long)]
    golden: Option<String>,

    /// 顯示前 N 幀詳細資訊
    #[arg(long, default_value = "10")]
    head: usize,
}

fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // 載入 replay
    let replay = match load_replay(&cli.input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("載入 replay 失敗：{e}");
            std::process::exit(1);
        }
    };

    println!("=== Replay 資訊 ===");
    println!("版本：{}", replay.version);
    println!("Session ID：{}", replay.session_id);
    println!("Seed：{}", replay.seed);
    println!("玩家數：{}", replay.player_count);
    println!("總幀數：{}", replay.frames.len());

    // 顯示前 N 幀
    let show_count = cli.head.min(replay.frames.len());
    println!("\n=== 前 {} 幀 ===", show_count);
    for frame in replay.frames.iter().take(show_count) {
        let hash_hex = hex_hash(&frame.state_hash);
        println!(
            "tick={:6} | inputs={} | hash={}",
            frame.tick,
            frame.inputs.len(),
            &hash_hex[..16]
        );
    }

    // Golden hash 比對
    if let Some(golden_path) = &cli.golden {
        println!("\n=== Golden Hash 比對 ===");
        match load_golden(golden_path) {
            Ok(golden) => {
                let mut mismatches = 0;
                for (i, gh) in golden.iter().enumerate() {
                    let frame_idx = i * 4; // Every(4)
                    if frame_idx < replay.frames.len()
                        && replay.frames[frame_idx].state_hash != gh.hash
                    {
                        mismatches += 1;
                        println!(
                            "DESYNC tick={}: expected={} actual={}",
                            gh.tick,
                            &hex_hash(&gh.hash)[..16],
                            &hex_hash(&replay.frames[frame_idx].state_hash)[..16],
                        );
                    }
                }
                if mismatches == 0 {
                    println!("全部一致（{} 筆 golden hash）", golden.len());
                } else {
                    println!("發現 {} 筆 desync", mismatches);
                }
            }
            Err(e) => eprintln!("載入 golden hash 失敗：{e}"),
        }
    }
}

/// TickHash（對齊上游 testing/01-determinism/test-data-management.md）
#[derive(Debug, Clone, Deserialize)]
#[repr(C)]
struct TickHash {
    tick: u64,
    hash: [u8; 32],
}

fn load_replay(path: &str) -> Result<ReplayFile, String> {
    let data = std::fs::read(path).map_err(|e| format!("讀取檔案失敗：{e}"))?;
    let mut decoder = GzDecoder::new(data.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("gzip 解壓失敗：{e}"))?;
    let file: ReplayFile =
        bincode::deserialize(&decompressed).map_err(|e| format!("bincode 解碼失敗：{e}"))?;
    if file.version != REPLAY_FORMAT_VERSION {
        return Err(format!(
            "版本不符：期望 {}，實際 {}",
            REPLAY_FORMAT_VERSION, file.version
        ));
    }
    Ok(file)
}

fn load_golden(path: &str) -> Result<Vec<TickHash>, String> {
    let data = std::fs::read(path).map_err(|e| format!("讀取 golden 失敗：{e}"))?;
    bincode::deserialize(&data).map_err(|e| format!("bincode 解碼 golden 失敗：{e}"))
}

fn hex_hash(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}
