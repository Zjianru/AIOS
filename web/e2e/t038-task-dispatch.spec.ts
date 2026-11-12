import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T038.1：Kind 派任务后立刻出「进行中」卡，点得进任务对话；任务汇报 relay 到达后
 * 同一张卡更新成有结果/卡住，一张任务只留一张卡（跨轮按 task id 归并）。
 */

const TITLE_A = '任务甲：查判重规则';
const TITLE_B = '任务乙：查重复链接';
const TITLE_C = '任务丙：查超时设置';
const TITLE_D = '任务丁：核对版本号';

const urlOf = (fixture: KernelFixture) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function sendKind(page: import('@playwright/test').Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('派任务后立刻出进行中卡，汇报后同一张卡更新、不重复', async ({ page, request }) => {
  test.setTimeout(150_000);
  // 四个任务的假模型流程（A/B 跑长流、C 报 ready、D 报 blocked），和 t029 同一套夹具。
  const fixture = await startKernelFixture({ t029Restart: 'main' });
  try {
    await apiPost(request, fixture, { method: 'settings-update', seats: 8 });
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我派四个任务');

    // 汇报 relay 之前：进行中的派发卡已经出现（A/B 的 ThinAgent 还在跑长流）。
    const dispatched = page.locator('[data-kind="task-result"][data-state="dispatched"]');
    await expect(dispatched.filter({ hasText: TITLE_A })).toHaveCount(1, { timeout: 30_000 });
    await expect(dispatched.filter({ hasText: TITLE_B })).toHaveCount(1, { timeout: 30_000 });
    await expect(
      dispatched.filter({ hasText: TITLE_A }).locator('[data-kind="task-state"]'),
    ).toContainText('进行中');
    await expect(
      dispatched.filter({ hasText: TITLE_A }).getByRole('button', { name: '打开任务对话' }),
    ).toBeVisible();
    // 跑中也能点进任务对话。
    await dispatched
      .filter({ hasText: TITLE_A })
      .getByRole('button', { name: '打开任务对话' })
      .click();
    await expect(page).toHaveURL(/#taskchat:/);
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 20_000 });
    await page.goto(urlOf(fixture));

    // 汇报 relay 到达后：同一张卡变成有结果/卡住；四张任务各一张卡，没有第二张。
    const cards = page.locator('[data-kind="task-result"]');
    await expect(
      page.locator('[data-kind="task-result"][data-state="ready"]').filter({ hasText: TITLE_C }),
    ).toHaveCount(1, { timeout: 60_000 });
    await expect(
      page
        .locator('[data-kind="task-result"][data-state="blocked"]')
        .filter({ hasText: TITLE_D }),
    ).toHaveCount(1, { timeout: 60_000 });
    await expect(cards).toHaveCount(4);
    for (const title of [TITLE_A, TITLE_B, TITLE_C, TITLE_D]) {
      await expect(cards.filter({ hasText: title })).toHaveCount(1);
    }
    // A/B 还没交报告：仍是进行中（relay 没把它们改成别的状态）。
    await expect(dispatched.filter({ hasText: TITLE_A })).toHaveCount(1);
    await expect(dispatched.filter({ hasText: TITLE_B })).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});
