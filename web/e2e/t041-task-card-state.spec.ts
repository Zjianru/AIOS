import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T041：Kind 任务卡状态与任务对话同源（task-read）。
 * 用户路径：派任务 → 任务对话点「停下这一轮」（只中断当前轮、不发 relay）→ 回 Kind（不刷新）→ 卡状态变「等待中」。
 */

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';
// 卡片标题槽截到 20 字：按前缀过滤（整串仍给发送语用）。
const CARD_TEXT = '查一下新闻归档里重复链接';

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function cardOf(page: import('@playwright/test').Page) {
  return page.locator('[data-kind="task-result"]').filter({ hasText: CARD_TEXT }).first();
}

function taskStateOf(page: import('@playwright/test').Page) {
  return cardOf(page).locator('[data-kind="task-state"]');
}

test('派任务→任务对话点「停下这一轮」→回 Kind：卡状态变等待中（不刷新）', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(cardOf(page)).toHaveCount(1, { timeout: 30_000 });
    await expect(taskStateOf(page)).toContainText('进行中', { timeout: 30_000 });

    // 进任务对话，点「停下这一轮」。
    await cardOf(page).getByRole('button', { name: '打开任务对话' }).click();
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-state"]')).toContainText('执行中', {
      timeout: 30_000,
    });
    await page.keyboard.press('Escape');
    await expect(pane.getByText(/停下了这一轮/)).toBeVisible({ timeout: 20_000 });
    await pane.getByRole('button', { name: /回 Kind 对话/ }).click();
    await expect(page).toHaveURL(/#kind/);

    // 不刷新：Kind 卡状态跟着任务库变「等待中」（红：还停在「进行中」）。
    await expect(taskStateOf(page)).toContainText('等待中', { timeout: 20_000 });
    await expect(taskStateOf(page)).not.toContainText('进行中');
  } finally {
    await fixture.close();
  }
});

test('立即停：走任务菜单停，Kind 卡状态变已停止', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(cardOf(page)).toHaveCount(1, { timeout: 30_000 });
    await expect(taskStateOf(page)).toContainText('进行中', { timeout: 30_000 });

    // T051.6：走任务菜单「立即停」（真实用户路径），不再从根日志取 task id 调接口。
    await cardOf(page).getByRole('button', { name: '打开任务对话' }).click();
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('执行中', {
      timeout: 30_000,
    });
    await pane.locator('[data-kind="task-menu"]').click();
    await page
      .locator('[role="menu"]')
      .getByRole('menuitem', { name: /立即停/ })
      .click();
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('已停止', {
      timeout: 20_000,
    });

    // 回 Kind（不刷新页面）：卡状态跟着任务库变「已停止」。
    await pane.getByRole('button', { name: /回 Kind 对话/ }).click();
    await expect(page).toHaveURL(/#kind/);
    await expect(cardOf(page)).toHaveAttribute('data-state', 'stopped', { timeout: 20_000 });
    await expect(taskStateOf(page)).toContainText('已停止');
  } finally {
    await fixture.close();
  }
});

test('汇报后：Kind 卡状态仍是 relay 的结果状态（有结果/待你核）', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(cardOf(page)).toHaveCount(1, { timeout: 30_000 });
    await expect(cardOf(page)).toHaveAttribute('data-state', /ready|review/, { timeout: 60_000 });
    await expect(taskStateOf(page)).toContainText(/有结果|待你核/);
  } finally {
    await fixture.close();
  }
});

test('轮询不叠：切走再切回标签 3 次，之后 6 秒内 task-read ≤3 次（T051.6）', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  // 全局暂停时派一个写任务：任务没有 Attempt、卡停在非终态，轮询会一直跑，能稳定测「不叠」。
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    let reads = 0;
    await page.route('**/api', async (route) => {
      const postData = route.request().postData() ?? '';
      if (postData.includes('"method":"task-read"')) reads += 1;
      await route.continue();
    });
    const paused = await apiPost(request, fixture, { method: 'settings-update', paused: true });
    expect(paused.ok()).toBe(true);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('把 hello.txt 的判重改成按 url');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const card = page.locator('[data-kind="task-result"]').first();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card.locator('[data-kind="task-state"]')).toHaveText(/等待中|进行中/, {
      timeout: 30_000,
    });
    // 等首次拉取落定，再从零观察。
    await page.waitForTimeout(700);

    // 模拟切走/切回 3 次（改 visibilityState 后派发 visibilitychange）。
    const setVisible = async (visible: boolean) => {
      await page.evaluate((next) => {
        Object.defineProperty(document, 'visibilityState', {
          configurable: true,
          get: () => next,
        });
        document.dispatchEvent(new Event('visibilitychange'));
      }, visible);
    };
    for (let i = 0; i < 3; i += 1) {
      await setVisible(false);
      await page.waitForTimeout(40);
      await setVisible(true);
      await page.waitForTimeout(40);
    }

    const baseline = reads;
    // 之后 6 秒：有在途拉取时不再发起第二次、定时器不叠；一个任务的 task-read 不超过 3 次。
    await page.waitForTimeout(6000);
    expect(reads - baseline, '6 秒内的 task-read 次数').toBeLessThanOrEqual(3);
  } finally {
    await fixture.close();
  }
});
