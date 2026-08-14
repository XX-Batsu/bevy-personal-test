#!/usr/bin/env bash
# 對 wasm-bindgen 產出的 _bg.wasm 就地執行 wasm-opt -Oz。
#
# 順序很重要：必須「先 wasm-bindgen、後 wasm-opt」。反過來做的話，wasm-opt 會
# 動到 wasm-bindgen 依賴的 function table，導致
#   failed to find `0` in function table
#
# feature 旗標必須顯式列出：binaryen 預設未啟用 bulk-memory-opt 等，會以
# "memory.copy operations require bulk memory operations" 驗證失敗；但也不可用
# `-all`，那會連 compact imports 等實驗性 proposal 一併開啟，產出的模組
# wasm-bindgen 解析不了（invalid leading byte 0x7F）。
#
# 用法：bash tools/ci_wasm_opt.sh <path-to-_bg.wasm>
set -euo pipefail

TARGET="${1:?用法：bash tools/ci_wasm_opt.sh <path-to-_bg.wasm>}"

if [ ! -f "$TARGET" ]; then
  echo "錯誤：找不到 $TARGET"
  exit 1
fi

BEFORE=$(wc -c < "$TARGET")

wasm-opt -Oz \
  --enable-bulk-memory \
  --enable-bulk-memory-opt \
  --enable-sign-ext \
  --enable-nontrapping-float-to-int \
  --enable-mutable-globals \
  --enable-multivalue \
  --enable-reference-types \
  -o "$TARGET" "$TARGET"

AFTER=$(wc -c < "$TARGET")
echo "wasm-opt：${BEFORE} bytes → ${AFTER} bytes"
