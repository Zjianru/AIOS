import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T013 验收 G1：独立的「已思考 N 秒」和「已完成，用时 X」重复；
/// 完成轮只保留工作过程，展开后能看到思考内容，且不再出现独立「已思考」行。
test('思考（验收 G1）：完成轮没有独立「已思考」，展开工作过程看到思考全文', async ({ page }) => {
  const fixture = await startKernelFixture({ realTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('请先用 read 工具读一下 hello.txt，再告诉我文件里写了什么。');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    // 等这一轮结束、完成轮的工作过程行出来，再看排布。
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    const process = page.locator('[data-kind="process"]').first();
    await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 20_000 });

    // G1：独立「已思考 N 秒」不再出现（和「已完成，用时 X」重复）。
    await expect(page.locator('[data-kind="thinking"]')).toHaveCount(0);
    await expect(page.getByText(/已思考 \d+ 秒/)).toHaveCount(0);

    // 展开工作过程：组 → 思考行，预览就是日志里那段思考的第一行。
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();
    const think = process.locator('[data-kind="think-row"]').first();
    await expect(think).toBeVisible({ timeout: 15_000 });
    await expect(think).toContainText('Simple task. Let me read the file.');

    // 再展开思考行：思考全文在工作过程里，不是独立块。
    await think.locator('button').first().click();
    await expect(process.getByText('No skill needed for this trivial task.')).toBeVisible();
    await expect(page.getByText(/已思考 \d+ 秒/)).toHaveCount(0);

    // T015 fix A：展开的思考内容比正文小一号（12px / 18px，次级文字色），不是正文的 15px。
    const thinkMd = think.locator('.md').first();
    const style = await thinkMd.evaluate((element) => {
      const computed = getComputedStyle(element);
      return { fontSize: computed.fontSize, lineHeight: computed.lineHeight, color: computed.color };
    });
    expect(style.fontSize).toBe('12px');
    expect(style.lineHeight).toBe('18px');
    expect(style.color).not.toBe('rgb(237, 237, 242)'); // --ink（正文色）
    // 折叠行本身的高度/图标不变：还是 h-6。
    expect(
      await think.locator('button').first().evaluate((element) => element.getBoundingClientRect().height),
    ).toBe(24);
  } finally {
    await fixture.close();
  }
});
