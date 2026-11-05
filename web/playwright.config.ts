import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  // 每条 e2e 都起 serve/replay 进程；并发太高会互相拖慢。
  workers: 2,
  use: {
    channel: 'chrome',
    headless: true,
  },
});
