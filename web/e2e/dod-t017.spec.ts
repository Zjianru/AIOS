import { test, expect, type Locator, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startT017DodFixture } from './service-fixture';
import { sessionLog } from './dod-log';

/**
 * T017.5 DoD2（真 key，MiniMax，数据目录 /tmp/aios-t017-dod/）三步：
 * ① 让 Kind 用 ask_user_question 问一个带三个选项的问题（单选），选一个提交：
 *    日志有 question/answered，页面收成回执，写着选的那一项；
 * ② 让 Kind 跑一条 bash 命令、用 edit 改一个文件：工作过程的终端卡、差异卡正常；
 * ③ 刷新页面：① 的回执和 ② 的卡片都还在。
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

/** 空库里走首次使用页（T008/T015 流程）：选 MiniMax（国内站）→ 填 key → 接上 → 开始。 */
async function connectMinimax(page: Page): Promise<void> {
  await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 60_000 });
  await page.getByRole('button', { name: 'MiniMax（国内站）' }).click();
  await page.locator('#first-use-key').fill(process.env.MINIMAX_API_KEY ?? '');
  await page.getByRole('button', { name: '接上', exact: true }).click();
  await expect(page.getByText('连接可用', { exact: false })).toBeVisible({ timeout: 180_000 });
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

    // ── ② 工作过程：跑一条命令、改一个文件；终端卡、差异卡照常 ──
    const toolStep: string[] = [];
    await send(
      page,
      '接下来请先调用 bash 工具运行命令：echo DOD17-TERMINAL；' +
        '然后用 edit 工具把 notes.md 里的 DOD17-NOTES 改成 DOD17-EDITED。' +
        '必须使用 edit 工具改文件，禁止用 bash 写文件。最后回复我「完成」。不要做别的事。',
    );
    await waitTurns(page, log, 2);
    const turnTwo = await openProcess(page.locator('[data-kind="process"]').last());
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

    const callsOut = jq(
      ['-s', '-c', '[ .[] | select(.type=="tool/call") | {seq, turn:.data.turn, step:.data.step, name:.data.name, arguments:.data.arguments} ]'],
      log,
      '② 两次调用：bash 跑命令、edit 改文件',
      toolStep,
    );
    const diffsOut = jq(
      ['-s', '-c', '[ .[] | select(.type=="tool/result" and .data.meta.diffs != null) | {seq, diffs:.data.meta.diffs} ]'],
      log,
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

    // ── ③ 刷新页面：① 的回执和 ② 的卡片都还在 ──
    const reloadStep: string[] = [];
    await page.reload();
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 60_000 });
    const closedAfter = page.locator('[data-kind="question-closed"]').last();
    await expect(closedAfter, '刷新后提问卡还是回执').toBeVisible({ timeout: 120_000 });
    await expect(closedAfter).toContainText('你回答了 Kind 的 1 个问题');
    await expectReceiptAnswer(closedAfter, chosen);
    const turnTwoAfter = await openProcess(page.locator('[data-kind="process"]').last());
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
    const persistedOut = jq(
      ['-s', '-c', '{ answered: ([ .[] | select(.type=="question/answered") ] | length), diffs: ([ .[] | select(.type=="tool/result" and .data.meta.diffs != null) ] | length), turns: ([ .[] | select(.type=="turn/end") ] | length) }'],
      log,
      '③ 刷新后日志里回执和差异事实都还在',
      reloadStep,
    );
    const persisted = JSON.parse(persistedOut) as { answered: number; diffs: number; turns: number };
    expect(persisted.answered).toBe(1);
    expect(persisted.diffs).toBe(1);
    expect(persisted.turns).toBeGreaterThanOrEqual(2);
    record('3-reload', reloadStep);

    summary.push('# 数据目录：' + fixture.dataDir);
    summary.push('# 会话日志：' + log);
  } finally {
    record('run', summary);
    await fixture.close();
  }
});
