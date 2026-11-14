import { test, expect } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { apiPost, startKernelFixture } from './service-fixture';

/** T053.1：里程碑通过——门卡由程序给「通过」「需修正」，判定只有用户点。 */

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const resultCard = (page: import('@playwright/test').Page, title: string) =>
  page
    .locator('[data-kind="task-result"]')
    .filter({ has: page.locator('[data-kind="task-title"]', { hasText: title }) })
    .first();

test('任务图里程碑：B 报回 → C 不跑 → 门卡等你判 → 通过 → C 才开跑；刷新回执还在', async ({
  page,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t053Gate: true });
  try {
    await page.goto(urlOf(fixture));
    await page
      .getByPlaceholder('和 Kind 说…')
      .fill('先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
    await page.getByRole('button', { name: '发送', exact: true }).click();

    // 确认卡三行：核查那行写明只读 + 里程碑，结果由用户判。
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    const facts = confirm.locator('[data-kind="confirm-fact"]');
    await expect(facts).toHaveCount(3);
    await expect(facts.nth(1)).toContainText('独立核查');
    await expect(facts.nth(1)).toContainText('里程碑，结果由你判');
    await expect(facts.nth(2)).toContainText('再写结果');

    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    // 被把关任务 B 报回：卡上写等核查 + 已自动开跑门。
    const guardedCard = resultCard(page, '先查行数');
    await expect(guardedCard).toBeVisible({ timeout: 60_000 });
    await expect(guardedCard).toContainText('等核查');
    await expect(guardedCard.locator('[data-kind="task-auto-started"]')).toContainText(
      '已自动开跑：独立核查（里程碑，结果由你判）',
    );

    // 门卡：里程碑小标题、核查对象、等你判和两个程序按钮。
    const gateCard = resultCard(page, '独立核查');
    await expect(gateCard).toBeVisible({ timeout: 60_000 });
    await expect(gateCard).toContainText('里程碑 · 只读核查');
    await expect(gateCard.locator('[data-kind="gate-candidate"]')).toContainText(
      '核查的是「先查行数」第 1 次交的结果',
    );
    await expect(gateCard.locator('[data-kind="task-state"]')).toContainText('等你判');
    await expect(gateCard.locator('[data-kind="gate-pass"]')).toBeVisible();
    await expect(gateCard.locator('[data-kind="gate-needs-fix"]')).toBeVisible();

    // 用户没判之前，下游 C 不跑。
    await expect(resultCard(page, '再写结果')).toHaveCount(0);

    // 点通过：回执；C 才被放出。
    await gateCard.locator('[data-kind="gate-pass"]').click();
    await expect(gateCard.locator('[data-kind="gate-verdict"]')).toContainText('你判了通过');
    // 被把关任务卡的状态词：判后从等核查变任务要求已满足。
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-state"]')).toContainText(
      '任务要求已满足',
    );
    await expect(resultCard(page, '再写结果')).toBeVisible({ timeout: 90_000 });

    // 刷新以后判定回执还在。
    await page.reload();
    await expect(resultCard(page, '独立核查')).toBeVisible({ timeout: 60_000 });
    await expect(resultCard(page, '独立核查').locator('[data-kind="gate-verdict"]')).toContainText(
      '你判了通过',
    );
    // 刷新后被把关任务卡的状态词还在（从 gate-verdict relay 重建）。
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-state"]')).toContainText(
      '任务要求已满足',
    );
  } finally {
    await fixture.close();
  }
});

test('任务图里程碑：候选变了以后点旧门卡 → 已经变了，不记判定', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t053Gate: true });
  try {
    await page.goto(urlOf(fixture));
    await page
      .getByPlaceholder('和 Kind 说…')
      .fill('先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    const gateCards = page
      .locator('[data-kind="task-result"]')
      .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '独立核查' }) });
    await expect(gateCards).toHaveCount(1, { timeout: 60_000 });
    const guardedCard = resultCard(page, '先查行数');
    await expect(guardedCard).toBeVisible({ timeout: 60_000 });

    // 被把关任务又交一份新结果（和页面同一个 task-say 调用）：候选变了，门重跑出第二张门卡。
    const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
    let guardedId = '';
    try {
      const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
      for (const row of rows) {
        const task = JSON.parse(row.value) as { id: string; title: string };
        if (task.title === '先查行数') guardedId = task.id;
      }
    } finally {
      db.close();
    }
    expect(guardedId).not.toBe('');
    await apiPost(request, fixture, {
      method: 'task-say',
      taskId: guardedId,
      text: '再改一版',
    });
    await expect(gateCards).toHaveCount(2, { timeout: 90_000 });

    // 点旧门卡：已经变了，不记判定。
    const oldCard = gateCards.first();
    await oldCard.locator('[data-kind="gate-pass"]').click();
    await expect(oldCard.locator('[data-kind="gate-changed"]')).toContainText(
      '已经变了：没有记判定',
    );
    // 任务库里门没有判定记录。
    await expect
      .poll(
        () => {
          const check = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), {
            readOnly: true,
          });
          try {
            const rows = check.prepare('SELECT value FROM task ORDER BY rowid').all() as {
              value: string;
            }[];
            for (const row of rows) {
              const task = JSON.parse(row.value) as { title: string; verdicts?: unknown[] };
              if (task.title === '独立核查') return (task.verdicts ?? []).length;
            }
            return -1;
          } finally {
            check.close();
          }
        },
        { timeout: 20_000 },
      )
      .toBe(0);
  } finally {
    await fixture.close();
  }
});

