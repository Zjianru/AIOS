import { test, expect, type Page } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T054.1：接受与已关闭。
 * 故事 1+2（写任务接受 → 你接受了 · 时间、任务已关闭、刷新/任务页一致）、
 * 3（调查任务也能接受）、7（旧卡不显示接受；直接点旧卡由服务端回「已经变了」）、
 * 8（有门任务门判通过后才出接受；接受后关闭）、11（刷新重建一致）。
 */

const WRITE_TITLE = '把 hello.txt 的判重改成按 url';
/** 卡上的标题槽只显示前 20 字。 */
const WRITE_CARD_TITLE = '判重改成按';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const resultCards = (page: Page, title: string) =>
  page
    .locator('[data-kind="task-result"]')
    .filter({ has: page.locator('[data-kind="task-title"]', { hasText: title }) });

const resultCard = (page: Page, title: string) => resultCards(page, title).first();
const lastResultCard = (page: Page, title: string) => resultCards(page, title).last();

const reviewCard = (page: Page) =>
  page.locator('[data-kind="task-result"][data-state="review"]').last();

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

interface TaskRow {
  id: string;
  title: string;
  requirement?: string;
  acceptances?: { version: number; report_id: string }[];
  versions?: { requirement?: string }[];
  reports?: { id: string; state: string }[];
  verdicts?: unknown[];
}

function taskRows(fixture: KernelFixture): TaskRow[] {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
    return rows.map((row) => JSON.parse(row.value) as TaskRow);
  } finally {
    db.close();
  }
}

function taskOf(fixture: KernelFixture, title: string): TaskRow {
  const task = taskRows(fixture).find((row) => row.title === title);
  expect(task, '任务 ' + title).toBeTruthy();
  return task as TaskRow;
}

/** Kind 日志里 task-report relay 的 request id（按到达顺序）。 */
async function relayIds(fixture: KernelFixture): Promise<string[]> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  const ids: string[] = [];
  for (const match of text.matchAll(/"rpcId":"(task-report:[^"]+)"/g)) ids.push(match[1]);
  return [...new Set(ids)];
}

test('写任务：结果卡「待你核」有接受/要求修改；接受后「你接受了 · 时间」+已关闭，刷新和任务页一致', async ({
  page,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const card = reviewCard(page);
    await expect(card).toBeVisible({ timeout: 40_000 });
    await expect(card.locator('[data-kind="task-title"]')).toContainText(WRITE_CARD_TITLE);
    await expect(card.locator('[data-kind="task-state"]')).toContainText('待你核');
    await expect(card.getByRole('button', { name: '接受', exact: true })).toBeVisible();
    await expect(
      card.getByRole('button', { name: '要求修改', exact: true }),
    ).toBeVisible();

    await card.getByRole('button', { name: '接受', exact: true }).click();
    await expect(card.locator('[data-kind="task-accepted"]')).toContainText('你接受了 · ');
    await expect(card.locator('[data-kind="task-closed-note"]')).toHaveText(
      '任务已关闭；要再改，跟 Kind 说',
    );
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('已关闭');
    await expect(card.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(card).not.toContainText('完成');

    // 刷新重建：回执、状态、没有按钮都一样。
    await page.reload();
    const again = reviewCard(page);
    await expect(again).toBeVisible({ timeout: 40_000 });
    await expect(again.locator('[data-kind="task-accepted"]')).toContainText('你接受了 · ');
    await expect(again.locator('[data-kind="task-state"]')).toHaveText('已关闭');

    // 任务页页头同一个词。
    const task = taskOf(fixture, WRITE_TITLE);
    await page.goto(urlOf(fixture).replace('#kind', '#taskchat:' + task.id));
    await expect(
      page.locator('[data-pane="task-conversation"] [data-kind="task-state"]'),
    ).toHaveText('已关闭', { timeout: 30_000 });

    // 任务库：一条接受，绑当前版本和这份报告。
    expect(task.acceptances?.length).toBe(1);
    expect(task.acceptances?.[0]?.version).toBe(1);
    expect(task.reports?.some((report) => report.id === task.acceptances?.[0]?.report_id)).toBe(
      true,
    );
  } finally {
    await fixture.close();
  }
});

test('调查任务：有结果的卡也有接受/要求修改；接受后已关闭', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t054: 'investigation' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '查一下 notes.md 有几行');
    const card = resultCard(page, '查 notes 有几行');
    await expect(card).toBeVisible({ timeout: 40_000 });
    await expect(card.locator('[data-kind="task-state"]')).toContainText('有结果');
    await expect(card.getByRole('button', { name: '接受', exact: true })).toBeVisible();
    await expect(card.getByRole('button', { name: '要求修改', exact: true })).toBeVisible();

    await card.locator('[data-kind="task-accept"]').click();
    await expect(card.locator('[data-kind="task-accepted"]')).toHaveText(/你接受了/);
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('已关闭');

    await page.reload();
    const again = resultCard(page, '查 notes 有几行');
    await expect(again.locator('[data-kind="task-accepted"]')).toHaveText(/你接受了/, {
      timeout: 40_000,
    });
    await expect(again.locator('[data-kind="task-state"]')).toHaveText('已关闭');
    expect(taskOf(fixture, '查 notes 有几行').acceptances?.length).toBe(1);
  } finally {
    await fixture.close();
  }
});

