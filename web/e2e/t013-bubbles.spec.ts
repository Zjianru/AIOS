import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

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
    // 文件卡要有类型和大小；日志的 file 块没有 mime，类型从文件名后缀推。
    await expect(row.locator('[data-kind="file-card"]')).toContainText('文字');
    await expect(row.locator('[data-kind="file-card"]')).toContainText('1KB');
    // 正文只放打的字，粘贴原文不进正文。
    const bubble = row.locator('[data-kind="user-bubble"]');
    await expect(bubble).toContainText('看这个粘贴和这个文件');
    await expect(bubble).not.toContainText('BUBBLE-PASTE-START');
    await expect(bubble).not.toContainText('yyyyyyyyyyyyyyyyyyyy');
    // 卡片在气泡上面（审查：原先只断言了左右，没断言上下）。
    const bubbleBox = await bubble.boundingBox();
    const pasteBox = await row.locator('[data-kind="paste-card"]').boundingBox();
    const fileBox = await row.locator('[data-kind="file-card"]').boundingBox();
    expect(bubbleBox && pasteBox && fileBox ? pasteBox.y < bubbleBox.y && fileBox.y < bubbleBox.y : false).toBe(true);
    // 点开看全文：粘贴卡显示粘贴原文；文件卡走 attachment-get 显示文件内容。
    await row.locator('[data-kind="paste-open"]').click();
    const pastePreview = row.locator('[data-kind="paste-preview"]');
    await expect(pastePreview).toContainText('BUBBLE-PASTE-START');
    // 预览内联在 user-message-row 里，不是 portal。
    await expect(pastePreview).toHaveCount(1);
    expect(await pastePreview.evaluate((element) => element.closest('[data-kind="user-message-row"]') !== null)).toBe(true);
    await row.locator('[data-kind="file-open"]').click();
    const filePreview = row.locator('[data-kind="file-preview"]');
    await expect(filePreview).toContainText('BUBBLE-FILE');
    await expect(filePreview).toHaveCount(1);
    expect(await filePreview.evaluate((element) => element.closest('[data-kind="user-message-row"]') !== null)).toBe(true);

    // 刷新后不变（回放和实时同源：都从 user/message 的块派生）。
    await page.reload();
    const reloaded = page.locator('[data-kind="user-message-row"]').last();
    await expect(reloaded).toBeVisible({ timeout: 30_000 });
    await expect(reloaded.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(reloaded.locator('[data-kind="file-card"]')).toHaveCount(1);
    await expect(reloaded.locator('[data-kind="user-bubble"]')).toContainText('看这个粘贴和这个文件');
    await expect(reloaded.locator('[data-kind="user-bubble"]')).not.toContainText('BUBBLE-PASTE-START');
    // 刷新后点开全文：粘贴卡和文件卡都还能走对路径（文件卡是 attachment-get）。
    await reloaded.locator('[data-kind="paste-open"]').click();
    const reloadedPaste = reloaded.locator('[data-kind="paste-preview"]');
    await expect(reloadedPaste).toHaveCount(1);
    await expect(reloadedPaste).toContainText('BUBBLE-PASTE-START');
    expect(await reloadedPaste.evaluate((element) => element.closest('[data-kind="user-message-row"]') !== null)).toBe(true);
    await reloaded.locator('[data-kind="file-open"]').click();
    const reloadedFile = reloaded.locator('[data-kind="file-preview"]');
    await expect(reloadedFile).toHaveCount(1);
    await expect(reloadedFile).toContainText('BUBBLE-FILE');
  } finally {
    await fixture.close();
  }
});


