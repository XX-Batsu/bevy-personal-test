set shell := ["zsh", "-cu"]

port := "7777"

# 列出所有指令
default:
    @just --list

# 完整重建並重啟 server（含 dev-asset-server）
dev: build bindgen build-shadow bindgen-shadow build-server serve start-dev-assets

# 純重啟 server（不重新編譯）
serve: stop
    #!/usr/bin/env zsh
    PORT={{port}} ./target/debug/gateway-server &
    echo $! > .server.pid
    echo "Server 啟動於 http://localhost:{{port}}"

# 停止 server（含 dev-asset-server）
stop: stop-dev-assets
    #!/usr/bin/env zsh
    if [ -f .server.pid ]; then
        kill $(cat .server.pid) 2>/dev/null || true
        rm .server.pid
        echo "Server 已停止"
    fi

# cargo build（wasm_loader, debug-mode + single-player）
# download-fonts 確保字型存在（include_bytes! 編譯時需要），已存在則直接跳過
build: download-fonts
    cargo build --package wasm_loader --target wasm32-unknown-unknown \
      --features "debug-mode,single-player"

# 產生 JS bindings
bindgen:
    wasm-bindgen --out-dir client/js --target web \
      target/wasm32-unknown-unknown/debug/wasm_loader.wasm

# 編譯 Shadow VM Web Worker（wasm_shadow_worker cdylib，debug-mode）
build-shadow:
    cargo build --package wasm_shadow_worker --target wasm32-unknown-unknown \
      --features "debug-mode"

# 產生 Shadow Worker JS bindings
bindgen-shadow:
    wasm-bindgen --out-dir client/js --target web \
      target/wasm32-unknown-unknown/debug/wasm_shadow_worker.wasm

# 編譯 gateway server
build-server:
    cargo build --package gateway --bin gateway-server

# 設置 CJK 字型（開發用，僅需執行一次）
# 優先順序：1) 已存在  2) macOS 系統字型  3) Homebrew 安裝  4) 手動說明
download-fonts:
    #!/usr/bin/env zsh
    mkdir -p client/js/assets/fonts
    dst=client/js/assets/fonts/NotoSansTC-Regular.ttf

    # 1. 已存在
    if [[ -f "$dst" ]]; then
      echo "字型已存在：$dst"
      exit 0
    fi

    # 2. 找 macOS 系統字型（Homebrew 安裝後位於 ~/Library/Fonts）
    sys_font=$(find ~/Library/Fonts /Library/Fonts \
      \( -name "NotoSansCJKtc-Regular.otf" -o -name "NotoSansTC-Regular.ttf" \) \
      2>/dev/null | head -1)
    if [[ -n "$sys_font" ]]; then
      echo "找到系統字型：$sys_font"
      cp "$sys_font" "$dst"
      echo "字型已就緒：$dst"
      exit 0
    fi

    # 3. 嘗試 Homebrew 安裝
    if command -v brew &>/dev/null; then
      echo "透過 Homebrew 安裝 font-noto-sans-cjk-tc..."
      brew install --cask font-noto-sans-cjk-tc
      sys_font=$(find ~/Library/Fonts /Library/Fonts \
        -name "NotoSansCJKtc-Regular.otf" 2>/dev/null | head -1)
      if [[ -n "$sys_font" ]]; then
        cp "$sys_font" "$dst"
        echo "字型已就緒：$dst"
        exit 0
      fi
    fi

    # 3.5. curl fallback（Linux / CI / 其他環境）— 從 stable URL 下載
    if command -v curl &>/dev/null; then
      url="https://github.com/notofonts/noto-cjk/raw/main/Sans/OTF/TraditionalChinese/NotoSansCJKtc-Regular.otf"
      echo "從網路下載字型：$url"
      if curl -fL -o "$dst" "$url" && [[ -s "$dst" ]]; then
        echo "字型已就緒：$dst"
        exit 0
      fi
      echo "curl 下載失敗，繼續嘗試手動指引"
      rm -f "$dst"  # 清除可能的不完整檔
    fi

    # 4. 手動說明
    echo ""
    echo "請手動安裝字型："
    echo "  brew install --cask font-noto-sans-cjk-tc"
    echo "  just download-fonts"
    echo ""
    echo "或手動下載："
    echo "  https://fonts.google.com/noto/specimen/Noto+Sans+TC"
    echo "  → 解壓後將 NotoSansTC-Regular.ttf 放至 client/js/assets/fonts/"
    exit 1

# Native 開發模式（桌面視窗 + 素材熱載入 + 自動 manifest）
# 關閉視窗或 Ctrl+C 後自動清理 dev-asset-server
dev-native: manifest start-dev-assets
    #!/usr/bin/env zsh
    cleanup() { just stop-dev-assets; }
    trap cleanup INT TERM EXIT
    cargo run --package bevy_runtime --example native_dev --features "hot-reload-native,debug-mode" || true

# ── 素材匯入鏈 ──

# 產生素材 manifest（手動觸發）
manifest:
    cargo run --package manifest-gen -- --assets-dir assets/ --output assets/manifest.ron

# 驗證 manifest 完整性（CI 用）
manifest-check:
    cargo run --package manifest-gen -- --assets-dir assets/ --verify assets/manifest.ron

# 開發模式：前景啟動 dev asset server（HTTP + WS + 自動 manifest-gen）
dev-assets port="8081":
    cd tools/dev-asset-server && cargo run -- --assets-dir {{justfile_directory()}}/assets/ --port {{port}}

# 背景啟動 dev-asset-server（just dev 呼叫）
start-dev-assets port="8081":
    #!/usr/bin/env zsh
    cd tools/dev-asset-server && cargo run -- --assets-dir {{justfile_directory()}}/assets/ --port {{port}} &
    echo $! > {{justfile_directory()}}/.dev-assets.pid
    echo "Dev Asset Server 啟動於 http://localhost:{{port}}"

# 停止 dev-asset-server
stop-dev-assets:
    #!/usr/bin/env zsh
    if [ -f .dev-assets.pid ]; then
        kill $(cat .dev-assets.pid) 2>/dev/null || true
        rm .dev-assets.pid
        echo "Dev Asset Server 已停止"
    fi
