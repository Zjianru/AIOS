import { test, expect, type APIRequestContext } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';
import { failNextSend } from './network';

let fixture: KernelFixture;

test.beforeAll(async () => {
  fixture = await startKernelFixture();
});

test.afterAll(async () => {
  await fixture?.close();
});

async function userMessages(request: APIRequestContext, text: string): Promise<number> {
  const response = await request.post(
    `${fixture.baseUrl}/api?token=${encodeURIComponent(fixture.token)}`,
    { data: { method: 'follow', lastSeq: 0 } },
  );
  const body = await response.json();
  return (body.events as { type: string; data: unknown }[]).filter(
    (event) => event.type === 'user/message' && JSON.stringify(event.data).includes(text),
  ).length;
}

/// T007.9 清单 8：送消息 POST 失败 → 「没送到内核 · 重发」；重发同 requestId，日志只有一条。
test('没送到内核：重发；Kind 只收到一次', async ({ page, request }) => {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  await failNextSend(page);
  await page.getByPlaceholder('和 Kind 说…').fill('这条会送不到');
  await page.getByRole('button', { name: '发送' }).click();
  await expect(page.getByText('没送到内核', { exact: false })).toBeVisible({ timeout: 10_000 });
  await page.getByRole('button', { name: '重发' }).click();
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  expect(await userMessages(request, '这条会送不到')).toBe(1);
  await expect(page.getByText('没送到内核', { exact: false })).toHaveCount(0);
});
