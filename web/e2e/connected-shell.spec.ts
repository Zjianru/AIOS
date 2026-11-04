import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeAll(async () => {
  fixture = await startKernelFixture();
});

test.afterAll(async () => {
  await fixture?.close();
});

test('接缝 5 骨架：真 serve 活着，连接视图只有 Kind、无原型专属外壳', async ({ page, request }) => {
  const response = await apiPost(request, fixture, { method: 'status' });
  expect(response.status()).toBe(200);
  const body = await response.json();
  expect(body.object.sessionId).toBeTruthy();

  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  await expect(page).toHaveTitle('AIOS');
  await expect(page.getByText('内核状态')).toHaveCount(0);
  await expect(page.getByText('原型：', { exact: false })).toHaveCount(0);
  await expect(page.getByText('AIOS 概念 ·', { exact: false })).toHaveCount(0);
  for (const label of ['现在', '工作', '项目', '团队', '资料', '记忆', '能力']) {
    await expect(page.getByRole('button', { name: label, exact: true })).toHaveCount(0);
  }
  await expect(page.getByText('Kind').first()).toBeVisible();
});
