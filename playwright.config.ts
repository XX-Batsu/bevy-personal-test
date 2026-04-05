// Playwright 跨瀏覽器確定性測試配置
// Phase 19 Task 13

import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/wasm',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: 'html',
  use: {
    baseURL: 'http://localhost:8080',
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] },
    },
  ],
  webServer: {
    command: 'npx serve pkg -p 8080',
    port: 8080,
    reuseExistingServer: !process.env.CI,
  },
});
