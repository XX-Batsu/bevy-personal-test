use anyhow::Context;
use crypto::{verify_signature, SigningKeyPair};
use std::path::Path;

/// 載入 32 bytes raw key 檔案（AES key / Ed25519 seed / public key）。
pub fn load_key_file(path: &Path) -> anyhow::Result<[u8; 32]> {
    if !path.exists() {
        anyhow::bail!("key-file 不存在：{}", path.display());
    }

    let bytes =
        std::fs::read(path).with_context(|| format!("無法讀取 key-file：{}", path.display()))?;

    if bytes.len() != 32 {
        anyhow::bail!(
            "key-file 長度必須為 32 bytes，實際：{} bytes（路徑：{}）",
            bytes.len(),
            path.display()
        );
    }

    Ok(bytes.try_into().unwrap())
}

/// AES-256-GCM 加密單一檔案。
/// 輸出格式：nonce(12) || ciphertext+auth_tag(N+16)。
/// Nonce 由 crypto::aes_encrypt 內部以 OsRng 自動生成。
pub fn encrypt_file(input_path: &Path, output_path: &Path, key: &[u8; 32]) -> anyhow::Result<()> {
    let plaintext = std::fs::read(input_path)
        .with_context(|| format!("無法讀取輸入檔案：{}", input_path.display()))?;

    let encrypted = crypto::aes_encrypt(key, &plaintext).context("AES-256-GCM 加密失敗")?;

    std::fs::write(output_path, &encrypted)
        .with_context(|| format!("無法寫入輸出檔案：{}", output_path.display()))?;

    Ok(())
}

/// AES-256-GCM 解密單一檔案。
/// 輸入格式：nonce(12) || ciphertext+auth_tag(N+16)。
pub fn decrypt_file(input_path: &Path, output_path: &Path, key: &[u8; 32]) -> anyhow::Result<()> {
    let data = std::fs::read(input_path)
        .with_context(|| format!("無法讀取加密檔案：{}", input_path.display()))?;

    // 長度檢查：nonce(12) + auth_tag(16) = 28 bytes 最小
    if data.len() < 12 + 16 {
        anyhow::bail!("加密檔案格式錯誤：長度不足（{}）", data.len());
    }

    let plaintext = crypto::aes_decrypt(key, &data)
        .context("AES-256-GCM 解密失敗（可能為錯誤 key 或資料損毀）")?;

    std::fs::write(output_path, &plaintext)
        .with_context(|| format!("無法寫入解密檔案：{}", output_path.display()))?;

    Ok(())
}

/// 批次加密目錄內的 .glb/.png 檔案，回傳加密檔案數量。
pub fn encrypt_dir(dir: &Path, key: &[u8; 32]) -> anyhow::Result<usize> {
    let mut count = 0usize;

    for entry in
        std::fs::read_dir(dir).with_context(|| format!("無法讀取目錄：{}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        if ext == "glb" || ext == "png" {
            let enc_name = format!("{}.enc", path.file_name().unwrap().to_string_lossy());
            let output_path = dir.join(&enc_name);

            encrypt_file(&path, &output_path, key)
                .with_context(|| format!("加密 {} 失敗", path.display()))?;

            count += 1;
        }
    }

    Ok(count)
}

/// Ed25519 簽名：生成 {file_path}.sig（64 bytes）。
pub fn sign_file(file_path: &Path, key_file: &Path) -> anyhow::Result<()> {
    let seed = load_key_file(key_file)?;
    let key_pair = SigningKeyPair::from_bytes(&seed);

    let content = std::fs::read(file_path)
        .with_context(|| format!("無法讀取待簽名檔案：{}", file_path.display()))?;

    let signature = key_pair.sign(&content);

    // 輸出 <file>.sig
    let sig_path = {
        let mut p = file_path.to_path_buf();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        p.set_file_name(format!("{name}.sig"));
        p
    };

    std::fs::write(&sig_path, signature)
        .with_context(|| format!("無法寫入簽名檔：{}", sig_path.display()))?;

    Ok(())
}

