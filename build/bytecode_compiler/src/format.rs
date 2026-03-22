//! .rhai.bc 二進位格式常數與組裝函式
//!
//! 對齊 file-format.md §5.2 定義的欄位順序：
//! ```text
//! Magic(4) + Version(2) + MetadataLen(2) + Metadata(M)
//! + Nonce(12) + Ciphertext(P) + AuthTag(16) + Signature(64)
//! ```

use crate::error::{CompileError, FormatError, LoadError};
use crate::metadata::ScriptMetadata;

/// .rhai.bc 檔案 Magic Number（§5.2）
pub const MAGIC: &[u8; 4] = b"RHBC";

/// 格式版本（§5.6）
pub const FORMAT_VERSION_MAJOR: u8 = 1;
pub const FORMAT_VERSION_MINOR: u8 = 0;

/// Debug 模式版本標記 — minor = 0xFF 表示未加密未簽章
/// load() 端偵測到此值時回傳 LoadError::DebugBuildBytecode，
/// 防止 debug bytecode 在 release build 被意外載入。
pub const DEBUG_VERSION_MINOR: u8 = 0xFF;

/// Header 固定大小（Magic 4 + Version 2 + MetadataLen 2）
pub const HEADER_FIXED_SIZE: usize = 4 + 2 + 2;

/// AES-GCM Nonce 大小
pub const NONCE_SIZE: usize = 12;

/// AES-GCM Auth Tag 大小
pub const AUTH_TAG_SIZE: usize = 16;

/// Ed25519 Signature 大小
pub const SIGNATURE_SIZE: usize = 64;

/// 組合最終的 .rhai.bc 二進位格式（§5.2）
///
/// 依據 file-format.md 定義的欄位順序線性組裝：
/// Magic(4) + Version(2) + MetadataLen(2) + Metadata(M) +
/// Nonce(12) + Ciphertext(P) + AuthTag(16) + Signature(64)
pub fn assemble_bytecode(
    metadata_bytes: &[u8],
    nonce: &[u8; 12],
    ciphertext: &[u8],
    auth_tag: &[u8; 16],
    signature: &[u8; 64],
    version_major: u8,
    version_minor: u8,
) -> Vec<u8> {
    let metadata_len = metadata_bytes.len() as u16;
    let total_size = HEADER_FIXED_SIZE
        + metadata_bytes.len()
        + NONCE_SIZE
        + ciphertext.len()
        + AUTH_TAG_SIZE
        + SIGNATURE_SIZE;

    let mut buf = Vec::with_capacity(total_size);

    // Magic (4 bytes)
    buf.extend_from_slice(MAGIC);
    // Version (2 bytes): major, minor
    buf.push(version_major);
    buf.push(version_minor);
    // MetadataLen (2 bytes LE)
    buf.extend_from_slice(&metadata_len.to_le_bytes());
    // Metadata (M bytes)
    buf.extend_from_slice(metadata_bytes);
    // Nonce (12 bytes)
    buf.extend_from_slice(nonce);
    // Ciphertext (P bytes)
    buf.extend_from_slice(ciphertext);
    // Auth Tag (16 bytes)
    buf.extend_from_slice(auth_tag);
    // Signature (64 bytes)
    buf.extend_from_slice(signature);

    buf
}

/// 解析 .rhai.bc header — 回傳 BytecodeHeader 供後續步驟使用
///
/// 對齊 file-format.md §5.2：檢查 Magic、資料長度、版本號、metadata 範圍
pub fn parse_header(data: &[u8]) -> Result<BytecodeHeader, FormatError> {
    if data.len() < HEADER_FIXED_SIZE {
        return Err(FormatError::TruncatedData {
            expected: HEADER_FIXED_SIZE,
            got: data.len(),
        });
    }
    let magic: [u8; 4] = data[0..4].try_into().unwrap();
    if &magic != MAGIC {
        return Err(FormatError::InvalidMagic(magic));
    }
    let version_major = data[4];
    let version_minor = data[5];
    let metadata_len = u16::from_le_bytes([data[6], data[7]]) as usize;
    // 檢查 metadata_len 不超出剩餘資料範圍
    if HEADER_FIXED_SIZE + metadata_len > data.len() {
        return Err(FormatError::MetadataLengthOutOfRange);
    }
    let metadata_bytes = &data[HEADER_FIXED_SIZE..HEADER_FIXED_SIZE + metadata_len];
    let remaining = &data[HEADER_FIXED_SIZE + metadata_len..];

    Ok(BytecodeHeader {
        version_major,
        version_minor,
        metadata_bytes: metadata_bytes.to_vec(),
        remaining: remaining.to_vec(),
    })
}

