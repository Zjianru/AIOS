import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T013 验收 F5：会话日志里的 reasoning 要渲染成「已思考 N 秒」，展开能看思考全文；
/// 位置在回答上面（在 ProcessView 之上）。
test('思考（验收 F5）：已思考 N 秒、展开看全文、在过程行上面', async ({ page }) => {
  const fixture = await startKernelFixture({ realTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('请先用 read 工具读一下 hello.txt，再告诉我文件里写了什么。');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    // 等这一轮结束（停止键消失）再看完成轮的排布。
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });

    const thinking = page.locator('[data-kind="thinking"]').first();
    await expect(thinking).toBeVisible({ timeout: 20_000 });
    await expect(thinking).toContainText(/已思考 \d+ 秒/);

    // 位置：思考行 → 工作过程 → 回答。
    const process = page.locator('[data-kind="process"]').first();
    const answer = page.locator('[data-kind="answer"]').first();
    const thinkingBox = await thinking.boundingBox();
    const processBox = await process.boundingBox();
    const answerBox = await answer.boundingBox();
    expect(thinkingBox && processBox && answerBox ? thinkingBox.y < processBox.y : false).toBe(true);
    expect(processBox && answerBox ? processBox.y < answerBox.y : false).toBe(true);

    // 展开：看到日志里那段思考全文（real.jsonl 第一轮）。
    await thinking.locator('button').first().click();
    await expect(page.getByText(/Simple task\. Let me read the file\./)).toBeVisible();
    await expect(page.getByText('No skill needed for this trivial task.')).toBeVisible();
    await thinking.locator('button').first().click();

    // 工作过程展开后也有思考步骤（process think）。
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();
    await expect(page.locator('[data-kind="think-row"]').first()).toBeVisible();
  } finally {
    await fixture.close();
  }
});
