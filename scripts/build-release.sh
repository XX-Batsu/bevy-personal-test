#!/usr/bin/env bash
# scripts/build-release.sh
#
# Release Build Pipeline
# ======================
# 必要工具安裝：
#   cargo install wasm-bindgen-cli
#   brew install binaryen      (提供 wasm-opt)
#   cargo install wasm-tools   (提供 wasm-mutate)
#   cargo build -p cfg_mutator -p asset_encryptor  (本專案工具)
#
# 使用方式：
#   ./scripts/build-release.sh
#   ED25519_KEY_FILE=keys/ed25519.key ./scripts/build-release.sh
#
# 環境變數：
#   BEVY_GAME_BUILD_SEED         覆蓋 CFG mutation seed（CI 使用，十進位 u64）
#   BEVY_GAME_BUILD_TIMESTAMP    覆蓋 seed 計算用的 Unix timestamp（CI 使用，十進位 u64）
#   AES_KEY_FILE                 AES-256 key 路徑（預設 keys/aes.key）
#   ED25519_KEY_FILE             Ed25519 private key 路徑（預設 keys/ed25519.key）
#   ED25519_PUB_KEY_FILE         Ed25519 public key 路徑（預設 keys/ed25519.pub，用於簽名自驗）

set -e  # 任何指令失敗即終止（Stage 3 除外，見 fallback 邏輯）

# ── 設定 ─────────────────────────────────────────────────────────────

CRATE_NAME="wasm_loader"  # wasm-bindgen 目標 crate
BUILD_MODE="${BUILD_MODE:-release}"  # debug 模式產生 source map
AES_KEY_FILE="${AES_KEY_FILE:-keys/aes.key}"
ED25519_KEY_FILE="${ED25519_KEY_FILE:-keys/ed25519.key}"
ED25519_PUB_KEY_FILE="${ED25519_PUB_KEY_FILE:-keys/ed25519.pub}"
BUILD_DIR="target/wasm32-unknown-unknown/release"
PKG_DIR="pkg"
DIST_DIR="dist"

# ── 前置驗證 ─────────────────────────────────────────────────────────

echo "=== 驗證工具可用性 ==="

# wasm-opt 版本檢查（上游 15-toolchain/core-tools.md 要求 ≥ 116）
WASM_OPT_VERSION=$(wasm-opt --version 2>/dev/null | grep -oE '[0-9]+' | head -1 || echo "0")
if [ "$WASM_OPT_VERSION" = "0" ]; then
  echo "錯誤：wasm-opt 未安裝（brew install binaryen）"; exit 1
fi
if [ "$WASM_OPT_VERSION" -lt 116 ]; then
  echo "錯誤：wasm-opt 版本 ${WASM_OPT_VERSION} 低於最低要求 116"; exit 1
fi
echo "wasm-opt 版本：${WASM_OPT_VERSION}（≥ 116）"

wasm-bindgen --version || { echo "錯誤：wasm-bindgen-cli 未安裝"; exit 1; }

if [ ! -f "$AES_KEY_FILE" ]; then
  echo "錯誤：AES key 檔案不存在：$AES_KEY_FILE"
  exit 1
fi

if [ ! -f "$ED25519_KEY_FILE" ]; then
  echo "錯誤：Ed25519 key 檔案不存在：$ED25519_KEY_FILE"
  exit 1
fi

# ── Stage 1: 編譯 WASM ───────────────────────────────────────────────

echo "=== Stage 1: 編譯 WASM ==="
cargo build --target wasm32-unknown-unknown --release
WASM_INPUT="${BUILD_DIR}/${CRATE_NAME}.wasm"

if [ ! -f "$WASM_INPUT" ]; then
  echo "錯誤：WASM 輸出不存在：$WASM_INPUT"
  exit 1
fi

# ── Stage 2: wasm-opt 最佳化 ─────────────────────────────────────────

echo "=== Stage 2: wasm-opt -Oz 最佳化 ==="
mkdir -p "${PKG_DIR}"
wasm-opt -Oz -o "${PKG_DIR}/optimized.wasm" "${WASM_INPUT}"

# Stage 2 中間產物驗證
[ -s "${PKG_DIR}/optimized.wasm" ] || { echo "錯誤：optimized.wasm 未生成或為空"; exit 1; }
echo "完成：最佳化輸出 ${PKG_DIR}/optimized.wasm"

