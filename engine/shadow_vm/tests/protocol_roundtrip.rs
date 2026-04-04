//! Shadow VM 協議型別序列化往返測試

use bridge_types::{
    DeterministicValue, EntityId, PlayerInput, ShadowFrame, ShadowInit, ShadowInitAck,
    ShadowRequest, ShadowResponse, ShadowStatus,
};

fn make_player_input(i: u64) -> PlayerInput {
    PlayerInput {
        player_id: EntityId(i),
        input_type: 0,
        data: DeterministicValue::Int(0),
        tick: i,
    }
}

#[test]
fn shadow_request_bincode_roundtrip() {
    let req = ShadowRequest {
        frames: vec![ShadowFrame {
            tick: 42,
            inputs: vec![],
            rng_state: [1u8; 16],
            ecs_mirror_hash: [2u8; 32],
        }],
    };
    let encoded = bincode::serialize(&req).unwrap();
    let decoded: ShadowRequest = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.frames[0].tick, 42);
    assert_eq!(decoded.frames[0].rng_state, [1u8; 16]);
    assert_eq!(decoded.frames[0].ecs_mirror_hash, [2u8; 32]);
}

#[test]
fn shadow_response_bincode_roundtrip() {
    let resp = ShadowResponse {
        status: ShadowStatus::AllMatch,
        checked_ticks: vec![42],
    };
    let encoded = bincode::serialize(&resp).unwrap();
    let decoded: ShadowResponse = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.status, ShadowStatus::AllMatch);
    assert_eq!(decoded.checked_ticks, vec![42]);
}

#[test]
fn shadow_status_all_variants_roundtrip() {
    // AllMatch
    let s1 = ShadowStatus::AllMatch;
    let enc = bincode::serialize(&s1).unwrap();
    let dec: ShadowStatus = bincode::deserialize(&enc).unwrap();
    assert_eq!(dec, ShadowStatus::AllMatch);

    // Mismatch
    let s2 = ShadowStatus::Mismatch {
        tick: 1,
        expected_hash: [0xFFu8; 32],
        actual_hash: [0x00u8; 32],
    };
    let enc2 = bincode::serialize(&s2).unwrap();
    let dec2: ShadowStatus = bincode::deserialize(&enc2).unwrap();
    assert_eq!(
        dec2,
        ShadowStatus::Mismatch {
            tick: 1,
            expected_hash: [0xFFu8; 32],
            actual_hash: [0x00u8; 32],
        }
    );

    // Error（含 UTF-8 中文字元）
    let s3 = ShadowStatus::Error("測試錯誤".to_string());
    let enc3 = bincode::serialize(&s3).unwrap();
    let dec3: ShadowStatus = bincode::deserialize(&enc3).unwrap();
    assert_eq!(dec3, ShadowStatus::Error("測試錯誤".to_string()));
}

#[test]
fn shadow_frame_field_sizes() {
    let frame = ShadowFrame {
        tick: 0,
        inputs: vec![],
        rng_state: [0u8; 16],
        ecs_mirror_hash: [0u8; 32],
    };
    assert_eq!(frame.rng_state.len(), 16);
    assert_eq!(frame.ecs_mirror_hash.len(), 32);
}

#[test]
fn shadow_init_bincode_roundtrip() {
    let bytecode_bytes = vec![0x52u8, 0x48, 0x42, 0x43]; // "RHBC" magic
    let hash = blake3::hash(&bytecode_bytes);
    let init = ShadowInit {
        bytecode: bytecode_bytes.clone(),
        bytecode_hash: *hash.as_bytes(),
    };
    let encoded = bincode::serialize(&init).unwrap();
    let decoded: ShadowInit = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.bytecode, init.bytecode);
    assert_eq!(decoded.bytecode_hash, init.bytecode_hash);
}

#[test]
fn shadow_init_ack_bincode_roundtrip() {
    let ack = ShadowInitAck;
    let encoded = bincode::serialize(&ack).unwrap();
    let _decoded: ShadowInitAck = bincode::deserialize(&encoded).unwrap();
}

#[test]
fn shadow_request_empty_frames() {
    let req = ShadowRequest { frames: vec![] };
    let encoded = bincode::serialize(&req).unwrap();
    let decoded: ShadowRequest = bincode::deserialize(&encoded).unwrap();
    assert!(decoded.frames.is_empty());
}

#[test]
fn shadow_response_empty_checked_ticks() {
    let resp = ShadowResponse {
        status: ShadowStatus::AllMatch,
        checked_ticks: vec![],
    };
    let encoded = bincode::serialize(&resp).unwrap();
    let decoded: ShadowResponse = bincode::deserialize(&encoded).unwrap();
    assert!(decoded.checked_ticks.is_empty());
}

#[test]
fn shadow_frame_large_inputs_roundtrip() {
    let large_inputs: Vec<PlayerInput> = (0..64).map(make_player_input).collect();
    let frame = ShadowFrame {
        tick: u64::MAX,
        inputs: large_inputs,
        rng_state: [0xFF; 16],
        ecs_mirror_hash: [0xFF; 32],
    };
    let req = ShadowRequest {
        frames: vec![frame; 4],
    };
    let encoded = bincode::serialize(&req).unwrap();
    let decoded: ShadowRequest = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.frames.len(), 4);
    assert_eq!(decoded.frames[0].inputs.len(), 64);
    assert_eq!(decoded.frames[0].tick, u64::MAX);
}

#[test]
fn shadow_status_mismatch_roundtrip() {
    let s = ShadowStatus::Mismatch {
        tick: 99,
        expected_hash: [0xABu8; 32],
        actual_hash: [0xCDu8; 32],
    };
    let enc = bincode::serialize(&s).unwrap();
    let dec: ShadowStatus = bincode::deserialize(&enc).unwrap();
    assert_eq!(
        dec,
        ShadowStatus::Mismatch {
            tick: 99,
            expected_hash: [0xABu8; 32],
            actual_hash: [0xCDu8; 32],
        }
    );
}

#[test]
fn shadow_status_error_roundtrip() {
    let s = ShadowStatus::Error("測試錯誤".to_string());
    let enc = bincode::serialize(&s).unwrap();
    let dec: ShadowStatus = bincode::deserialize(&enc).unwrap();
    assert_eq!(dec, ShadowStatus::Error("測試錯誤".to_string()));
}
