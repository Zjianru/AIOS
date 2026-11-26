import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

// T095.5：双击 Esc（照 app-v14 c6c7073）——清空/↑ 找回、菜单、四个动作、被拒、跑着先停。

let fixture: KernelFixture;

test.beforeEach(async () => {
  fixture = await startKernelFixture();
});

test.afterEach(async () => {
  await fixture?.close();
});

const input = (page: Page) => page.locator('[data-kind="composer-input"]');
const menu = (page: Page) => page.locator('[data-kind="rewind-menu"]');
const conversation = (page: Page) => page.locator('[data-pane="conversation"]');

async function openKind(page: Page) {
  await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
  await expect(input(page)).toBeVisible({ timeout: 20_000 });
}

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

async function turnEnds(request: APIRequestContext): Promise<number> {
  return (await events(request)).filter((event) => event.type === 'turn/end').length;
}

async function sendAndWait(page: Page, request: APIRequestContext, text: string): Promise<void> {
  const before = await turnEnds(request);
  await input(page).fill(text);
  await page.keyboard.press('Enter');
  await expect.poll(() => turnEnds(request), { timeout: 30_000 }).toBeGreaterThan(before);
  await expect(input(page)).toBeEnabled();
}

async function waitTurns(request: APIRequestContext, count: number): Promise<void> {
  await expect.poll(() => turnEnds(request), { timeout: 30_000 }).toBeGreaterThanOrEqual(count);
}

async function doubleEsc(page: Page): Promise<void> {
  await page.keyboard.press('Escape');
  await page.waitForTimeout(120);
  await page.keyboard.press('Escape');
}

async function openMenu(page: Page): Promise<void> {
  await doubleEsc(page);
  await expect(menu(page)).toBeVisible({ timeout: 10_000 });
  await expect(menu(page)).toHaveAttribute('data-phase', 'list');
}

async function selectRow(page: Page, text: string): Promise<void> {
  await menu(page).locator('[data-kind="rewind-row"]').filter({ hasText: text }).first().click();
  await expect(menu(page)).toHaveAttribute('data-phase', 'actions');
}

test('输入框有字：第一下 Esc 提示 500ms 后消失；两下清空，↑ 找回且光标在末尾', async ({ page }) => {
  await openKind(page);
  await input(page).fill('草稿别丢');
  await page.keyboard.press('Escape');
  const hint = page.locator('[data-kind="esc-hint"]');
  await expect(hint).toHaveText('再按一次 Esc 清空输入框');
  // F1①：500ms 内没有第二下，提示自己消失，输入框内容不动。
  await expect(hint).toHaveCount(0, { timeout: 2000 });
  await expect(input(page)).toHaveValue('草稿别丢');
  await page.keyboard.press('Escape');
  await expect(hint).toHaveText('再按一次 Esc 清空输入框');
  await page.keyboard.press('Escape');
  await expect(input(page)).toHaveValue('');
  await input(page).press('ArrowUp');
  await expect(input(page)).toHaveValue('草稿别丢');
  // F1④：找回后光标在末尾。
  const caretAtEnd = await input(page).evaluate((element) => {
    const textarea = element as HTMLTextAreaElement;
    return textarea.selectionStart === textarea.value.length;
  });
  expect(caretAtEnd).toBe(true);
});