/// 反序列化 BytecodeHeader 中的 metadata bytes 為 ScriptMetadata
pub fn deserialize_metadata(header: &BytecodeHeader) -> Result<ScriptMetadata, FormatError> {
    let meta: ScriptMetadata = bincode::deserialize(&header.metadata_bytes)?;
    Ok(meta)
}

/// parse_header() 的回傳結構
#[derive(Debug)]
pub struct BytecodeHeader {
    pub version_major: u8,
    pub version_minor: u8,
    pub metadata_bytes: Vec<u8>,
    pub remaining: Vec<u8>,
}

/// 編譯 .rhai 原始碼為 .rhai.bc 加密 bytecode（§5.1 sign-then-encrypt）
///
/// 流程：驗證非空 → blake3 hash → Rhai compile（僅驗證語法，丟棄 AST）→
/// source bytes 作為 payload → 建構 ScriptMetadata →
/// Ed25519 sign(metadata‖source_bytes) → AES-GCM encrypt → assemble
///
/// ## Fallback 策略（Phase 5 Task 01 spike 結論）
/// Rhai AST 無法 bincode 序列化（未實作 serde::Serialize），
/// 因此 payload 為 source UTF-8 bytes，載入端需 runtime compile。
///
/// # 參數
/// - `encryption_key`: AES-256-GCM session key（32 bytes，
///   由呼叫端完成 HKDF 衍生，info=b"bevy-game-bytecode-v1"，見 crypto-spec.md §5.4）
pub fn compile(
    source: &str,
    script_id: &str,
    priority: u8,
    signing_key: &crypto::SigningKeyPair,
    encryption_key: &[u8; 32],
) -> Result<Vec<u8>, CompileError> {
    // 1. 驗證 source 非空
    if source.is_empty() {
        return Err(CompileError::EmptySource);
    }

    // 2. blake3 hash
    let source_hash: [u8; 32] = blake3::hash(source.as_bytes()).into();

    // 3. Rhai compile — 僅驗證語法正確性，AST 丟棄（無法序列化）
    let engine = rhai::Engine::new();
    let _ast = engine.compile(source)?;

    // 4. source bytes 作為 payload（fallback：取代 bincode::serialize(&ast)）
    let plaintext_payload = source.as_bytes().to_vec();

    // 5. build_timestamp
    let build_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 6. 建構 ScriptMetadata
    let metadata = ScriptMetadata {
        script_id: script_id.to_string(),
        priority,
        source_hash,
        build_timestamp,
    };

    // 7. bincode serialize metadata（ScriptMetadata 為簡單 struct，序列化不應失敗）
    let metadata_bytes =
        bincode::serialize(&metadata).expect("ScriptMetadata bincode 序列化不應失敗");

    // 8. 長度檢查（build tool 層不變式）
    assert!(
        metadata_bytes.len() <= u16::MAX as usize,
        "metadata 超出 u16 上限：{} bytes",
        metadata_bytes.len()
    );

    // 9. 組裝簽章訊息：metadata_bytes + source_bytes
    let mut sign_message = Vec::with_capacity(metadata_bytes.len() + plaintext_payload.len());
    sign_message.extend_from_slice(&metadata_bytes);
    sign_message.extend_from_slice(&plaintext_payload);

    // 10. Ed25519 簽章（sign-then-encrypt：先簽署明文）
    let signature: [u8; 64] = signing_key.sign(&sign_message);

    // 11. AES-GCM 加密 payload（#[from] CryptoError → EncryptionFailed）
    let enc = crypto::aes_encrypt_parts(encryption_key, &plaintext_payload)?;

    // 12. 組裝最終二進位格式
    Ok(assemble_bytecode(
        &metadata_bytes,
        &enc.nonce,
        &enc.ciphertext,
        &enc.tag,
        &signature,
        FORMAT_VERSION_MAJOR,
        FORMAT_VERSION_MINOR,
    ))
}