# ── Stage 3: CFG Mutation（wasm-mutate）─────────────────────────────
# wasm-opt 必須在 wasm-mutate 之前，此順序由腳本保證。

echo "=== Stage 3: CFG Mutation ==="
TIMESTAMP="${BEVY_GAME_BUILD_TIMESTAMP:-$(date +%s)}"
COMMIT=$(git rev-parse HEAD 2>/dev/null || echo "unknown")

# cfg_mutator CLI mutate 子命令
# BEVY_GAME_BUILD_SEED 由 cfg_mutator 內部 resolve_seed() 讀取
if cargo run -p cfg_mutator -- mutate \
    "${PKG_DIR}/optimized.wasm" \
    "${PKG_DIR}/mutated.wasm" \
    --timestamp "${TIMESTAMP}" \
    --commit "${COMMIT}"; then
  echo "完成：CFG mutation 輸出 ${PKG_DIR}/mutated.wasm"
else
  # Stage 3 為 set -e 例外：wasm-mutate 失敗不終止 pipeline
  echo "警告：wasm-mutate 失敗，使用未 mutate 的 WASM"
  cp "${PKG_DIR}/optimized.wasm" "${PKG_DIR}/mutated.wasm"
fi

# Stage 3.5: wasm-validate（mutation 後驗證）
if command -v wasm-validate &>/dev/null; then
  wasm-validate "${PKG_DIR}/mutated.wasm" || {
    echo "錯誤：mutated.wasm 驗證失敗，fallback 至 optimized.wasm"
    cp "${PKG_DIR}/optimized.wasm" "${PKG_DIR}/mutated.wasm"
  }
  echo "完成：wasm-validate 驗證通過"
else
  echo "警告：wasm-validate 未安裝，跳過 mutation 後驗證"
fi

# Stage 3 中間產物驗證
[ -s "${PKG_DIR}/mutated.wasm" ] || { echo "錯誤：mutated.wasm 未生成或為空"; exit 1; }

# ── Stage 4: wasm-bindgen ─────────────────────────────────────────────

echo "=== Stage 4: wasm-bindgen --target web ==="
wasm-bindgen --target web --out-dir "${PKG_DIR}" "${PKG_DIR}/mutated.wasm"

# Stage 4 中間產物驗證
[ -s "${PKG_DIR}/${CRATE_NAME}.js" ] || { echo "錯誤：${CRATE_NAME}.js 未生成或為空"; exit 1; }
[ -s "${PKG_DIR}/${CRATE_NAME}_bg.wasm" ] || { echo "錯誤：${CRATE_NAME}_bg.wasm 未生成或為空"; exit 1; }
echo "完成：JS glue 生成至 ${PKG_DIR}/"

# ── Stage 5a: WASM binary 加密 ────────────────────────────────────────

echo "=== Stage 5a: WASM binary AES-256-GCM 加密 ==="
cargo run -p asset_encryptor -- \
  encrypt "${PKG_DIR}/${CRATE_NAME}_bg.wasm" \
          "${PKG_DIR}/${CRATE_NAME}_bg.wasm.enc" \
  --key-file "${AES_KEY_FILE}"

# ── Stage 5b: WASM binary Ed25519 簽名 ───────────────────────────────

echo "=== Stage 5b: WASM binary Ed25519 簽名 ==="
cargo run -p asset_encryptor -- \
  sign "${PKG_DIR}/${CRATE_NAME}_bg.wasm.enc" \
  --key-file "${ED25519_KEY_FILE}"

# 簽名後自驗（確保 key pair 正確）
if [ -f "$ED25519_PUB_KEY_FILE" ]; then
  echo "=== Stage 5b 驗證：簽名自檢 ==="
  cargo run -p asset_encryptor -- \
    verify "${PKG_DIR}/${CRATE_NAME}_bg.wasm.enc" \
    --key-file "${ED25519_PUB_KEY_FILE}" \
    --sig-file "${PKG_DIR}/${CRATE_NAME}_bg.wasm.enc.sig"
else
  echo "警告：公鑰 ${ED25519_PUB_KEY_FILE} 不存在，跳過簽名自驗"
fi

# ── Stage 5c: Rhai Script 編譯 ──────────────────────────────────────────

echo "=== Stage 5c: Rhai script → .rhai.bc 編譯 ==="
SCRIPTS_SRC="scripts/rhai"
SCRIPTS_OUT="${DIST_DIR}/scripts"