test('↑ 找回：两次清空后进先出、草稿按会话 id 存；有附件时不召回', async ({ page, request }) => {
  await openKind(page);
  const statusResponse = await apiPost(request, fixture, { method: 'status' });
  const status = (await statusResponse.json()) as { object?: { sessionId?: string } };
  const sessionId = status.object?.sessionId ?? '';
  expect(sessionId).not.toBe('');

  await input(page).fill('第一条');
  await doubleEsc(page);
  await expect(input(page)).toHaveValue('');
  // F1⑤：草稿存在 aios.kind.draft.<会话 id>，清掉的也跟草稿一起存。
  const stored = await page.evaluate(
    (key) => localStorage.getItem(key),
    'aios.kind.draft.' + sessionId,
  );
  expect(stored).toContain('第一条');

  await input(page).fill('第二条');
  await doubleEsc(page);
  await expect(input(page)).toHaveValue('');

  // F1③：LIFO——先取回最近清掉的，再取回更早的。
  await input(page).press('ArrowUp');
  await expect(input(page)).toHaveValue('第二条');
  await input(page).fill('');
  await input(page).press('ArrowUp');
  await expect(input(page)).toHaveValue('第一条');

  // F1②：输入框里有附件时按 ↑ 不召回（草稿栈里还有一条可被误取）。
  await input(page).fill('');
  await input(page).fill('会被清掉的');
  await doubleEsc(page);
  await expect(input(page)).toHaveValue('');
  await input(page).evaluate((element, fileName) => {
    const data = new DataTransfer();
    data.items.add(new File(['附件正文'], fileName, { type: 'text/plain' }));
    const event = new ClipboardEvent('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: data });
    element.dispatchEvent(event);
  }, '不召回.txt');
  await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1);
  await input(page).press('ArrowUp');
  await expect(input(page)).toHaveValue('');
  await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1);
});

test('空输入两下 Esc：菜单列出说过的话，默认选最新；恢复对话后收起并回输入框，刷新一致', async ({ page, request }) => {
  await openKind(page);
  await sendAndWait(page, request, '第一句');
  await sendAndWait(page, request, '第二句');
  await sendAndWait(page, request, '第三句');

  await openMenu(page);
  const rows = menu(page).locator('[data-kind="rewind-row"]');
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(2)).toHaveAttribute('aria-selected', 'true');
  await expect(rows.nth(2)).toContainText('第三句');

  await selectRow(page, '第一句');
  await menu(page).locator('[data-kind="rewind-action"][data-action="restore"]').click();
  await expect(menu(page)).toHaveCount(0);
  const bubbles = conversation(page).locator('[data-kind="user-bubble"]');
  await expect(bubbles.filter({ hasText: '第一句' })).toHaveCount(0);
  await expect(bubbles.filter({ hasText: '第二句' })).toHaveCount(0);
  await expect(input(page)).toHaveValue('第一句');

  await page.reload();
  await expect(input(page)).toBeVisible({ timeout: 20_000 });
  await expect(bubbles.filter({ hasText: '第一句' })).toHaveCount(0);
  await expect(bubbles.filter({ hasText: '第二句' })).toHaveCount(0);
  await expect(input(page)).toHaveValue('第一句');
});

test('恢复带附件/结果卡/画布的消息：原文回输入框，各是一张卡', async ({ page, request }) => {
  await openKind(page);
  const put = await apiPost(request, fixture, {
    method: 'attachment-put',
    name: 'pic.png',
    data: 'aGVsbG8=',
  });
  const attachment = (await put.json()) as { id?: string };
  expect(attachment.id).toBeTruthy();
  const before = await turnEnds(request);
  const response = await apiPost(request, fixture, {
    method: 'send',
    requestId: 'rewind-rich-' + String(Date.now()),
    content: '带附件的话',
    delivery: 'follow-up',
    attachments: [{ id: attachment.id, name: 'pic.png', bytes: 5, mime: 'image/png' }],
    resultCards: [{ taskId: 'task-a', title: '结果甲', version: 1 }],
    canvases: [{ id: 'cv-a', title: '画布甲' }],
  });
  expect(response.ok()).toBeTruthy();
  await waitTurns(request, before + 1);

  await openMenu(page);
  await selectRow(page, '带附件的话');
  await menu(page).locator('[data-kind="rewind-action"][data-action="restore"]').click();
  await expect(input(page)).toHaveValue('带附件的话');
  await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1);
  await expect(page.locator('[data-kind="file-card"]')).toContainText('image/png');
  await expect(page.locator('[data-kind="result-card-chip"]')).toHaveCount(1);
  await expect(page.locator('[data-kind="attachment-canvas"]')).toHaveCount(1);
});

