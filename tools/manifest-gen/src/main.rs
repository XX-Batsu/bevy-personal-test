//! manifest-gen — 掃描 assets/ 目錄，產生 manifest.ron。
//!
//! 用法：
//!   manifest-gen --assets-dir assets/ --output assets/manifest.ron
//!   manifest-gen --assets-dir assets/ --verify assets/manifest.ron

use asset_manifest::{AssetEntry, AssetFormat, AssetId, AssetManifest};
use clap::Parser;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "manifest-gen", about = "素材 manifest 產生工具")]
struct Cli {
    #[arg(long)]
    assets_dir: PathBuf,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    verify: Option<PathBuf>,
}

struct ScannedAsset {
    primary_path: String,
    format: AssetFormat,
    size_bytes: u64,
    blake3_hex: String,
    sidecars: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    if let Some(verify_path) = &cli.verify {
        return verify_manifest(&cli.assets_dir, verify_path);
    }

    let output = cli
        .output
        .unwrap_or_else(|| cli.assets_dir.join("manifest.ron"));

    let assets = scan_directory(&cli.assets_dir)?;
    let manifest = build_manifest(assets);
    let ron_str = ron::ser::to_string_pretty(
        &manifest,
        ron::ser::PrettyConfig::default().struct_names(true),
    )?;
    std::fs::write(&output, &ron_str)?;
    tracing::info!("Manifest 已產生：{}", output.display());
    tracing::info!("共 {} 筆素材", manifest.assets.len());

    Ok(())
}

fn scan_directory(
    assets_dir: &Path,
) -> Result<BTreeMap<String, ScannedAsset>, Box<dyn std::error::Error>> {
    let mut groups: BTreeMap<String, Vec<(String, AssetFormat, u64, String)>> = BTreeMap::new();

    for entry in WalkDir::new(assets_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == "manifest.ron") {
            continue;
        }

        // 一律以 '/' 分隔：asset id 與 path 會進入 manifest 並跨平台比對，
        // 若沿用 OS 分隔符，Windows 產生的 manifest 會與其他平台不一致。
        let rel_path = path
            .strip_prefix(assets_dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");

        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let format = AssetFormat::from_extension(&ext);
        let size = entry.metadata()?.len();

        let mut hasher = blake3::Hasher::new();
        let mut file = std::fs::File::open(path)?;
        std::io::copy(&mut file, &mut hasher)?;
        let hash = hasher.finalize();
        let hash_hex = hash.to_hex().to_string();

        let stem = rel_path
            .rsplit_once('.')
            .map(|(s, _)| s.to_string())
            .unwrap_or(rel_path.clone());

        groups
            .entry(stem)
            .or_default()
            .push((rel_path, format, size, hash_hex));
    }

    let mut result = BTreeMap::new();
    for (asset_id, mut files) in groups {
        files.sort_by_key(|(_, fmt, _, _)| !fmt.is_primary());
        let (primary_path, format, size_bytes, blake3_hex) = files.remove(0);
        let sidecars: Vec<String> = files.into_iter().map(|(p, _, _, _)| p).collect();
        result.insert(
            asset_id,
            ScannedAsset {
                primary_path,
                format,
                size_bytes,
                blake3_hex,
                sidecars,
            },
        );
    }

    Ok(result)
}

/// **注意：** blake3_bytes 為全零。若要 diff，需先序列化再 parse()。
fn build_manifest(assets: BTreeMap<String, ScannedAsset>) -> AssetManifest {
    let mut entries = BTreeMap::new();
    for (id_str, scanned) in assets {
        entries.insert(
            AssetId::new(id_str),
            AssetEntry {
                path: scanned.primary_path,
                format: scanned.format,
                size_bytes: scanned.size_bytes,
                blake3: scanned.blake3_hex,
                blake3_bytes: [0u8; 32],
                encrypted: false,
                sidecars: scanned.sidecars,
                tags: BTreeSet::new(),
            },
        );
    }

    AssetManifest {
        version: "1.0.0".to_string(),
        generated_at: utc_now_iso8601(),
        assets: entries,
    }
}

fn utc_now_iso8601() -> String {
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = dur.as_secs();

    // 手動計算 UTC 日期時間
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // 從 Unix epoch (1970-01-01) 計算年月日
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // 簡化版 civil_from_days（基於 Howard Hinnant 的算法）
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn verify_manifest(
    assets_dir: &Path,
    manifest_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let ron_str = std::fs::read_to_string(manifest_path)?;
    let manifest = AssetManifest::parse(&ron_str)?;

    let scanned = scan_directory(assets_dir)?;
    let mut errors = Vec::new();

    for (id, entry) in &manifest.assets {
        let full_path = assets_dir.join(&entry.path);
        if !full_path.exists() {
            errors.push(format!("Manifest 有 {id} 但檔案不存在：{}", entry.path));
        }
    }

    for id_str in scanned.keys() {
        if manifest.get(&AssetId::new(id_str.as_str())).is_none() {
            errors.push(format!("檔案 {id_str} 不在 manifest 中"));
        }
    }

    for (id_str, scanned_asset) in &scanned {
        if let Some(entry) = manifest.get(&AssetId::new(id_str.as_str())) {
            if entry.blake3 != scanned_asset.blake3_hex {
                errors.push(format!(
                    "{id_str} blake3 不一致：manifest={}, 實際={}",
                    &entry.blake3[..8],
                    &scanned_asset.blake3_hex[..8]
                ));
            }
        }
    }

    if errors.is_empty() {
        tracing::info!("Manifest 驗證通過 ✓");
        Ok(())
    } else {
        for e in &errors {
            tracing::error!("{e}");
        }
        Err(format!("{} 項驗證失敗", errors.len()).into())
    }
}
