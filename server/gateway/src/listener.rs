use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use key_exchange::{HandshakeManager, SessionInfo};
use protocol::{codec, NetMessage};
use std::{collections::HashMap, error::Error, net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, Mutex};
use tower_http::services::ServeDir;
use tracing::{debug, info};
use zeroize::Zeroizing;

use crate::config::GatewayConfig;

// ── ClientRegistry ────────────────────────────────────────────────────────────

/// 每個活躍 session 的 outbound channel sender（傳送已加密 frame）
type OutboundSender = mpsc::Sender<Vec<u8>>;

/// 單一 session 的內部記錄：(session_key, outbound sender)。
///
/// `Zeroizing` 確保 session_key 在 drop 時清零記憶體。
type SessionEntry = (Zeroizing<[u8; 32]>, OutboundSender);

/// 活躍連線的全域 registry（session_id → (session_key, outbound sender)）
///
/// Layer 2 game loop 透過此結構取得 session 資訊並廣播 StateUpdate。
/// 握手完成後由 handle_ws_connection 登記；斷線時自動移除。
#[derive(Clone, Default)]
pub struct ClientRegistry(Arc<Mutex<HashMap<u64, SessionEntry>>>);

impl ClientRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    async fn register(&self, id: u64, key: Zeroizing<[u8; 32]>, tx: OutboundSender) {
        self.0.lock().await.insert(id, (key, tx));
    }

    async fn unregister(&self, id: u64) {
        self.0.lock().await.remove(&id);
    }
}

// ── Listener ──────────────────────────────────────────────────────────────────

/// 啟動 WebSocket listener、/health endpoint 及靜態檔案伺服
///
/// 回傳 `(綁定地址, shutdown channel sender, client registry)`。
/// `registry` 供 Layer 2 game loop 廣播 StateUpdate 至所有活躍連線。
///
/// # 架構說明
/// - axum `Router` + `with_state(registry)` 將 registry 注入每個 WS handler
/// - `tokio::spawn` 背景執行 server，主 task 立即回傳
pub async fn start_ws_listener(
    config: GatewayConfig,
) -> Result<(SocketAddr, oneshot::Sender<()>, ClientRegistry), Box<dyn Error + Send + Sync>> {
    let client_dir = config.client_dir.clone();
    let registry = ClientRegistry::new();

    let router = Router::new()
        .route("/health", get(health_handler))
        .route("/ws", get(ws_handler))
        .with_state(registry.clone())
        .fallback_service(ServeDir::new(&client_dir));

    let listener = TcpListener::bind(config.ws_addr).await?;
    let addr = listener.local_addr()?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
                info!("Gateway WebSocket listener 正在關閉");
            })
            .await
            .expect("axum server 不應在正常運行中失敗");
    });

    info!(%addr, client_dir = %client_dir.display(), "Gateway 已啟動（WebSocket + 靜態檔案）");
    Ok((addr, shutdown_tx, registry))
}

/// /health endpoint — 回傳 "OK"（HTTP 200）
async fn health_handler() -> impl IntoResponse {
    "OK"
}

/// /ws endpoint — 執行 WebSocket upgrade（注入 registry 至 handler）
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(registry): State<ClientRegistry>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, registry))
}

