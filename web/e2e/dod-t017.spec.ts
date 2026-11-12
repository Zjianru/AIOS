import { test, expect, type Locator, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startT017DodFixture } from './service-fixture';
import { sessionLog } from './dod-log';

/**
 * T017.5 DoD2（真 key，MiniMax，数据目录 /tmp/aios-t017-dod/）三步：
 * ① 让 Kind 用 ask_user_question 问一个带三个选项的问题（单选），选一个提交：
 *    日志有 question/answered，页面收成回执，写着选的那一项；
 * ② 派一个写任务（先开 bash）：任务里跑一条 bash 命令、用 edit 改一个文件，
 *    任务对话里的终端卡、差异卡正常；
 * ③ 刷新：任务对话的 ② 卡片和 Kind 对话的 ① 回执都还在。
 *
 * key：只在运行时从环境变量读（/tmp/run-t017-dod.cjs 从 ~/.zshrc 取，不打印、不进 argv）。
 * 证据：jq 输出写 WORK/T017/evidence/，首行是跑之前冻结的 sha。
 */

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? path.join(repoRoot, '../aios-work/T017/evidence');
const NL = String.fromCharCode(10);

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（T017.5 真 key DoD）');
test.setTimeout(900_000);

function renderCommand(args: string[]): string {
  return args
    .map((arg) => (arg.includes(' ') || arg.includes(NL) ? "'" + arg + "'" : arg))
    .join(' ');
}

function jq(args: string[], file: string, label: string, lines: string[]): string {
  const out = execFileSync('jq', [...args, file], { encoding: 'utf8' }).trim();
  lines.push('$ jq ' + renderCommand([...args, file]) + '  # ' + label + NL + out);
  return out;
}

function record(name: string, lines: string[]): void {
  mkdirSync(EVIDENCE, { recursive: true });
  const head = execFileSync('git', ['-C', repoRoot, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  writeFileSync(
    path.join(EVIDENCE, 'T017.5-dod2-jq-' + name + '.txt'),
    ['# frozen sha: ' + head, ...lines].join(NL + NL) + NL,
  );
}

/** 根会话日志（Kind 派任务的那条）。 */
function rootLog(dataDir: string): string {
  const root = JSON.parse(readFileSync(path.join(dataDir, 'kind-root.json'), 'utf8')) as {
    sessionId: string;
  };
  return path.join(dataDir, 'sessions', root.sessionId + '.jsonl');
}

/** 根会话日志里 task_accept 回执带的任务 id（Kind 派出的写任务）。 */
async function taskIdOf(dataDir: string): Promise<string> {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    try {
      const match = /已派出任务 ([0-9a-f]+)。/.exec(readFileSync(rootLog(dataDir), 'utf8'));
      if (match) return match[1];
    } catch {
      // 根会话文件还没写出来；继续等。
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error('根会话日志里等不到 task_accept 的任务 id');
}

/** 任务当前 Attempt 的会话日志；任务对话里的工具调用和差异事实都在这里。 */
function taskSessionLog(dataDir: string, taskId: string): string {
  const db = new DatabaseSync(path.join(dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const row = db.prepare('SELECT value FROM task WHERE id = ?').get(taskId) as
      | { value: string }
      | undefined;
    if (!row) throw new Error('任务库里没有 ' + taskId);
    const task = JSON.parse(row.value) as {
      runs?: { attempts?: { conversation?: string | null }[] }[];
    };
    const conversations = (task.runs ?? [])
      .flatMap((run) => run.attempts ?? [])
      .map((attempt) => attempt.conversation)
      .filter((value): value is string => typeof value === 'string' && value.length > 0);
    const conversation = conversations[conversations.length - 1];
    if (!conversation) throw new Error('任务会话还没建：' + taskId);
    return path.join(dataDir, 'sessions', conversation + '.jsonl');
  } finally {
    db.close();
  }
}

/** 写任务要用 bash：先在设置里打开开关。 */
async function enableBash(page: Page, fixture: { baseUrl: string; token: string }): Promise<void> {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#settings:exec`);
  await expect(page.locator('[data-kind="exec-settings"]')).toBeVisible({ timeout: 20_000 });
  const toggle = page.locator('[data-kind="bash-toggle"]');
  if ((await toggle.getAttribute('aria-checked')) !== 'true') {
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
  }
}

/** 空库里走首次使用页（T008/T015 流程）：选 MiniMax（国内站）→ 填 key → 接上 → 开始。 */
async function connectMinimax(page: Page): Promise<void> {
  await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 60_000 });
  await page.getByRole('button', { name: 'MiniMax（国内站）' }).click();
  await page.locator('#first-use-key').fill(process.env.MINIMAX_API_KEY ?? '');
  await page.getByRole('button', { name: '接上', exact: true }).click();
  await expect(page.getByText('已保存到 Keyring', { exact: false })).toBeVisible({ timeout: 180_000 });
  await page.getByRole('button', { name: '开始', exact: true }).click();
  await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 60_000 });
}

