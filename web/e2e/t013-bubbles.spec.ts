import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const send = async (page: import('@playwright/test').Page, text: string) => {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
};

/// T013.6 清单 12：我的话在右边气泡里；Kind 的回答不加气泡；停在消息上出复制按钮。
test('气泡：我的话在右侧气泡、Kind 回答没有气泡、复制原文', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await send(page, '你好呀');
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 30_000 });
    // 我的话：右侧气泡。
    const bubble = page.locator('[data-kind="user-bubble"]').first();
    await expect(bubble).toBeVisible();
    await expect(bubble).toContainText('你好呀');
    // 回答没有气泡。
    await expect(page.locator('[data-kind="assistant-bubble"]')).toHaveCount(0);
    // 位置：气泡在回答的右侧（回答左对齐）。
    const answer = page.getByText('（T007 e2e 假回放）').first();
    const bubbleBox = await bubble.boundingBox();
    const answerBox = await answer.boundingBox();
    expect(bubbleBox && answerBox ? bubbleBox.x > answerBox.x : false).toBe(true);
    // 停在消息上出复制按钮；点一下复制原文。
    await bubble.hover();
    const copy = bubble.locator('[data-kind="copy"]').first();
    await expect(copy).toBeVisible();
    await copy.click();
    await expect(copy).toHaveAttribute('aria-label', '已复制');
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toBe('你好呀');
  } finally {
    await fixture.close();
  }
});

/// T013.6 故事 4：Kind 回答 hover 出复制，复制的是回答原文。
test('回答复制：停在 Kind 回答上复制原文', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await send(page, '你好呀');
    const answer = page.locator('[data-kind="answer"]').first();
    await expect(answer).toContainText('（T007 e2e 假回放）', { timeout: 30_000 });
    await answer.hover();
    const copy = answer.locator('[data-kind="copy"]').first();
    await expect(copy).toBeVisible();
    await copy.click();
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toBe('（T007 e2e 假回放）');
  } finally {
    await fixture.close();
  }
});

/// T013.6 清单 16：占用/缓存命中不到 1% 显示「不到 1%」；0 显示 0%。
test('百分比：不到 1% 与 0%', async ({ page }) => {
  const empty = await startKernelFixture();
  try {
    await page.goto(urlOf(empty));
    await expect(page.locator('[data-kind="context-meter"]')).toContainText('0%', { timeout: 15_000 });
  } finally {
    await empty.close();
  }
  const fixture = await startKernelFixture({ tinyPercent: true });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '你好');
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 30_000 });
    // 缓存命中 1/1000 → 不到 1%；占用在 128k 窗口里也远不到 1%。
    await expect(page.locator('[data-kind="stats-usage"]')).toContainText('缓存命中 不到 1%', { timeout: 15_000 });
    await expect(page.locator('[data-kind="context-meter"]')).toContainText('不到 1%', { timeout: 15_000 });
  } finally {
    await fixture.close();
  }
});

/// T013.6 清单 12：代码块右上角复制，复制出来是原文。
test('代码块右上角复制：复制原文', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const fixture = await startKernelFixture({ codeBlock: true });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '给我一段代码');
    await expect(page.getByText('const answer = 42;')).toBeVisible({ timeout: 30_000 });
    const copy = page.locator('[data-kind="code-copy"]').first();
    await expect(copy).toBeVisible();
    await copy.click();
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toContain('const answer = 42;');
    expect(copied.startsWith('const answer')).toBe(true);
  } finally {
    await fixture.close();
  }
});

/// T013.6 清单 16：设置入口在侧栏左下角。
test('设置入口在侧栏左下角', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const footer = page.locator('[data-kind="sidebar-footer"]');
    await expect(footer).toBeVisible();
    await expect(footer.getByRole('button', { name: '设置', exact: true })).toBeVisible();
    // 侧栏内容区（上半部分）没有设置入口。
    await expect(page.locator('[data-kind="sidebar-content"]').getByRole('button', { name: '设置', exact: true })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

/// T013.6 清单 16 + C10 守护：空会话没有停止键、发送按钮是灰的。
test('C10：空会话没有停止键、发送灰', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0);
    const sendButton = page.locator('[data-kind="send"]');
    await expect(sendButton).toHaveAttribute('aria-disabled', 'true');
  } finally {
    await fixture.close();
  }
});


