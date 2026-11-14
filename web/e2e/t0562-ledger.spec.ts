import { test, expect, type Page } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T056.2：任务页账本（约束、进展、待答、下一步、否掉的做法、历史）。
 * 故事 1（约束/进展）、2（叙述条与历史）、7（刷新一致）、8（旧任务无账本不显示）。
 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

function taskIdOf(fixture: KernelFixture, titleStart: string): string {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task').all() as { value: string }[];
    const task = rows
      .map((row) => JSON.parse(row.value) as { id: string; title: string })
      .find((row) => row.title.startsWith(titleStart));
    expect(task, '任务 ' + titleStart).toBeTruthy();
    return (task as { id: string }).id;
  } finally {
    db.close();
  }
}

test('任务页账本：约束/进展/叙述条/历史都在，刷新一致', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t056: 'ledger' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '查 notes.md 有几行');
    const card = page.locator('[data-kind="task-result"]').first();
    await expect(card).toBeVisible({ timeout: 60_000 });
    const taskId = taskIdOf(fixture, '查 notes 有几行');
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));

    // 等 ThinAgent 跑完账本脚本（两条叙述 + 历史 + ready 报告）再打开任务页。
    await expect
      .poll(
        async () => {
          const response = await apiPost(request, fixture, { method: 'task-read', taskId });
          const body = (await response.json()) as {
            task?: { report?: { state?: string } };
            ledger?: { notes?: unknown[]; history?: unknown[] };
          };
          return {
            state: body.task?.report?.state ?? '',
            notes: body.ledger?.notes?.length ?? 0,
            history: body.ledger?.history?.length ?? 0,
          };
        },
        { timeout: 60_000, intervals: [100, 200, 500] },
      )
      .toEqual({ state: 'blocked', notes: 2, history: 1 });
    const ledger = page.locator('[data-kind="task-ledger"]');
    await expect(ledger).toBeVisible({ timeout: 60_000 });
    await expect(ledger.locator('[data-kind="task-ledger-toggle"]')).toContainText('账本 ·');
    await expect(ledger.locator('[data-kind="task-ledger-toggle"]')).toContainText('条在用');
    await ledger.locator('[data-kind="task-ledger-toggle"]').click();

    await expect(ledger.locator('[data-kind="ledger-constraints"]')).toContainText(
      '「查 notes.md 有几行」',
    );
    await expect(ledger.locator('[data-kind="ledger-constraints"]')).toContainText('你说的');
    await expect(ledger.locator('[data-kind="ledger-progress"]')).toContainText(
      '缺一个样例文件',
    );
    await expect(ledger.locator('[data-kind="ledger-question"]')).toContainText('给哪个样例？');
    await expect(ledger.locator('[data-kind="ledger-next"]')).toContainText('先跑测试');
    await expect(ledger.locator('[data-kind="ledger-rejected"]')).toContainText('缓存不可行');
    await expect(ledger.locator('[data-kind="ledger-rejected"]')).toContainText('第 1 次记');
    // 故事 2：ThinAgent 的两个账本工具在任务对话里用中文行标题（工作过程默认收起）。
    const conversation = page.locator('[data-pane="task-conversation"]');
    const process = conversation.locator('[data-kind="process"]').last();
    await expect(process).toHaveAttribute('data-open', 'false');
    await process.locator('button').first().click();
    await expect(process).toHaveAttribute('data-open', 'true');
    await process.locator('[data-kind="process-group"] button').first().click();
    await expect(conversation).toContainText('记账本');
    await expect(conversation).toContainText('查账本历史');

    const historyToggle = ledger.locator('[data-kind="ledger-history-toggle"]');
    await expect(historyToggle).toContainText('历史 1 条（不再带给它）');
    await historyToggle.click();
    const history = ledger.locator('[data-kind="ledger-history"]');
    await expect(history).toContainText('先试了缓存');
    await expect(history).toContainText('被新的一条替换');

    // 故事 7：刷新后还是同一份。
    await page.reload();
    const again = page.locator('[data-kind="task-ledger"]');
    await expect(again).toBeVisible({ timeout: 60_000 });
    await again.locator('[data-kind="task-ledger-toggle"]').click();
    await expect(again.locator('[data-kind="ledger-constraints"]')).toContainText(
      '查 notes.md 有几行',
    );
    await expect(again.locator('[data-kind="ledger-next"]')).toContainText('先跑测试');
    await expect(again.locator('[data-kind="ledger-rejected"]')).toContainText('缓存不可行');
  } finally {
    await fixture.close();
  }
});

test('旧任务没有账本：不显示账本块', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，再把行数写进 other.txt');
    const card = page.locator('[data-kind="confirm-card"]').first();
    await expect(card).toBeVisible({ timeout: 60_000 });
    // 未确认图的子任务：没开跑、没报告、没账本。
    const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
    const rows = (db.prepare('SELECT value FROM task').all() as { value: string }[]).map(
      (row) => JSON.parse(row.value) as { id: string; parent?: string },
    );
    db.close();
    const child = rows.find((row) => row.parent);
    expect(child, '图子任务').toBeTruthy();
    const taskId = (child as { id: string }).id;
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 60_000 });
    await expect(page.locator('[data-kind="task-ledger"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
