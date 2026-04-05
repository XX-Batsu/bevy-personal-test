set shell := ["zsh", "-cu"]

port := "7777"

# 列出所有指令
default:
    @just --list

# 完整重建並重啟 server
dev: build bindgen build-shadow bindgen-shadow build-server serve

# 純重啟 server（不重新編譯）
serve: stop
    #!/usr/bin/env zsh
    PORT={{port}} ./target/debug/gateway-server &
    echo $! > .server.pid
    echo "Server 啟動於 http://localhost:{{port}}"

# 停止 server
stop:
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
