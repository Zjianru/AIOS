import { test, expect, type APIRequestContext } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeEach(async () => {
  fixture = await startKernelFixture({ askTurn: true });
});

test.afterEach(async () => {
  await fixture?.close();
});

async function call(request: APIRequestContext, body: Record<string, unknown>) {
  const response = await apiPost(request, fixture, body);
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

/// T090.2：问题卡等回答时直接在输入框发送 = 回答这张卡；Kind 同轮继续，卡写「你直接回复了」。
test('提问卡等时直接发送：当回答、同轮继续、卡回执「你直接回复了」', async ({ page }) => {
  await openAndAsk(page);
  await page.locator('[data-kind="composer-input"]').fill('我直接说一句');
  await page.keyboard.press('Enter');
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText('你直接回复了', { exact: false })).toBeVisible();
  await expect(page.getByText('你没回答 Kind 的问题', { exact: false })).toHaveCount(0);
});

/// T090.2：带粘贴/附件的发送也当回答（不是只认纯文字）。
test('提问卡等时带粘贴发送：也当回答', async ({ page }) => {
  await openAndAsk(page);
  await page.locator('[data-kind="composer-input"]').evaluate((element) => {
    const data = new DataTransfer();
    data.setData('text/plain', '粘'.repeat(1200));
    const event = new ClipboardEvent('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: data });
    element.dispatchEvent(event);
  });
  await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1, { timeout: 10_000 });
  await page.keyboard.press('Enter');
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText('你直接回复了', { exact: false })).toBeVisible();
});

/// T090.2：带附件的发送也当回答（不是只认纯文字/粘贴）。
test('提问卡等时带附件发送：也当回答', async ({ page }) => {
  await openAndAsk(page);
  await page.locator('[data-kind="composer-input"]').evaluate((element) => {
    const data = new DataTransfer();
    data.items.add(new File(['附件正文'], '提问附件.txt', { type: 'text/plain' }));
    const event = new ClipboardEvent('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: data });
    element.dispatchEvent(event);
  });
  await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 10_000 });
  await page.keyboard.press('Enter');
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText('你直接回复了', { exact: false })).toBeVisible();
});

/// T007.6 清单 4：整批不答，模型收到用户没答后照常收尾。
test('提问整批：整批不答', async ({ page, request }) => {
  await openAndAsk(page);
  await page.getByRole('button', { name: '不答' }).click();
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  await expect.poll(() => cancelledQuestions(request), { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
});
