import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: 'http://localhost:4173',
    channel: 'chrome',
    headless: true,
  },
  webServer: {
    command: 'npm run preview',
    url: 'http://localhost:4173',
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
