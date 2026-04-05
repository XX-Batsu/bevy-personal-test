//! gateway-server — WebSocket / WebTransport 雙協議閘道伺服器主進入點

use gateway::{listener, GatewayConfig};
use std::error::Error;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // 初始化結構化日誌（開發階段使用 DEBUG 等級）
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    info!("Gateway server 正在啟動");

    // 載入預設設定，PORT 環境變數可覆寫監聽 port
    let mut config = GatewayConfig::default();
    if let Ok(port) = std::env::var("PORT") {
        if let Ok(p) = port.parse::<u16>() {
            config.ws_addr = ([0, 0, 0, 0], p).into();
        }
    }
    info!(
        ws_addr = %config.ws_addr,
        wt_addr = %config.wt_addr,
        max_connections = config.max_connections,
        "設定載入完成"
    );

    // 啟動 WebSocket listener
    let (addr, shutdown_tx) = listener::start_ws_listener(config).await?;
    info!(%addr, "WebSocket listener 已就緒，等待連線");

    // 等待 Ctrl-C 關閉信號
    tokio::signal::ctrl_c().await?;
    info!("收到關閉信號，開始 graceful shutdown");

    // 發送 shutdown 信號給 listener
    let _ = shutdown_tx.send(());
    info!("Gateway server 已關閉");

    Ok(())
}
