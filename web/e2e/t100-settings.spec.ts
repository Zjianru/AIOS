//! T100.4 #5/#9：看图的设置链顺序持久化和截桌面假权限。
import { expect, test } from '@playwright/test';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(180_000);

async function configureModels(request: any, fixture: any) {
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
    model: 'MiniMax-M3.1-Flash',
    models: [
      { id: 'MiniMax-M3.1-Flash', input: ['text'] },
      { id: 'MiniMax-M3', input: ['text', 'image'] },
      { id: 'MiniMax-M3-Alt', name: 'MiniMax M3 Alt', input: ['text', 'image'] },
    ],
  });
  await apiPost(request, fixture, { method: 'recheck' });
}

test('#5 看图模型顺序刷新后还在，增删都走 settings-update', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await configureModels(request, fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    await expect(section.locator('[data-kind="vision-empty"]')).toBeVisible();

    const add = section.locator('[data-kind="vision-add"]');
    await add.locator('select').selectOption('minimax-cn/MiniMax-M3');
    await add.getByRole('button', { name: '添加看图模型' }).click();
    await expect(section.locator('[data-kind="vision-model"]')).toHaveCount(1);
    await add.locator('select').selectOption('minimax-cn/MiniMax-M3-Alt');
    await add.getByRole('button', { name: '添加备用看图模型' }).click();
    await expect(section.locator('[data-kind="vision-model"]')).toHaveCount(2);

    // 调顺序：第一行往后挪，MiniMax-M3-Alt 变成先用。
    await section.locator('[data-kind="vision-model"]').first().getByRole('button', { name: '往后挪' }).click();
    await expect(section.locator('[data-kind="vision-model"]').first()).toContainText('MiniMax M3 Alt');

    const beforeReload = await apiPost(request, fixture, { method: 'model-settings' });
    const beforeView = (await beforeReload.json()) as { visionModels: { connection: string; model: string }[] };
    expect(beforeView.visionModels.map((item) => item.model)).toEqual(['MiniMax-M3-Alt', 'MiniMax-M3']);

    await page.reload();
    const reloaded = page.locator('[data-settings="vision"] [data-kind="vision-model"]');
    await expect(reloaded).toHaveCount(2, { timeout: 60_000 });
    await expect(reloaded.first()).toContainText('MiniMax M3 Alt');

    // 移除备用：第二行移除，只剩一个。
    await reloaded.nth(1).getByRole('button', { name: '移除' }).click();
    await expect(page.locator('[data-settings="vision"] [data-kind="vision-model"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});

test('#9 截桌面默认关；假权限端口 denied 再查一次仍 denied', async ({ page, request }) => {
  process.env.AIOS_TEST_SCREEN_PERMISSION = 'denied';
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    const toggle = section.locator('[data-kind="screen-toggle"]');
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute('data-state', 'unchecked');

    await toggle.click();
    const permission = section.locator('[data-kind="screen-permission"]');
    await expect(permission).toHaveAttribute('data-phase', 'denied', { timeout: 20_000 });
    await expect(permission).toContainText('系统设置 › 隐私与安全性 › 录屏与系统录音');
    await permission.getByRole('button', { name: '再查一次' }).click();
    await expect(permission).toHaveAttribute('data-phase', 'denied');
  } finally {
    delete process.env.AIOS_TEST_SCREEN_PERMISSION;
    await fixture.close();
  }
});

test('F4 测试看图按钮：normal/text-only/rate-limited/timeout/error 六态都画对', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await configureModels(request, fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    await section.locator('[data-kind="vision-add"] select').selectOption('minimax-cn/MiniMax-M3');
    await section.getByRole('button', { name: '添加看图模型' }).click();
    const row = section.locator('[data-kind="vision-model"]').first();
    await expect(row).toHaveCount(1);
    const key = 'minimax-cn/MiniMax-M3';
    const cases: [string, string][] = [
      ['normal', 'ok'],
      ['text-only', 'textOnly'],
      ['rate-limited', 'rateLimited'],
      ['timeout', 'timeout'],
      ['error', 'error'],
    ];
    for (const [inject, state] of cases) {
      await page.evaluate(
        ([name, value]) => {
          window.__AIOS_VISION_TEST__ = { [name]: value };
        },
        [key, inject],
      );
      await row.getByRole('button', { name: /测试看图|重新测试/ }).click();
      await expect(row).toHaveAttribute('data-test-state', state);
    }
    await expect(row).toContainText(/只认文字|超时|太频繁|测试失败|看图正常/);
  } finally {
    await fixture.close();
  }
});