/// 生成金鑰組：AES-256 key + Ed25519 key pair。
///
/// 輸出三個檔案：
/// - `{prefix}.aes.key`      — 32 bytes，AES-256-GCM 對稱金鑰
/// - `{prefix}.ed25519.key`  — 32 bytes，Ed25519 signing seed（私鑰）
/// - `{prefix}.ed25519.pub`  — 32 bytes，Ed25519 verifying key（公鑰）
pub fn gen_key(prefix: &Path) -> anyhow::Result<()> {
    use ed25519_dalek::SigningKey;
    use rand::RngCore;

    // 建立輸出目錄
    if let Some(parent) = prefix.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("無法建立目錄：{}", parent.display()))?;
        }
    }

    // AES-256 key：32 隨機 bytes
    let mut aes_key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut aes_key);
    let aes_path = {
        let mut p = prefix.to_path_buf();
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        p.set_file_name(format!("{name}.aes.key"));
        p
    };
    std::fs::write(&aes_path, aes_key)
        .with_context(|| format!("無法寫入 AES key：{}", aes_path.display()))?;
    eprintln!("生成：{}", aes_path.display());

    // Ed25519 seed（私鑰）：32 隨機 bytes
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_bytes = signing_key.verifying_key().to_bytes();

    let priv_path = {
        let mut p = prefix.to_path_buf();
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        p.set_file_name(format!("{name}.ed25519.key"));
        p
    };
    std::fs::write(&priv_path, seed)
        .with_context(|| format!("無法寫入 Ed25519 私鑰：{}", priv_path.display()))?;
    eprintln!("生成：{}", priv_path.display());

    let pub_path = {
        let mut p = prefix.to_path_buf();
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        p.set_file_name(format!("{name}.ed25519.pub"));
        p
    };
    std::fs::write(&pub_path, verifying_bytes)
        .with_context(|| format!("無法寫入 Ed25519 公鑰：{}", pub_path.display()))?;
    eprintln!("生成：{}", pub_path.display());

    eprintln!("\n金鑰生成完成。請妥善保管私鑰，勿提交至 git。");
    Ok(())
}

