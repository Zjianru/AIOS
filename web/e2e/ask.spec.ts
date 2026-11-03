import { test, expect, type APIRequestContext } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeEach(async () => {
  fixture = await startKernelFixture({ askTurn: true });
});

test.afterEach(async () => {
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

async function cancelledQuestions(request: APIRequestContext): Promise<number> {
  const body = await call(request, { method: 'follow', lastSeq: 0 });
  return (body.events as { type: string }[]).filter((event) => event.type === 'question/cancelled')
    .length;
}

async function openAndAsk(page: import('@playwright/test').Page) {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  await page.getByPlaceholder('和 Kind 说…').fill('问我两个问题');
  await page.getByRole('button', { name: '发送' }).click();
  await expect(page.getByText('先选一个？', { exact: false })).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText('再自己写一句？', { exact: false })).toBeVisible();
}

/// T007.6 清单 4：两题整批作答（一题选项、一题自己写）。
test('提问整批：选项 + 自己写作答', async ({ page }) => {
  await openAndAsk(page);
  await page.getByRole('radio', { name: /Yes/ }).click();
  await page.getByPlaceholder('写下你的回答').fill('我自己写的答');
  await page.getByRole('button', { name: '提交回答' }).click();
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText('我自己写的答', { exact: true })).toBeVisible();
});

/// T007.6 清单 4：整批不答，模型收到用户没答后照常收尾。
test('提问整批：整批不答', async ({ page, request }) => {
  await openAndAsk(page);
  await page.getByRole('button', { name: '不答' }).click();
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  await expect.poll(() => cancelledQuestions(request), { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
});
