import { test, expect, type Page } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { apiPost, startKernelFixture } from './service-fixture';

/** T059 F1–F5：阶段 5 走查修复（结果卡 Markdown、上游卡、侧栏、核查判定/续改、滚轮跟随）。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function taskRow(
  fixture: { dataDir: string },
  title: string,
): { id: string; reports?: { id: string; state: string }[]; needs?: { from: string }[] } {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
    for (const row of rows) {
      const task = JSON.parse(row.value) as {
        id: string;
        title: string;
        reports?: { id: string; state: string }[];
        needs?: { from: string }[];
      };
      if (task.title === title) return task;
    }
  } finally {
    db.close();
  }
  throw new Error('找不到任务：' + title);
}

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

const resultCard = (page: Page, title: string) =>
  page
    .locator('[data-kind="task-result"]')
    .filter({ has: page.locator('[data-kind="task-title"]', { hasText: title }) })
    .first();

async function startT053(page: Page, text: string) {
  await page.goto(urlOf(fixtureUrl));
  await sendKind(page, text);
  const confirm = page.locator('[data-kind="confirm-card"]').first();
  await expect(confirm).toBeVisible({ timeout: 40_000 });
  await confirm.locator('[data-action="card-option"]').first().click();
  await confirm.locator('[data-action="card-confirm"]').click();
  await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
}

// startT053 需要 fixture 地址；测试里用闭包变量避免每个调用都重算。
let fixtureUrl = '';

test('T059 F1：结果卡摘要里的表格/加粗/代码按 Markdown 渲染', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t053Gate: true });
  fixtureUrl = urlOf(fixture);
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    const gate = resultCard(page, '独立核查');
    await expect(gate).toBeVisible({ timeout: 90_000 });
    const rich = gate.locator('[data-kind="task-rich-body"]');
    await expect(rich.locator('table')).toBeVisible();
    await expect(rich.locator('strong')).toContainText('结论');
    await expect(rich.locator('pre')).toContainText('notes.md 有 3 行');
  } finally {
    await fixture.close();
  }
});

test('T059 F2：上游结果默认折叠且限高，展开后卡内滚动，不盖掉会话', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，再把行数写进 other.txt');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    await expect(resultCard(page, '再写结果')).toBeVisible({ timeout: 90_000 });
    const downstream = taskRow(fixture, '再写结果');
    await page.goto(urlOf(fixture, '#taskchat:' + downstream.id));
    const upstream = page.locator('[data-kind="upstream-results"]');
    await expect(upstream).toBeVisible({ timeout: 30_000 });
    const viewport = page.viewportSize();
    expect(viewport).not.toBeNull();
    const box = await upstream.boundingBox();
    expect(box?.height ?? 0).toBeLessThanOrEqual((viewport?.height ?? 0) * 0.36);
    const conversation = page.locator('[data-kind="task-scroll"]');
    expect((await conversation.boundingBox())?.height ?? 0).toBeGreaterThan(0);
    await upstream.locator('[data-kind="upstream-toggle"]').click();
    const inner = upstream.locator('[data-kind="upstream-scroll"]');
    await expect(inner).toBeVisible();
    const sizes = await inner.evaluate((element) => ({
      scroll: element.scrollHeight,
      client: element.clientHeight,
    }));
    expect(sizes.scroll).toBeGreaterThan(sizes.client);
  } finally {
    await fixture.close();
  }
});

test('T059 F3：侧栏现在/Kind/工作/记忆四项，工作和现在可进入', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(urlOf(fixture));
    const nav = page.locator('[data-kind="sidebar-content"] button');
    await expect(nav).toHaveCount(4);
    await expect(nav.filter({ hasText: '现在' })).toHaveCount(1);
    await expect(nav.filter({ hasText: 'Kind' })).toHaveCount(1);
    await expect(nav.filter({ hasText: '工作' })).toHaveCount(1);
    await expect(nav.filter({ hasText: '记忆' })).toHaveCount(1);
    await nav.filter({ hasText: '工作' }).click();
    await expect(page).toHaveURL(/#work/);
    await page.getByRole('button', { name: '现在', exact: true }).click();
    await expect(page).toHaveURL(/#now/);
    await nav.filter({ hasText: '记忆' }).click();
    await expect(page).toHaveURL(/#memory/);
    await expect(page.locator('[data-kind="memory-page"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('T059 F4：里程碑页两轮判定与核查 Attempt；被把关任务有结构化续改块', async ({
  page,
  request,
}) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t053Gate: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    const gate = resultCard(page, '独立核查');
    await expect(gate).toBeVisible({ timeout: 90_000 });
    const note = '空标题的别直接放过，用 *域名* 加 _路径_ 当键';
    await gate.locator('[data-kind="gate-needs-fix"]').click();
    await gate.locator('[data-kind="gate-note-input"]').fill(note);
    await gate.locator('[data-kind="gate-needs-fix-submit"]').click();
    await expect(gate.locator('[data-kind="gate-note"]')).toContainText('*域名*');

    const guarded = taskRow(fixture, '先查行数');
    const gateTask = taskRow(fixture, '独立核查');
    await apiPost(request, fixture, { method: 'task-continue', taskId: guarded.id, note });
    await expect
      .poll(
        async () => {
          const row = taskRow(fixture, '独立核查');
          return row.reports?.length ?? 0;
        },
        { timeout: 120_000 },
      )
      .toBeGreaterThanOrEqual(2);

    // 第二轮核查报回后，用户判通过：门任务页应有两轮判定。
    const gateRow = taskRow(fixture, '独立核查');
    const secondReport = gateRow.reports?.[gateRow.reports.length - 1];
    expect(secondReport).toBeTruthy();
    const judged = await apiPost(request, fixture, {
      method: 'gate-judge',
      gateTask: gateTask.id,
      gateReport: secondReport!.id,
      conclusion: 'pass',
    });
    expect(judged.ok()).toBeTruthy();

    await page.goto(urlOf(fixture, '#taskchat:' + gateTask.id));
    const review = page.locator('[data-kind="review-section"]');
    await expect(review).toBeVisible({ timeout: 30_000 });
    await expect(review.locator('[data-kind="review-verdict"]')).toHaveCount(2, { timeout: 30_000 });
    await expect(review.locator('[data-kind="review-verdict"]').first()).toContainText('候选第');
    // 两次判定按候选报告顺序编号，第 1 次需修正、第 2 次通过。
    await expect(
      review.locator('[data-kind="review-verdict"]').filter({ hasText: '需修正' }),
    ).toContainText('候选第 1 次');
    await expect(
      review.locator('[data-kind="review-verdict"]').filter({ hasText: '通过' }),
    ).toContainText('候选第 2 次');
    await expect(review.locator('[data-kind="review-verdict"]').filter({ hasText: '需修正' })).toContainText(note);
    await expect(review.locator('[data-kind="review-attempt"]')).toHaveCount(2);

    await page.goto(urlOf(fixture, '#taskchat:' + guarded.id));
    const block = page.locator('[data-kind="continue-block"]').first();
    await expect(block).toBeVisible({ timeout: 30_000 });
    await expect(block.locator('[data-kind="continue-title"]')).toContainText('Kind 让你接着改');
    await expect(block.locator('[data-kind="continue-user"]')).toContainText(note);
    await expect(block.locator('[data-kind="continue-finding"] table')).toBeVisible();
    await expect(block.locator('[data-kind="continue-finding"] strong')).toContainText('结论');

    // R1：用户在任务页自己打出「你要按这个改。」开头的句子，按来源认，不该画成续改块。
    const taskComposer = page.getByPlaceholder(/ThinAgent/).last();
    await taskComposer.fill('你要按这个改。' + String.fromCharCode(10) + '我自己打的这一句。');
    await taskComposer.press('Enter');
    await expect(
      page.locator('[data-kind="user-bubble"]').filter({ hasText: '我自己打的这一句。' }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      page.locator('[data-kind="continue-block"]').filter({ hasText: '我自己打的这一句。' }),
    ).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('T059 F5：Kind 长流式追加中滚轮上推不被拉回；点回到底再跟随', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize({ width: 900, height: 340 });
  const fixture = await startKernelFixture({ t059LongStream: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '你现在都能做什么卡片');
    // 流式还在跑、内容已超过视口：等 stop 按钮和足够长的流式文字。
    await expect(page.locator('[data-kind="stop"]')).toBeVisible({ timeout: 30_000 });
    const answer = page.locator('[data-kind="assistant-answer"]').last();
    await expect
      .poll(async () => (await answer.innerText().catch(() => '')).length, { timeout: 30_000 })
      .toBeGreaterThan(300);
    const scroller = page.locator('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
    await expect(scroller).toBeVisible();
    const metrics = await scroller.evaluate((element) => ({
      scrollHeight: element.scrollHeight,
      clientHeight: element.clientHeight,
    }));
    expect(metrics.scrollHeight).toBeGreaterThan(metrics.clientHeight + 300);
    await scroller.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    const before = await scroller.evaluate((element) => element.scrollTop);
    expect(before).toBeGreaterThan(200);
    const box = await scroller.boundingBox();
    expect(box).not.toBeNull();
    await page.mouse.move((box?.x ?? 0) + (box?.width ?? 0) / 2, (box?.y ?? 0) + (box?.height ?? 0) / 2);
    await page.mouse.wheel(0, -720);
    await expect
      .poll(async () => scroller.evaluate((element) => element.scrollTop), { timeout: 10_000 })
      .toBeLessThan(before - 300);
    const afterWheel = await scroller.evaluate((element) => element.scrollTop);
    // 流式仍在追加：等一会儿，旧机制会在这里被拉回底部；新机制保持原位。
    await page.waitForTimeout(1200);
    expect(await page.locator('[data-kind="stop"]')).toBeVisible();
    const settled = await scroller.evaluate((element) => element.scrollTop);
    expect(settled).toBeLessThanOrEqual(afterWheel + 24);
    await page.getByRole('button', { name: '回到最新' }).click();
    await expect
      .poll(async () => scroller.evaluate((element) => element.scrollTop), { timeout: 10_000 })
      .toBeGreaterThan(settled + 100);
  } finally {
    await fixture.close();
  }
});

test('T059 F6：交付记录时间按本机时区显示，不再是 3/5', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const card = page.locator('[data-kind="task-result"][data-state="review"]').last();
    await expect(card).toBeVisible({ timeout: 40_000 });
    const task = taskRow(fixture, '把 hello.txt 的判重改成按 url');
    await page.goto(urlOf(fixture, '#taskchat:' + task.id));
    const delivery = page.locator('[data-kind="task-delivery"]').first();
    await expect(delivery).toBeVisible({ timeout: 30_000 });
    await delivery.locator('[data-kind="delivery-toggle"]').click();
    const what = delivery.locator('[data-kind="delivery-what"]');
    await expect(what).toContainText(/\d{1,2}\/\d{1,2} \d{2}:\d{2}/);
    await expect(what).not.toContainText('3/5');
  } finally {
    await fixture.close();
  }
});

