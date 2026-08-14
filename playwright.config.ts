// Playwright 瀏覽器煙霧測試配置
//
// 受測目標為組裝好的 client/js/：index.html + wasm-bindgen 產物 + 編譯後的
// wasm_loader_bg.wasm（CI Stage 2 產出並覆蓋至該目錄）。

import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/wasm',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: 'html',
  use: {
    baseURL: 'http://127.0.0.1:8080',
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        // CI runner 無 GPU，WebGL2 由 SwiftShader 提供；新版 Chrome 需顯式允許
        launchOptions: { args: ['--enable-unsafe-swiftshader'] },
      },
    },
    // Firefox 暫不納入：headless 環境的 WebGL2 支援不穩定，加入只會產生
    // 與 client 無關的偶發失敗。跨瀏覽器確定性比對待 hash API 對外暴露後再補。
  ],
  webServer: {
    // 靜態服務 client/js（不引入額外 npm 相依）
    command: 'python3 -m http.server 8080 --directory client/js --bind 127.0.0.1',
    port: 8080,
    reuseExistingServer: !process.env.CI,
  },
});