test('旧卡：同一任务交第二份结果后，旧卡没有接受；直接点旧卡回「已经变了」不记', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t054: 'oldCard' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(reviewCard(page)).toBeVisible({ timeout: 40_000 });
    const task = taskOf(fixture, WRITE_TITLE);

    // 让同一任务再开一次 Attempt 并交第二份 ready 报告。
    await apiPost(request, fixture, {
      method: 'task-say',
      taskId: task.id,
      text: '再核对一遍',
    });
    await expect(resultCards(page, WRITE_CARD_TITLE)).toHaveCount(2, {
      timeout: 60_000,
    });

    // 旧卡只剩「打开任务对话」，新卡才有接受。
    const oldCard = resultCard(page, WRITE_CARD_TITLE);
    const newCard = lastResultCard(page, WRITE_CARD_TITLE);
    await expect(oldCard.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(oldCard.locator('[data-kind="open-task"]')).toBeVisible();
    await expect(newCard.locator('[data-kind="task-accept"]')).toBeVisible();

    // 模拟用户点了旧卡（正常按钮不出，服务端再挡一次）。
    const ids = await relayIds(fixture);
    expect(ids.length).toBeGreaterThanOrEqual(2);
    const response = await apiPost(request, fixture, {
      method: 'card-act',
      callId: ids[0],
      verb: 'confirm',
    });
    const body = (await response.json()) as {
      decision?: { phase?: string; data?: { summary?: string } };
    };
    expect(body.decision?.phase).toBe('refused');
    expect(String(body.decision?.data?.summary)).toContain('已经变了');
    expect(taskRows(fixture).find((row) => row.id === task.id)?.acceptances?.length ?? 0).toBe(0);
  } finally {
    await fixture.close();
  }
});

