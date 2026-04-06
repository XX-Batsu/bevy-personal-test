//! 加密 wire frame 模組
//!
//! Wire format: `[4B LE len] | [12B nonce] | [AES-256-GCM(plaintext)]`
//! 其中 `len = 12 + len(ciphertext + 16B auth_tag)`。
//!
//! nonce 由 OsRng 隨機生成（密碼學真隨機）。
//! OsRng 失敗屬非預期系統錯誤，以 panic 處理（WASM = abort）。

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FrameError {
    #[error("AES-GCM 認證失敗（資料被竄改或金鑰錯誤）")]
    AuthFailed,
    #[error("frame 長度不足（需要 {needed} bytes，實際 {available} bytes）")]
    TooShort { needed: usize, available: usize },
}

use rand_core::{OsRng, RngCore};

pub fn encrypt_frame(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, FrameError> {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext =
        crate::aes::encrypt(key, &nonce, plaintext, &[]).map_err(|_| FrameError::AuthFailed)?;

    let payload_len = 12 + ciphertext.len();
    let mut frame = Vec::with_capacity(4 + payload_len);
    frame.extend_from_slice(&(payload_len as u32).to_le_bytes());
    frame.extend_from_slice(&nonce);
    frame.extend_from_slice(&ciphertext);
    Ok(frame)
}

pub fn decrypt_frame(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, FrameError> {
    if data.len() < 4 {
        return Err(FrameError::TooShort {
            needed: 4,
            available: data.len(),
        });
    }
    let payload_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
    if payload_len < 12 {
        return Err(FrameError::TooShort {
            needed: 12,
            available: payload_len,
        });
    }
    if data.len() < 4 + payload_len {
        return Err(FrameError::TooShort {
            needed: 4 + payload_len,
            available: data.len(),
        });
    }
    let nonce: [u8; 12] = data[4..16].try_into().unwrap();
    let ciphertext = &data[16..4 + payload_len];
    crate::aes::decrypt(key, &nonce, ciphertext, &[]).map_err(|_| FrameError::AuthFailed)
}

pub fn encrypt_frame_aad(
    plaintext: &[u8],
    key: &[u8; 32],
    seq: u64,
) -> Result<Vec<u8>, FrameError> {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let aad = seq.to_le_bytes();
    let ciphertext =
        crate::aes::encrypt(key, &nonce, plaintext, &aad).map_err(|_| FrameError::AuthFailed)?;

    let payload_len = 12 + ciphertext.len();
    let mut frame = Vec::with_capacity(4 + payload_len);
    frame.extend_from_slice(&(payload_len as u32).to_le_bytes());
    frame.extend_from_slice(&nonce);
    frame.extend_from_slice(&ciphertext);
    Ok(frame)
}

pub fn decrypt_frame_aad(data: &[u8], key: &[u8; 32], seq: u64) -> Result<Vec<u8>, FrameError> {
    if data.len() < 4 {
        return Err(FrameError::TooShort {
            needed: 4,
            available: data.len(),
        });
    }
    let payload_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
    if payload_len < 12 {
        return Err(FrameError::TooShort {
            needed: 12,
            available: payload_len,
        });
    }
    if data.len() < 4 + payload_len {
        return Err(FrameError::TooShort {
            needed: 4 + payload_len,
            available: data.len(),
        });
    }
    let nonce: [u8; 12] = data[4..16].try_into().unwrap();
    let ciphertext = &data[16..4 + payload_len];
    let aad = seq.to_le_bytes();
    crate::aes::decrypt(key, &nonce, ciphertext, &aad).map_err(|_| FrameError::AuthFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [0x42u8; 32];
    const WRONG_KEY: [u8; 32] = [0x99u8; 32];

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"hello encrypted world";
        let frame = encrypt_frame(plaintext, &KEY).unwrap();
        let decrypted = decrypt_frame(&frame, &KEY).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let frame = encrypt_frame(b"data", &KEY).unwrap();
        let mut tampered = frame.clone();
        *tampered.last_mut().unwrap() ^= 0xFF;
        assert_eq!(decrypt_frame(&tampered, &KEY), Err(FrameError::AuthFailed));
    }

    #[test]
    fn tampered_nonce_fails() {
        let frame = encrypt_frame(b"data", &KEY).unwrap();
        let mut tampered = frame.clone();
        tampered[4] ^= 0xFF;
        assert_eq!(decrypt_frame(&tampered, &KEY), Err(FrameError::AuthFailed));
    }

    #[test]
    fn wrong_key_fails() {
        let frame = encrypt_frame(b"secret", &KEY).unwrap();
        assert_eq!(
            decrypt_frame(&frame, &WRONG_KEY),
            Err(FrameError::AuthFailed)
        );
    }

    #[test]
    fn too_short_data_fails() {
        let data = [0u8; 5];
        assert!(matches!(
            decrypt_frame(&data, &KEY),
            Err(FrameError::TooShort { .. })
        ));
    }

    #[test]
    fn nonce_field_too_short() {
        let mut data = vec![0u8; 4 + 11];
        data[0..4].copy_from_slice(&11u32.to_le_bytes());
        assert_eq!(
            decrypt_frame(&data, &KEY),
            Err(FrameError::TooShort {
                needed: 12,
                available: 11
            })
        );
    }

    #[test]
    fn aad_roundtrip() {
        let plaintext = b"important packet";
        let seq: u64 = 42;
        let frame = encrypt_frame_aad(plaintext, &KEY, seq).unwrap();
        let decrypted = decrypt_frame_aad(&frame, &KEY, seq).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aad_wrong_seq_fails() {
        let frame = encrypt_frame_aad(b"packet", &KEY, 100).unwrap();
        assert_eq!(
            decrypt_frame_aad(&frame, &KEY, 99),
            Err(FrameError::AuthFailed)
        );
    }

    #[test]
    fn empty_plaintext_roundtrip() {
        let frame = encrypt_frame(&[], &KEY).unwrap();
        assert_eq!(frame.len(), 4 + 12 + 16);
        let decrypted = decrypt_frame(&frame, &KEY).unwrap();
        assert_eq!(decrypted, b"");
    }

    #[test]
    fn large_plaintext_roundtrip() {
        let plaintext = vec![0xABu8; 4096];
        let frame = encrypt_frame(&plaintext, &KEY).unwrap();
        let decrypted = decrypt_frame(&frame, &KEY).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn nonce_uniqueness() {
        let frame1 = encrypt_frame(b"same", &KEY).unwrap();
        let frame2 = encrypt_frame(b"same", &KEY).unwrap();
        assert_ne!(frame1, frame2);
    }
}
