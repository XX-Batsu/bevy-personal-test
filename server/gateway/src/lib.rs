//! server/gateway — 雙協議網路閘道
//! 支援 WebSocket（tokio-tungstenite）與 WebTransport（wtransport）
//! native-only target，不需 WASM 編譯

pub mod config;
pub mod listener;
pub mod room;
pub mod session;
pub mod ws_transport;
#[cfg(feature = "webtransport")]
pub mod wt_transport;

// 公開再匯出（方便外部 crate 直接使用，不需記憶模組路徑）
pub use config::GatewayConfig;
pub use room::{ConnectionId, Room, RoomError, RoomId, RoomManager, RoomState};
pub use session::{Session, SessionId, SessionManager};