/// 載入 .rhai.bc — decrypt-then-verify（§5.3）
///
/// 流程：parse_header → version check → deserialize_metadata →
/// AES-GCM decrypt → Ed25519 verify → UTF-8 decode → Rhai compile
///
/// ## Fallback 策略（Phase 5 Task 01 spike 結論）
/// payload 為 source UTF-8 bytes（非 AST bytes），載入端需 runtime compile。
///
/// # 參數
/// - `decryption_key`: AES-256-GCM session key（32 bytes，
///   由呼叫端完成 HKDF 衍生，info=b"bevy-game-bytecode-v1"，見 crypto-spec.md §5.4）
pub fn load(
    data: &[u8],
    verifying_key: &[u8; 32],
    decryption_key: &[u8; 32],
) -> Result<(ScriptMetadata, rhai::AST), LoadError> {
    // 1. 長度檢查
    if data.len() < HEADER_FIXED_SIZE {
        return Err(FormatError::TruncatedData {
            expected: HEADER_FIXED_SIZE,
            got: data.len(),
        }
        .into());
    }

    // 2. Magic 檢查
    let magic: [u8; 4] = data[0..4].try_into().unwrap();
    if magic != *MAGIC {
        return Err(FormatError::InvalidMagic(magic).into());
    }

    // 3. 版本檢查（先 debug 後 major，對齊 version-compatibility.md 不變式 #4）
    let major = data[4];
    let minor = data[5];
    if minor == DEBUG_VERSION_MINOR {
        return Err(LoadError::DebugBuildBytecode);
    }
    if major != FORMAT_VERSION_MAJOR {
        return Err(LoadError::IncompatibleVersion {
            expected: FORMAT_VERSION_MAJOR,
            got: major,
        });
    }

    // 4. 讀取 metadata_len
    let metadata_len = u16::from_le_bytes([data[6], data[7]]) as usize;

    // 5. 驗證剩餘資料足夠（metadata + nonce + tag + signature）
    let min_total = HEADER_FIXED_SIZE + metadata_len + NONCE_SIZE + AUTH_TAG_SIZE + SIGNATURE_SIZE;
    if min_total > data.len() {
        return Err(FormatError::MetadataLengthOutOfRange.into());
    }

    // 6. 反序列化 metadata
    let metadata_bytes = &data[HEADER_FIXED_SIZE..HEADER_FIXED_SIZE + metadata_len];
    let metadata: ScriptMetadata =
        bincode::deserialize(metadata_bytes).map_err(FormatError::MetadataDeserializationFailed)?;

    // 7. 解析各欄位位置
    let metadata_end = HEADER_FIXED_SIZE + metadata_len;
    let nonce_bytes: [u8; 12] = data[metadata_end..metadata_end + NONCE_SIZE]
        .try_into()
        .unwrap();
    let signature: [u8; 64] = data[data.len() - SIGNATURE_SIZE..].try_into().unwrap();
    let tag_start = data.len() - SIGNATURE_SIZE - AUTH_TAG_SIZE;
    let auth_tag: [u8; 16] = data[tag_start..tag_start + AUTH_TAG_SIZE]
        .try_into()
        .unwrap();
    let ciphertext = &data[metadata_end + NONCE_SIZE..tag_start];

    // 8. AES-GCM 解密（decrypt-then-verify）
    let parts = crypto::AesEncryptedParts {
        nonce: nonce_bytes,
        ciphertext: ciphertext.to_vec(),
        tag: auth_tag,
    };
    let plaintext =
        crypto::aes_decrypt_parts(decryption_key, &parts).map_err(LoadError::DecryptionFailed)?;

    // 9. Ed25519 驗簽（sign_message = metadata_bytes + plaintext）
    let mut sign_message = Vec::with_capacity(metadata_bytes.len() + plaintext.len());
    sign_message.extend_from_slice(metadata_bytes);
    sign_message.extend_from_slice(&plaintext);
    crypto::verify_signature(verifying_key, &sign_message, &signature)
        .map_err(|_| LoadError::SignatureVerificationFailed)?;

    // 10. UTF-8 解碼 source（fallback：取代 bincode::deserialize::<AST>）
    let source = String::from_utf8(plaintext).map_err(|_| LoadError::SourceDecodeFailed)?;

    // 11. Rhai runtime compile
    let engine = rhai::Engine::new();
    let ast = engine
        .compile(&source)
        .map_err(LoadError::RhaiCompileFailed)?;

    Ok((metadata, ast))
}

