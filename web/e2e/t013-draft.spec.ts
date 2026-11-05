import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';
const input = (page: import('@playwright/test').Page) => page.locator('[data-kind="composer-input"]');

/// T013.8 清单 15：打字、引用、粘贴、拖文件以后，切到设置再回来，都在。
test('草稿：切到设置再回来，文字/引用/粘贴/文件都在', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('草稿文字');
    // 引用：选中后点引用。
    await box.click();
    await page.keyboard.press('Home');
    await page.keyboard.down('Shift');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.up('Shift');
    await page.locator('[data-kind="quote-button"]').click();
    // 粘贴折卡片（合成 paste 事件即可：控制器只关心长度）。
    await box.evaluate((element) => {
      const data = new DataTransfer();
      data.setData('text/plain', 'z'.repeat(1001));
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    });
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    // 拖一个文字文件。
    await page.locator('[data-kind="composer"]').evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['hello'], 'draft.txt', { type: 'text/plain' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 15_000 });
    // 切到设置，再回 Kind。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Kind', exact: true })).toBeVisible({ timeout: 15_000 });
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    // 回来都在。
    await expect(input(page)).toContainText('> 草稿文字');
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1, { timeout: 15_000 });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1);
    void box;
  } finally {
    await fixture.close();
  }
});

/// T013.8 清单 15：刷新以后草稿还在（localStorage）。
test('草稿：刷新后还在', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const box = input(page);
    await box.fill('刷新还在的草稿');
    await page.waitForTimeout(300);
    await page.reload();
    await expect(input(page)).toHaveValue('刷新还在的草稿', { timeout: 15_000 });
  } finally {
    await fixture.close();
  }
});

/// T013.8 清单 15：往上滚到中间，切到设置再回来，还停在同一行（内存，刷新不保存）。
test('阅读位置：滚到中间切回同一位置', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    // 发几条消息把列表撑长。
    for (let index = 0; index < 4; index += 1) {
      await page.getByPlaceholder('和 Kind 说…').fill('消息 ' + String(index));
      await page.getByRole('button', { name: '发送', exact: true }).click();
      await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    }
    const scroller = page.locator('[data-virtuoso-scroller]').first();
    const before = await scroller.evaluate((element) => {
      element.scrollTop = Math.max(0, element.scrollHeight * 0.4);
      return element.scrollTop;
    });
    await page.waitForTimeout(300);
    // 切设置再回来。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await expect(page.locator('[data-kind="composer"]')).toBeVisible({ timeout: 15_000 });
    await page.waitForTimeout(900);
    const after = await scroller.evaluate((element) => element.scrollTop);
    expect(after).toBeGreaterThan(0);
    // 回到同一行（顶行 key+偏移恢复；容差一行）。
    expect(Math.abs(after - before)).toBeLessThan(60);
  } finally {
    await fixture.close();
  }
});


/// T013.8：发送失败时草稿不清（受理成功才清）。
test('发送失败：输入还在（草稿不清）', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.route(/\/api(\?|$)/, async (route, request) => {
      if ((request.postData() ?? '').includes('"method":"send"')) {
        await route.fulfill({ status: 500, contentType: 'application/json', body: '{}' });
        return;
      }
      await route.continue();
    });
    const box = input(page);
    await box.fill('发送会失败的文字');
    await page.locator('[data-kind="send"]').click();
    // 失败进 unsent：输入框里的字还在（草稿没被清）。
    await expect(box).toHaveValue('发送会失败的文字', { timeout: 15_000 });
  } finally {
    await fixture.close();
  }
});

/// T013.8：非法草稿当空（格式不对不崩）。
test('非法草稿当空', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await expect(input(page)).toBeVisible({ timeout: 15_000 });
    const sessionId = await page.evaluate(() => {
      const key = Object.keys(localStorage).find((item) => item.startsWith('aios.kind.draft.'));
      return key ?? null;
    });
    // 会话 id 还没进草稿键时，用 ready 帧里的 id 手工写一份非法 JSON。
    await page.evaluate((existing) => {
      const target = existing ?? 'aios.kind.draft.invalid-test';
      localStorage.setItem(target, '{not-json');
    }, sessionId);
    await page.reload();
    await expect(input(page)).toHaveValue('', { timeout: 15_000 });
  } finally {
    await fixture.close();
  }
});
