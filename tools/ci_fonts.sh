#!/usr/bin/env bash
# CI 字型準備：下載 Noto Sans CJK TC 並依 build/font-chars.txt 子集化。
#
# bevy_runtime/build.rs 要求此檔存在才能編譯（WASM 透過 include_bytes! 內嵌，
# native 由 AssetServer 於執行期載入）。
#
# 不使用 justfile 的 download-fonts recipe：justfile 的 shell 設為 zsh
# （GitHub ubuntu/windows runner 未安裝，會以 exit 127 失敗），且其路徑探測
# 針對 macOS 的 ~/Library/Fonts 與 Homebrew。
#
# 用法：bash tools/ci_fonts.sh
set -euo pipefail

DST="client/js/assets/fonts/NotoSansTC-Regular.ttf"
SRC_URL="https://github.com/notofonts/noto-cjk/raw/main/Sans/OTF/TraditionalChinese/NotoSansCJKtc-Regular.otf"
CHARS="build/font-chars.txt"

mkdir -p "$(dirname "$DST")"
curl -fL -o "$DST" "$SRC_URL"

python3 -m pip install --user --quiet fonttools \
  || python3 -m pip install --user --quiet --break-system-packages fonttools

# 不指定 --flavor：來源為 CFF/OTF 輪廓，fontTools 的 flavor 僅接受 woff/woff2，
# 傳入 truetype 會以 "Unknown flavor" AssertionError 中止。
python3 -m fontTools.subset "$DST" \
  --text-file="$CHARS" \
  --output-file="$DST" \
  --no-hinting

ls -l "$DST"
