//! Dev Asset Server — HTTP 靜態伺服 + WebSocket 素材變更通知 + 自動 manifest-gen。

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

#[derive(Parser)]
#[command(name = "dev-asset-server", about = "開發用素材伺服器")]
struct Cli {
    #[arg(long, default_value = "assets")]
    assets_dir: PathBuf,
    #[arg(long, default_value = "8081")]
    port: u16,
    #[arg(long, default_value = "true")]
    auto_manifest: bool,
}

#[derive(Clone)]
struct AppState {
    tx: broadcast::Sender<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let (tx, _) = broadcast::channel::<String>(100);
    let state = AppState { tx: tx.clone() };

    // 將相對路徑轉為絕對路徑，避免 cd 後路徑失效
    let assets_dir = cli.assets_dir.canonicalize().unwrap_or_else(|_| {
        // canonicalize 失敗時嘗試建立目錄再重試
        std::fs::create_dir_all(&cli.assets_dir).ok();
        cli.assets_dir.canonicalize().unwrap_or_else(|e| {
            panic!("Assets 目錄不存在且無法建立：{} — {e}", cli.assets_dir.display());
        })
    });

    let auto_manifest = cli.auto_manifest;
    let watcher_tx = tx.clone();
    let watcher_dir = assets_dir.clone();
    std::thread::spawn(move || {
        watch_assets(watcher_dir, watcher_tx, auto_manifest);
    });

    // axum 0.8: 根路徑不能用 nest_service，改用 fallback_service
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new(&assets_dir))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], cli.port));
    tracing::info!("Dev Asset Server 啟動：http://{addr}");
    tracing::info!("WebSocket：ws://{addr}/ws");
    tracing::info!("Assets 目錄：{}", cli.assets_dir.display());

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.tx.subscribe();
    tracing::info!("WebSocket 客戶端已連線");

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(json) => {
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // 忽略 ping/pong 等其他客戶端訊息
                }
            }
        }
    }

    tracing::info!("WebSocket 客戶端已斷線");
}

fn watch_assets(assets_dir: PathBuf, tx: broadcast::Sender<String>, auto_manifest: bool) {
    let (notify_tx, notify_rx) = std::sync::mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                let _ = notify_tx.send(event);
            }
        }
    })
    .expect("建立 file watcher 失敗");

    watcher
        .watch(&assets_dir, RecursiveMode::Recursive)
        .expect("監聽 assets 目錄失敗");

    tracing::info!("File watcher 已啟動：{}", assets_dir.display());

    let mut last_event = Instant::now();
    let mut pending = BTreeSet::new();
    let debounce = Duration::from_millis(500);

    loop {
        match notify_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                for path in &event.paths {
                    if let Ok(rel) = path.strip_prefix(&assets_dir) {
                        let rel_str = rel.to_string_lossy();
                        if rel_str == "manifest.ron" {
                            continue;
                        }
                        if let Some((stem, _)) = rel_str.rsplit_once('.') {
                            pending.insert(stem.to_string());
                        }
                    }
                }
                last_event = Instant::now();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }

        if !pending.is_empty() && last_event.elapsed() >= debounce {
            let changed: Vec<String> = pending.iter().cloned().collect();
            pending.clear();
            tracing::info!("偵測到 {} 個素材變更", changed.len());

            if auto_manifest {
                tracing::info!("自動重新產生 manifest...");
                let status = Command::new("cargo")
                    .args([
                        "run",
                        "--package",
                        "manifest-gen",
                        "--",
                        "--assets-dir",
                        assets_dir.to_str().unwrap(),
                        "--output",
                        assets_dir.join("manifest.ron").to_str().unwrap(),
                    ])
                    .status();
                match status {
                    Ok(s) if s.success() => tracing::info!("Manifest 重新產生完成"),
                    Ok(s) => tracing::error!("manifest-gen 失敗：exit code {s}"),
                    Err(e) => tracing::error!("manifest-gen 執行失敗：{e}"),
                }
            }

            let json = serde_json::json!({ "changed": changed }).to_string();
            let _ = tx.send(json);
        }
    }
}
