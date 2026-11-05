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
    // 1000 个字符不折：走真实输入（合成 ClipboardEvent 不会触发浏览器的默认插入）。
    await box.fill('y'.repeat(1000));
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
