import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T013 验收 F2：完成轮的工作过程行在回答上面（照原型 app-v14 a5d6f58 的 TurnRow 顺序：
/// 过程 → 问题卡 → 回答）。原来完成轮的过程行渲染在回答下面。
test('完成轮：工作过程在回答上面', async ({ page }) => {
  const fixture = await startKernelFixture({ usageTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    // 等这一轮真的结束（停止键消失），才是完成轮的排布。
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    const process = page.locator('[data-kind="process"]').last();
    const answer = page.locator('[data-kind="answer"]').last();
    const processBox = await process.boundingBox();
    const answerBox = await answer.boundingBox();
    expect(processBox && answerBox ? processBox.y < answerBox.y : false).toBe(true);
  } finally {
    await fixture.close();
  }
});