if [ -d "${SCRIPTS_SRC}" ]; then
  mkdir -p "${SCRIPTS_OUT}"
  SCRIPT_ID=0
  for rhai_file in "${SCRIPTS_SRC}"/*.rhai; do
    [ -f "$rhai_file" ] || continue
    base_name=$(basename "$rhai_file" .rhai)
    output_file="${SCRIPTS_OUT}/${base_name}.rhai.bc"
    echo "  編譯 ${rhai_file} → ${output_file}"
    cargo run -p bytecode_compiler -- compile \
      "$rhai_file" \
      "$output_file" \
      --signing-key "${ED25519_KEY_FILE}" \
      --encryption-key "${AES_KEY_FILE}" \
      --script-id "${SCRIPT_ID}" \
      --priority 0
    SCRIPT_ID=$((SCRIPT_ID + 1))
  done
  echo "完成：${SCRIPT_ID} 個 script 編譯至 ${SCRIPTS_OUT}/"
else
  echo "跳過：${SCRIPTS_SRC}/ 目錄不存在"
fi

# ── Stage 5d: Assets 批次加密 ─────────────────────────────────────────

echo "=== Stage 5d: 資產批次加密 ==="
if [ -d "assets" ]; then
  cargo run -p asset_encryptor -- \
    encrypt-dir "assets/" \
    --key-file "${AES_KEY_FILE}"
else
  echo "警告：assets/ 目錄不存在，跳過資產加密"
fi

# ── Stage 6: 組裝 dist/ ──────────────────────────────────────────────

# ── Source Map 產生（僅 debug build）─────────────────────────────────

if [ "${BUILD_MODE}" = "debug" ]; then
  echo "=== Source Map 產生（debug build）==="

  WASM_FILE=$(ls "${PKG_DIR}"/*.wasm 2>/dev/null | head -1)
  if [ -z "$WASM_FILE" ]; then
    echo "警告：${PKG_DIR}/ 目錄中未找到 .wasm 檔案，跳過 source map"
  else
    # 方案 A：wasm-bindgen --keep-debug
    if wasm-bindgen --out-dir "${PKG_DIR}" --target web --keep-debug \
        "target/wasm32-unknown-unknown/debug/${CRATE_NAME}.wasm" 2>/dev/null; then
      echo "Source map 透過 wasm-bindgen --keep-debug 產生"
    # 方案 B：wasm2map
    elif command -v wasm2map &>/dev/null; then
      echo "改用 wasm2map..."
      for wasm in "${PKG_DIR}"/*.wasm; do
        if wasm2map "$wasm" -o "${wasm}.map" 2>/dev/null; then
          echo "  產生：${wasm}.map"
        else
          echo "  警告：wasm2map 處理 ${wasm} 失敗"
        fi
      done
    else
      echo "警告：wasm-bindgen --keep-debug 不支援且 wasm2map 未安裝"
      echo "  安裝 wasm2map: cargo install wasm2map"
    fi
  fi
fi

# ── Stage 6: 組裝 dist/ ──────────────────────────────────────────────

echo "=== Stage 6: 組裝 dist/ ==="
mkdir -p "${DIST_DIR}/assets" "${DIST_DIR}/js" "${DIST_DIR}/scripts"

# WASM + 簽名
cp "${PKG_DIR}/${CRATE_NAME}_bg.wasm.enc"     "${DIST_DIR}/"
cp "${PKG_DIR}/${CRATE_NAME}_bg.wasm.enc.sig" "${DIST_DIR}/"

# wasm-bindgen JS glue
cp "${PKG_DIR}/${CRATE_NAME}.js" "${DIST_DIR}/"

# JS bootloader（4 檔）
cp client/js/loader.js          "${DIST_DIR}/js/"
cp client/js/wasm_loader.js     "${DIST_DIR}/js/"
cp client/js/transport.js       "${DIST_DIR}/js/"
cp client/js/bootstrap.js       "${DIST_DIR}/js/"
cp client/js/shadow_worker.js   "${DIST_DIR}/js/" 2>/dev/null || true  # 選用

# HTML + CSS
cp client/js/index.html         "${DIST_DIR}/"
cp client/js/style.css          "${DIST_DIR}/"

# 加密後的 assets
if [ -d "assets" ]; then
  find assets/ -name "*.enc" -exec cp {} "${DIST_DIR}/assets/" \;
fi

echo ""
echo "=== Build 完成 ==="
echo "輸出目錄：${DIST_DIR}/"
ls -la "${DIST_DIR}/"