/// Debug 模式編譯 — 無加密、無簽章（minor = 0xFF）
///
/// 流程與 compile() 相同（驗空、hash、Rhai compile 驗語法、source bytes payload、metadata），
/// 但跳過 sign 和 encrypt：nonce/auth_tag/signature 填全零，payload 為明文 source bytes。
pub fn compile_debug(source: &str, script_id: &str, priority: u8) -> Result<Vec<u8>, CompileError> {
    // 1. 驗證 source 非空
    if source.is_empty() {
        return Err(CompileError::EmptySource);
    }

    // 2. blake3 hash
    let source_hash: [u8; 32] = blake3::hash(source.as_bytes()).into();

    // 3. Rhai compile — 僅驗證語法正確性，AST 丟棄
    let engine = rhai::Engine::new();
    let _ast = engine.compile(source)?;

    // 4. source bytes 作為 payload（fallback）
    let plaintext_payload = source.as_bytes().to_vec();

    // 5. build_timestamp
    let build_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 6. 建構 ScriptMetadata
    let metadata = ScriptMetadata {
        script_id: script_id.to_string(),
        priority,
        source_hash,
        build_timestamp,
    };

    // 7. bincode serialize metadata
    let metadata_bytes =
        bincode::serialize(&metadata).expect("ScriptMetadata bincode 序列化不應失敗");
    assert!(
        metadata_bytes.len() <= u16::MAX as usize,
        "metadata 超出 u16 上限：{} bytes",
        metadata_bytes.len()
    );

    // Debug：nonce/auth_tag/signature 全零，payload 不加密
    let nonce = [0u8; 12];
    let auth_tag = [0u8; 16];
    let signature = [0u8; 64];

    Ok(assemble_bytecode(
        &metadata_bytes,
        &nonce,
        &plaintext_payload,
        &auth_tag,
        &signature,
        FORMAT_VERSION_MAJOR,
        DEBUG_VERSION_MINOR,
    ))
}