async function send(page: Page, text: string): Promise<void> {
  await page.locator('[data-kind="composer-input"]').fill(text);
  await page.locator('[data-kind="send"]').click();
}

/** 日志里已经有 N 个 turn/end：比只看停止按钮稳，避开按钮还没渲染出来的窗口。 */
async function waitTurns(page: Page, log: string, count: number): Promise<void> {
  await expect
    .poll(
      () => {
        try {
          return Number(
            execFileSync('jq', ['-s', '-r', '[ .[] | select(.type=="turn/end") ] | length', log], {
              encoding: 'utf8',
            }).trim(),
          );
        } catch {
          return -1;
        }
      },
      { timeout: 300_000, intervals: [1000, 2000, 2000, 5000] },
    )
    .toBeGreaterThanOrEqual(count);
  await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 30_000 });
}

/** 打开某一轮的工作过程：点开过程头、点开所有分组。 */
async function openProcess(process: ReturnType<Page['locator']>) {
  await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 120_000 });
  await process.locator('button').first().click();
  const groups = process.locator('[data-kind="process-group"] button');
  const count = await groups.count();
  for (let index = 0; index < count; index += 1) await groups.nth(index).click();
  return process;
}

/** 回执里的答案：问题文本也含选项词，整卡 toContainText(chosen) 会假过；断答案那一段的全文。 */
async function expectReceiptAnswer(closed: Locator, chosen: string): Promise<void> {
  await expect(closed.getByText(chosen, { exact: true })).toBeVisible();
}

