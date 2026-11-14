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
  acceptances?: { version: number; report_id: string }[];
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
