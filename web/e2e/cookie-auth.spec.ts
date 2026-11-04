import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('D49 cookie：跳转后地址无令牌，api/ws 都不带 query token', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    const apiUrls: string[] = [];
    const wsUrls: string[] = [];
    page.on('request', (request) => {
      const url = request.url();
      if (url.includes('/api') || url.includes('/ws')) apiUrls.push(url);
    });
    page.on('websocket', (socket) => wsUrls.push(socket.url()));
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 20_000 });
    expect(page.url()).not.toContain('token=');
    expect(apiUrls.some((url) => url.includes('/api'))).toBeTruthy();
    expect(apiUrls.every((url) => !url.includes('token='))).toBeTruthy();
    await expect.poll(() => wsUrls.length, { timeout: 20_000 }).toBeGreaterThan(0);
    expect(wsUrls.every((url) => !url.includes('token='))).toBeTruthy();
  } finally {
    await fixture.close();
  }
});