/// T013 故事 6：气泡里开头的 `> ` 行画成引用块（照 DSH QuotedUserText 的样式）。
test('气泡引用块：> 开头的行渲染成引用块', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = page.locator('[data-kind="composer-input"]');
    await box.fill('> 引用第一行\n> 引用第二行\n普通的一句话');
    await page.locator('[data-kind="send"]').click();
    const bubble = page.locator('[data-kind="user-bubble"]').first();
    await expect(bubble).toBeVisible({ timeout: 15_000 });
    const quote = bubble.locator('[data-kind="quote-block"]');
    await expect(quote).toHaveCount(1);
    await expect(quote).toContainText('引用第一行');
    await expect(quote).toContainText('引用第二行');
    await expect(bubble).toContainText('普通的一句话');
  } finally {
    await fixture.close();
  }
});

/// T013 F1（Claude fix-now）：发出去以后，气泡上面是一排卡片，正文只放打的字。
/// 照原型 app-v14 TurnRow.tsx 的 UserBubble：粘贴卡（行数/字数，点开全文）、文件卡（名字/大小，点开走 attachment-get）。
test('气泡卡片：文字+1001字粘贴+一个文件 → 一张粘贴卡、一张文件卡，正文不含粘贴原文，刷新后不变', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    // 一个文件：拖进来变成卡片。
    await page.locator('[data-kind="composer"]').evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['文件里的内容：BUBBLE-FILE' + String.fromCharCode(10)], 'bubble.txt', { type: 'text/plain' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 20_000 });
    // 一段 1001 字符以上的粘贴：折成一张卡片。
    const pasted = 'BUBBLE-PASTE-START' + String.fromCharCode(10) + 'y'.repeat(1000);
    await page.locator('[data-kind="composer-input"]').evaluate((element, value) => {
      const data = new DataTransfer();
      data.setData('text/plain', value);
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    }, pasted);
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await page.locator('[data-kind="composer-input"]').fill('看这个粘贴和这个文件');
    await page.locator('[data-kind="send"]').click();

    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row).toBeVisible({ timeout: 15_000 });
    // 气泡上面一排卡片：一张粘贴卡、一张文件卡。
    await expect(row.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(row.locator('[data-kind="file-card"]')).toHaveCount(1);
    await expect(row.locator('[data-kind="file-card"]')).toContainText('bubble.txt');
    // 正文只放打的字，粘贴原文不进正文。
    const bubble = row.locator('[data-kind="user-bubble"]');
    await expect(bubble).toContainText('看这个粘贴和这个文件');
    await expect(bubble).not.toContainText('BUBBLE-PASTE-START');
    await expect(bubble).not.toContainText('yyyyyyyyyyyyyyyyyyyy');
    // 点开看全文：粘贴卡显示粘贴原文；文件卡走 attachment-get 显示文件内容。
    await row.locator('[data-kind="paste-open"]').click();
    await expect(row.locator('[data-kind="paste-preview"]')).toContainText('BUBBLE-PASTE-START');
    await row.locator('[data-kind="file-open"]').click();
    await expect(row.locator('[data-kind="file-preview"]')).toContainText('BUBBLE-FILE');

    // 刷新后不变（回放和实时同源：都从 user/message 的块派生）。
    await page.reload();
    const reloaded = page.locator('[data-kind="user-message-row"]').last();
    await expect(reloaded).toBeVisible({ timeout: 30_000 });
    await expect(reloaded.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(reloaded.locator('[data-kind="file-card"]')).toHaveCount(1);
    await expect(reloaded.locator('[data-kind="user-bubble"]')).toContainText('看这个粘贴和这个文件');
    await expect(reloaded.locator('[data-kind="user-bubble"]')).not.toContainText('BUBBLE-PASTE-START');
  } finally {
    await fixture.close();
  }
});