/// Ed25519 驗簽。
pub fn verify_file(file_path: &Path, key_file: &Path, sig_file: &Path) -> anyhow::Result<()> {
    let pub_bytes = load_key_file(key_file)?;

    let content = std::fs::read(file_path)
        .with_context(|| format!("無法讀取檔案：{}", file_path.display()))?;

    let sig_bytes = std::fs::read(sig_file)
        .with_context(|| format!("無法讀取簽名檔：{}", sig_file.display()))?;

    if sig_bytes.len() != 64 {
        anyhow::bail!("簽名長度錯誤：預期 64 bytes，實際 {}", sig_bytes.len());
    }

    let signature: [u8; 64] = sig_bytes.as_slice().try_into().unwrap();

    verify_signature(&pub_bytes, &content, &signature).context("Ed25519 簽名驗證失敗")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        tempfile::TempDir::new().unwrap()
    }
    fn test_key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    // ── key-file 驗證 ──────────────────────────────────────────

    #[test]
    fn test_load_key_file_not_found() {
        let d = temp_dir();
        let r = load_key_file(&d.path().join("nonexistent.key"));
        assert!(r.is_err());
        let e = r.unwrap_err().to_string();
        assert!(
            e.contains("不存在") || e.contains("not found") || e.contains("No such"),
            "{e}"
        );
    }

    #[test]
    fn test_load_key_file_wrong_length() {
        let d = temp_dir();
        let p = d.path().join("short.key");
        std::fs::write(&p, [0u8; 16]).unwrap();
        let e = load_key_file(&p).unwrap_err().to_string();
        assert!(e.contains("32") || e.contains("長度"), "{e}");
    }

    #[test]
    fn test_load_key_file_valid() {
        let d = temp_dir();
        let p = d.path().join("valid.key");
        std::fs::write(&p, [0xABu8; 32]).unwrap();
        assert_eq!(load_key_file(&p).unwrap(), [0xABu8; 32]);
    }

    // ── 加解密 round-trip ──────────────────────────────────────

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let d = temp_dir();
        let (inp, enc, dec) = (
            d.path().join("t.bin"),
            d.path().join("t.enc"),
            d.path().join("t.dec"),
        );
        let original = b"Hello, WASM World! This is test data.";
        std::fs::write(&inp, original).unwrap();
        let k = test_key();
        encrypt_file(&inp, &enc, &k).unwrap();
        decrypt_file(&enc, &dec, &k).unwrap();
        assert_eq!(std::fs::read(&dec).unwrap(), original.as_slice());
    }

    #[test]
    fn test_encrypted_file_larger() {
        let d = temp_dir();
        let (inp, enc) = (d.path().join("t.bin"), d.path().join("t.enc"));
        std::fs::write(&inp, b"test data").unwrap();
        encrypt_file(&inp, &enc, &test_key()).unwrap();
        let orig = std::fs::metadata(&inp).unwrap().len();
        let encl = std::fs::metadata(&enc).unwrap().len();
        assert_eq!(encl, orig + 28, "nonce(12)+tag(16)=28 bytes 開銷");
    }

    #[test]
    fn test_wrong_key_decryption_fails() {
        let d = temp_dir();
        let (inp, enc) = (d.path().join("t.bin"), d.path().join("t.enc"));
        std::fs::write(&inp, b"secret data").unwrap();
        encrypt_file(&inp, &enc, &test_key()).unwrap();
        assert!(decrypt_file(&enc, &d.path().join("t.dec"), &[0xFFu8; 32]).is_err());
    }

    // ── decrypt_file 錯誤情境 ──────────────────────────────────

    #[test]
    fn test_decrypt_file_truncated() {
        let d = temp_dir();
        let p = d.path().join("trunc.enc");
        std::fs::write(&p, [0u8; 20]).unwrap(); // < nonce(12)+tag(16)=28
        assert!(decrypt_file(&p, &d.path().join("o.dec"), &test_key()).is_err());
    }

    #[test]
    fn test_decrypt_file_tampered_ciphertext() {
        let d = temp_dir();
        let (inp, enc) = (d.path().join("t.bin"), d.path().join("t.enc"));
        std::fs::write(&inp, b"original data for tamper test").unwrap();
        let k = test_key();
        encrypt_file(&inp, &enc, &k).unwrap();
        let mut b = std::fs::read(&enc).unwrap();
        b[12] ^= 0xFF; // 竄改 nonce 之後第一個 byte
        std::fs::write(&enc, &b).unwrap();
        assert!(decrypt_file(&enc, &d.path().join("o.dec"), &k).is_err());
    }

    // ── 大檔案 + nonce 獨立性 ──────────────────────────────────

    #[test]
    fn test_encrypt_decrypt_large_file() {
        let d = temp_dir();
        let (inp, enc, dec) = (
            d.path().join("l.bin"),
            d.path().join("l.enc"),
            d.path().join("l.dec"),
        );
        let orig: Vec<u8> = (0u8..=255).cycle().take(1024 * 1024).collect();
        std::fs::write(&inp, &orig).unwrap();
        let k = test_key();
        encrypt_file(&inp, &enc, &k).unwrap();
        decrypt_file(&enc, &dec, &k).unwrap();
        assert_eq!(std::fs::read(&dec).unwrap(), orig);
    }

    #[test]
    fn test_multiple_files_independent_nonces() {
        let d = temp_dir();
        let k = test_key();
        let c = b"same content";
        let (p1, p2) = (d.path().join("a.bin"), d.path().join("b.bin"));
        let (e1, e2) = (d.path().join("a.enc"), d.path().join("b.enc"));
        std::fs::write(&p1, c).unwrap();
        std::fs::write(&p2, c).unwrap();
        encrypt_file(&p1, &e1, &k).unwrap();
        encrypt_file(&p2, &e2, &k).unwrap();
        assert_ne!(
            std::fs::read(&e1).unwrap(),
            std::fs::read(&e2).unwrap(),
            "相同明文應因不同 nonce 產生不同密文"
        );
    }

    // ── encrypt-dir ────────────────────────────────────────────

    #[test]
    fn test_encrypt_dir_processes_glb_and_png() {
        let d = temp_dir();
        std::fs::write(d.path().join("model.glb"), b"glb").unwrap();
        std::fs::write(d.path().join("texture.png"), b"png").unwrap();
        std::fs::write(d.path().join("readme.txt"), b"txt").unwrap();
        assert_eq!(encrypt_dir(d.path(), &test_key()).unwrap(), 2);
        assert!(d.path().join("model.glb.enc").exists());
        assert!(d.path().join("texture.png.enc").exists());
        assert!(!d.path().join("readme.txt.enc").exists());
    }

    #[test]
    fn test_encrypt_dir_empty_directory() {
        assert_eq!(encrypt_dir(temp_dir().path(), &test_key()).unwrap(), 0);
    }

    #[test]
    fn test_encrypt_dir_nonexistent() {
        let d = temp_dir();
        assert!(encrypt_dir(&d.path().join("no_such"), &test_key()).is_err());
    }

    // ── Ed25519 簽名/驗簽 ──────────────────────────────────────

    fn test_ed25519_keypair() -> ([u8; 32], [u8; 32]) {
        let mut seed = [0u8; 32];
        for (i, b) in seed.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(0x42);
        }
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&seed);
        (seed, sk.verifying_key().to_bytes())
    }

    #[test]
    fn test_sign_verify_roundtrip() {
        let d = temp_dir();
        let (seed, pk) = test_ed25519_keypair();
        let f = d.path().join("data.wasm");
        std::fs::write(&f, b"wasm binary content").unwrap();
        let kp = d.path().join("ed25519.priv");
        std::fs::write(&kp, seed).unwrap();
        sign_file(&f, &kp).unwrap();
        let sig = d.path().join("data.wasm.sig");
        assert!(sig.exists());
        assert_eq!(std::fs::read(&sig).unwrap().len(), 64);
        let pp = d.path().join("ed25519.pub");
        std::fs::write(&pp, pk).unwrap();
        assert!(verify_file(&f, &pp, &sig).is_ok());
    }

    #[test]
    fn test_verify_tampered_file_fails() {
        let d = temp_dir();
        let (seed, pk) = test_ed25519_keypair();
        let f = d.path().join("data.wasm");
        std::fs::write(&f, b"original").unwrap();
        let kp = d.path().join("ed25519.priv");
        std::fs::write(&kp, seed).unwrap();
        sign_file(&f, &kp).unwrap();
        let sig = d.path().join("data.wasm.sig");
        std::fs::write(&f, b"tampered").unwrap();
        let pp = d.path().join("ed25519.pub");
        std::fs::write(&pp, pk).unwrap();
        assert!(verify_file(&f, &pp, &sig).is_err());
    }

    #[test]
    fn test_verify_wrong_public_key_fails() {
        let d = temp_dir();
        let (seed, _) = test_ed25519_keypair();
        let f = d.path().join("data.wasm");
        std::fs::write(&f, b"content").unwrap();
        let kp = d.path().join("ed25519.priv");
        std::fs::write(&kp, seed).unwrap();
        sign_file(&f, &kp).unwrap();
        let sig = d.path().join("data.wasm.sig");
        let wp = d.path().join("wrong.pub");
        std::fs::write(&wp, [0xFFu8; 32]).unwrap();
        assert!(verify_file(&f, &wp, &sig).is_err());
    }

    #[test]
    fn test_verify_invalid_sig_length() {
        let d = temp_dir();
        let (_, pk) = test_ed25519_keypair();
        let f = d.path().join("data.wasm");
        std::fs::write(&f, b"content").unwrap();
        let pp = d.path().join("ed25519.pub");
        std::fs::write(&pp, pk).unwrap();
        let sig = d.path().join("data.wasm.sig");
        std::fs::write(&sig, [0u8; 32]).unwrap(); // 不足 64
        assert!(verify_file(&f, &pp, &sig).is_err());
    }
}
