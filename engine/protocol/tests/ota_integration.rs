//! OTA 熱更新整合測試
//!
//! 驗證 OTA 分片傳輸、版本管理的 round-trip。
//!
//! 使用 `cargo test -p protocol --features integration` 執行。

#![cfg(feature = "integration")]

use protocol::{AckStatus, NetMessage};

#[test]
fn ota_chunk_serialization_round_trip() {
    let msg = NetMessage::OtaChunk {
        update_id: 1,
        seq: 0,
        total: 4,
        data: vec![0xAA; 64 * 1024], // 64 KB chunk
    };

    let bytes = bincode::serialize(&msg).unwrap();
    let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();

    if let NetMessage::OtaChunk {
        update_id,
        seq,
        total,
        data,
    } = decoded
    {
        assert_eq!(update_id, 1);
        assert_eq!(seq, 0);
        assert_eq!(total, 4);
        assert_eq!(data.len(), 64 * 1024);
    } else {
        panic!("應解碼為 OtaChunk");
    }
}

#[test]
fn ota_chunk_empty_data() {
    let msg = NetMessage::OtaChunk {
        update_id: 1,
        seq: 0,
        total: 1,
        data: vec![],
    };

    let bytes = bincode::serialize(&msg).unwrap();
    let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
    if let NetMessage::OtaChunk { data, .. } = decoded {
        assert!(data.is_empty());
    } else {
        panic!("應解碼為 OtaChunk");
    }
}

#[test]
fn ota_ack_round_trip() {
    let msg = NetMessage::OtaAck {
        update_id: 42,
        status: AckStatus::Ok,
        version_hash: [0xBB; 32],
        tick: 100,
    };

    let bytes = bincode::serialize(&msg).unwrap();
    let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
    if let NetMessage::OtaAck {
        update_id,
        status,
        tick,
        ..
    } = decoded
    {
        assert_eq!(update_id, 42);
        assert_eq!(status, AckStatus::Ok);
        assert_eq!(tick, 100);
    } else {
        panic!("應解碼為 OtaAck");
    }
}

#[test]
fn ota_multiple_chunks_different_seq() {
    // 驗證同一 update_id 的不同 seq 分片可序列化/反序列化
    for seq in 0..4u16 {
        let msg = NetMessage::OtaChunk {
            update_id: 1,
            seq,
            total: 4,
            data: vec![seq as u8; 100],
        };
        let bytes = bincode::serialize(&msg).unwrap();
        let decoded: NetMessage = bincode::deserialize(&bytes).unwrap();
        if let NetMessage::OtaChunk {
            seq: decoded_seq, ..
        } = decoded
        {
            assert_eq!(decoded_seq, seq);
        }
    }
}
