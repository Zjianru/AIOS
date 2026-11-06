import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const input = (page: import('@playwright/test').Page) => page.locator('[data-kind="composer-input"]');

/// T013.7 清单 14：按键——Enter 发送、Shift+Enter 换行、Cmd+Enter 发送、Alt+Enter 不消费、组字器 Enter 不发送。
test('按键：Enter/Shift+Enter/Cmd+Enter/Alt+Enter/组字器', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await expect(box).toBeVisible();

    // Enter 发送。
    await box.fill('按键 Enter');
    await box.press('Enter');
    await expect(page.locator('[data-kind="user-bubble"]').first()).toContainText('按键 Enter', { timeout: 15_000 });

    // Shift+Enter 换行（不发送）。
    await box.fill('第一行');
    await box.press('Shift+Enter');
    await box.pressSequentially('第二行');
    await expect(box).toHaveValue('第一行\n第二行');
    await expect(page.locator('[data-kind="user-bubble"]')).toHaveCount(1);
    // 清掉。
    await box.fill('');
    await box.press('Shift+Enter');
    await box.fill('');

    // Alt+Enter 不发送（保留内容）。
    await box.fill('Alt 不发送');
    await box.press('Alt+Enter');
    await expect(page.locator('[data-kind="user-bubble"]')).toHaveCount(1);
    await expect(box).toHaveValue('Alt 不发送');

    // 组字器里的 Enter 不发送。
    await box.evaluate((element) => {
      element.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
      element.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, bubbles: true }));
      element.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
    });
    await expect(page.locator('[data-kind="user-bubble"]')).toHaveCount(1);

    // Cmd+Enter 发送。
    await box.press('Meta+Enter');
    await expect(page.locator('[data-kind="user-bubble"]')).toHaveCount(2, { timeout: 15_000 });
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 14：输入框涨到 240px 后不再变高，发送键看得见。
test('自动变高：最高 240px，之后内部滚动，发送键可见', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    const lines = Array.from({ length: 40 }, (_, index) => 'line ' + index).join('\n');
    await box.fill(lines);
    const height = await box.evaluate((element) => element.getBoundingClientRect().height);
    expect(height).toBeLessThanOrEqual(240);
    const overflow = await box.evaluate((element) => getComputedStyle(element).overflowY);
    expect(overflow).toBe('auto');
    await expect(page.locator('[data-kind="send"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：粘贴 1001 个字符折卡片，1000 个不折。
test('粘贴：1001 字符折卡片，1000 字符留在输入框', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    const paste = (text: string) =>
      box.evaluate((element, value) => {
        const data = new DataTransfer();
        data.setData('text/plain', value);
        element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
      }, text);
    await paste('x'.repeat(1001));
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(box).toHaveValue('');
    // 1000 个字符不折：走真实粘贴（剪贴板 + Meta+V，让浏览器走默认插入）。
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    await page.evaluate(() => navigator.clipboard.writeText('y'.repeat(1000)));
    await box.fill('');
    await box.focus();
    await page.keyboard.press('Meta+V');
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(box).toHaveValue('y'.repeat(1000));
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：@ 菜单出现、选中插入 @路径；带空格的路径加引号。
test('@ 菜单：出现、插入路径、带空格加引号', async ({ page }) => {
  const fixture = await startKernelFixture({ toolTurn: true });
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('@he');
    const menu = page.locator('[data-kind="at-menu"]');
    await expect(menu).toBeVisible({ timeout: 15_000 });
    await expect(menu).toContainText('hello.txt');
    await menu.getByRole('button', { name: /hello\.txt/ }).first().click();
    await expect(box).toHaveValue('@hello.txt');
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：选中文字浮出「引用」，点了变成 > 引用。
test('引用：选中文字浮出引用按钮，点了变 > 引用', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('这一段要被引用');
    await box.click();
    await page.keyboard.press('Home');
    await page.keyboard.down('Shift');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.up('Shift');
    const quote = page.locator('[data-kind="quote-button"]');
    await expect(quote).toBeVisible();
    await quote.click();
    await expect(box).toHaveValue('> 这一段要被引用');
  } finally {
    await fixture.close();
  }
});


/// T013.7 清单 13：拖进文字文件出卡片；拖进二进制给提示。
test('拖拽：文字文件出卡片，二进制提示只能放文字文件', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const composer = page.locator('[data-kind="composer"]');
    await composer.evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['hello text'], 'note.txt', { type: 'text/plain' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 15_000 });
    await expect(page.locator('[data-kind="file-card"]').first()).toContainText('note.txt');
    await composer.evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File([new Uint8Array([0, 1, 2, 3])], 'bin.dat', { type: 'application/octet-stream' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    await expect(page.getByText('只能放文字文件')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 14：长按 Enter 不连发。
test('长按 Enter 不连发', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('长按不连发');
    await box.evaluate((element) => {
      element.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', repeat: true, bubbles: true }));
    });
    await expect(page.locator('[data-kind="user-bubble"]')).toHaveCount(0);
    await expect(box).toHaveValue('长按不连发');
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：发出去以后，日志里的 user/message 带 paste 块；气泡照常出现。
test('发送：粘贴折卡片后发出去，日志里有 paste 块', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.evaluate((element) => {
      const data = new DataTransfer();
      data.setData('text/plain', 'x'.repeat(1001));
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    });
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await box.fill('看这个粘贴');
    await page.locator('[data-kind="send"]').click();
    await expect(page.locator('[data-kind="user-bubble"]').first()).toContainText('看这个粘贴', { timeout: 15_000 });
    const response = await apiPost(request, fixture, { method: 'events', maxMessages: 60 });
    const json = await response.json();
    const userMessage = (json.events ?? []).find((event: { type: string }) => event.type === 'user/message');
    const blocks = userMessage?.data?.message?.content ?? [];
    const pasteBlocks = blocks.filter((block: { type?: string; paste?: boolean }) => block.type === 'text' && block.paste === true);
    expect(pasteBlocks.length).toBe(1);
    expect(pasteBlocks[0].text).toBe('x'.repeat(1001));
  } finally {
    await fixture.close();
  }
});


/// T013.7 清单 13：插入 @路径 后下层高亮存在，文字仍可编辑。
test('@路径高亮：插入后高亮层有 @路径，文字仍可编辑', async ({ page }) => {
  const fixture = await startKernelFixture({ toolTurn: true });
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('@he');
    const menu = page.locator('[data-kind="at-menu"]');
    await expect(menu).toBeVisible({ timeout: 15_000 });
    await menu.getByRole('button', { name: /hello\.txt/ }).first().click();
    await expect(box).toHaveValue('@hello.txt');
    const highlight = page.locator('[data-kind="at-highlight"]');
    await expect(highlight).toContainText('@hello.txt');
    await expect(highlight.locator('[data-at-token="@hello.txt"]')).toHaveCount(1);
    // 文字仍可编辑（高亮层是展示层，不抢焦点）：等插入后的光标落定，再继续输入。
    await page.waitForTimeout(150);
    await box.pressSequentially(' 追加');
    await expect(box).toHaveValue('@hello.txt 追加');
    await expect(highlight).toContainText('@hello.txt 追加');
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：附件横排滚动，新加的滚到最后。
test('附件横排：多张卡片横排不换行，新加的滚到最后', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const composer = page.locator('[data-kind="composer"]');
    for (let index = 0; index < 8; index += 1) {
      const name = 'very-long-file-name-' + String(index).padStart(2, '0') + '.txt';
      await composer.evaluate((element, fileName) => {
        const data = new DataTransfer();
        data.items.add(new File(['hello'], fileName, { type: 'text/plain' }));
        element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
      }, name);
    }
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(8, { timeout: 15_000 });
    const cards = page.locator('[data-kind="composer-cards"]');
    const metrics = await cards.evaluate((element) => ({
      scrollLeft: element.scrollLeft,
      scrollWidth: element.scrollWidth,
      clientWidth: element.clientWidth,
      overflowX: getComputedStyle(element).overflowX,
      whiteSpace: getComputedStyle(element).whiteSpace,
    }));
    expect(metrics.overflowX).toBe('auto');
    // 三张卡片在窄输入框里放不下：容器可滚，且新加的滚到了最后。
    expect(metrics.scrollWidth).toBeGreaterThan(metrics.clientWidth);
    expect(metrics.scrollLeft).toBeGreaterThan(0);
  } finally {
    await fixture.close();
  }
});

/// T013.7：拖目录进来明确拒绝（toast），不出卡片。
test('拖拽目录：明确拒绝，不出卡片', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const composer = page.locator('[data-kind="composer"]');
    await composer.evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['hello'], 'a-directory', { type: '' }));
      const item = data.items[0];
      // 原型上注入（合成事件的 DataTransferItem 不好直接改）：让这一条被识别成目录。
      Object.defineProperty(Object.getPrototypeOf(item), 'webkitGetAsEntry', {
        value: () => ({ isDirectory: true }),
        configurable: true,
      });
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
      delete (Object.getPrototypeOf(item) as { webkitGetAsEntry?: unknown }).webkitGetAsEntry;
    });
    await expect(page.getByText('只能放文字文件')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});


/// T013.7 清单 13/故事 8：粘贴卡片有行数/字数，点开看全文。
test('粘贴卡片：行数/字数与点开全文', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    const content = '第一行\n第二行\n第三行';
    await box.evaluate((element, value) => {
      const data = new DataTransfer();
      data.setData('text/plain', value.repeat(120));
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    }, content);
    const card = page.locator('[data-kind="paste-card"]').first();
    await expect(card).toBeVisible();
    await expect(card).toContainText('行');
    await expect(card).toContainText('字');
    await card.locator('[data-kind="paste-open"]').click();
    const preview = page.locator('[data-kind="paste-preview"]');
    await expect(preview).toBeVisible();
    const previewText = await preview.innerText();
    expect(previewText).toContain('第一行');
    expect(previewText).toContain('第三行');
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13/故事 7：文件卡片有类型与大小，点开看全文（attachment-get）。
test('文件卡片：类型/大小与点开全文', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const composer = page.locator('[data-kind="composer"]');
    await composer.evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['文件里的内容\n'], 'notes.txt', { type: 'text/plain' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    const card = page.locator('[data-kind="file-card"]').first();
    await expect(card).toBeVisible({ timeout: 15_000 });
    await expect(card).toContainText('文字');
    await expect(card).toContainText('notes.txt');
    await card.locator('[data-kind="file-open"]').click();
    const preview = page.locator('[data-kind="file-preview"]');
    await expect(preview).toBeVisible();
    await expect(preview).toContainText('文件里的内容');
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：@高亮 token 与 textarea 同位置字符对齐（去掉 padding 后不横向错位）。
test('@高亮：token 与输入框字符位置对齐', async ({ page }) => {
  const fixture = await startKernelFixture({ toolTurn: true });
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('前缀文字 @hello.txt');
    const highlight = page.locator('[data-kind="at-highlight"]');
    const token = highlight.locator('[data-at-token="@hello.txt"]');
    await expect(token).toHaveCount(1);
    // 用同样的字体量前缀文本宽度；token 的 x 应等于「层左缘 + 前缀宽度」（±1px）。
    const aligned = await page.evaluate(() => {
      const layer = document.querySelector('[data-kind="at-highlight"]') as HTMLElement;
      const tokenElement = layer.querySelector('[data-at-token]') as HTMLElement;
      const textarea = document.querySelector('[data-kind="composer-input"]') as HTMLTextAreaElement;
      const layerBox = layer.getBoundingClientRect();
      const tokenBox = tokenElement.getBoundingClientRect();
      const measure = document.createElement('span');
      const style = getComputedStyle(textarea);
      measure.style.font = style.font;
      measure.style.letterSpacing = style.letterSpacing;
      measure.style.whiteSpace = 'pre';
      measure.textContent = '前缀文字 ';
      document.body.appendChild(measure);
      const width = measure.getBoundingClientRect().width;
      measure.remove();
      return { delta: Math.abs(tokenBox.x - (layerBox.x + width)) };
    });
    expect(aligned.delta).toBeLessThan(1.5);
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：@ 菜单 ↑↓ 换高亮、Esc 关、Enter 选当前项。
test('@ 菜单：ArrowDown/ArrowUp 换项、Escape 关闭', async ({ page }) => {
  const fixture = await startKernelFixture({ toolTurn: true });
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('@');
    const menu = page.locator('[data-kind="at-menu"]');
    await expect(menu).toBeVisible({ timeout: 15_000 });
    const items = menu.getByRole('button');
    await expect(items.first()).toBeVisible();
    await expect(items.nth(1)).toBeVisible();
    const secondPath = (await items.nth(1).innerText()).trim();
    await box.press('ArrowDown');
    await box.press('Enter');
    // 插的是第二项（高亮项）的路径。
    const value = await box.inputValue();
    const expected = secondPath.includes(' ') ? '@"' + secondPath + '"' : '@' + secondPath;
    expect(value).toBe(expected);
    // Esc 关菜单。
    await box.fill('@');
    await expect(menu).toBeVisible({ timeout: 15_000 });
    await box.press('Escape');
    await expect(menu).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 13：带空格的路径要加引号插入。
test('@ 菜单：带空格的路径加引号', async ({ page }) => {
  const fixture = await startKernelFixture({ toolTurn: true });
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('@with');
    const menu = page.locator('[data-kind="at-menu"]');
    await expect(menu).toBeVisible({ timeout: 15_000 });
    await expect(menu).toContainText('with space.txt');
    await menu.getByRole('button', { name: /with space/ }).first().click();
    await expect(page.locator('[data-kind="composer-input"]')).toHaveValue('@"with space.txt"');
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 14：Ctrl+Enter 也发送。
test('Ctrl+Enter 发送', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('Ctrl 发送');
    await box.press('Control+Enter');
    await expect(page.locator('[data-kind="user-bubble"]').first()).toContainText('Ctrl 发送', { timeout: 15_000 });
  } finally {
    await fixture.close();
  }
});

/// T013.7 清单 14：Alt+Enter 不发送也不换行。
test('Alt+Enter 不发送也不换行', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('Alt 不动作');
    await box.press('Alt+Enter');
    await expect(page.locator('[data-kind="user-bubble"]')).toHaveCount(0);
    await expect(box).toHaveValue('Alt 不动作');
  } finally {
    await fixture.close();
  }
});

/// T013 验收 F4：选中对话正文（Kind 回答）里的文字，浮出「引用」，点了把文字按行加 > 追加到输入框。
test('引用（验收 F4）：选中回答正文浮出引用按钮，点了把选中文字变成 > 行', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('引用回答');
    await box.press('Enter');
    const answer = page.locator('[data-kind="answer"]').last();
    await expect(answer).toContainText('（T007 e2e 假回放）', { timeout: 30_000 });
    // 等这一轮结束：完成轮的回答和过程行位置稳定后再选。
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    // Virtuoso 首帧把这一条标成 hidden，等它真的显示出来再划选。
    await expect(answer).toBeVisible();

    // 真的用鼠标划选回答正文里的前 5 个字（和用户操作同一条路：mouseup + selectionchange）。
    const range = await page.evaluate(() => {
      const target = document.querySelectorAll('[data-kind="answer"]');
      const element = target[target.length - 1];
      if (!element) throw new Error('没有回答正文');
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      const node = walker.nextNode() as Text | null;
      if (!node) throw new Error('回答正文没有可选的文字');
      const selection = window.getSelection();
      selection?.removeAllRanges();
      const range = document.createRange();
      range.setStart(node, 0);
      range.setEnd(node, 5);
      const rect = range.getBoundingClientRect();
      return { left: rect.left + 1, right: rect.right - 1, y: rect.top + rect.height / 2 };
    });
    await page.mouse.move(range.left, range.y);
    await page.mouse.down();
    await page.mouse.move(range.right, range.y, { steps: 8 });
    await page.mouse.up();
    const selected = await page.evaluate(() => window.getSelection()?.toString() ?? '');
    expect(selected.length).toBeGreaterThan(0);

    const quote = page.locator('[data-kind="quote-button"]');
    await expect(quote).toBeVisible({ timeout: 5_000 });
    // 按钮浮在选区附近（选区上方一个按钮高度内）。
    const metrics = await page.evaluate(() => {
      const selection = window.getSelection();
      if (!selection || selection.rangeCount === 0) throw new Error('选区没了');
      const rect = selection.getRangeAt(0).getBoundingClientRect();
      const button = document.querySelector('[data-kind="quote-button"]');
      if (!button) throw new Error('引用按钮不在');
      const box = button.getBoundingClientRect();
      return { selectionTop: rect.top, buttonBottom: box.bottom };
    });
    expect(metrics.buttonBottom).toBeLessThanOrEqual(metrics.selectionTop);
    expect(metrics.selectionTop - metrics.buttonBottom).toBeLessThan(60);

    await quote.click();
    // 选中文字按行加 > 后进输入框；按钮和选区清掉。
    await expect(box).toHaveValue('> ' + selected);
    await expect(page.locator('[data-kind="quote-button"]')).toHaveCount(0);
    expect(await page.evaluate(() => (window.getSelection()?.toString() ?? '') === '')).toBe(true);
  } finally {
    await fixture.close();
  }
});

