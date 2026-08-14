// WASM client 啟動煙霧測試
//
// 以離線模式（?offline）載入 client/js/index.html：跳過 WebSocket 與 ECDH 握手，
// 因此不需要 gateway server 即可驗證 WASM 啟動 → Bevy 初始化 → 首幀顯示的完整路徑。
//
// 跨瀏覽器確定性 hash 比對尚未涵蓋：需先由 WASM 對外暴露 hash 計算 API。
// 目前確定性驗證由 native 測試與 tools/golden_hash.sh 負責。

import { test, expect } from '@playwright/test';

const BOOT_TIMEOUT = 60_000;

test.describe('WASM client 啟動', () => {
  test('離線模式啟動後顯示 game canvas', async ({ page }) => {
    await page.goto('/index.html?offline');

    const canvas = page.locator('#game-canvas');
    await expect(canvas).toBeVisible({ timeout: BOOT_TIMEOUT });
    await expect(page.locator('#loading-screen')).toBeHidden();

    // winit 會在建立 window 時把 canvas 設為 display:none；bootstrap 的
    // keepCanvasVisible() 負責復原。尺寸為 0 代表該修復失效。
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.width).toBeGreaterThan(0);
    expect(box!.height).toBeGreaterThan(0);
  });

  test('啟動過程無 console error', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error') errors.push(msg.text());
    });
    page.on('pageerror', (err) => errors.push(String(err)));

    await page.goto('/index.html?offline');
    await expect(page.locator('#game-canvas')).toBeVisible({ timeout: BOOT_TIMEOUT });

    expect(errors).toEqual([]);
  });
});
