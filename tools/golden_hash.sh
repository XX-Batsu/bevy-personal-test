#!/usr/bin/env bash
# Golden hash fixture 管理工具
# 測試程式碼由 Phase 19 Task 5 建立，本腳本提供 CLI 包裝。
#
# 核心價值：
# 1. generate 後自動提示 diff review，降低忘記審閱 hash 變化的風險
# 2. 提示 WASM 驗證步驟，確保跨平台一致性
# 3. 為 CI 提供統一的 verify 入口
set -euo pipefail

FIXTURES_DIR="tests/fixtures/replay"

case "${1:-}" in
    generate)
        echo "產生 golden hash fixtures..."

        if [ ! -d "${FIXTURES_DIR}" ]; then
            echo "錯誤：${FIXTURES_DIR} 目錄不存在。"
            exit 1
        fi

        # Phase 19 Task 5 建立的測試，使用 --ignored 執行 regenerate 邏輯
        if ! cargo test --features integration -- --ignored regenerate_golden; then
            echo "錯誤：golden hash 生成失敗。"
            exit 1
        fi

        echo ""
        echo "完成。接下來請："
        echo "  1. review 變更：git diff ${FIXTURES_DIR}/"
        echo "  2. WASM 驗證：wasm-pack test --chrome --headless -- --test verify_golden_matches_wasm"
        echo "  3. 確認無誤後 commit：git add ${FIXTURES_DIR}/"
        ;;
    verify)
        echo "驗證 golden hash fixtures..."

        if [ ! -d "${FIXTURES_DIR}" ]; then
            echo "錯誤：${FIXTURES_DIR} 目錄不存在。"
            exit 1
        fi

        if ! cargo test --features integration -- golden_hash_regression; then
            echo "Golden hash regression 失敗。"
            echo "可能原因：game logic 非預期變更、bincode 版本不相容。"
            exit 1
        fi

        echo "Golden hash regression 通過。"
        ;;
    *)
        echo "用法: $0 {generate|verify}"
        echo ""
        echo "  generate  產生/更新 golden hash fixtures（需人工 review diff + WASM 驗證）"
        echo "  verify    驗證當前 codebase 與 fixtures 一致"
        exit 1
        ;;
esac
