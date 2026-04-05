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
build:
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
