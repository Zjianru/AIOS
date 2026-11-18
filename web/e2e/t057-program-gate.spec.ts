import { test, expect } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { apiPost, startKernelFixture } from './service-fixture';

const FIXTURE_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures');

/** T057：程序门——命令由程序跑、程序判；通过/需修正/未确定三态；任务页和看板读同一份事实。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

const resultCard = (page: import('@playwright/test').Page, title: string) =>
  page
    .locator('[data-kind="task-result"]')
    .filter({ has: page.locator('[data-kind="task-title"]', { hasText: title }) })
    .first();

async function findTask(dataDir: string, title: string): Promise<Record<string, any>> {
  const db = new DatabaseSync(path.join(dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
    for (const row of rows) {
      const task = JSON.parse(row.value) as Record<string, any>;
      if (task.title === title) return task;
    }
  } finally {
    db.close();
  }
  throw new Error('找不到任务：' + title);
}

async function startGraph(page: import('@playwright/test').Page, fixture: { baseUrl: string; token: string }) {
  await page.goto(urlOf(fixture));
  await page
    .getByPlaceholder('和 Kind 说…')
    .fill('先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  const confirm = page.locator('[data-kind="confirm-card"]').first();
  await expect(confirm).toBeVisible({ timeout: 40_000 });
  return confirm;
}

async function confirmGraph(confirm: import('@playwright/test').Locator) {
  await confirm.locator('[data-action="card-option"]').first().click();
  await confirm.locator('[data-action="card-confirm"]').click();
  await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
}

test('程序门：命令全绿 → 程序判了通过；任务页/看板读同一事实，没有判定按钮', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t057: 'pass' });
  try {
    const confirm = await startGraph(page, fixture);
    const facts = confirm.locator('[data-kind="confirm-fact"]');
    await expect(facts).toHaveCount(3);
    await expect(facts.nth(1)).toContainText('独立核查');
    await expect(facts.nth(1)).toContainText('程序门，全部退出码为 0 就通过');
    await expect(facts.nth(1)).toContainText('(1) true');
    await expect(facts.nth(1)).toContainText('(2) test -d .');
    await confirmGraph(confirm);

    // 被把关卡：说等核查和程序门自动行。
    const guarded = resultCard(page, '先查行数');
    await expect(guarded).toBeVisible({ timeout: 60_000 });
    await expect(guarded).toContainText('先查行数');
    await expect(guarded.locator('[data-kind="task-auto-started"]')).toContainText('独立核查');

    // 程序门卡：程序判了通过、逐条检查、没有判定按钮、没有接受按钮。
    const gate = resultCard(page, '独立核查');
    await expect(gate).toBeVisible({ timeout: 90_000 });
    await expect(gate).toContainText('程序门 · 自动检查');
    await expect(gate.locator('[data-kind="task-state"]')).toContainText('程序判了通过');
    await expect(gate.locator('[data-kind="program-check"]')).toHaveCount(2);
    await expect(gate.locator('[data-kind="program-check"]').nth(0)).toContainText('退出码 0');
    await expect(gate.locator('[data-kind="gate-pass"]')).toHaveCount(0);
    await expect(gate.locator('[data-kind="gate-needs-fix"]')).toHaveCount(0);
    await expect(gate.locator('[data-kind="task-accept"]')).toHaveCount(0);
    await expect(gate.locator('[data-kind="program-next"]')).toContainText(
      /全部退出码为 0|后面的任务已自动开跑/,
    );

    // 库事实：报告 origin=program、checks 两条、判定 judged_by=program、绑定命令版本 1。
    const gateTask = await findTask(fixture.dataDir, '独立核查');
    expect(gateTask.gate.commands_version).toBe(1);
    expect(gateTask.gate.commands.map((item: { command: string }) => item.command)).toEqual([
      'true',
      'test -d .',
    ]);
    const report = gateTask.reports[gateTask.reports.length - 1];
    expect(report.origin).toBe('program');
    expect(report.checks).toHaveLength(2);
    expect(report.checks.every((check: { outcome: string; exit_code: number }) => check.outcome === 'ran' && check.exit_code === 0)).toBe(true);
    const verdict = gateTask.verdicts[gateTask.verdicts.length - 1];
    expect(verdict.judged_by).toBe('program');
    expect(verdict.conclusion).toBe('pass');
    expect(verdict.commands_version).toBe(1);

    // 看板：程序门 · 2 条命令，负责人程序，列在做、词已判；不在等你/排队。
    await page.goto(urlOf(fixture, '#work'));
    const tile = page.locator('[data-kind="work-card"]').filter({ hasText: '独立核查' }).first();
    await expect(tile).toBeVisible({ timeout: 30_000 });
    await expect(tile).toContainText('程序门 · 2 条命令');
    await expect(tile).toContainText('程序');
    await expect(tile.locator('[data-kind="work-word"]')).toContainText('已判');
    await expect(
      page.locator('[data-kind="work-board"] section[data-column="wait"]'),
    ).not.toContainText('独立核查');
    await expect(
      page.locator('[data-kind="work-board"] section[data-column="queue"]'),
    ).not.toContainText('独立核查');

    // 任务页：程序检查段、命令、历次检查；没有对话输入；固定句说明程序门没有对话。
    await page.goto(urlOf(fixture, '#taskchat:' + gateTask.id));
    const section = page.locator('[data-kind="program-section"]');
    await expect(section).toBeVisible({ timeout: 30_000 });
    await expect(section.locator('[data-kind="program-commands"]')).toContainText('true');
    await expect(section.locator('[data-kind="program-commands"]')).toContainText('test -d .');
    await expect(section.locator('[data-kind="program-run"]')).toHaveAttribute('data-outcome', 'pass');
    await expect(section.locator('[data-kind="program-check"]')).toHaveCount(2);
    await expect(page.locator('[data-kind="task-footer"]')).toContainText('程序门没有对话');
    await expect(page.locator('[data-kind="task-send"]')).toHaveCount(0);
    await expect(page.locator('[data-kind="task-stop-turn"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('程序门：一条命令退出码非 0 → 需修正；失败输出在卡和任务页；下游不跑', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t057: 'fail' });
  try {
    const confirm = await startGraph(page, fixture);
    await confirmGraph(confirm);
    const gate = resultCard(page, '独立核查');
    await expect(gate).toBeVisible({ timeout: 90_000 });
    await expect(gate.locator('[data-kind="task-state"]')).toContainText('程序判了需修正');
    await expect(gate.locator('[data-kind="program-check"]')).toHaveCount(2);
    await expect(gate.locator('[data-kind="program-check"]').nth(1)).toContainText('退出码 1');
    await expect(gate.locator('[data-kind="program-tail"]')).toBeVisible();
    await expect(gate.locator('[data-kind="program-next"]')).toContainText('接着改');
    await expect(gate.locator('[data-kind="gate-pass"]')).toHaveCount(0);

    const gateTask = await findTask(fixture.dataDir, '独立核查');
    const verdict = gateTask.verdicts[gateTask.verdicts.length - 1];
    expect(verdict).toMatchObject({ judged_by: 'program', conclusion: 'needs_fix', commands_version: 1 });
    // 下游没跑。
    const downstream = await findTask(fixture.dataDir, '再写结果');
    expect(downstream.runs ?? []).toHaveLength(0);

    await page.goto(urlOf(fixture, '#work'));
    const tile = page.locator('[data-kind="work-card"]').filter({ hasText: '独立核查' }).first();
    await expect(tile.locator('[data-kind="work-word"]')).toContainText('已判');
    await expect(tile).toContainText('程序门 · 2 条命令');

    await page.goto(urlOf(fixture, '#taskchat:' + gateTask.id));
    await expect(page.locator('[data-kind="program-run"]')).toHaveAttribute('data-outcome', 'needs_fix');
    await expect(page.locator('[data-kind="program-tail"]')).toContainText('t057-needle');
  } finally {
    await fixture.close();
  }
});

test('程序门：命令超时 → 未确定；后面命令没跑；看板在排队缺条件，不是完成', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t057: 'timeout' });
  try {
    const confirm = await startGraph(page, fixture);
    await confirmGraph(confirm);
    const gate = resultCard(page, '独立核查');
    await expect(gate).toBeVisible({ timeout: 90_000 });
    await expect(gate.locator('[data-kind="task-state"]')).toContainText('未确定');
    await expect(gate.locator('[data-kind="program-check"]').nth(0)).toContainText('超时');
    await expect(gate.locator('[data-kind="program-check"]').nth(1)).toContainText('没跑');
    await expect(gate.locator('[data-kind="program-next"]')).toContainText('重跑还是换命令');

    const gateTask = await findTask(fixture.dataDir, '独立核查');
    const report = gateTask.reports[gateTask.reports.length - 1];
    expect(report.checks.map((check: { outcome: string }) => check.outcome)).toEqual([
      'timed_out',
      'skipped',
    ]);
    expect(gateTask.verdicts[gateTask.verdicts.length - 1].conclusion).toBe('undetermined');
    expect(gateTask.verdicts[gateTask.verdicts.length - 1].judged_by).toBe('program');

    await page.goto(urlOf(fixture, '#work'));
    const tile = page.locator('[data-kind="work-card"]').filter({ hasText: '独立核查' }).first();
    await expect(tile.locator('[data-kind="work-word"]')).toContainText('缺条件');
    await expect(
      page.locator('[data-kind="work-board"] section[data-column="queue"]'),
    ).toContainText('独立核查');
    await expect(
      page.locator('[data-kind="work-board"] section[data-column="wait"]'),
    ).not.toContainText('独立核查');
    await expect(
      page.locator('[data-kind="work-board"] section[data-column="done"]'),
    ).not.toContainText('独立核查');

    await page.goto(urlOf(fixture, '#taskchat:' + gateTask.id));
    await expect(page.locator('[data-kind="program-run"]')).toHaveAttribute('data-outcome', 'undetermined');
    await expect(page.locator('[data-kind="program-run"]')).toContainText('未确定');
  } finally {
    await fixture.close();
  }
});

test('程序门：改命令出新确认卡 → 确认后第 2 版按新列表重跑，旧判定作废', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t057: 'fail' });
  try {
    const firstConfirm = await startGraph(page, fixture);
    await confirmGraph(firstConfirm);
    const oldGate = resultCard(page, '独立核查');
    await expect(oldGate).toBeVisible({ timeout: 90_000 });
    await expect(oldGate.locator('[data-kind="task-state"]')).toContainText('程序判了需修正');
    const gateTask = await findTask(fixture.dataDir, '独立核查');

    // 当前候选的旧命令是 true + printf…exit 1；换成两条全绿再跑一遍。
    const template = await readFile(path.join(FIXTURE_DIR, 't057-kind-update.jsonl'), 'utf8');
    const kindFile = path.join(fixture.dataDir, 't057-kind-update.jsonl');
    await writeFile(kindFile, template.split('__GATE_TASK_ID__').join(gateTask.id));
    const routesPath = path.join(fixture.dataDir, 't057-routes-update.json');
    await writeFile(
      routesPath,
      JSON.stringify({
        routes: [
          { contains: '把命令换成', file: kindFile, paceMs: 50 },
          { contains: '"name":"task_report"', file: path.join(FIXTURE_DIR, 't057-thin.jsonl'), paceMs: 60 },
        ],
      }),
    );
    const replayUrl = await fixture.switchReplay?.(routesPath);
    expect(replayUrl).toBeTruthy();
    const saved = await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: replayUrl,
      model: 'deepseek-flash',
    });
    expect(saved.ok()).toBeTruthy();
    await fixture.restart?.();
    await page.goto(urlOf(fixture));

    await page
      .getByPlaceholder('和 Kind 说…')
      .fill('把命令换成 true 和 test -d .，按新列表重跑');
    await page.getByRole('button', { name: '发送', exact: true }).click();

    const updateCard = page.locator('[data-kind="confirm-card"]').last();
    await expect(updateCard).toBeVisible({ timeout: 40_000 });
    const facts = updateCard.locator('[data-kind="confirm-fact"]');
    await expect(facts).toHaveCount(3);
    await expect(facts.nth(0)).toContainText('改前：true；printf t057-needle; exit 1');
    await expect(facts.nth(0)).toContainText('改后：true；test -d .');
    await expect(facts.nth(1)).toContainText('第 1 版 → 第 2 版');
    await expect(facts.nth(2)).toContainText('旧判定作废');
    await updateCard.locator('[data-action="card-option"]').first().click();
    await updateCard.locator('[data-action="card-confirm"]').click();
    await expect(updateCard).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    await expect
      .poll(async () => (await findTask(fixture.dataDir, '独立核查')).gate.commands_version, {
        timeout: 20_000,
      })
      .toBe(2);

    const newGate = page
      .locator('[data-kind="task-result"]')
      .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '独立核查' }) })
      .last();
    await expect(newGate).toBeVisible({ timeout: 90_000 });
    await expect(newGate).toContainText('第 2 版命令');
    await expect(newGate.locator('[data-kind="task-state"]')).toContainText('程序判了通过');
    await expect(newGate.locator('[data-kind="program-check"]')).toHaveCount(2);
    await expect(newGate.locator('[data-kind="program-check"]').nth(1)).toContainText('退出码 0');

    const updated = await findTask(fixture.dataDir, '独立核查');
    expect(updated.gate.commands_version).toBe(2);
    expect(updated.gate.commands.map((item: { command: string }) => item.command)).toEqual([
      'true',
      'test -d .',
    ]);
    const latestVerdict = updated.verdicts[updated.verdicts.length - 1];
    expect(latestVerdict).toMatchObject({
      judged_by: 'program',
      conclusion: 'pass',
      commands_version: 2,
    });

    // T066.6：程序门框型：直角、等宽字、data-box=program，状态环/线动效有数据。
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    const programBox = page.locator('[data-kind="graph-node"][data-box="program"]').first();
    await expect(programBox).toBeVisible({ timeout: 30_000 });
    expect(await programBox.evaluate((el) => getComputedStyle(el).borderRadius)).toBe('0px');
    expect(await programBox.evaluate((el) => getComputedStyle(el).fontFamily)).toContain('mono');
  } finally {
    await fixture.close();
  }
});

