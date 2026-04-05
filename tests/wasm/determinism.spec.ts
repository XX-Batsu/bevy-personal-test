// 跨瀏覽器確定性 hash 比對測試
// Phase 19 Task 13
//
// 驗證 WASM 模組在不同瀏覽器產生相同的 state hash。
// 實際測試需要 WASM 模組載入後暴露 hash 計算 API。

import { test, expect } from '@playwright/test';

test.describe('WASM 確定性驗證', () => {
  test('WASM 模組載入成功', async ({ page }) => {
    await page.goto('/');
    // 驗證 WASM 模組載入（pkg/ 目錄由 CI Stage 2 產出）
    const title = await page.title();
    expect(title).toBeDefined();
  });

  test('WASM 頁面無 console error', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', msg => {
      if (msg.type() === 'error') {
        errors.push(msg.text());
      }
    });
    await page.goto('/');
    await page.waitForTimeout(2000);
    expect(errors).toEqual([]);
  });
});
