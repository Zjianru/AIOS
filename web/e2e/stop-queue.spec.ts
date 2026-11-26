import { test, expect, type APIRequestContext } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

// T090.1：停止清掉排队消息时，页面从日志投影把字/粘贴/附件放回输入框；
// 每条 canceled splice 最多放回一次；页面关着时按的停止，重开页面补放回；断线发送不静默丢。

let fixture: KernelFixture;

test.beforeEach(async () => {
  // 第一轮扣慢一点，保证「运行中再排一条」有稳定窗口。
  fixture = await startKernelFixture({ slowMs: 6000 });
});

test.afterEach(async () => {
  await fixture?.close();
});

interface LoggedEvent {
  seq: number;
  type: string;
  data?: Record<string, unknown>;
}

async function events(request: APIRequestContext): Promise<LoggedEvent[]> {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  const body = (await response.json()) as { events?: LoggedEvent[] };
  return body.events ?? [];
}

async function turnStarts(request: APIRequestContext): Promise<number> {
  return (await events(request)).filter((event) => event.type === 'turn/start').length;
}

async function abortedTurns(request: APIRequestContext): Promise<number> {
  return (await events(request)).filter(
    (event) =>
      event.type === 'turn/end' &&
      (event.data?.reason as { kind?: string } | undefined)?.kind === 'aborted',
  ).length;
}

/** 等到这句真的进了 next-turn 队列的 splice（再点停止，测试才不抖）。 */
async function waitQueued(request: APIRequestContext, needle: string): Promise<void> {
  await expect
    .poll(
      async () =>
        (await events(request)).some(
          (event) =>
            event.type === 'agent/inbox/spliced' &&
            event.data?.target === 'next-turn' &&
            JSON.stringify(event.data?.inserted ?? []).includes(needle),
        ),
      { timeout: 20_000 },
    )
    .toBe(true);
}

async function deliveredAsUserMessage(request: APIRequestContext, needle: string): Promise<number> {
  return (await events(request)).filter(
    (event) => event.type === 'user/message' && JSON.stringify(event.data ?? {}).includes(needle),
  ).length;
}

const input = (page: import('@playwright/test').Page) => page.locator('[data-kind="composer-input"]');

async function sendSlowTurn(page: import('@playwright/test').Page): Promise<void> {
  await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
  await input(page).fill('第一句');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('button', { name: '停止', exact: true })).toBeVisible({
    timeout: 15_000,
  });
}

async function queueMessage(page: import('@playwright/test').Page, text: string): Promise<void> {
  await input(page).fill(text);
  await page.keyboard.press('Enter');
}

async function stopTurn(page: import('@playwright/test').Page): Promise<void> {
  await page.getByRole('button', { name: '停止', exact: true }).click();
}

test('停止：排队的一句话放回输入框，不开新一轮；刷新还在', async ({ page, request }) => {
  await sendSlowTurn(page);
  await queueMessage(page, '排队的一句');
  await waitQueued(request, '排队的一句');
  await stopTurn(page);
  await expect(input(page)).toHaveValue('排队的一句', { timeout: 15_000 });
  await expect.poll(() => abortedTurns(request), { timeout: 15_000 }).toBe(1);
  await expect.poll(() => turnStarts(request)).toBe(1);
  expect(await deliveredAsUserMessage(request, '排队的一句')).toBe(0);

  await page.reload();
  await expect(input(page)).toHaveValue('排队的一句', { timeout: 15_000 });
});

test('停止：删掉放回的字再刷新，不会再冒出来（最多放回一次）', async ({ page, request }) => {
  await sendSlowTurn(page);
  await queueMessage(page, '会被删掉的字');
  await waitQueued(request, '会被删掉的字');
  await stopTurn(page);
  await expect(input(page)).toHaveValue('会被删掉的字', { timeout: 15_000 });

  await input(page).fill('');
  await expect(input(page)).toHaveValue('');

  await page.reload();
  await expect(input(page)).toHaveValue('', { timeout: 15_000 });
});

test('停止：输入框已有字时，放回的字接在后面', async ({ page, request }) => {
  await sendSlowTurn(page);
  await queueMessage(page, '排队的字');
  await waitQueued(request, '排队的字');
  await input(page).fill('手头还有字');
  await stopTurn(page);

  await expect(input(page)).toHaveValue('手头还有字\n排队的字', { timeout: 15_000 });
});