/// T013 F1（覆盖缺口）：只粘一段、输入框一个字都没打，也能发；卡片在、没有空气泡。
test('气泡卡片：只发一段粘贴（输入框为空）→ 一条我的消息、粘贴卡、没有空正文气泡', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const input = page.locator('[data-kind="composer-input"]');
    await input.evaluate((element) => {
      const data = new DataTransfer();
      data.setData('text/plain', 'PASTE-ONLY-START' + String.fromCharCode(10) + 'p'.repeat(1000));
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    });
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(input).toHaveValue('');
    await page.locator('[data-kind="send"]').click();

    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row).toBeVisible({ timeout: 15_000 });
    await expect(row.locator('[data-kind="user-cards"]')).toContainText('PASTE-ONLY-START');
    await expect(row.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(row.locator('[data-kind="paste-card"]')).toContainText('行');
    // 正文一个字都没有：不画空气泡。
    await expect(row.locator('[data-kind="user-bubble"]')).toHaveCount(0);
    // 这一轮照常跑完（不是只发了个空消息）。
    await expect(page.locator('[data-kind="answer"]').last()).toContainText('（T007 e2e 假回放）', {
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});

/// T013 F1（覆盖缺口）：只拖一个文件直接发、输入框为空；文件卡有类型和大小，没有空气泡。
test('气泡卡片：只发一个文件（输入框为空）→ 文件卡有类型和大小、没有空正文气泡', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.locator('[data-kind="composer"]').evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['FILE-ONLY-CONTENT' + String.fromCharCode(10)], 'only.md', { type: 'text/markdown' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    await expect(page.locator('[data-kind="composer"] [data-kind="file-card"]')).toHaveCount(1, { timeout: 20_000 });
    await expect(page.locator('[data-kind="composer-input"]')).toHaveValue('');
    await page.locator('[data-kind="send"]').click();

    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row).toBeVisible({ timeout: 15_000 });
    const card = row.locator('[data-kind="file-card"]');
    await expect(card).toHaveCount(1);
    await expect(card).toContainText('only.md');
    // 日志的 file 块没有 mime：类型从 .md 后缀推。
    await expect(card).toContainText('Markdown');
    await expect(card).toContainText('1KB');
    // 正文一个字都没有：不画空气泡；文件内容也不会漏进正文。
    await expect(row.locator('[data-kind="user-bubble"]')).toHaveCount(0);
    await expect(row).not.toContainText('FILE-ONLY-CONTENT');
    await expect(page.locator('[data-kind="answer"]').last()).toContainText('（T007 e2e 假回放）', {
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});

/// T013 F1：失败块的「再发一次」把你发过的粘贴一起再发一次（不是只剩打的字）。
test('再发一次：粘贴跟着重发（两次 user/message 都有 paste 块）', async ({ page, request }) => {
  const fixture = await startKernelFixture({ failure: 'model' });
  try {
    await page.goto(urlOf(fixture));
    await page.locator('[data-kind="composer-input"]').evaluate((element) => {
      const data = new DataTransfer();
      data.setData('text/plain', '重发也要带上的粘贴' + String.fromCharCode(10) + 'z'.repeat(1000));
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    });
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await send(page, '看这个粘贴');
    await expect(page.getByText('这一轮没做完', { exact: false })).toBeVisible({ timeout: 40_000 });
    await page.getByRole('button', { name: '再发一次' }).click();
    // 第二轮也会失败；两轮各有一条你自己的消息，第二条的块里要有那段粘贴。
    await expect
      .poll(
        async () => {
          const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
          const events = (await response.json()).events as {
            type: string;
            data?: { message?: { source?: { kind?: string }; content?: { type?: string; paste?: boolean }[] } };
          }[];
          const mine = events.filter(
            (event) => event.type === 'user/message' && event.data?.message?.source?.kind === 'user',
          );
          const second = mine[mine.length - 1];
          const blocks = second?.data?.message?.content ?? [];
          return mine.length >= 2 && blocks.some((block) => block.type === 'text' && block.paste === true) ? 1 : 0;
        },
        { timeout: 40_000 },
      )
      .toBe(1);
  } finally {
    await fixture.close();
  }
});


/// T013 验收 F1：真实浏览器里写剪贴板可能被拒（e2e 不授 clipboard 权限时就会）。
/// 写失败、execCommand 兜底也失败 → 按钮显示「复制失败」，不装作成功。
test('复制失败：写剪贴板被拒、兜底也失败 → 显示复制失败', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: () => Promise.reject(new DOMException('Write permission denied.', 'NotAllowedError')),
      },
    });
    document.execCommand = () => false;
  });
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await send(page, '你好呀');
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible({ timeout: 30_000 });
    const bubble = page.locator('[data-kind="user-bubble"]').first();
    await bubble.hover();
    const copy = bubble.locator('[data-kind="copy"]').first();
    await expect(copy).toBeVisible();
    await copy.click();
    await expect(copy).toHaveAttribute('aria-label', '复制失败');
    await expect(copy).toHaveAttribute('title', '复制失败');
  } finally {
    await fixture.close();
  }
});

/// T013 验收 F1：clipboard 被拒时的兜底——隐藏 textarea + execCommand('copy')，兜底成功仍算复制成功。
test('复制兜底：clipboard 被拒 → execCommand 复制到原文，显示已复制', async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as { copied: string }).copied = '';
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: () => Promise.reject(new DOMException('Write permission denied.', 'NotAllowedError')),
      },
    });
    document.execCommand = (command: string) => {
      if (command !== 'copy') return false;
      const area = document.activeElement;
      (window as unknown as { copied: string }).copied = area instanceof HTMLTextAreaElement ? area.value : '';
      return true;
    };
  });
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await send(page, '你好呀');
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible({ timeout: 30_000 });
    const bubble = page.locator('[data-kind="user-bubble"]').first();
    await bubble.hover();
    const copy = bubble.locator('[data-kind="copy"]').first();
    await copy.click();
    await expect(copy).toHaveAttribute('aria-label', '已复制');
    const copied = await page.evaluate(() => (window as unknown as { copied: string }).copied);
    expect(copied).toBe('你好呀');
  } finally {
    await fixture.close();
  }
});

/// T013 验收 F6：消息里的 @文件引用画成引用片（图标+路径，不附带内容），不能是 @"..." 生文本。
test('@文件引用（验收 F6）：气泡里是引用片、不是生文本', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await send(page, '看 @"my notes.txt"');
    const bubble = page.locator('[data-kind="user-bubble"]').first();
    await expect(bubble).toContainText('看', { timeout: 30_000 });
    const chip = bubble.locator('[data-kind="file-reference"]');
    await expect(chip).toHaveCount(1);
    await expect(chip).toContainText('my notes.txt');
    // 不出现 @" 生文本；引用片上只给路径，不自动读文件。
    await expect(bubble).not.toContainText('@"');
    await expect(chip).toHaveAttribute('data-ref-path', 'my notes.txt');
    await expect(chip).toHaveAttribute('title', 'my notes.txt');
  } finally {
    await fixture.close();
  }
});

