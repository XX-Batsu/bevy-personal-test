use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use protocol::{codec, NetMessage, Transport, TransportError, TransportType};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

/// WebSocket Transport 實作
/// 泛型 S 為底層 IO 串流（TCP socket 或測試用 loopback）
pub struct WsTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    sink: Mutex<SplitSink<WebSocketStream<S>, Message>>,
    stream: Mutex<SplitStream<WebSocketStream<S>>>,
}

impl<S> WsTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    /// 從已完成 WebSocket 握手的 `WebSocketStream` 建立 Transport
    pub fn new(ws_stream: WebSocketStream<S>) -> Self {
        let (sink, stream) = ws_stream.split();
        Self {
            sink: Mutex::new(sink),
            stream: Mutex::new(stream),
        }
    }
}

#[async_trait]
impl<S> Transport for WsTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// 可靠有序傳輸：編碼訊息後以 Binary frame 送出
    async fn send_reliable(&self, msg: &NetMessage) -> Result<(), TransportError> {
        let frame = codec::encode(msg).map_err(TransportError::Codec)?;
        let binary = Message::Binary(Bytes::from(frame));
        self.sink
            .lock()
            .await
            .send(binary)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    /// 不可靠傳輸：WebSocket 為 TCP，無 unreliable 語義，退化為 send_reliable
    async fn send_unreliable(&self, msg: &NetMessage) -> Result<(), TransportError> {
        self.send_reliable(msg).await
    }

    /// 接收下一筆訊息（跳過 Ping/Pong/Text frame，等待 Binary frame）
    async fn recv(&self) -> Result<NetMessage, TransportError> {
        loop {
            let item = self.stream.lock().await.next().await;
            match item {
                None => return Err(TransportError::ConnectionClosed),
                Some(Err(e)) => return Err(TransportError::Io(e.to_string())),
                Some(Ok(Message::Binary(data))) => {
                    return codec::decode(&data).map_err(TransportError::Codec);
                }
                Some(Ok(Message::Close(_))) => {
                    return Err(TransportError::ConnectionClosed);
                }
                Some(Ok(_)) => {
                    // 跳過 Ping/Pong/Text frame
                    // Ping/Pong：tungstenite 底層已自動回覆 Pong
                    // Text：本協定僅使用 Binary frame，收到 Text 為非預期行為
                    continue;
                }
            }
        }
    }

    /// 送出 WebSocket close frame（RFC 6455 close code 1000 Normal Closure）
    async fn close(&self) -> Result<(), TransportError> {
        self.sink
            .lock()
            .await
            .send(Message::Close(None))
            .await
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    fn transport_type(&self) -> TransportType {
        TransportType::WebSocket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::TransportError;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, client_async};

    async fn make_ws_pair() -> (
        WsTransport<tokio::net::TcpStream>,
        WsTransport<tokio::net::TcpStream>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_handle = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let ws = accept_async(tcp).await.unwrap();
            WsTransport::new(ws)
        });

        let url = format!("ws://{}", addr);
        let client_tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (ws_client, _) = client_async(&url, client_tcp).await.unwrap();
        let client_transport = WsTransport::new(ws_client);
        let server_transport = server_handle.await.unwrap();
        (server_transport, client_transport)
    }

    #[tokio::test]
    async fn send_recv_reliable() {
        let (server, client) = make_ws_pair().await;
        let msg = NetMessage::Ping { timestamp: 1 };
        client.send_reliable(&msg).await.unwrap();
        let received = server.recv().await.unwrap();
        assert_eq!(received, msg);
    }

    #[tokio::test]
    async fn send_unreliable_degrades_to_reliable() {
        let (server, client) = make_ws_pair().await;
        let msg = NetMessage::Ping { timestamp: 2 };
        client.send_unreliable(&msg).await.unwrap();
        let received = server.recv().await.unwrap();
        assert_eq!(received, msg);
    }

    #[tokio::test]
    async fn transport_type_is_websocket() {
        let (_, client) = make_ws_pair().await;
        assert_eq!(client.transport_type(), TransportType::WebSocket);
    }

    #[tokio::test]
    async fn close_connection() {
        let (server, client) = make_ws_pair().await;
        client.close().await.unwrap();
        let result = server.recv().await;
        assert!(
            matches!(result, Err(TransportError::ConnectionClosed)),
            "預期 ConnectionClosed，實際得到 {:?}",
            result
        );
    }

    #[tokio::test]
    async fn send_multiple_messages_in_order() {
        let (server, client) = make_ws_pair().await;
        let msgs = [
            NetMessage::Ping { timestamp: 10 },
            NetMessage::Ping { timestamp: 20 },
            NetMessage::Ping { timestamp: 30 },
        ];
        for msg in &msgs {
            client.send_reliable(msg).await.unwrap();
        }
        for expected in &msgs {
            let received = server.recv().await.unwrap();
            assert_eq!(received, *expected);
        }
    }
}