/// Debug 模式載入 — 跳過解密和簽章驗證
///
/// minor ≠ DEBUG_VERSION_MINOR 時回傳錯誤（release bytecode 不可被 debug load 載入）
pub fn load_debug(data: &[u8]) -> Result<(ScriptMetadata, rhai::AST), LoadError> {
    // 1. 長度檢查
    if data.len() < HEADER_FIXED_SIZE {
        return Err(FormatError::TruncatedData {
            expected: HEADER_FIXED_SIZE,
            got: data.len(),
        }
        .into());
    }

    // 2. Magic 檢查
    let magic: [u8; 4] = data[0..4].try_into().unwrap();
    if magic != *MAGIC {
        return Err(FormatError::InvalidMagic(magic).into());
    }

    // 3. 版本檢查：minor 必須為 DEBUG_VERSION_MINOR
    let major = data[4];
    let minor = data[5];
    if minor != DEBUG_VERSION_MINOR {
        return Err(LoadError::IncompatibleVersion {
            expected: FORMAT_VERSION_MAJOR,
            got: major,
        });
    }
    if major != FORMAT_VERSION_MAJOR {
        return Err(LoadError::IncompatibleVersion {
            expected: FORMAT_VERSION_MAJOR,
            got: major,
        });
    }

    // 4. 讀取 metadata_len
    let metadata_len = u16::from_le_bytes([data[6], data[7]]) as usize;

    // 5. 驗證剩餘資料足夠
    let min_total = HEADER_FIXED_SIZE + metadata_len + NONCE_SIZE + AUTH_TAG_SIZE + SIGNATURE_SIZE;
    if min_total > data.len() {
        return Err(FormatError::MetadataLengthOutOfRange.into());
    }

    // 6. 反序列化 metadata
    let metadata_bytes = &data[HEADER_FIXED_SIZE..HEADER_FIXED_SIZE + metadata_len];
    let metadata: ScriptMetadata =
        bincode::deserialize(metadata_bytes).map_err(FormatError::MetadataDeserializationFailed)?;

    // 7. 直接讀取明文 payload（跳過解密和驗簽）
    let metadata_end = HEADER_FIXED_SIZE + metadata_len;
    let tag_start = data.len() - SIGNATURE_SIZE - AUTH_TAG_SIZE;
    let payload = &data[metadata_end + NONCE_SIZE..tag_start];

    // 8. UTF-8 解碼 source（fallback：取代 bincode::deserialize::<AST>）
    let source = String::from_utf8(payload.to_vec()).map_err(|_| LoadError::SourceDecodeFailed)?;

    // 9. Rhai runtime compile
    let engine = rhai::Engine::new();
    let ast = engine
        .compile(&source)
        .map_err(LoadError::RhaiCompileFailed)?;

    Ok((metadata, ast))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{CompileError, FormatError, LoadError};
    use crate::metadata::ScriptMetadata;
    use crypto::aes::aes_encrypt_parts;
    use crypto::signing::SigningKeyPair;

    const TEST_SOURCE: &str = "let x = 40 + 2; x";

    /// 確定性測試金鑰：(signing_key, verifying_key_bytes, encryption_key)
    fn test_keys() -> (SigningKeyPair, [u8; 32], [u8; 32]) {
        let signing_key = SigningKeyPair::from_seed(&[42u8; 32]);
        let verifying_key: [u8; 32] = signing_key.public_key();
        let encryption_key = [1u8; 32];
        (signing_key, verifying_key, encryption_key)
    }

    // === Release mode tests ===

    #[test]
    fn compile_load_round_trip_ast_eval() {
        let (signing_key, verify_key, enc_key) = test_keys();
        let bytecode = compile(TEST_SOURCE, "test_script", 0, &signing_key, &enc_key).unwrap();
        let (metadata, ast) = load(&bytecode, &verify_key, &enc_key).unwrap();
        assert_eq!(metadata.script_id, "test_script");
        assert_eq!(metadata.priority, 0);
        let engine = rhai::Engine::new();
        let result: i64 = engine.eval_ast(&ast).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn compile_produces_valid_header() {
        let (signing_key, _verify_key, enc_key) = test_keys();
        let bytecode = compile(TEST_SOURCE, "test", 5, &signing_key, &enc_key).unwrap();
        assert_eq!(&bytecode[0..4], b"RHBC");
        assert_eq!(bytecode[4], FORMAT_VERSION_MAJOR);
        assert_eq!(bytecode[5], FORMAT_VERSION_MINOR);
        let metadata_len = u16::from_le_bytes([bytecode[6], bytecode[7]]) as usize;
        assert!(metadata_len > 0);
    }

    #[test]
    fn compile_metadata_contains_correct_source_hash() {
        let (signing_key, verify_key, enc_key) = test_keys();
        let bytecode = compile(TEST_SOURCE, "hash_test", 0, &signing_key, &enc_key).unwrap();
        let (metadata, _ast) = load(&bytecode, &verify_key, &enc_key).unwrap();
        let expected_hash: [u8; 32] = blake3::hash(TEST_SOURCE.as_bytes()).into();
        assert_eq!(metadata.source_hash, expected_hash);
    }

    #[test]
    fn compile_empty_source_fails() {
        let (signing_key, _verify_key, enc_key) = test_keys();
        let result = compile("", "test", 0, &signing_key, &enc_key);
        assert!(matches!(result, Err(CompileError::EmptySource)));
    }

    #[test]
    fn tampered_payload_fails_verification() {
        let (signing_key, verify_key, enc_key) = test_keys();
        let mut bytecode = compile(TEST_SOURCE, "test", 0, &signing_key, &enc_key).unwrap();
        // payload 區段起始 = HEADER_FIXED_SIZE(8) + metadata_len + NONCE_SIZE(12)
        let metadata_len = u16::from_le_bytes([bytecode[6], bytecode[7]]) as usize;
        let payload_start = HEADER_FIXED_SIZE + metadata_len + NONCE_SIZE;
        // payload 區段結束 = bytecode.len() - SIGNATURE_SIZE(64) - AUTH_TAG_SIZE(16)
        let payload_end = bytecode.len() - SIGNATURE_SIZE - AUTH_TAG_SIZE;
        if payload_start < payload_end {
            bytecode[payload_start] ^= 0xFF;
        }
        let result = load(&bytecode, &verify_key, &enc_key);
        // 密文篡改：GCM tag 驗證失敗 → DecryptionFailed，或解密後簽章不符 → SignatureVerificationFailed
        assert!(result.is_err());
    }

    #[test]
    fn wrong_decryption_key_fails() {
        let (signing_key, verify_key, enc_key) = test_keys();
        let bytecode = compile(TEST_SOURCE, "test", 0, &signing_key, &enc_key).unwrap();
        let wrong_key = [99u8; 32];
        let result = load(&bytecode, &verify_key, &wrong_key);
        assert!(matches!(result, Err(LoadError::DecryptionFailed(_))));
    }

    #[test]
    fn wrong_verifying_key_fails() {
        let (signing_key, _verify_key, enc_key) = test_keys();
        let bytecode = compile(TEST_SOURCE, "test", 0, &signing_key, &enc_key).unwrap();
        let wrong_verify = SigningKeyPair::from_seed(&[99u8; 32]).public_key();
        let result = load(&bytecode, &wrong_verify, &enc_key);
        assert!(matches!(
            result,
            Err(LoadError::SignatureVerificationFailed)
        ));
    }

    #[test]
    fn invalid_magic_fails() {
        let data = b"XXXX_not_a_bytecode_file_at_all_padding_to_100_bytes_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let result = load(data, &[0u8; 32], &[0u8; 32]);
        assert!(matches!(
            result,
            Err(LoadError::Format(FormatError::InvalidMagic(_)))
        ));
    }

    #[test]
    fn truncated_data_fails() {
        let data = [0u8; 3]; // 小於 HEADER_FIXED_SIZE(8)
        let result = load(&data, &[0u8; 32], &[0u8; 32]);
        assert!(matches!(
            result,
            Err(LoadError::Format(FormatError::TruncatedData {
                expected: 8,
                got: 3
            }))
        ));
    }

    #[test]
    fn metadata_len_out_of_range_fails() {
        // 構造合法 Magic + Version 但 metadata_len 指向超出資料範圍
        let mut data = Vec::new();
        data.extend_from_slice(b"RHBC"); // Magic
        data.push(FORMAT_VERSION_MAJOR); // major
        data.push(FORMAT_VERSION_MINOR); // minor
        data.extend_from_slice(&u16::MAX.to_le_bytes()); // metadata_len = 65535
        data.extend_from_slice(&[0u8; 20]); // 不足的後續資料
        let result = load(&data, &[0u8; 32], &[0u8; 32]);
        assert!(matches!(
            result,
            Err(LoadError::Format(FormatError::MetadataLengthOutOfRange))
        ));
    }

    #[test]
    fn corrupted_metadata_fails() {
        let (signing_key, verify_key, enc_key) = test_keys();
        let mut bytecode = compile(TEST_SOURCE, "test", 0, &signing_key, &enc_key).unwrap();
        // 破壞 metadata 區段（偏移 8 開始）使 bincode 無法反序列化
        let metadata_len = u16::from_le_bytes([bytecode[6], bytecode[7]]) as usize;
        if metadata_len > 0 {
            for i in HEADER_FIXED_SIZE..HEADER_FIXED_SIZE + metadata_len {
                bytecode[i] = 0xFF;
            }
        }
        let result = load(&bytecode, &verify_key, &enc_key);
        assert!(matches!(
            result,
            Err(LoadError::Format(
                FormatError::MetadataDeserializationFailed(_)
            ))
        ));
    }

    #[test]
    fn incompatible_version_fails() {
        let (signing_key, verify_key, enc_key) = test_keys();
        let mut bytecode = compile(TEST_SOURCE, "test", 0, &signing_key, &enc_key).unwrap();
        bytecode[4] = 99; // 篡改 major version
        let result = load(&bytecode, &verify_key, &enc_key);
        assert!(matches!(
            result,
            Err(LoadError::IncompatibleVersion {
                expected: 1,
                got: 99
            })
        ));
    }

    #[test]
    fn debug_build_rejected_by_release_load() {
        let bytecode = compile_debug(TEST_SOURCE, "test", 0).unwrap();
        let result = load(&bytecode, &[0u8; 32], &[0u8; 32]);
        assert!(matches!(result, Err(LoadError::DebugBuildBytecode)));
    }

    #[test]
    fn corrupted_plaintext_fails_source_decode() {
        // 手動組裝：合法 metadata + 非 UTF-8 payload → 解密/驗簽成功但 UTF-8 解碼失敗
        let (signing_key, verify_key, enc_key) = test_keys();
        let garbage: &[u8] = &[0xFF, 0xFE, 0x80, 0x81, 0x82]; // 無效 UTF-8
        let meta = ScriptMetadata {
            script_id: "test".to_string(),
            priority: 0,
            source_hash: blake3::hash(b"whatever").into(),
            build_timestamp: 0,
        };
        let meta_bytes = bincode::serialize(&meta).unwrap();
        let mut msg = meta_bytes.clone();
        msg.extend_from_slice(garbage);
        let sig = signing_key.sign(&msg);
        let enc = aes_encrypt_parts(&enc_key, garbage).unwrap();
        let bytecode = assemble_bytecode(
            &meta_bytes,
            &enc.nonce,
            &enc.ciphertext,
            &enc.tag,
            &sig,
            FORMAT_VERSION_MAJOR,
            FORMAT_VERSION_MINOR,
        );
        let result = load(&bytecode, &verify_key, &enc_key);
        assert!(matches!(result, Err(LoadError::SourceDecodeFailed)));
    }

    #[test]
    fn invalid_rhai_source_fails_compile() {
        // 手動組裝：合法 metadata + 合法 UTF-8 但無效 Rhai 語法 → compile 失敗
        let (signing_key, verify_key, enc_key) = test_keys();
        let bad_source = b"let x = ;"; // 合法 UTF-8 但無效 Rhai
        let meta = ScriptMetadata {
            script_id: "test".to_string(),
            priority: 0,
            source_hash: blake3::hash(bad_source).into(),
            build_timestamp: 0,
        };
        let meta_bytes = bincode::serialize(&meta).unwrap();
        let mut msg = meta_bytes.clone();
        msg.extend_from_slice(bad_source);
        let sig = signing_key.sign(&msg);
        let enc = aes_encrypt_parts(&enc_key, bad_source).unwrap();
        let bytecode = assemble_bytecode(
            &meta_bytes,
            &enc.nonce,
            &enc.ciphertext,
            &enc.tag,
            &sig,
            FORMAT_VERSION_MAJOR,
            FORMAT_VERSION_MINOR,
        );
        let result = load(&bytecode, &verify_key, &enc_key);
        assert!(matches!(result, Err(LoadError::RhaiCompileFailed(_))));
    }

    // === Debug mode tests ===

    #[test]
    fn compile_debug_load_debug_round_trip() {
        let bytecode = compile_debug(TEST_SOURCE, "test_debug", 3).unwrap();
        let (metadata, ast) = load_debug(&bytecode).unwrap();
        assert_eq!(metadata.script_id, "test_debug");
        assert_eq!(metadata.priority, 3);
        let engine = rhai::Engine::new();
        let result: i64 = engine.eval_ast(&ast).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn debug_format_has_correct_version_minor() {
        let bytecode = compile_debug(TEST_SOURCE, "test", 0).unwrap();
        assert_eq!(&bytecode[0..4], b"RHBC");
        assert_eq!(bytecode[4], FORMAT_VERSION_MAJOR);
        assert_eq!(bytecode[5], DEBUG_VERSION_MINOR);
    }

    #[test]
    fn release_build_rejected_by_debug_load() {
        let (signing_key, _verify_key, enc_key) = test_keys();
        let bytecode = compile(TEST_SOURCE, "test", 0, &signing_key, &enc_key).unwrap();
        let result = load_debug(&bytecode);
        // 具體 variant 由 task-06 實作決定
        assert!(result.is_err());
    }

    #[test]
    fn debug_compile_empty_source_fails() {
        let result = compile_debug("", "test", 0);
        assert!(matches!(result, Err(CompileError::EmptySource)));
    }
}
