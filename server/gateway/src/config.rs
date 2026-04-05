use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Gateway 伺服器組態
/// native-only：允許使用 `std::time::Duration`、`PathBuf`、`SocketAddr`
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// `WebSocket` 監聽地址（TLS upgrade 端點）
    pub ws_addr: SocketAddr,
    /// `WebTransport`（QUIC）監聽地址
    pub wt_addr: SocketAddr,
    /// TLS 憑證路徑（PEM 格式）
    pub tls_cert: PathBuf,
    /// TLS 私鑰路徑（PEM 格式）
    pub tls_key: PathBuf,
    /// 同時最大連線數上限
    pub max_connections: usize,
    /// 最大房間數上限
    pub max_rooms: usize,
    /// 每房最大玩家數
    pub room_max_players: usize,
    /// 握手超時時間
    pub handshake_timeout: Duration,
    /// 斷線後允許重連的超時時間
    pub reconnect_timeout: Duration,
    /// 前端靜態檔案目錄（本地開發用）
    pub client_dir: PathBuf,
}

impl Default for GatewayConfig {
    /// 回傳適用於本地開發的預設組態
    fn default() -> Self {
        Self {
            ws_addr: "0.0.0.0:8443".parse().expect("預設 ws_addr 解析不應失敗"),
            wt_addr: "0.0.0.0:4433".parse().expect("預設 wt_addr 解析不應失敗"),
            tls_cert: PathBuf::from("certs/server.crt"),
            tls_key: PathBuf::from("certs/server.key"),
            max_connections: 1000,
            max_rooms: 100,
            room_max_players: 8,
            handshake_timeout: Duration::from_secs(5),
            reconnect_timeout: Duration::from_secs(30),
            client_dir: PathBuf::from("client/js"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_valid() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.ws_addr, "0.0.0.0:8443".parse::<SocketAddr>().unwrap());
        assert_eq!(cfg.wt_addr, "0.0.0.0:4433".parse::<SocketAddr>().unwrap());
        assert_eq!(cfg.max_connections, 1000);
        assert_eq!(cfg.max_rooms, 100);
        assert_eq!(cfg.room_max_players, 8);
        assert_eq!(cfg.handshake_timeout, Duration::from_secs(5));
        assert_eq!(cfg.reconnect_timeout, Duration::from_secs(30));
    }

    #[test]
    fn tls_cert_default_path() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.tls_cert, PathBuf::from("certs/server.crt"));
    }

    #[test]
    fn tls_key_default_path() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.tls_key, PathBuf::from("certs/server.key"));
    }

    #[test]
    fn config_is_clone() {
        let cfg = GatewayConfig::default();
        let _cloned = cfg.clone();
        // 紅燈階段：todo!() 會在 default() 就 panic，
        // 不會執行到 clone()；綠燈後才真正驗證 Clone 可用
    }

    #[test]
    fn handshake_timeout_less_than_reconnect_timeout() {
        // 握手逾時應嚴格小於重連逾時，否則握手未完成就被重連機制清理
        let cfg = GatewayConfig::default();
        assert!(
            cfg.handshake_timeout < cfg.reconnect_timeout,
            "handshake_timeout ({:?}) 應小於 reconnect_timeout ({:?})",
            cfg.handshake_timeout,
            cfg.reconnect_timeout,
        );
    }

    #[test]
    fn max_connections_ge_rooms_times_players() {
        // 最大連線數應 ≥ max_rooms * room_max_players，
        // 否則房間全滿前就會達到連線上限
        let cfg = GatewayConfig::default();
        assert!(
            cfg.max_connections >= cfg.max_rooms * cfg.room_max_players,
            "max_connections ({}) 應 ≥ max_rooms * room_max_players ({})",
            cfg.max_connections,
            cfg.max_rooms * cfg.room_max_players,
        );
    }

    #[test]
    fn tls_paths_are_relative() {
        // 預設 TLS 路徑為相對路徑（certs/ 子目錄），
        // 部署時由工作目錄決定實際位置
        let cfg = GatewayConfig::default();
        assert!(
            cfg.tls_cert.is_relative(),
            "tls_cert 預設應為相對路徑，實際為 {:?}",
            cfg.tls_cert,
        );
        assert!(
            cfg.tls_key.is_relative(),
            "tls_key 預設應為相對路徑，實際為 {:?}",
            cfg.tls_key,
        );
    }

    #[test]
    fn clone_produces_independent_copy() {
        // 驗證 Clone 為深複製：修改 clone 不影響原始值
        let cfg = GatewayConfig::default();
        let mut cloned = cfg.clone();
        cloned.max_connections = 9999;
        assert_ne!(cfg.max_connections, cloned.max_connections);
    }
}
