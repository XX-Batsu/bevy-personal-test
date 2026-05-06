//! Build script — 編譯時檢查 `include_bytes!` 字型資產存在。
//!
//! `welcome.rs` 透過 `include_bytes!("../../../client/js/assets/fonts/NotoSansTC-Regular.ttf")`
//! 嵌入字型;該路徑於 `.gitignore` 中排除(README.md「External binary assets」規則),
//! 由 `justfile` 的 `download-fonts` recipe 提供。
//!
//! 直接執行 `cargo` 命令(如品質關卡 `cargo check`)會繞過 `just build`,因此本 script
//! 在編譯前檢查字型存在,缺則 panic 並引導使用者跑 `just download-fonts`。

use std::path::PathBuf;

fn main() {
    // 路徑假設(脆弱):本 build.rs 假設 crate 位於 `<workspace_root>/engine/bevy_runtime/`,
    // 兩次 `.parent()` 才到 workspace root。若未來 crate 路徑深度改變(例如搬到
    // `crates/bevy_runtime/`),下方 path resolution 必須同步更新。
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent() // engine/
        .and_then(|p| p.parent()) // workspace root
        .expect("無法定位 workspace root(CARGO_MANIFEST_DIR 結構異常)");
    let font_path = workspace_root.join("client/js/assets/fonts/NotoSansTC-Regular.ttf");

    println!("cargo:rerun-if-changed={}", font_path.display());

    if !font_path.exists() {
        println!("cargo:warning=字型檔缺失:{}", font_path.display());
        println!("cargo:warning=請執行 `just download-fonts` 下載字型");
        println!("cargo:warning=或手動下載 NotoSansTC-Regular.ttf 至上述路徑");
        panic!("bevy_runtime 編譯需要 NotoSansTC-Regular.ttf — 請跑 `just download-fonts`");
    }
}