/// WebSocket 連線處理：ECDH 握手 → outbound task → 加密訊息迴圈
///
/// ## 流程
/// 1. 等待 codec-encoded ClientHello
/// 2. ECDH 握手，取得 session_key
/// 3. 送 ServerHello
/// 4. 建立 outbound channel，向 registry 登記
/// 5. split socket → spawn outbound task（writes frames to WebSocket）
/// 6. 加密訊息接收迴圈（decrypt_frame_aad + dispatch）
/// 7. 清理 registry
async fn handle_ws_connection(mut socket: WebSocket, registry: ClientRegistry) {
    info!("WebSocket 連線已建立");

    // ── Step 1: 等待 codec-encoded ClientHello ──
    let client_public_key: [u8; 32] = loop {
        match socket.recv().await {
            Some(Ok(Message::Binary(data))) => match codec::decode(&data) {
                Ok(NetMessage::ClientHello { public_key }) => break public_key,
                _ => {
                    tracing::warn!("收到非法 ClientHello（非 codec 格式），關閉連線");
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
            },
            Some(Ok(Message::Close(_))) | None => {
                info!("連線在握手前關閉");
                return;
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => {
                info!(error = %e, "握手階段接收錯誤");
                return;
            }
        }
    };

    info!("收到 codec ClientHello，開始 ECDH 握手");

    // ── Step 2: ECDH 握手 ──
    let manager = HandshakeManager::new();
    let session_info = SessionInfo {
        session_id: 1,
        server_tick: 0,
        asset_manifest_hash: [0u8; 32],
    };
    let (payload, session_key_raw) =
        match manager.begin_handshake(&client_public_key, &session_info) {
            Ok(result) => result,
            Err(e) => {
                info!(error = %e, "ECDH 握手失敗");
                return;
            }
        };
    let session_key: Zeroizing<[u8; 32]> = session_key_raw;

    // ── Step 3: 送 ServerHello ──
    let server_hello = NetMessage::ServerHello {
        public_key: payload.server_public_key,
        encrypted_payload: payload.encrypted_session_info,
        nonce: payload.nonce,
    };
    let frame = match codec::encode(&server_hello) {
        Ok(f) => f,
        Err(e) => {
            info!(error = %e, "ServerHello 編碼失敗");
            return;
        }
    };
    if let Err(e) = socket.send(Message::Binary(frame.into())).await {
        info!(error = %e, "ServerHello 發送失敗");
        return;
    }
    info!("ECDH 握手完成，進入加密訊息模式");

    // ── Step 4: 建立 outbound channel 並登記至 registry ──
    let session_id: u64 = 1; // TODO: Phase 15 分配唯一 session_id
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Vec<u8>>(64);
    // 向 registry 登記 session_key clone 與 outbound sender（供 Layer 2 broadcast）
    registry
        .register(session_id, session_key.clone(), outbound_tx.clone())
        .await;

    // ── Step 5: split socket，spawn outbound task ──
    let (mut ws_write, mut ws_read) = socket.split();
    tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            if ws_write.send(Message::Binary(frame.into())).await.is_err() {
                break;
            }
        }
    });

    // ── Step 6: 加密訊息接收迴圈（含 AAD replay 保護）──
    // recv_seq：驗證 client 送來的訊息序號（防 replay）
    // send_seq：對 client 送出訊息的序號（Pong 等直接回應）
    // 注意：Layer 2 game loop 廣播時需與 send_seq 協調（Phase 15 補齊）
    let mut recv_seq: u64 = 0;
    let mut send_seq: u64 = 0;
    while let Some(msg) = ws_read.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // 解密（AAD = recv_seq，解密成功才遞增以防攻擊者推進計數器）
                let plaintext = match crypto::decrypt_frame_aad(&data, &session_key, recv_seq) {
                    Ok(p) => {
                        recv_seq += 1;
                        p
                    }
                    Err(_) => {
                        tracing::warn!("收到竄改或無效訊息，關閉連線");
                        break;
                    }
                };
                // 反序列化
                let net_msg: NetMessage = match bincode::deserialize(&plaintext) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(error = %e, "訊息反序列化失敗，忽略");
                        continue;
                    }
                };
                // 基礎分派（Phase 15 補齊完整分派邏輯）
                match net_msg {
                    NetMessage::Ping { timestamp } => {
                        let pong = NetMessage::Pong { timestamp };
                        let pong_bytes = bincode::serialize(&pong).expect("Pong 序列化不應失敗");
                        let pong_frame =
                            crypto::encrypt_frame_aad(&pong_bytes, &session_key, send_seq)
                                .expect("Pong 加密不應失敗");
                        send_seq += 1;
                        let _ = outbound_tx.send(pong_frame).await;
                    }
                    _ => {
                        debug!("收到遊戲訊息（待 Phase 15 實作完整分派）");
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("WebSocket 連線已關閉");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                info!(error = %e, "WebSocket 接收錯誤");
                break;
            }
        }
    }

    // ── Step 7: 清理 registry ──
    registry.unregister(session_id).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;
    use std::time::Duration;

    /// 輔助函式：建立測試用 GatewayConfig（port 0 動態分配）
    fn test_config() -> GatewayConfig {
        GatewayConfig {
            ws_addr: "127.0.0.1:0".parse().unwrap(), // port 0 = OS 動態分配
            ..GatewayConfig::default()
        }
    }

    // ── 必要測試（2 個）──────────────────────────────

    #[tokio::test]
    async fn health_check_returns_200() {
        let (addr, _shutdown_tx, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");

        let url = format!("http://{}/health", addr);
        let response = tokio::time::timeout(Duration::from_secs(5), reqwest::get(&url))
            .await
            .expect("health check 不應逾時")
            .expect("HTTP GET 應成功");

        assert_eq!(response.status(), 200);
        let body = response.text().await.unwrap();
        assert_eq!(body, "OK");
    }

    #[tokio::test]
    async fn websocket_upgrade_works() {
        let (addr, _shutdown_tx, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");

        let url = format!("ws://{}/ws", addr);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .expect("WebSocket 連線不應逾時");

        assert!(
            result.is_ok(),
            "WebSocket upgrade 應成功: {:?}",
            result.err()
        );
    }

    // ── 選用測試（6 個）─────────────────────────────

    #[tokio::test]
    async fn shutdown_signal_accepted() {
        let (addr, shutdown_tx, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");

        // 先確認 health check 可達（證明 listener 已就緒）
        let url = format!("http://{}/health", addr);
        tokio::time::timeout(Duration::from_secs(5), reqwest::get(&url))
            .await
            .expect("health check 不應逾時")
            .expect("HTTP GET 應成功");

        // 發送 shutdown 信號
        let _ = shutdown_tx.send(());

        // 等待 graceful shutdown 完成
        tokio::time::sleep(Duration::from_millis(200)).await;

        // 嘗試新連線應被拒絕
        let connect_result = tokio::net::TcpStream::connect(addr).await;
        assert!(
            connect_result.is_err(),
            "shutdown 後連線應被拒絕，實際結果: {:?}",
            connect_result
        );
    }

    #[tokio::test]
    async fn port_zero_assigns_dynamic() {
        let (addr, _shutdown_tx, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");
        assert!(addr.port() > 0, "動態分配的 port 應大於 0");
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let (addr, _shutdown_tx, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");

        let url = format!("http://{}/nonexistent", addr);
        let response = tokio::time::timeout(Duration::from_secs(5), reqwest::get(&url))
            .await
            .expect("請求不應逾時")
            .expect("HTTP GET 應成功");

        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn bind_failure_returns_error() {
        // 先成功 bind 一個 port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_addr = listener.local_addr().unwrap();

        // 嘗試 bind 同一個 port 應失敗
        let config = GatewayConfig {
            ws_addr: occupied_addr,
            ..GatewayConfig::default()
        };
        let result = start_ws_listener(config).await;
        assert!(result.is_err(), "重複 bind 同一 port 應回傳 Err");
    }

    #[tokio::test]
    async fn concurrent_health_checks() {
        let (addr, _shutdown_tx, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");

        let url = format!("http://{}/health", addr);
        let (r1, r2, r3, r4, r5) = tokio::join!(
            reqwest::get(&url),
            reqwest::get(&url),
            reqwest::get(&url),
            reqwest::get(&url),
            reqwest::get(&url),
        );
        assert_eq!(r1.unwrap().status(), 200);
        assert_eq!(r2.unwrap().status(), 200);
        assert_eq!(r3.unwrap().status(), 200);
        assert_eq!(r4.unwrap().status(), 200);
        assert_eq!(r5.unwrap().status(), 200);
    }

    #[tokio::test]
    async fn websocket_invalid_path_returns_404() {
        let (addr, _shutdown_tx, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");

        let url = format!("ws://{}/invalid", addr);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .expect("連線嘗試不應逾時");

        assert!(result.is_err(), "WebSocket upgrade 到不存在的路徑應失敗");
    }

    use crypto::{build_hkdf_salt, derive_session_key, EcdhKeyPair, HKDF_INFO};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    /// 輔助：與 server 完成完整 ECDH 握手，回傳 session_key [u8; 32]
    async fn do_handshake(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> [u8; 32] {
        let client_pair = EcdhKeyPair::generate();
        let client_pk = client_pair.public_key();

        // 送 codec-encoded ClientHello
        let hello = NetMessage::ClientHello {
            public_key: client_pk,
        };
        let frame = codec::encode(&hello).expect("encode 應成功");
        ws.send(TungsteniteMessage::Binary(frame.into()))
            .await
            .expect("送出應成功");

        // 收 codec-encoded ServerHello
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("收 ServerHello 不應逾時")
            .expect("stream 不應結束")
            .expect("接收應成功");
        let data = match msg {
            TungsteniteMessage::Binary(d) => d,
            other => panic!("預期 Binary，收到 {:?}", other),
        };
        let server_hello = codec::decode(&data).expect("decode ServerHello 應成功");
        let (server_pub, nonce_raw, enc_payload) = match server_hello {
            NetMessage::ServerHello {
                public_key,
                nonce,
                encrypted_payload,
            } => (public_key, nonce, encrypted_payload),
            other => panic!("預期 ServerHello，收到 {:?}", other),
        };

        // 計算 session_key（client 側）
        let shared = client_pair
            .derive_shared_secret(&server_pub)
            .expect("ECDH 應成功");
        let salt = build_hkdf_salt(&client_pk, &server_pub);
        let session_key = derive_session_key(&shared, &salt, HKDF_INFO).expect("HKDF 應成功");

        // 驗證解密 enc_payload 不 panic（可選）
        let mut combined = Vec::with_capacity(12 + enc_payload.len());
        combined.extend_from_slice(&nonce_raw);
        combined.extend_from_slice(&enc_payload);
        let _ = crypto::aes_decrypt(&session_key, &combined);

        session_key
    }

    #[tokio::test]
    async fn clienthello_codec_format_required() {
        let (addr, _shutdown, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");
        let url = format!("ws://{}/ws", addr);
        let (mut ws, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .expect("連線不應逾時")
        .expect("WS 連線應成功");

        // 送裸 32 bytes（舊格式）→ server 應關閉連線
        ws.send(TungsteniteMessage::Binary(vec![0x42u8; 32].into()))
            .await
            .expect("送出不應失敗");

        let response = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("等待 server 回應不應逾時");
        assert!(
            matches!(
                response,
                Some(Ok(TungsteniteMessage::Close(_))) | None | Some(Err(_))
            ),
            "裸 32 bytes → server 應關閉連線，實際：{:?}",
            response
        );
    }

    #[tokio::test]
    async fn tampered_game_message_disconnects() {
        let (addr, _shutdown, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");
        let url = format!("ws://{}/ws", addr);
        let (mut ws, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .expect("連線不應逾時")
        .expect("WS 連線應成功");

        let _ = do_handshake(&mut ws).await;

        // 送竄改的密文（auth tag 不符）
        let fake_frame = {
            // 使用錯誤 key 且竄改 frame → server decrypt_frame_aad 應失敗
            let mut f = crypto::encrypt_frame_aad(b"legitimate message", &[0x42u8; 32], 0).unwrap();
            *f.last_mut().unwrap() ^= 0xFF;
            f
        };
        ws.send(TungsteniteMessage::Binary(fake_frame.into()))
            .await
            .expect("送出不應失敗");

        let response = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("等待 server 回應不應逾時");
        assert!(
            matches!(
                response,
                Some(Ok(TungsteniteMessage::Close(_))) | None | Some(Err(_))
            ),
            "竄改密文 → server 應關閉連線"
        );
    }

    #[tokio::test]
    async fn full_handshake_then_encrypted_echo() {
        let (addr, _shutdown, _registry) = start_ws_listener(test_config())
            .await
            .expect("listener 啟動應成功");
        let url = format!("ws://{}/ws", addr);
        let (mut ws, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .expect("連線不應逾時")
        .expect("WS 連線應成功");

        let session_key = do_handshake(&mut ws).await;

        // 送加密 Ping（client send_seq=0 → server recv_seq=0）
        let ping = NetMessage::Ping { timestamp: 12345 };
        let plaintext = bincode::serialize(&ping).unwrap();
        let frame = crypto::encrypt_frame_aad(&plaintext, &session_key, 0).unwrap();
        ws.send(TungsteniteMessage::Binary(frame.into()))
            .await
            .expect("送出 Ping 不應失敗");

        // 收加密 Pong（server send_seq=0 → client recv_seq=0）
        let resp = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("等待 Pong 不應逾時")
            .expect("stream 不應結束")
            .expect("接收應成功");
        let data = match resp {
            TungsteniteMessage::Binary(d) => d,
            other => panic!("預期 Binary Pong，收到 {:?}", other),
        };
        let pong_bytes =
            crypto::decrypt_frame_aad(&data, &session_key, 0).expect("Pong 解密應成功");
        let pong: NetMessage = bincode::deserialize(&pong_bytes).expect("Pong 反序列化應成功");
        assert_eq!(pong, NetMessage::Pong { timestamp: 12345 });
    }
}
