//! State Inspector CLI
//!
//! TCP JSON-RPC 伺服器（port 9632），支援查詢遊戲狀態。
//! 可選 JSON stdout 模式（--json）。
//!
//! 用法：
//!   state_inspector [--port 9632] [--json]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use clap::Parser;

/// Debug server 預設 port（對齊上游 task-12 定義）
const DEFAULT_PORT: u16 = 9632;

#[derive(Parser)]
#[command(name = "state_inspector", about = "遊戲狀態檢查工具")]
struct Cli {
    /// TCP 監聽 port
    #[arg(short, long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// JSON stdout 模式（不啟動 TCP server）
    #[arg(long)]
    json: bool,
}

fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    if cli.json {
        // JSON stdout 模式：輸出空狀態（實際整合需注入遊戲狀態）
        let state = serde_json::json!({
            "status": "ready",
            "port": cli.port,
            "entities": [],
            "tick": 0,
        });
        println!("{}", serde_json::to_string_pretty(&state).unwrap());
        return;
    }

    // TCP JSON-RPC server
    let addr = format!("127.0.0.1:{}", cli.port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => {
            tracing::info!("State Inspector 啟動，監聽 {}", addr);
            l
        }
        Err(e) => {
            eprintln!("綁定 {} 失敗：{e}", addr);
            std::process::exit(1);
        }
    };

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let reader = BufReader::new(stream.try_clone().unwrap());
                for line in reader.lines() {
                    match line {
                        Ok(request) => {
                            let response = handle_request(&request);
                            if let Err(e) = writeln!(stream, "{response}") {
                                tracing::warn!("寫入回應失敗：{e}");
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("讀取請求失敗：{e}");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("接受連線失敗：{e}");
            }
        }
    }
}

fn handle_request(request: &str) -> String {
    // 簡易 JSON-RPC 處理
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(request);
    match parsed {
        Ok(val) => {
            let method = val.get("method").and_then(|m| m.as_str()).unwrap_or("");
            match method {
                "get_state" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "result": { "entities": [], "tick": 0 },
                    "id": val.get("id"),
                })
                .to_string(),
                "ping" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "result": "pong",
                    "id": val.get("id"),
                })
                .to_string(),
                _ => serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32601, "message": "Method not found" },
                    "id": val.get("id"),
                })
                .to_string(),
            }
        }
        Err(_) => serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": -32700, "message": "Parse error" },
            "id": null,
        })
        .to_string(),
    }
}
