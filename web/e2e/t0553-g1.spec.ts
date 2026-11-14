import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T055.3：G1「已带进输入框」+ 审计备注 1（卡块在用户文字上方）。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.locator('[data-kind="composer-input"]').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('G1：要求修改放进输入框后卡上写「已带进输入框」，刷新还在，删掉/发送后消失', async ({
  page,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t054: 'update' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card).toBeVisible({ timeout: 60_000 });
    await expect(card.locator('[data-kind="task-modify-requirement"]')).toBeVisible();
    await card.locator('[data-kind="task-modify-requirement"]').click();

    const chip = page.locator('[data-kind="composer"] [data-kind="result-card-chip"]');
    await expect(chip).toBeVisible({ timeout: 30_000 });
    await expect(card.locator('[data-kind="result-card-in-composer"]')).toHaveText('已带进输入框');

    // 刷新从草稿恢复后仍在。
    await page.reload();
    await expect(chip).toBeVisible({ timeout: 30_000 });
    await expect(
      page.locator('[data-kind="task-result"]').last().locator('[data-kind="result-card-in-composer"]'),
    ).toHaveText('已带进输入框');

    // 删掉卡：状态消失。
    await chip.getByRole('button', { name: '移除结果卡' }).click();
    await expect(chip).toHaveCount(0);
    await expect(
      page.locator('[data-kind="task-result"]').last().locator('[data-kind="result-card-in-composer"]'),
    ).toHaveCount(0);

    // 再点要求修改并发送：卡随消息进气泡，输入框里的状态消失；卡块在用户文字上方（DOM 顺序）。
    await page
      .locator('[data-kind="task-result"]')
      .last()
      .locator('[data-kind="task-modify-requirement"]')
      .click();
    await expect(chip).toBeVisible();
    const input = page.locator('[data-kind="composer-input"]');
    await input.fill('把要求改成按 url 判重');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(chip).toHaveCount(0, { timeout: 30_000 });
    await expect(
      page.locator('[data-kind="task-result"]').last().locator('[data-kind="result-card-in-composer"]'),
    ).toHaveCount(0);

    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row).toBeVisible({ timeout: 30_000 });
    const chipInBubble = row.locator('[data-kind="result-card-chip"]');
    await expect(chipInBubble).toHaveCount(1, { timeout: 30_000 });
    const bubble = row.locator('[data-kind="user-bubble"]').filter({ hasText: '把要求改成按 url 判重' });
    await expect(bubble).toHaveCount(1);
    const chipBox = await chipInBubble.boundingBox();
    const bubbleBox = await bubble.boundingBox();
    expect(chipBox).not.toBeNull();
    expect(bubbleBox).not.toBeNull();
    // 卡块在用户文字上方（DOM 顺序 + 画出来 y 更小）。
    expect(chipBox?.y ?? 0).toBeLessThan(bubbleBox?.y ?? 0);
    const domOrder = await row.evaluate((element, text) => {
      const chip = element.querySelector('[data-kind="result-card-chip"]');
      const bubbles = Array.from(element.querySelectorAll('[data-kind="user-bubble"]'));
      const target = bubbles.find((node) => (node.textContent ?? '').includes(text));
      if (!chip || !target) return false;
      return Boolean(
        chip.compareDocumentPosition(target) & Node.DOCUMENT_POSITION_FOLLOWING,
      );
    }, '把要求改成按 url 判重');
    expect(domOrder).toBe(true);
  } finally {
    await fixture.close();
  }
});
