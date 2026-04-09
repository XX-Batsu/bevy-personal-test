//! 素材匯入鏈整合測試：manifest 產生 → parse → IdResolver → verify。

use asset_manifest::{AssetId, AssetManifest};
use bevy_runtime::asset_source::id_resolver::IdResolver;
use std::fs;
use std::process::Command;

/// 建立測試用 assets 目錄。
fn setup_test_assets() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path();

    fs::create_dir_all(assets.join("sprites/player")).unwrap();
    fs::write(assets.join("sprites/player/idle.png"), b"fake png data").unwrap();
    fs::write(assets.join("sprites/player/idle.ron"), b"()").unwrap();

    fs::create_dir_all(assets.join("audio/bgm")).unwrap();
    fs::write(assets.join("audio/bgm/theme.ogg"), b"fake ogg data").unwrap();

    dir
}

#[test]
fn test_manifest_gen_produces_valid_manifest() {
    let dir = setup_test_assets();
    let assets_path = dir.path();
    let manifest_path = assets_path.join("manifest.ron");

    let status = Command::new("cargo")
        .args([
            "run",
            "--package",
            "manifest-gen",
            "--",
            "--assets-dir",
            assets_path.to_str().unwrap(),
            "--output",
            manifest_path.to_str().unwrap(),
        ])
        .status()
        .expect("執行 manifest-gen 失敗");
    assert!(status.success());

    let ron_str = fs::read_to_string(&manifest_path).unwrap();
    let manifest = AssetManifest::parse(&ron_str).unwrap();

    assert_eq!(manifest.assets.len(), 2);

    let player = manifest.get(&AssetId::new("sprites/player/idle")).unwrap();
    assert_eq!(player.path, "sprites/player/idle.png");
    assert_eq!(player.sidecars, vec!["sprites/player/idle.ron"]);
    assert!(!player.blake3.is_empty());

    let bgm = manifest.get(&AssetId::new("audio/bgm/theme")).unwrap();
    assert_eq!(bgm.path, "audio/bgm/theme.ogg");
}

#[test]
fn test_id_resolver_with_generated_manifest() {
    let dir = setup_test_assets();
    let assets_path = dir.path();
    let manifest_path = assets_path.join("manifest.ron");

    Command::new("cargo")
        .args([
            "run",
            "--package",
            "manifest-gen",
            "--",
            "--assets-dir",
            assets_path.to_str().unwrap(),
            "--output",
            manifest_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    let ron_str = fs::read_to_string(&manifest_path).unwrap();
    let manifest = AssetManifest::parse(&ron_str).unwrap();

    let resolver = IdResolver::new(&manifest);
    let resolved = resolver.resolve("sprites/player/idle").unwrap();
    assert_eq!(resolved.entry.path, "sprites/player/idle.png");

    assert!(resolver.resolve("nonexistent").is_none());
}

#[test]
fn test_manifest_verify_mode() {
    let dir = setup_test_assets();
    let assets_path = dir.path();
    let manifest_path = assets_path.join("manifest.ron");

    Command::new("cargo")
        .args([
            "run",
            "--package",
            "manifest-gen",
            "--",
            "--assets-dir",
            assets_path.to_str().unwrap(),
            "--output",
            manifest_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    // Verify should pass
    let status = Command::new("cargo")
        .args([
            "run",
            "--package",
            "manifest-gen",
            "--",
            "--assets-dir",
            assets_path.to_str().unwrap(),
            "--verify",
            manifest_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    // Modify file → verify should fail
    fs::write(
        assets_path.join("sprites/player/idle.png"),
        b"modified data",
    )
    .unwrap();
    let status = Command::new("cargo")
        .args([
            "run",
            "--package",
            "manifest-gen",
            "--",
            "--assets-dir",
            assets_path.to_str().unwrap(),
            "--verify",
            manifest_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(!status.success());
}
