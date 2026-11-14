import { test, expect } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

/** T052：任务图纵向 e2e——确认卡两行事实→确认/暂不、暂停排队、上游失败不放下游。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

interface StoredTask {
  title: string;
  parent?: string;
  needs?: { handoff?: unknown }[];
  runs?: { attempts?: unknown[] }[];
  arrangement?: { confirmed?: unknown };
}

function taskRows(dataDir: string): StoredTask[] {
  const db = new DatabaseSync(path.join(dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
    return rows.map((row) => JSON.parse(row.value) as StoredTask);
  } finally {
    db.close();
  }
}

function attemptsOf(task: StoredTask | undefined): number {
  return (task?.runs ?? []).reduce((sum, run) => sum + (run.attempts?.length ?? 0), 0);
}

function graphTasks(dataDir: string): { total: StoredTask | undefined; a: StoredTask | undefined; b: StoredTask | undefined } {
  const rows = taskRows(dataDir);
  return {
    total: rows.find((task) => task.parent === undefined),
    a: rows.find((task) => task.title === '先查行数'),
    b: rows.find((task) => task.title === '再写结果'),
  };
}

const card = (page: import('@playwright/test').Page) => page.locator('[data-kind="confirm-card"]').first();
const resultCard = (page: import('@playwright/test').Page, title: string) =>
  page
    .locator('[data-kind="task-result"]')
    .filter({ has: page.locator('[data-kind="task-title"]', { hasText: title }) })
    .first();

test('任务图：先查再写 → 确认卡两行事实 → 确认 → 第一项开跑 → 第二项自动开跑', async ({
  page,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('先查 notes.md 有几行，再把行数写进 other.txt');
    await page.getByRole('button', { name: '发送', exact: true }).click();

    // 确认卡：两个子任务各一行事实（标题 + 等谁 + 写入范围）。
    const confirm = card(page);
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    const facts = confirm.locator('[data-kind="confirm-fact"]');
    await expect(facts).toHaveCount(2);
    await expect(facts.nth(0)).toContainText('先查行数');
    await expect(facts.nth(0)).toContainText('等：无');
    await expect(facts.nth(0)).toContainText('写入范围：只读');
    await expect(facts.nth(1)).toContainText('再写结果');
    await expect(facts.nth(1)).toContainText('等：先查行数');
    await expect(facts.nth(1)).toContainText('写入范围：other.txt');

    // 确认：卡片收成回执。
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    // 第一项开跑并报回：Kind 对话里出现它的结果卡（标题「先查行数」）。
    const firstCard = resultCard(page, '先查行数');
    await expect(firstCard).toBeVisible({ timeout: 60_000 });
    // 卡上程序事实一行：这张 ready 报告自动放出了哪个下游。
    await expect(firstCard.locator('[data-kind="task-auto-started"]')).toContainText(
      '已自动开跑：再写结果',
    );
    // 第二项由第一项的 ready 结果自动放出：它的卡也出现（按卡标题认，A 卡上有「已自动开跑」字样）。
    const secondCard = resultCard(page, '再写结果');
    await expect(secondCard).toBeVisible({ timeout: 60_000 });

    // 打开第二项的任务对话：任务包里有「上游结果」一段（第一项的标题和摘要）。
    await secondCard.getByRole('button', { name: /打开任务对话/ }).click();
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    const upstream = pane.locator('[data-kind="upstream-results"]');
    await expect(upstream).toBeVisible({ timeout: 20_000 });
    await expect(upstream).toContainText('先查行数');
    await expect(upstream).toContainText('判重用的是原始链接');
  } finally {
    await fixture.close();
  }
});

test('任务图：暂不 → 图留着、一项都不跑', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('先查 notes.md 有几行，再把行数写进 other.txt');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = card(page);
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-dismiss"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    // 等一会儿：没有任何结果卡，任务库里一项 Attempt 都没有、图仍是未确认。
    await page.waitForTimeout(2000);
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(0);
    const { total, a, b } = graphTasks(fixture.dataDir);
    expect(attemptsOf(a)).toBe(0);
    expect(attemptsOf(b)).toBe(0);
    expect(total?.arrangement?.confirmed).toBeUndefined();
    // T053.4：暂不就是取消（Dismissed），图留着但一项都不跑。
    expect(total?.arrangement?.cancelled?.reason).toBe('dismissed');
  } finally {
    await fixture.close();
  }
});

test('任务图：全局暂停时确认只排队，开闸后 A 先 B 后跑完', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    // 先开全局暂停。
    await page.goto(urlOf(fixture, '#settings:exec'));
    await expect(page.locator('[data-kind="exec-settings"]')).toBeVisible({ timeout: 20_000 });
    const pause = page.locator('[data-kind="global-pause"]');
    await pause.click();
    await expect(pause).toHaveAttribute('aria-checked', 'true');

    // 暂停中发一句话、确认任务图：子任务只排队。
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('先查 notes.md 有几行，再把行数写进 other.txt');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = card(page);
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    await page.waitForTimeout(1500);
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(0);
    expect(attemptsOf(graphTasks(fixture.dataDir).a)).toBe(0);

    // 开闸：A 先开跑并报回，B 自动开跑。
    await page.goto(urlOf(fixture, '#settings:exec'));
    await expect(page.locator('[data-kind="global-pause"]')).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await page.locator('[data-kind="global-pause"]').click();
    await page.goto(urlOf(fixture));
    await expect(resultCard(page, '先查行数')).toBeVisible({ timeout: 90_000 });
    await expect(resultCard(page, '再写结果')).toBeVisible({ timeout: 90_000 });
    const { a, b } = graphTasks(fixture.dataDir);
    expect(attemptsOf(a)).toBe(1);
    expect(attemptsOf(b)).toBe(1);
  } finally {
    await fixture.close();
  }
});

test('任务图：上游失败不放下游，Kind 收到失败卡', async ({ page }) => {
  test.setTimeout(150_000);
  const fixture = await startKernelFixture({ t052Fail: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('先查 notes.md 有几行，再把行数写进 other.txt');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = card(page);
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    // Kind 收到第一项的失败卡；卡上没有「已自动开跑」。
    const failed = resultCard(page, '先查行数');
    await expect(failed).toBeVisible({ timeout: 60_000 });
    await expect(failed).toHaveAttribute('data-state', 'failed');
    await expect(failed).toContainText('查失败：notes.md 读不了');
    await expect(failed.locator('[data-kind="task-auto-started"]')).toHaveCount(0);

    // 第二项不跑：没有它的卡，任务库里也没有 Attempt。
    await page.waitForTimeout(2000);
    await expect(resultCard(page, '再写结果')).toHaveCount(0);
    expect(attemptsOf(graphTasks(fixture.dataDir).b)).toBe(0);
  } finally {
    await fixture.close();
  }
});
