//! T100.4 #9：截桌面默认关，关着问会指设置；打开后假权限 denied 指系统设置。
import { expect, test } from '@playwright/test';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(240_000);
const MAIN = 'MiniMax-M3.1-Flash';

async function askScreenshot(page: any, text: string) {
  const input = page.locator('[data-kind="composer-input"]');
  await expect(input).toBeVisible({ timeout: 60_000 });
  const turns = page.locator('[data-turn]');
  const before = await turns.count();
  await input.fill(text);
  await page.locator('[data-kind="send"]').click();
  await expect.poll(async () => turns.count(), { timeout: 30_000 }).toBeGreaterThan(before);
  await expect(
    turns.last().getByRole('button', { name: /已完成，用时|停下了|已派出/ }),
  ).toBeVisible({ timeout: 120_000 });
  return turns.last();
}

test('#9 截桌面关/没权限各自指设置', async ({ page, request }) => {
  process.env.AIOS_TEST_SCREEN_PERMISSION = 'denied';
  const fixture = await startD78KernelFixture();
  try {
    const settingsResponse = await apiPost(request, fixture, { method: 'model-settings' });
    const settings = (await settingsResponse.json()) as {
      settings: { connections: { id: string; endpoint: string; preset?: string }[] };
    };
    const connection = settings.settings.connections.find((item) => item.id === 'minimax-cn')!;
    await apiPost(request, fixture, {
      method: 'connection-update',
      id: connection.id,
      endpoint: connection.endpoint,
      preset: connection.preset,
      model: MAIN,
      models: [{ id: MAIN, input: ['text'] }],
    });
    await apiPost(request, fixture, { method: 'recheck' });
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const first = await askScreenshot(page, '用 vision_screenshot 截一下我现在的桌面。');
    await expect(first).toContainText(/设置|截桌面|没打开|关着/);

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    await section.locator('[data-kind="screen-toggle"]').click();
    await expect(section.locator('[data-kind="screen-permission"]')).toHaveAttribute('data-phase', 'denied', { timeout: 20_000 });

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const second = await askScreenshot(page, '再用 vision_screenshot 截一下桌面。');
    await expect(second).toContainText(/屏幕录制|录屏|系统设置|权限/);
  } finally {
    delete process.env.AIOS_TEST_SCREEN_PERMISSION;
    await fixture.close();
  }
});
