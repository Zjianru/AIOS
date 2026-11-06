import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';
import { answeredAfterRead, runProbe, sessionLog } from './dod-log';

/**
 * DoD2 断言的守护（T013 F2，不带 key，走回放夹具）：
 * - 标记只出现在你自己的消息里时，最后一个 [data-kind="answer"] 里没有它；
 *   （旧写法 page.getByText(标记).last() 会命中你自己的消息——这就是 F2 的自命中。）
 * - 日志里没有 read 时，DoD2 ①② 用的同一个 probe 必须报「没有回答之前 read」。
 * 这条不代替真 key 的四条，只保证断言本身不再自己命中。
 */

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

test('DoD2 断言：标记只在你的消息里时，回答里没有它、日志里没有 read', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const marker = 'DOD13-SELFHIT';
    const typed = marker + '：这句话里有标记，但 Kind 还没读过任何文件';
    await page.locator('[data-kind="composer-input"]').fill(typed);
    await page.locator('[data-kind="send"]').click();
    // 回放夹具的回答是固定的一句，里面没有标记。
    const answer = page.locator('[data-kind="answer"]').last();
    await expect(answer).toContainText('（T007 e2e 假回放）', { timeout: 30_000 });
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 30_000 });
    // 新写法：只认回答。
    await expect(answer).not.toContainText(marker);
    // 旧写法为什么不能用：它命中的是你自己的消息。
    await expect(page.getByText(marker, { exact: false }).last()).toBeVisible();
    // 日志事实：没有 read，probe 也不能说「回答之前 read 过」。
    const { probe } = runProbe(sessionLog(fixture.dataDir), marker, 'notes.md');
    expect(answeredAfterRead(probe)).toBe(false);
    expect(probe.readSeqs).toEqual([]);
  } finally {
    await fixture.close();
  }
});
