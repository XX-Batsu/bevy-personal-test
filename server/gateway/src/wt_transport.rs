use async_trait::async_trait;
use protocol::{codec, NetMessage, Transport, TransportError, TransportType};
use wtransport::Connection;

/// WebTransport Transport 骨架
/// 底層使用 wtransport 0.7（QUIC/HTTP3）
///
/// 注意：`recv()` 完整實作延後至 Phase 15 Integration Glue，
/// 本模組僅建立骨架以確保 `cargo build -p gateway` 通過。
pub struct WtTransport {
    connection: Connection,
}

impl WtTransport {
    /// 從已建立的 `wtransport::Connection` 建立 Transport
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl Transport for WtTransport {
    /// 可靠有序傳輸：開啟 QUIC 雙向串流（bi-stream），寫入 codec frame
    async fn send_reliable(&self, msg: &NetMessage) -> Result<(), TransportError> {
        let frame = codec::encode(msg).map_err(TransportError::Codec)?;
        let (mut send, _recv) = self
            .connection
            .open_bi()
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        send.write_all(&frame)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        // finish 通知對端此串流的寫入已完成（訊息邊界）
        send.finish()
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(())
    }

    /// 不可靠傳輸：使用 QUIC datagram（真正的 unreliable 語義）
    async fn send_unreliable(&self, msg: &NetMessage) -> Result<(), TransportError> {
        let frame = codec::encode(msg).map_err(TransportError::Codec)?;
        self.connection
            .send_datagram(frame)
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    /// 接收訊息：Phase 15 Integration Glue 完成
    /// 預期實作：tokio::select! 同時監聽 datagram + bi-stream
    #[allow(clippy::todo)]
    async fn recv(&self) -> Result<NetMessage, TransportError> {
        todo!("WtTransport::recv 於 Phase 15 完整實作")
    }

    /// 關閉連線（QUIC varint error code 0 = 正常關閉）
    async fn close(&self) -> Result<(), TransportError> {
        // wtransport 0.7 close() 為同步方法，無回傳值
        self.connection.close(0u32.into(), "正常關閉".as_bytes());
        Ok(())
    }

    fn transport_type(&self) -> TransportType {
        TransportType::WebTransport
    }
}
