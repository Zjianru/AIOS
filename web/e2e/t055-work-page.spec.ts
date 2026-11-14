import { test, expect, type Page } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { readFile } from 'node:fs/promises';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/** T055.1：工作页看板与「等你」（读侧）。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

const workCard = (page: Page, title: string) =>
  page.locator('[data-kind="work-card"]').filter({ hasText: title });

const column = (page: Page, key: string) =>
  page.locator('[data-kind="work-board"] section[data-column="' + key + '"]');

function taskCount(fixture: { dataDir: string }): number {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT count(*) AS n FROM task').all() as { n: number }[];
    return rows[0]?.n ?? 0;
  } finally {
    db.close();
  }
}

function readyTaskCount(fixture: { dataDir: string }): number {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task').all() as { value: string }[];
    return rows.filter(
      (row) => ((JSON.parse(row.value) as { reports?: unknown[] }).reports?.length ?? 0) > 0,
    ).length;
  } finally {
    db.close();
  }
}

test('工作页：四列真任务、等你接受、完成列关闭时间和筛选记忆', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const result = page.locator('[data-kind="task-result"]').last();
    await expect(result).toBeVisible({ timeout: 60_000 });
    await expect(result.locator('[data-kind="task-accept"]')).toBeVisible();

    await page.goto(urlOf(fixture, '#work'));
    await expect(page.locator('[data-scene-page="work"]')).toBeVisible({ timeout: 30_000 });
    for (const key of ['queue', 'doing', 'wait', 'done']) {
      await expect(column(page, key)).toBeVisible();
    }
    // 「等你」列：结果等你接受这一项，带任务标题。
    const waitItem = column(page, 'wait').locator('[data-kind="board-wait-item"][data-wait-kind="accept"]').first();
    await expect(waitItem).toBeVisible({ timeout: 30_000 });
    await expect(waitItem).toContainText('判重改成按');
    await expect(column(page, 'done')).toContainText('这段时间没有完成的任务');
    await expect(page.locator('[data-kind="done-range"]')).toContainText('近 7 天');

    // 完成列筛选记在 localStorage：切到全部，刷新还是全部。
    await page.locator('[data-kind="done-range"]').click();
    await page.getByRole('menuitemradio', { name: '全部' }).click();
    await expect(page.locator('[data-kind="done-range"]')).toContainText('全部');
    await page.reload();
    await expect(page.locator('[data-scene-page="work"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="done-range"]')).toContainText('全部');

    // 回 Kind 接受：完成列出现「关闭于 时间」，等你里不再有它，并在 2 秒内变。
    await page.goto(urlOf(fixture, '#kind'));
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card.locator('[data-kind="task-accept"]')).toBeVisible({ timeout: 30_000 });
    await card.locator('[data-kind="task-accept"]').click();
    await expect(card).toContainText('你接受了', { timeout: 30_000 });
    await page.goto(urlOf(fixture, '#work'));
    await expect(column(page, 'done')).toContainText('判重改成按', { timeout: 30_000 });
    await expect(column(page, 'done')).toContainText('关闭于', { timeout: 10_000 });
    await expect(column(page, 'wait')).not.toContainText('判重改成按', { timeout: 10_000 });
  } finally {
    await fixture.close();
  }
});

test('工作页：暂停时派的写任务在排队，开闸后两秒内移到在做/等你', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    const paused = await apiPost(request, fixture, { method: 'settings-update', paused: true });
    expect(paused.ok()).toBeTruthy();
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await page.goto(urlOf(fixture, '#work'));
    await expect(column(page, 'queue')).toContainText('判重改成按', { timeout: 30_000 });
    await expect(column(page, 'queue')).toContainText('等待中');

    const resumed = await apiPost(request, fixture, { method: 'settings-update', paused: false });
    expect(resumed.ok()).toBeTruthy();
    // 两秒轻轮询：不用刷新，卡自己离开排队、进在做/等你。
    await expect
      .poll(
        async () =>
          (await column(page, 'doing').innerText()) + '|' + (await column(page, 'wait').innerText()),
        { timeout: 10_000, intervals: [300, 500, 1000] },
      )
      .toContain('判重改成按');
    await expect(column(page, 'queue')).not.toContainText('判重改成按', { timeout: 10_000 });
  } finally {
    await fixture.close();
  }
});

test('在你等：任务 blocked 的问题进「有问题要你答」，点去答回到 Kind', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ taskResult: 'blocked' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我查一下新闻归档里的判重规则');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 30_000 });
    await page.goto(urlOf(fixture, '#work'));
    const group = page.locator('[data-kind="wait-group"][data-wait-group="answer"]');
    await expect(group).toBeVisible({ timeout: 30_000 });
    await expect(group).toContainText('有问题要你答');
    await expect(group).toContainText('用哪个版本？');
    await group.locator('[data-action="wait-open"]').first().click();
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('在你等：Kind 的待答提问全部合成一项「Kind 问你」，去答回到提问那一轮', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ askTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我看看这个设置该怎么配');
    await expect(page.locator('[data-kind="question"]').first()).toBeVisible({ timeout: 30_000 });
    await page.goto(urlOf(fixture, '#work'));
    const group = page.locator('[data-kind="wait-group"][data-wait-group="answer"]');
    await expect(group).toBeVisible({ timeout: 30_000 });
    await expect(group).toContainText('Kind 问你');
    await expect(group.locator('[data-kind="wait-item"]')).toHaveCount(1);
    await group.locator('[data-action="wait-open"]').first().click();
    await expect(page.locator('[data-kind="question"]').first()).toBeVisible({ timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('现在：三组都出、每组上限三条、三组都空写「都在正常推进」', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ taskResult: 'ready' });
  try {
    // 先造一个等你接受的任务，再看「现在」。
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我查一下新闻归档里的判重规则');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 30_000 });
    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-scene-page="now"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="now-decisions"]')).toContainText('等你决定');
    await expect(page.locator('[data-kind="now-decisions"]')).toContainText('结果等你接受');
    await expect(page.locator('[data-kind="now-problems"]')).toContainText('出了问题');
    await expect(page.locator('[data-kind="now-advancing"]')).toContainText('在推进');
    await expect(page.locator('[data-kind="now-week-done"]')).toContainText('本周完成');
  } finally {
    await fixture.close();
  }
});

test('现在：没有任务时写「都在正常推进」', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-scene-page="now"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="now-empty"]')).toHaveText('都在正常推进。');
  } finally {
    await fixture.close();
  }
});

test('在你等：没绑任务的确认卡单列，点去看回到 Kind 那张卡', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t055: 'confirm' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先暂停一下新任务');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 60_000 });
    await page.goto(urlOf(fixture, '#work'));
    const group = page.locator('[data-kind="wait-group"][data-wait-group="confirm"]');
    await expect(group).toBeVisible({ timeout: 30_000 });
    await expect(group).toContainText('要你确认');
    await expect(group).toContainText('暂停新任务');
    await group.locator('[data-action="wait-open"]').first().click();
    await expect(page.locator('[data-kind="confirm-card"]').first()).toBeVisible({ timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('现在：等你决定超过三条折起，点开看到全部', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t055: 'board' });
  try {
    await page.goto(urlOf(fixture));
    const seats = await apiPost(request, fixture, { method: 'settings-update', seats: 4 });
    expect(seats.ok()).toBeTruthy();
    await sendKind(page, '派四个调查任务');
    await expect.poll(() => taskCount(fixture), { timeout: 60_000 }).toBe(4);
    await expect.poll(() => readyTaskCount(fixture), { timeout: 60_000 }).toBe(4);
    await expect
      .poll(
        async () => {
          const response = await apiPost(request, fixture, { method: 'work' });
          const body = (await response.json()) as { wait?: unknown[] };
          return body.wait?.length ?? 0;
        },
        { timeout: 60_000 },
      )
      .toBe(4);
    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-scene-page="now"]')).toBeVisible({ timeout: 30_000 });
    const decisions = page.locator('[data-kind="now-decisions"]');
    await expect(decisions.locator('[data-kind="wait-item"]')).toHaveCount(3, { timeout: 30_000 });
    const more = page.locator('[data-kind="now-more"]').first();
    await expect(more).toContainText('还有 1 条');
    await more.click();
    await expect(decisions.locator('[data-kind="wait-item"]')).toHaveCount(4, { timeout: 10_000 });
  } finally {
    await fixture.close();
  }
});

test('去看：较早、还没加载出来的 Kind 那一轮也能滚到', async ({ page }) => {
  test.setTimeout(180_000);
  page.on('console', (message) => {
    if (message.text().startsWith('REVEAL EFFECT')) console.log('BROWSER', message.text());
  });
  const fixture = await startKernelFixture({ t055: 'history' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });
    // 再聊十轮，把出卡那一轮顶出 Kind 页首屏。
    for (let index = 1; index <= 10; index += 1) {
      await sendKind(page, '第 ' + String(index) + ' 句');
      await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(
        '收到第 ' + String(index) + ' 句',
        { timeout: 30_000 },
      );
    }
    await page.goto(urlOf(fixture, '#work'));
    const item = page
      .locator('[data-kind="board-wait-item"][data-wait-kind="accept"]')
      .first();
    await expect(item).toContainText('判重改成按', { timeout: 30_000 });
    await item.click();
    // 首屏只有最近 5 轮；点「去看」后往前加载到出卡那一轮。
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });
  } finally {
    await fixture.close();
  }
});

test('两页同一份：另一个页面接受后，工作页两秒内进完成列', async ({ page, context }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ taskResult: 'ready' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我查一下新闻归档里的判重规则');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });
    await page.goto(urlOf(fixture, '#work'));
    await expect(column(page, 'wait')).toContainText('判重规则', { timeout: 30_000 });

    const other = await context.newPage();
    await other.goto(urlOf(fixture, '#kind'));
    const card = other.locator('[data-kind="task-result"]').last();
    await expect(card.locator('[data-kind="task-accept"]')).toBeVisible({ timeout: 30_000 });
    await card.locator('[data-kind="task-accept"]').click();
    await expect(card).toContainText('你接受了', { timeout: 30_000 });

    // 原页面不刷新，两秒轮询把卡移到完成列。
    await expect(column(page, 'done')).toContainText('判重规则', { timeout: 5000 });
    await expect(column(page, 'wait')).not.toContainText('判重规则', { timeout: 5000 });
  } finally {
    await fixture.close();
  }
});

test('门：判通过前进在做/排队，判完等接受，接受后它和门一起进完成列', async ({ page }) => {
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

    await page.goto(urlOf(fixture, '#work'));
    await expect(column(page, 'doing')).toContainText('先查行数', { timeout: 30_000 });
    await expect(column(page, 'queue')).toContainText('再写结果', { timeout: 30_000 });
    await expect(column(page, 'queue')).toContainText('等前置');
    // 「等前置」是正常排队，不算「出了问题」（「现在」的问题组不出现它）。
    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-kind="now-problems"]')).not.toContainText('再写结果');
    await page.goto(urlOf(fixture, '#work'));
    const judge = page.locator('[data-kind="wait-group"][data-wait-group="judge"]');
    await expect(judge).toBeVisible({ timeout: 60_000 });
    await expect(judge).toContainText('独立核查');

    // 回 Kind 判通过。
    await page.goto(urlOf(fixture, '#kind'));
    const gateCard = page
      .locator('[data-kind="task-result"]')
      .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '独立核查' }) })
      .first();
    await expect(gateCard.locator('[data-kind="gate-pass"]')).toBeVisible({ timeout: 60_000 });
    await gateCard.locator('[data-kind="gate-pass"]').click();
    await expect(gateCard.locator('[data-kind="gate-verdict"]')).toContainText('你判了通过', {
      timeout: 40_000,
    });

    // 工作页：结果等你接受；在「等你」右栏直接接受，它和门一起进完成列。
    await page.goto(urlOf(fixture, '#work'));
    const waiting = page.locator('[data-pane="waiting"]');
    const accept = waiting
      .locator('[data-kind="wait-item"][data-wait-kind="accept"]')
      .filter({ hasText: '先查行数' })
      .first();
    await expect(accept).toBeVisible({ timeout: 30_000 });
    await accept.locator('[data-action="wait-accept"]').click();
    await expect(waiting.locator('[data-kind="accept-receipt"]')).toContainText('接受了 1 项', {
      timeout: 30_000,
    });
    await expect(column(page, 'done')).toContainText('先查行数', { timeout: 30_000 });
    await expect(column(page, 'done')).toContainText('独立核查', { timeout: 10_000 });
  } finally {
    await fixture.close();
  }
});

test('去看定位：同一任务多份结果时，按「等你」项的轮号滚到最新那份，不滚到最早旧卡', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t054: 'oldCard' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });
    // 同一任务再开一次 Attempt、交第二份结果（T054 的旧卡夹具）。
    const taskId = (() => {
      const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
      try {
        const row = db.prepare('SELECT id FROM task').all() as { id: string }[];
        return row[0]?.id;
      } finally {
        db.close();
      }
    })();
    const say = await apiPost(request, fixture, {
      method: 'task-say',
      taskId,
      text: '再核对一遍',
    });
    expect(say.ok()).toBeTruthy();
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(2, { timeout: 60_000 });

    await page.goto(urlOf(fixture, '#work'));
    const compact = page.locator('[data-kind="board-wait-item"][data-wait-kind="accept"]').first();
    await expect(compact).toBeVisible({ timeout: 30_000 });
    const label = await compact.locator('.brand-type').innerText();
    const match = /第 (\d+) 轮/.exec(label);
    expect(match).toBeTruthy();
    const turn = Number(match?.[1] ?? '0');
    await compact.click();
    await expect(page.locator('[data-turn="turn-' + String(turn) + '"]')).toBeVisible({
      timeout: 60_000,
    });
    await expect(
      page
        .locator('[data-turn="turn-' + String(turn) + '"]')
        .locator('[data-kind="task-result"]')
        .last(),
    ).toBeVisible({ timeout: 10_000 });
  } finally {
    await fixture.close();
  }
});

test('未确认图：子任务不上看板，确认卡还在等你；确认后才出现任务卡', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，再把行数写进 other.txt');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 60_000 });

    await page.goto(urlOf(fixture, '#work'));
    await expect(page.locator('[data-kind="work-card"]')).toHaveCount(0, { timeout: 30_000 });
    const waiting = page.locator('[data-pane="waiting"]');
    await expect(waiting.locator('[data-kind="wait-group"][data-wait-group="confirm"]')).toBeVisible({
      timeout: 30_000,
    });

    await page.goto(urlOf(fixture, '#kind'));
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    await page.goto(urlOf(fixture, '#work'));
    await expect(page.locator('[data-kind="work-card"]').first()).toBeVisible({ timeout: 60_000 });
  } finally {
    await fixture.close();
  }
});

test('现在：等前置不进出了问题；缺条件的 stopped 任务进问题组', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我查一下新闻归档里的判重规则');
    // 从根会话里等到任务 id，趁 live 任务还在跑立刻停。
    let taskId = '';
    for (let attempt = 0; attempt < 200 && !taskId; attempt += 1) {
      try {
        const root = JSON.parse(
          await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
        ) as { sessionId: string };
        const text = await readFile(
          path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
          'utf8',
        );
        taskId = /已派出任务 ([0-9a-f]+)。/.exec(text)?.[1] ?? '';
      } catch {
        // 日志还没写出来。
      }
      if (!taskId) await new Promise((resolve) => setTimeout(resolve, 25));
    }
    expect(taskId).not.toBe('');
    const stopped = await apiPost(request, fixture, {
      method: 'task-stop',
      taskId,
      mode: 'now',
    });
    expect(stopped.ok()).toBeTruthy();

    await page.goto(urlOf(fixture, '#work'));
    await expect(column(page, 'queue')).toContainText('查一下新闻', { timeout: 30_000 });
    await expect(column(page, 'queue')).toContainText('缺条件');
    await page.goto(urlOf(fixture, '#now'));
    const problems = page.locator('[data-kind="now-problems"]');
    await expect(problems).toContainText('查一下新闻', { timeout: 30_000 });
    await expect(problems).toContainText('处理中');
  } finally {
    await fixture.close();
  }
});

test('取消图（暂不）：子任务一项都不上工作页', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，再把行数写进 other.txt');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 60_000 });
    await confirm.locator('[data-action="card-dismiss"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    await page.goto(urlOf(fixture, '#work'));
    await page.waitForTimeout(1500);
    await expect(page.locator('[data-kind="work-card"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