/// DoD2 ①②③：真 key 走一条对话——提问回执、终端/差异卡、刷新后都还在。
test('DoD2：真 key 三步（MiniMax）', async ({ page }) => {
  const fixture = await startT017DodFixture();
  const summary: string[] = [];
  try {
    // 小项目：T017 自己的测试文件（夹具已写 notes.md；这里换成 DOD17 标记）。
    writeFileSync(
      path.join(path.dirname(fixture.dataDir), 'work', 'notes.md'),
      ['# 项目笔记', '', '第一行：DOD17-NOTES', ''].join(NL),
    );
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await connectMinimax(page);
    // ② 的写任务要跑 bash：先在设置里打开开关，再回 Kind。
    await enableBash(page, fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');

    // ── ① 提问：三个选项，选一个提交；日志 question/answered，页面收成回执 ──
    const questionStep: string[] = [];
    await send(
      page,
      '请用 ask_user_question 工具问我一个问题：从「苹果」「香蕉」「橘子」三个水果里选一个。' +
        '只问这一道题，单选，三个选项都要给我，然后等我回答。不要做别的事。',
    );
    const card = page.locator('[data-kind="question"]').last();
    await expect(card).toBeVisible({ timeout: 300_000 });
    const radios = card.getByRole('radio');
    await expect(radios, '问题卡要有三个选项').toHaveCount(3, { timeout: 30_000 });
    const labels = (await radios.allInnerTexts()).map((text) => text.split(NL)[0].trim());
    const chosen = labels[0];
    expect(chosen.length).toBeGreaterThan(0);
    await radios.nth(0).click();
    await card.getByRole('button', { name: '提交回答' }).click();
    const closed = page.locator('[data-kind="question-closed"]').last();
    await expect(closed).toBeVisible({ timeout: 120_000 });
    await expect(closed).toContainText('你回答了 Kind 的 1 个问题');
    await expectReceiptAnswer(closed, chosen);
    const log = sessionLog(fixture.dataDir);
    await waitTurns(page, log, 1);
    summary.push('# ① 选中的选项：' + chosen);
    const askedOut = jq(
      ['-s', '-c', '[ .[] | select(.type=="question/asked") | {seq, questionId:.data.question_id, question:.data.question, options:.data.options} ]'],
      log,
      '① question/asked：一道题、三个选项',
      questionStep,
    );
    const answeredOut = jq(
      ['-s', '-c', '[ .[] | select(.type=="question/answered") | {seq, questionId:.data.question_id, selected:.data.selected, custom:.data.custom} ]'],
      log,
      '① question/answered：选中的那一项',
      questionStep,
    );
    const asked = JSON.parse(askedOut) as { questionId: string; options: unknown[] }[];
    const answered = JSON.parse(answeredOut) as { questionId: string; selected: string[] }[];
    expect(asked.length, '只有一道题').toBe(1);
    expect(asked[0].options.length, '问题带三个选项').toBe(3);
    expect(answered.length, '一条 question/answered').toBe(1);
    expect(answered[0].selected, '回答就是选中的那一项').toContain(chosen);
    record('1-question', questionStep);

    // ── ② 工作过程：写任务里跑一条命令、改一个文件；终端卡、差异卡照常 ──
    const toolStep: string[] = [];
    await send(
      page,
      '派一个写任务，写入范围 notes.md：任务里先只用 bash 工具运行命令 echo DOD17-TERMINAL；' +
        '然后用 edit 工具把 notes.md 里的 DOD17-NOTES 改成 DOD17-EDITED。' +
        '最后用 task_report 上报。不要做别的事。',
    );
    await waitTurns(page, log, 2);
    const taskId = await taskIdOf(fixture.dataDir);
    // Kind 没有 bash/edit：过程在写任务的任务对话里，和 t013-process 一样打开再看。
    await page.goto(fixture.baseUrl + '/#taskchat:' + taskId);
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-full-title"]')).toBeVisible({ timeout: 20_000 });
    const turnTwo = await openProcess(pane.locator('[data-kind="process"]').first());
    const bashRow = turnTwo.locator('[data-kind="tool-row"][data-tool="bash"]').first();
    const diffRow = turnTwo
      .locator('[data-kind="tool-row"][data-tool="edit"], [data-kind="tool-row"][data-tool="apply_patch"]')
      .first();
    await expect(bashRow, '工作过程里有 bash 调用').toHaveCount(1);
    await expect(diffRow, '工作过程里有 edit/apply_patch 调用').toHaveCount(1);
    await bashRow.locator('button').first().click();
    await expect(bashRow.getByText('DOD17-TERMINAL', { exact: false }).first()).toBeVisible();
    await expect(bashRow.getByText('完成', { exact: true }).first()).toBeVisible();
    await diffRow.locator('button').first().click();
    await expect(diffRow.getByText('DOD17-EDITED', { exact: false }).first()).toBeVisible();
    await expect(diffRow.getByText('DOD17-NOTES', { exact: false }).first()).toBeVisible();

    const taskLog = taskSessionLog(fixture.dataDir, taskId);
    const callsOut = jq(
      ['-s', '-c', '[ .[] | select(.type=="tool/call") | {seq, turn:.data.turn, step:.data.step, name:.data.name, arguments:.data.arguments} ]'],
      taskLog,
      '② 任务里的 bash 与 edit 调用',
      toolStep,
    );
    const diffsOut = jq(
      ['-s', '-c', '[ .[] | select(.type=="tool/result" and .data.meta.diffs != null) | {seq, diffs:.data.meta.diffs} ]'],
      taskLog,
      '② 改文件那次调用的 tool/result.meta（差异事实）',
      toolStep,
    );
    const calls = JSON.parse(callsOut) as { name: string; arguments: string }[];
    const bashCall = calls.find((call) => call.name === 'bash');
    const editCall = calls.find((call) => call.name === 'edit' || call.name === 'apply_patch');
    expect(bashCall, '日志里有 bash 调用').toBeTruthy();
    expect(bashCall!.arguments, 'bash 跑的是 echo DOD17-TERMINAL').toContain('echo DOD17-TERMINAL');
    expect(editCall, '日志里有 edit/apply_patch 调用').toBeTruthy();
    const diffs = JSON.parse(diffsOut) as { diffs: { newText: string; oldText: string; path: string }[] }[];
    const edited = diffs.flatMap((row) => row.diffs).find((diff) => diff.newText.includes('DOD17-EDITED'));
    expect(edited, '差异里有 DOD17-NOTES → DOD17-EDITED').toBeTruthy();
    expect(edited!.oldText).toContain('DOD17-NOTES');
    expect(edited!.path).toContain('notes.md');
    record('2-tools', toolStep);

    // ── ③ 刷新：任务对话里的 ② 卡片还在，Kind 对话里的 ① 回执也还在 ──
    const reloadStep: string[] = [];
    await page.reload();
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 60_000 });
    const turnTwoAfter = await openProcess(
      page.locator('[data-pane="task-conversation"] [data-kind="process"]').first(),
    );
    const bashRowAfter = turnTwoAfter.locator('[data-kind="tool-row"][data-tool="bash"]').first();
    const diffRowAfter = turnTwoAfter
      .locator('[data-kind="tool-row"][data-tool="edit"], [data-kind="tool-row"][data-tool="apply_patch"]')
      .first();
    await expect(bashRowAfter).toHaveCount(1);
    await expect(diffRowAfter).toHaveCount(1);
    await bashRowAfter.locator('button').first().click();
    await expect(bashRowAfter.getByText('DOD17-TERMINAL', { exact: false }).first()).toBeVisible();
    await diffRowAfter.locator('button').first().click();
    await expect(diffRowAfter.getByText('DOD17-EDITED', { exact: false }).first()).toBeVisible();

    // 回执在 Kind 根对话里；刷新后仍是回执、答案还是选中的那一项。
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.reload();
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 60_000 });
    const closedAfter = page.locator('[data-kind="question-closed"]').last();
    await expect(closedAfter, '刷新后提问卡还是回执').toBeVisible({ timeout: 120_000 });
    await expect(closedAfter).toContainText('你回答了 Kind 的 1 个问题');
    await expectReceiptAnswer(closedAfter, chosen);

    const persistedOut = jq(
      ['-s', '-c', '{ answered: ([ .[] | select(.type=="question/answered") ] | length), turns: ([ .[] | select(.type=="turn/end") ] | length) }'],
      log,
      '③ 刷新后根会话日志里回执和轮次都还在',
      reloadStep,
    );
    const persistedTaskOut = jq(
      ['-s', '-c', '{ diffs: ([ .[] | select(.type=="tool/result" and .data.meta.diffs != null) ] | length) }'],
      taskLog,
      '③ 刷新后任务会话日志里差异事实还在',
      reloadStep,
    );
    const persisted = JSON.parse(persistedOut) as { answered: number; turns: number };
    const persistedTask = JSON.parse(persistedTaskOut) as { diffs: number };
    expect(persisted.answered).toBe(1);
    expect(persistedTask.diffs).toBe(1);
    expect(persisted.turns).toBeGreaterThanOrEqual(2);
    record('3-reload', reloadStep);

    summary.push('# 数据目录：' + fixture.dataDir);
    summary.push('# 会话日志：' + log);
    summary.push('# 任务会话日志：' + taskLog);
  } finally {
    record('run', summary);
    await fixture.close();
  }
});