test('有门任务：判通过前没有接受；通过后出接受，接受后已关闭；下游同样关闭', async ({
  page,
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

    const guardedCard = resultCard(page, '先查行数');
    await expect(guardedCard).toBeVisible({ timeout: 60_000 });
    await expect(guardedCard.locator('[data-kind="task-state"]')).toContainText('等核查');
    // 门判通过之前没有接受。
    await expect(guardedCard.locator('[data-kind="task-accept"]')).toHaveCount(0);

    const gateCard = resultCard(page, '独立核查');
    await expect(gateCard).toBeVisible({ timeout: 60_000 });
    await gateCard.locator('[data-kind="gate-pass"]').click();
    await expect(gateCard.locator('[data-kind="gate-verdict"]')).toContainText('你判了通过');

    // 判通过后：被把关任务卡「任务要求已满足」且出现接受；下游 C 开跑。
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-state"]')).toContainText(
      '任务要求已满足',
    );
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-accept"]')).toBeVisible();
    await expect(resultCard(page, '再写结果')).toBeVisible({ timeout: 90_000 });

    // 接受被把关任务 → 已关闭。
    await resultCard(page, '先查行数').locator('[data-kind="task-accept"]').click();
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-state"]')).toHaveText(
      '已关闭',
      { timeout: 30_000 },
    );
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-accepted"]')).toHaveText(
      /你接受了/,
    );

    // 下游 C 也接受 → 已关闭；刷新都还在。
    const cCard = resultCard(page, '再写结果');
    await expect(cCard.locator('[data-kind="task-accept"]')).toBeVisible();
    await cCard.locator('[data-kind="task-accept"]').click();
    await expect(cCard.locator('[data-kind="task-state"]')).toHaveText('已关闭', {
      timeout: 30_000,
    });
    await page.reload();
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-state"]')).toHaveText(
      '已关闭',
      { timeout: 40_000 },
    );
    await expect(resultCard(page, '再写结果').locator('[data-kind="task-state"]')).toHaveText(
      '已关闭',
    );

    // 被把关任务和下游各一条接受；门的判定记录还在。
    const guarded = taskOf(fixture, '先查行数');
    const downstream = taskOf(fixture, '再写结果');
    const gate = taskOf(fixture, '独立核查');
    expect(guarded.acceptances?.length).toBe(1);
    expect(downstream.acceptances?.length).toBe(1);
    expect(gate.verdicts?.length).toBe(1);
  } finally {
    await fixture.close();
  }
});