test('停止：粘贴和附件放回成卡', async ({ page, request }) => {
  await sendSlowTurn(page);
  const paste = '粘'.repeat(1200);
  await input(page).evaluate((element, text) => {
    const data = new DataTransfer();
    data.setData('text/plain', text);
    const event = new ClipboardEvent('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: data });
    element.dispatchEvent(event);
  }, paste);
  await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1, { timeout: 10_000 });
  const name = '排队附件.txt';
  await input(page).evaluate((element, fileName) => {
    const data = new DataTransfer();
    data.items.add(new File(['附件正文'], fileName, { type: 'text/plain' }));
    const event = new ClipboardEvent('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: data });
    element.dispatchEvent(event);
  }, name);
  await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 10_000 });
  await page.keyboard.press('Enter');
  await waitQueued(request, name);
  await stopTurn(page);

  await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1, { timeout: 15_000 });
  await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 15_000 });
  await expect(page.getByText('排队附件.txt')).toBeVisible();
});

test('停止：画布块放回输入框，显示「来自画布」标记', async ({ page, request }) => {
  await sendSlowTurn(page);
  const response = await apiPost(request, fixture, {
    method: 'send',
    requestId: 't090-canvas-' + String(Date.now()) + '-' + String(Math.random()).slice(2),
    content: '画布那句',
    delivery: 'follow-up',
    canvas: { id: 'cv-t090', title: '测试画布' },
  });
  expect(response.ok()).toBeTruthy();
  await waitQueued(request, '测试画布');
  await stopTurn(page);

  await expect(page.locator('[data-kind="from-canvas"]')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('[data-kind="from-canvas"]')).toContainText('测试画布');
  await expect(input(page)).toHaveValue('画布那句', { timeout: 15_000 });
});

test('停止：页面关着时取消，重开页面把字补回输入框', async ({ page, request }) => {
  await sendSlowTurn(page);
  await queueMessage(page, '页面关着时排的');
  await waitQueued(request, '页面关着时排的');
  await page.close();

  const response = await apiPost(request, fixture, { method: 'cancel' });
  expect(response.ok()).toBeTruthy();

  const reopened = await page.context().newPage();
  await reopened.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
  await expect(input(reopened)).toHaveValue('页面关着时排的', { timeout: 20_000 });
});
test('停止：排队两条各带标记的消息逐条放回，不合并成一条', async ({ page, request }) => {
  await sendSlowTurn(page);
  const sendOne = async (requestId: string, text: string, cardTitle: string, canvasTitle: string) => {
    const response = await apiPost(request, fixture, {
      method: 'send',
      requestId,
      content: text,
      delivery: 'follow-up',
      resultCards: [{ taskId: 'task-' + requestId, title: cardTitle, version: 1 }],
      canvases: [{ id: 'cv-' + requestId, title: canvasTitle }],
    });
    expect(response.ok()).toBeTruthy();
  };
  await sendOne('t095-2-a-' + String(Date.now()), '第一条', '结果甲', '画布甲');
  await waitQueued(request, '结果甲');
  await sendOne('t095-2-b-' + String(Date.now()), '第二条', '结果乙', '画布乙');
  await waitQueued(request, '结果乙');
  await stopTurn(page);

  await expect(input(page)).toHaveValue('第一条' + String.fromCharCode(10) + '第二条', {
    timeout: 15_000,
  });
  await expect(page.locator('[data-kind="result-card-chip"]')).toHaveCount(2, { timeout: 15_000 });
  await expect(page.locator('[data-kind="result-card-chip"]')).toContainText(['结果甲', '结果乙']);
  await expect(page.locator('[data-kind="from-canvas"]')).toHaveCount(2, { timeout: 15_000 });
  await expect(page.locator('[data-kind="from-canvas"]')).toContainText(['画布甲', '画布乙']);

  await page.locator('[aria-label="移除结果卡"]').first().click();
  await expect(page.locator('[data-kind="result-card-chip"]')).toHaveCount(1);
  await page.locator('[aria-label="移除来自画布的标记"]').first().click();
  await expect(page.locator('[data-kind="from-canvas"]')).toHaveCount(1);
});

test('停止：旧日志没有 mime 的图片附件按扩展名回图片', async ({ page, request }) => {
  await sendSlowTurn(page);
  const put = await apiPost(request, fixture, {
    method: 'attachment-put',
    name: 'pic.png',
    data: 'aGVsbG8=',
  });
  const putBody = (await put.json()) as { id?: string };
  expect(putBody.id).toBeTruthy();
  const response = await apiPost(request, fixture, {
    method: 'send',
    requestId: 't095-2-mime-' + String(Date.now()) + '-' + String(Math.random()).slice(2),
    content: '看这张图',
    delivery: 'follow-up',
    // 旧日志的发送：附件不带 mime，放回时按扩展名 .png 兜底成 image/png。
    attachments: [{ id: putBody.id, name: 'pic.png', bytes: 5 }],
  });
  expect(response.ok()).toBeTruthy();
  await waitQueued(request, 'pic.png');
  await stopTurn(page);

  await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 15_000 });
  await expect(page.locator('[data-kind="file-card"]')).toContainText('image/png');
});