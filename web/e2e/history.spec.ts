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

/** 已有多少轮跑完（follow 从 0 拿全部事件）。 */
async function finishedTurns(request: APIRequestContext): Promise<number> {
  const body = await call(request, { method: 'follow', lastSeq: 0 });
  return (body.events as { type: string }[]).filter((event) => event.type === 'turn/end')
    .length;
}

/// T007.3：打开地址回到同一条对话；往上翻按轮补齐。
/// 先通过服务发 6 条（假 DeepSeek 回放），再打开页面看历史：首屏是最近 5 轮，
/// 翻到顶加载更早 1 轮；刷新后仍是同一条对话。
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

  const scroller = page.locator('[data-virtuoso-scroller="true"]');
  await scroller.evaluate((element) => {
    (element as HTMLElement).scrollTop = 0;
    element.dispatchEvent(new WheelEvent('wheel', { deltaY: -120, bubbles: true }));
  });
  await expect(page.getByText('历史消息 1', { exact: false })).toBeVisible({
    timeout: 15_000,
  });

  await page.reload();
  await expect(page.getByText('历史消息 6', { exact: false })).toBeVisible({
    timeout: 15_000,
  });
});