test('从这里起总结：已总结分隔（from）、原文回输入框', async ({ page, request }) => {
  await openKind(page);
  await sendAndWait(page, request, '第一句');
  await sendAndWait(page, request, '第二句');
  await openMenu(page);
  await selectRow(page, '第一句');
  await menu(page).locator('[data-kind="rewind-action"][data-action="summarize-from"]').click();
  await expect(menu(page)).toHaveCount(0);
  await expect(page.locator('[data-kind="compaction"][data-manual="from"]')).toBeVisible({
    timeout: 20_000,
  });
  await expect(input(page)).toHaveValue('第一句');
  await expect(
    conversation(page).locator('[data-kind="user-bubble"]').filter({ hasText: '第一句' }),
  ).toBeVisible();
  // 「从这里起总结」之后：菜单只剩之后新说的话（这里没有新的 → 空列表文案）。
  await input(page).fill('');
  await openMenu(page);
  await expect(menu(page).locator('[data-kind="rewind-row"]')).toHaveCount(0);
  await expect(menu(page)).toContainText('最后一次压缩之后，你还没说过话。');
  await menu(page).locator('[data-kind="rewind-cancel"]').click();
});

test('总结到这里为止：第一句变灰；第二句的已总结分隔（upTo）在它前面', async ({ page, request }) => {
  await openKind(page);
  await sendAndWait(page, request, '第一句');
  await sendAndWait(page, request, '第二句');
  await sendAndWait(page, request, '第三句');
  await openMenu(page);
  await selectRow(page, '第一句');
  const blocked = menu(page).locator('[data-kind="rewind-action"][data-action="summarize-up-to"]');
  await expect(blocked).toHaveAttribute('aria-disabled', 'true');
  await expect(blocked).toContainText('这句前面没有可以总结的');
  await page.keyboard.press('Escape');
  await expect(menu(page)).toHaveAttribute('data-phase', 'list');
  await selectRow(page, '第二句');
  await menu(page).locator('[data-kind="rewind-action"][data-action="summarize-up-to"]').click();
  await expect(menu(page)).toHaveCount(0);
  const divider = page.locator('[data-kind="compaction"][data-manual="upTo"]').last();
  await expect(divider).toBeVisible({ timeout: 20_000 });
  const dividerBox = await divider.boundingBox();
  const secondBox = await conversation(page)
    .locator('[data-kind="user-bubble"]')
    .filter({ hasText: '第二句' })
    .first()
    .boundingBox();
  expect(dividerBox).not.toBeNull();
  expect(secondBox).not.toBeNull();
  expect(dividerBox!.y).toBeLessThan(secondBox!.y);
});

test('恢复后画布当前版重算：被收起的第 2 版不算，回到第 1 版', async ({ page }) => {
  const canvasFixture = await startKernelFixture({ canvas: 'versions' });
  try {
    await page.goto(
      canvasFixture.baseUrl + '/?token=' + encodeURIComponent(canvasFixture.token) + '#kind',
    );
    const field = page.locator('[data-kind="composer-input"]');
    await field.fill('每月存 3000，十年后有多少，画出来');
    await page.keyboard.press('Enter');
    const frame = page.frameLocator('iframe[data-kind="canvas-frame"]');
    await expect(page.locator('[data-kind="canvas"]')).toContainText('每月存钱算一算', {
      timeout: 40_000,
    });
    await expect(frame.locator('#v')).toHaveText('1');

    await field.fill('把柱子换成折线');
    await page.keyboard.press('Enter');
    await expect(page.locator('[data-kind="canvas-versions"]')).toContainText('第 2 版', {
      timeout: 40_000,
    });
    await expect(page.frameLocator('iframe[data-kind="canvas-frame"]').locator('#v')).toHaveText('2');

    await field.click();
    await doubleEsc(page);
    await expect(menu(page)).toBeVisible({ timeout: 10_000 });
    await selectRow(page, '把柱子换成折线');
    await menu(page).locator('[data-kind="rewind-action"][data-action="restore"]').click();
    await expect(menu(page)).toHaveCount(0);
    await expect(page.frameLocator('iframe[data-kind="canvas-frame"]').locator('#v')).toHaveText('1', {
      timeout: 20_000,
    });
    // 只剩第 1 版：标题栏的版本切换也收掉了（第 2 版那一版不再算当前版）。
    await expect(page.locator('[data-kind="canvas-versions"]')).toHaveCount(0);
  } finally {
    await canvasFixture.close();
  }
});