test('任务图里程碑：判需修正 → 原话输入框 → 回执留原话 → 刷新还在；B 卡变需修正', async ({
  page,
  request,
}) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t053Gate: true });
  try {
    await page.goto(urlOf(fixture));
    await page
      .getByPlaceholder('和 Kind 说…')
      .fill('先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    const gateCard = resultCard(page, '独立核查');
    await expect(gateCard).toBeVisible({ timeout: 60_000 });
    const guardedCard = resultCard(page, '先查行数');
    await expect(guardedCard.locator('[data-kind="task-state"]')).toContainText('等核查');

    // 点需修正先出输入框，写原话后提交。
    const note = '空标题的别直接放过，用域名加路径当键';
    await gateCard.locator('[data-kind="gate-needs-fix"]').click();
    const input = gateCard.locator('[data-kind="gate-note-input"]');
    await expect(input).toBeVisible({ timeout: 10_000 });
    await input.fill(note);
    await gateCard.locator('[data-kind="gate-needs-fix-submit"]').click();
    await expect(gateCard.locator('[data-kind="gate-verdict"]')).toContainText('你判了需修正');
    await expect(gateCard.locator('[data-kind="gate-note"]')).toContainText(note);
    // 被把关卡状态词：判后变需修正。
    await expect(guardedCard.locator('[data-kind="task-state"]')).toContainText('需修正');

    // 刷新：回执和原话都在。
    await page.reload();
    const gateAfter = resultCard(page, '独立核查');
    await expect(gateAfter).toBeVisible({ timeout: 60_000 });
    await expect(gateAfter.locator('[data-kind="gate-note"]')).toContainText(note);
    await expect(gateAfter.locator('[data-kind="gate-verdict"]')).toContainText('你判了需修正');
    await expect(resultCard(page, '先查行数').locator('[data-kind="task-state"]')).toContainText(
      '需修正',
    );

    // 故事 6：Kind 让 B 在原要求里改完再报 → 门自动重跑；B 卡回到「等核查」，
    // 自动行改成「核查自动重跑」。
    const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
    let guardedId = '';
    try {
      const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
      for (const row of rows) {
        const task = JSON.parse(row.value) as { id: string; title: string };
        if (task.title === '先查行数') guardedId = task.id;
      }
    } finally {
      db.close();
    }
    expect(guardedId).not.toBe('');
    await apiPost(request, fixture, {
      method: 'task-continue',
      taskId: guardedId,
      note,
    });
    // 历史里那张（判需修正的）保留旧结论；最新一张在门重跑期间回到「等核查」。
    const bCards = page
      .locator('[data-kind="task-result"]')
      .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '先查行数' }) });
    await expect(bCards).toHaveCount(2, { timeout: 40_000 });
    await expect(bCards.last().locator('[data-kind="task-state"]')).toContainText('等核查', {
      timeout: 90_000,
    });
    await expect(bCards.last().locator('[data-kind="gate-recheck"]')).toContainText(
      '核查自动重跑：独立核查（里程碑，结果由你判）',
    );
    // 刷新重建同一派生。
    await page.reload();
    const bCardsAfter = page
      .locator('[data-kind="task-result"]')
      .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '先查行数' }) });
    await expect(bCardsAfter.last().locator('[data-kind="task-state"]')).toContainText('等核查', {
      timeout: 60_000,
    });
    await expect(bCardsAfter.last().locator('[data-kind="gate-recheck"]')).toContainText(
      '核查自动重跑：独立核查',
    );
  } finally {
    await fixture.close();
  }
});

test('任务图：上游又交新结果 → 下游卡标「上游有新结果，待按新结果核对」，刷新还在', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('先查 notes.md 有几行，再把行数写进 other.txt');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    await expect(resultCard(page, '先查行数')).toBeVisible({ timeout: 60_000 });
    await expect(resultCard(page, '再写结果')).toBeVisible({ timeout: 60_000 });

    // 读上游任务 id，用页面同一个 task-say 让它再交一份新结果。
    const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
    let upstreamId = '';
    try {
      const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
      for (const row of rows) {
        const task = JSON.parse(row.value) as { id: string; title: string };
        if (task.title === '先查行数') upstreamId = task.id;
      }
    } finally {
      db.close();
    }
    expect(upstreamId).not.toBe('');
    await apiPost(request, fixture, {
      method: 'task-say',
      taskId: upstreamId,
      text: '再查一版',
    });
    const downstreamCard = resultCard(page, '再写结果');
    await expect(downstreamCard.locator('[data-kind="upstream-changed"]')).toContainText(
      '上游有新结果，待按新结果核对',
      { timeout: 90_000 },
    );
    // 刷新后还在（从 upstream_changed relay 重建）。
    await page.reload();
    await expect(
      resultCard(page, '再写结果').locator('[data-kind="upstream-changed"]'),
    ).toContainText('上游有新结果，待按新结果核对', { timeout: 60_000 });
  } finally {
    await fixture.close();
  }
});
