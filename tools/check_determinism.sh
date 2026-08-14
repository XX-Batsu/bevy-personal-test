#!/usr/bin/env bash
# 確定性模組靜態掃描：偵測 native float（f32/f64）與非確定性集合（HashMap/HashSet）
# 整合至 CI Stage 1 quick-checks job，執行時間 < 5 秒，不需 Rust 編譯。
#
# 用法：bash tools/check_determinism.sh
# Exit code：0 = 通過，1 = 有違規
#
# 掃描範圍：DETERMINISTIC_CRATES 陣列中的所有 crate
# 排除範圍：#[cfg(test)] 和 mod tests {} 區塊、單行註解
# 白名單：SoftF32、softfloat、allow-native-float、from_f32/from_f64/to_f64/to_native
#          Rhai bridge 層（bridge_api.rs、dynamic_convert.rs、dynamic_value.rs）
#          BTreeMap/BTreeSet/IndexMap、allow-hash-collection
#
# 行號使用 FNR（單檔行號）；awk 以 `-exec ... {} +` 一次處理多檔時 NR 為跨檔累計值。
#
# 已知限制：字串字面值中的 f32/HashMap 會產生 false positive。
# 精確偵測由 Phase 1 task-11 Clippy lint（MIR 層）負責。
set -euo pipefail

DETERMINISTIC_CRATES=(
    "engine/deterministic"
    "engine/state_hash"
    "engine/vm_runtime"
    "engine/netcode"
    "engine/vm_bevy_bridge"
    "engine/shadow_vm"
    "engine/bridge_types"
    "engine/crypto"
)
# 明確排除：engine/bevy_runtime（渲染層，允許 native float）

# 允許使用 native float 的檔案（Rhai bridge 層、SoftF32 轉換、診斷工具）
# 這些檔案在 Rhai↔SoftF32 邊界上必須使用 f64（Rhai 原生型別）
FLOAT_ALLOW_FILES=(
    "soft_float.rs"         # SoftF32↔f32/f64 轉換函式
    "bridge_api.rs"         # Rhai callback 綁定（f64 為 Rhai 原生型別）
    "dynamic_convert.rs"    # DynamicValue↔Rhai 型別轉換
    "dynamic_value.rs"      # DynamicValue::Float(f64) 定義
    "bridge_event.rs"       # BridgeEvent 攜帶 Rhai f64 值
    "dev_tools.rs"          # 開發者工具（診斷用，非 game logic）
    "ops_cost.rs"           # 效能監控統計（時間 ms，非 game logic）
    "performance.rs"        # 效能監控（同上）
    "lifecycle.rs"          # ScriptManager 計時（診斷用）
    "fallback.rs"           # Fallback 測試 helper（time_ms 模擬）
    "scope_limiter.rs"      # 字串比對（"f64"非實際型別使用）
    "errors.rs"             # 錯誤型別攜帶 elapsed_ms
    "deterministic_value.rs" # 測試中 to_f64 驗證
    "executor.rs"           # FIXED_DELTA_TIME 常數（Shadow VM 初始化）
    "test_helpers.rs"       # 測試 helper
    "flush.rs"              # BridgeEvent flush（weight: f64 來自 Rhai）
    "conditioner.rs"        # NetworkCondition 測試工具（#[cfg(test)]，非 game logic）
    "camera_effects.rs"     # 攝影機屬渲染層子系統（見該檔頭註解），CameraBridgeOp 欄位為 f32
    "bridge_helpers.rs"     # Rhai bridge 參數驗證（f64 為 Rhai 原生型別，同 bridge_api.rs）
    "camera_module.rs"      # Rhai 攝影機模組（f64 為 Rhai 原生型別，轉換後交渲染層）
    "sandbox.rs"            # 幀預算與腳本逾時（ms 計時），非 game logic
    "clock.rs"              # Clock 抽象的 epoch_ms（時間來源，非狀態計算）
)

EXIT_CODE=0

# 構建 awk 排除模式（將檔案名稱列表轉為 awk 正則）
FLOAT_EXCLUDE_PATTERN=""
for f in "${FLOAT_ALLOW_FILES[@]}"; do
    if [ -n "$FLOAT_EXCLUDE_PATTERN" ]; then
        FLOAT_EXCLUDE_PATTERN="$FLOAT_EXCLUDE_PATTERN|"
    fi
    FLOAT_EXCLUDE_PATTERN="$FLOAT_EXCLUDE_PATTERN$f"
done

for crate_path in "${DETERMINISTIC_CRATES[@]}"; do
    if [ ! -d "$crate_path/src" ]; then
        echo "跳過（目錄不存在）：$crate_path"
        continue
    fi
    echo "掃描 $crate_path ..."

    # === 檢查 1：native float (f32/f64) ===
    # 使用 awk 排除整個 #[cfg(test)] / mod tests 區塊
    FLOAT_HITS=$(find "$crate_path/src/" -name '*.rs' -exec \
        awk -v exclude="$FLOAT_EXCLUDE_PATTERN" '
            BEGIN { split(exclude, arr, "|"); for (i in arr) excl[arr[i]] = 1 }
            # 換檔即重置 skip 狀態：awk 一次處理多檔，未重置會讓前一檔未閉合的
            # test 區塊把後續整個檔案吃掉（掃描順序因平台而異，會造成漏檢）
            FNR == 1 { skip = 0; depth = 0 }
            {
                # 提取檔案名稱
                n = split(FILENAME, parts, "/")
                fname = parts[n]
                if (fname in excl) next
            }
            /^[[:space:]]*\/\// { next }
            /^#\[cfg\(test\)\]/ || /mod tests \{/ { skip=1; depth=0 }
            skip && /\{/ { depth++ }
            skip && /\}/ { depth--; if(depth<=0) skip=0; next }
            skip { next }
            /(^|[^a-zA-Z0-9_])f32([^a-zA-Z0-9_]|$)|(^|[^a-zA-Z0-9_])f64([^a-zA-Z0-9_]|$)/ && !/SoftF32/ && !/softfloat/ && !/allow-native-float/ && !/#\[cfg.*render/ && !/from_f32/ && !/from_f64/ && !/to_f64/ && !/to_native/ { print FILENAME ":" FNR ": " $0 }
        ' {} + 2>/dev/null || true)

    if [ -n "$FLOAT_HITS" ]; then
        echo "錯誤：在 $crate_path 中發現 native float 使用："
        echo "$FLOAT_HITS"
        EXIT_CODE=1
    fi

    # === 檢查 2：HashMap/HashSet（非確定性集合）===
    HASH_HITS=$(find "$crate_path/src/" -name '*.rs' -exec \
        awk '
            FNR == 1 { skip = 0; depth = 0 }
            /^[[:space:]]*\/\// { next }
            /^#\[cfg\(test\)\]/ || /mod tests \{/ { skip=1; depth=0 }
            skip && /\{/ { depth++ }
            skip && /\}/ { depth--; if(depth<=0) skip=0; next }
            skip { next }
            /HashMap|HashSet/ && !/BTreeMap/ && !/BTreeSet/ && !/IndexMap/ && !/allow-hash-collection/ { print FILENAME ":" FNR ": " $0 }
        ' {} + 2>/dev/null || true)

    if [ -n "$HASH_HITS" ]; then
        echo "錯誤：在 $crate_path 中發現非確定性集合（HashMap/HashSet）："
        echo "$HASH_HITS"
        EXIT_CODE=1
    fi
done

if [ $EXIT_CODE -eq 0 ]; then
    echo "確定性模組檢查通過（float + 集合）"
fi
exit $EXIT_CODE