test('算了：列表里关掉菜单；动作里回到列表', async ({ page, request }) => {
  await openKind(page);
  await sendAndWait(page, request, '第一句');
  await sendAndWait(page, request, '第二句');
  await openMenu(page);
  await menu(page).locator('[data-kind="rewind-cancel"]').click();
  await expect(menu(page)).toHaveCount(0);

  await openMenu(page);
  await selectRow(page, '第一句');
  await menu(page).locator('[data-kind="rewind-action"][data-action="cancel"]').click();
  await expect(menu(page)).toHaveAttribute('data-phase', 'list');
  await menu(page).locator('[data-kind="rewind-cancel"]').click();
  await expect(menu(page)).toHaveCount(0);
});

test('Kind 在跑：第一下 Esc 先停，再两下才开菜单', async ({ page, request }) => {
  const slow = await startKernelFixture({ slowMs: 4000 });
  try {
    await page.goto(slow.baseUrl + '/?token=' + encodeURIComponent(slow.token) + '#kind');
    const field = page.locator('[data-kind="composer-input"]');
    await field.fill('跑一会儿');
    await page.keyboard.press('Enter');
    await expect(page.locator('[data-kind="stop"]')).toBeVisible({ timeout: 15_000 });
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 15_000 });
    await doubleEsc(page);
    await expect(menu(page)).toBeVisible({ timeout: 10_000 });
  } finally {
    await slow.close();
  }
});

test('被拒：菜单开着时内核压缩了，选的那句在压缩前 → 红条给原因', async ({ page, request }) => {
  await openKind(page);
  await sendAndWait(page, request, '第一句');
  await sendAndWait(page, request, '第二句');
  await openMenu(page);
  await selectRow(page, '第一句');
  const before = (await events(request)).filter((event) => event.type === 'compaction/end').length;
  const compact = await apiPost(request, fixture, { method: 'compact' });
  expect(compact.ok()).toBeTruthy();
  await expect
    .poll(async () => (await events(request)).filter((event) => event.type === 'compaction/end').length, {
      timeout: 30_000,
    })
    .toBeGreaterThan(before);
  await menu(page).locator('[data-kind="rewind-action"][data-action="restore"]').click();
  const notice = menu(page).locator('[data-kind="rewind-notice"]');
  await expect(notice).toBeVisible({ timeout: 15_000 });
  await expect(notice).toContainText(/压缩|藏起来/);
  await expect(menu(page)).toHaveAttribute('data-phase', 'list');
});

test('刚压缩完：菜单空列表照原型文案，只有「算了」', async ({ page, request }) => {
  await openKind(page);
  await sendAndWait(page, request, '第一句');
  const compact = await apiPost(request, fixture, { method: 'compact' });
  expect(compact.ok()).toBeTruthy();
  await expect
    .poll(async () => (await events(request)).filter((event) => event.type === 'compaction/end').length, {
      timeout: 30_000,
    })
    .toBeGreaterThan(0);
  await openMenu(page);
  await expect(menu(page).locator('[data-kind="rewind-row"]')).toHaveCount(0);
  await expect(menu(page)).toContainText('最后一次压缩之后，你还没说过话。');
  await expect(menu(page).locator('[data-kind="rewind-cancel"]')).toBeVisible();
});