test('写任务：点「要求修改」→ 卡进输入框 → 带一句话发出去 → Kind 出改要求确认卡 → 确认后第 2 版要求的结果卡，刷新还在', async ({
  page,
}) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t054: 'update' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');

    // 第一版结果卡：待你核，接受/要求修改两个按钮。
    const first = reviewCard(page);
    await expect(first).toBeVisible({ timeout: 60_000 });
    await expect(first.locator('[data-kind="task-accept"]')).toBeVisible();
    const modify = first.locator('[data-kind="task-modify-requirement"]');
    await expect(modify).toBeVisible();
    await expect(first).not.toContainText('第 1 版要求');

    // 点「要求修改」：卡进输入框，占位话变了，焦点在输入框。
    await modify.click();
    const composerCard = page.locator('[data-kind="composer"] [data-kind="result-card-chip"]');
    await expect(composerCard).toBeVisible();
    await expect(composerCard).toContainText('第 1 版的结果');
    const input = page.locator('[data-kind="composer-input"]');
    await expect(input).toHaveAttribute(
      'placeholder',
      '要改什么？Kind 会按你说的出一版新要求给你确认',
    );
    // 故事 11：输入框里这张卡跟着草稿存；刷新后还在（占位还是改要求那句）。
    await expect
      .poll(() =>
        page.evaluate(() =>
          Object.keys(localStorage).some(
            (key) =>
              key.startsWith('aios.kind.draft.') &&
              (localStorage.getItem(key) ?? '').includes('"resultCard"'),
          ),
        ),
      )
      .toBe(true);
    await page.reload();
    await expect(composerCard).toBeVisible({ timeout: 30_000 });
    await expect(composerCard).toContainText('第 1 版的结果');
    await expect(input).toHaveAttribute(
      'placeholder',
      '要改什么？Kind 会按你说的出一版新要求给你确认',
    );
    await input.fill('标题也拿不到的别丢，用域名加路径当键');
    await page.getByRole('button', { name: '发送', exact: true }).click();

    // 发出去以后气泡上还画着这张卡。
    await expect(
      page.locator('[data-kind="user-message-row"] [data-kind="result-card-chip"]').last(),
    ).toContainText('第 1 版的结果');

    // Kind 出改要求确认卡：要求一行（改前 → 改后）、版本一行（第 1 版 → 第 2 版）。
    const confirm = page.locator('[data-kind="confirm-card"]').last();
    await expect(confirm).toBeVisible({ timeout: 60_000 });
    const facts = confirm.locator('[data-kind="confirm-fact"]');
    await expect(facts.nth(0)).toContainText('把 hello.txt 里按整条链接判重的写法改成按 url');
    await expect(facts.nth(0)).toContainText('标题也拿不到的别丢，用域名加路径当键');
    await expect(facts.nth(1)).toContainText('第 1 版 → 第 2 版');
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    // T054.2 F1：回执一到、新版本结果卡还没出来时，旧卡就已经不能接受/要求修改。
    const oldFirst = resultCards(page, WRITE_CARD_TITLE).first();
    await expect(oldFirst.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(oldFirst.locator('[data-kind="task-modify-requirement"]')).toHaveCount(0);

    // 第 2 版要求的结果卡出现；旧卡不再有接受按钮，标题带「· 第 2 版要求」。
    const second = lastResultCard(page, WRITE_CARD_TITLE);
    await expect(second).toContainText('第 2 版要求', { timeout: 60_000 });
    await expect(second).toHaveAttribute('data-state', 'review', { timeout: 60_000 });
    await expect(second.locator('[data-kind="task-accept"]')).toBeVisible();
    await expect(second).toContainText('空标题也不丢了');
    const oldCard = resultCards(page, WRITE_CARD_TITLE).first();
    await expect(oldCard.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(oldCard.locator('[data-kind="task-modify-requirement"]')).toHaveCount(0);

    // DB：第 2 版文本进了版本历史，第 1 版原文还在。
    await expect
      .poll(() => (taskOf(fixture, WRITE_TITLE).versions ?? []).length, { timeout: 30_000 })
      .toBe(2);
    const task = taskOf(fixture, WRITE_TITLE);
    expect(task.versions?.[0]?.requirement).toContain('把 hello.txt 里按整条链接判重');
    expect(task.versions?.[1]?.requirement).toContain('标题也拿不到的别丢，用域名加路径当键');
    expect(task.requirement).toContain('标题也拿不到的别丢，用域名加路径当键');

    // 刷新：两张卡都还在，第 2 版仍在，输入框里没有残留的卡。
    await page.reload();
    await expect(lastResultCard(page, WRITE_CARD_TITLE)).toContainText('第 2 版要求', {
      timeout: 60_000,
    });
    await expect(
      lastResultCard(page, WRITE_CARD_TITLE).locator('[data-kind="task-accept"]'),
    ).toBeVisible();
    await expect(page.locator('[data-kind="composer"] [data-kind="result-card-chip"]')).toHaveCount(0);

    // 接受第 2 版：你接受了 · 时间 / 任务已关闭。
    await lastResultCard(page, WRITE_CARD_TITLE).locator('[data-kind="task-accept"]').click();
    await expect(lastResultCard(page, WRITE_CARD_TITLE)).toContainText('你接受了', {
      timeout: 40_000,
    });
    await expect(lastResultCard(page, WRITE_CARD_TITLE)).toContainText('任务已关闭；要再改，跟 Kind 说');
  } finally {
    await fixture.close();
  }
});

