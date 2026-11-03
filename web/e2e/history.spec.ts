import { test, expect, type APIRequestContext } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeAll(async () => {
  fixture = await startKernelFixture();
});

test.afterAll(async () => {
  await fixture?.close();
});

async function call(request: APIRequestContext, body: Record<string, unknown>) {
  const response = await request.post(
    `${fixture.baseUrl}/api?token=${encodeURIComponent(fixture.token)}`,
    { data: body },
  );
  expect(response.ok()).toBeTruthy();
  return response.json();
}

async function finishedTurns(request: APIRequestContext): Promise<number> {
  const body = await call(request, { method: 'follow', lastSeq: 0 });
  return (body.events as { type: string }[]).filter((event) => event.type === 'turn/end')
    .length;
}

test('打开地址回到同一条对话；往上翻按轮补齐', async ({ request, page }) => {
  for (let index = 1; index <= 6; index += 1) {
    await call(request, {
      method: 'send',
      requestId: `t007-3-${index}`,
      content: `历史消息 ${index}`,
    });
    await expect
      .poll(() => finishedTurns(request), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(index);
  }

  const url = `${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`;
  await page.goto(url);
  await expect(page.getByText('历史消息 6', { exact: false })).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText('历史消息 1', { exact: false })).toHaveCount(0);

  await page.locator('[data-pane="conversation"]').hover();
  await page.mouse.wheel(0, -3000);
  await expect(page.getByText('历史消息 1', { exact: false })).toBeVisible({
    timeout: 15_000,
  });

  await page.reload();
  await expect(page.getByText('历史消息 6', { exact: false })).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText('历史消息 1', { exact: false })).toHaveCount(0);
});