test('已关闭的任务再改要求：卡上没有按钮也从 Kind 说一句，出同样的确认卡；旧接受留历史', async ({
  page,
}) => {
  test.setTimeout(300_000);
  const fixture = await startKernelFixture({ t054: 'update' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');

    // 先改一版：要求修改 → 带一句发出去 → 确认 → 第 2 版结果卡。
    const first = reviewCard(page);
    await expect(first).toBeVisible({ timeout: 60_000 });
    await first.locator('[data-kind="task-modify-requirement"]').click();
    const input = page.locator('[data-kind="composer-input"]');
    await input.fill('标题也拿不到的别丢，用域名加路径当键');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const updateConfirm = page.locator('[data-kind="confirm-card"]').last();
    await expect(updateConfirm).toBeVisible({ timeout: 60_000 });
    await updateConfirm.locator('[data-action="card-option"]').first().click();
    await updateConfirm.locator('[data-action="card-confirm"]').click();
    await expect(updateConfirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    const second = lastResultCard(page, WRITE_CARD_TITLE);
    await expect(second).toContainText('第 2 版要求', { timeout: 60_000 });
    await expect(second).toHaveAttribute('data-state', 'review', { timeout: 60_000 });

    // 接受第 2 版：已关闭，卡上不再有接受/要求修改。
    await second.locator('[data-kind="task-accept"]').click();
    await expect(second).toContainText('你接受了', { timeout: 40_000 });
    await expect(second.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(second.locator('[data-kind="task-modify-requirement"]')).toHaveCount(0);
    expect(taskOf(fixture, WRITE_TITLE).acceptances).toHaveLength(1);

    // 已关闭卡上没有按钮：跟 Kind 说一句再改一点，Kind 仍然出确认卡。
    await page.getByPlaceholder('和 Kind 说…').fill('再改一点：按域名归并');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = page.locator('[data-kind="confirm-card"]').last();
    await expect(confirm).toBeVisible({ timeout: 60_000 });
    const facts = confirm.locator('[data-kind="confirm-fact"]');
    await expect(facts.nth(1)).toContainText('第 2 版 → 第 3 版');
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    const third = lastResultCard(page, WRITE_CARD_TITLE);
    await expect(third).toContainText('第 3 版要求', { timeout: 60_000 });
    await expect(third).toHaveAttribute('data-state', 'review', { timeout: 60_000 });
    await expect(third.locator('[data-kind="task-accept"]')).toBeVisible();
    await expect(third).toContainText('按域名归并好了');

    await expect
      .poll(() => (taskOf(fixture, WRITE_TITLE).versions ?? []).length, { timeout: 30_000 })
      .toBe(3);
    const task = taskOf(fixture, WRITE_TITLE);
    expect(task.acceptances).toHaveLength(1);
    expect(task.acceptances?.[0]?.version).toBe(2);
    expect(task.requirement).toContain('按域名归并');
    expect(task.versions?.[0]?.requirement).toContain('把 hello.txt 里按整条链接判重');
  } finally {
    await fixture.close();
  }
});

test('F1 窗口：确认改要求后、新结果卡出来前，旧卡已经不能接受/要求修改', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t054: 'updateWindow' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const first = reviewCard(page);
    await expect(first).toBeVisible({ timeout: 60_000 });
    await expect(first.locator('[data-kind="task-accept"]')).toBeVisible();
    await first.locator('[data-kind="task-modify-requirement"]').click();
    const input = page.locator('[data-kind="composer-input"]');
    await input.fill('标题也拿不到的别丢，用域名加路径当键');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = page.locator('[data-kind="confirm-card"]').last();
    await expect(confirm).toBeVisible({ timeout: 60_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    // 第 2 版这次 Attempt 的 ThinAgent 回放被卡住：新结果卡不会出来，窗口是稳定的。
    await expect(first.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(first.locator('[data-kind="task-modify-requirement"]')).toHaveCount(0);
    await expect(first.locator('[data-kind="open-task"]')).toBeVisible();

    // 刷新重建同一条派生：旧卡还是只剩打开任务对话。
    await page.reload();
    await expect(first).toBeVisible({ timeout: 30_000 });
    await expect(first.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(first.locator('[data-kind="task-modify-requirement"]')).toHaveCount(0);
    await page.waitForTimeout(500);
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(1);
    expect((taskOf(fixture, WRITE_TITLE).versions ?? []).length).toBe(2);
  } finally {
    await fixture.close();
  }
});
