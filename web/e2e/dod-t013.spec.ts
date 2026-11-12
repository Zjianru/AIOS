import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startT013DodFixture } from './service-fixture';
import { answeredAfterRead, renderCommand, runProbe, sessionLog } from './dod-log';

/**
 * T013 DoD2（真 key，MiniMax）：四件事各一条用例。
 * F2（Claude fix-now）：
 * - ①②③ 的断言只认最后一个 [data-kind="answer"]（你自己的话里不会自己命中），等这一轮结束再看；
 * - 问句里不写标记原文（标记只在文件里 / 粘贴里）；
 * - ①② 另外从会话日志核「回答之前有 read 那个路径」（jq 读，不是打印）；
 * - 每条用例用自己的数据目录，跑完不清；jq 的命令和输出写进 WORK/T013/evidence/。
 */

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? path.join(repoRoot, '../aios-work/T013/evidence');

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（T013.9 真 key DoD）');
test.setTimeout(600_000);

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

function jq(lines: string[], args: string[], label: string): string {
  const out = execFileSync('jq', args, { encoding: 'utf8' }).trim();
  lines.push('$ jq ' + renderCommand(args) + '  # ' + label + String.fromCharCode(10) + out);
  return out;
}

/** DoD2 的四条都要的日志事实：user/message 的块、tool/call、tool/result.meta。 */
function jqExamples(log: string, tag: string, lines: string[]): void {
  jq(lines, ['-c', 'select(.type=="user/message") | .data.message.content', log], tag + ' user/message 块');
  jq(lines, ['-c', 'select(.type=="tool/call") | {name: .data.name, arguments: .data.arguments}', log], tag + ' tool/call');
  jq(lines, ['-c', 'select(.type=="tool/result") | .data.meta.diffs', log], tag + ' tool/result.meta');
}

/**
 * ①②：回答之前必须有一次 read 那个路径。
 * 答案（带标记的那条 assistant/message）之前要有 seq 更小的 read 调用。
 */
async function assertReadBeforeAnswer(options: {
  log: string;
  marker: string;
  needle: string;
  tag: string;
  lines: string[];
}): Promise<void> {
  let raw = '（还没跑）';
  let args: string[] = [];
  try {
    await expect
      .poll(
        () => {
          const result = runProbe(options.log, options.marker, options.needle);
          raw = result.raw;
          args = result.args;
          return answeredAfterRead(result.probe) ? 1 : 0;
        },
        { timeout: 180_000, intervals: [500, 1000] },
      )
      .toBe(1);
  } finally {
    options.lines.push(
      '$ jq ' +
        renderCommand(args) +
        '  # ' +
        options.tag +
        ' 回答之前有没有 read「' +
        options.needle +
        '」' +
        String.fromCharCode(10) +
        raw,
    );
  }
}

/** 每条用例自己的证据文件（frozen sha + jq 命令 + 输出），写进 WORK。 */
function recordCase(caseName: string, lines: string[]): void {
  mkdirSync(EVIDENCE, { recursive: true });
  const head = execFileSync('git', ['-C', repoRoot, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  writeFileSync(
    path.join(EVIDENCE, 'T013.10-dod2-jq-' + caseName + '.txt'),
    ['# frozen sha: ' + head, ...lines].join(String.fromCharCode(10) + String.fromCharCode(10)) +
      String.fromCharCode(10),
  );
}

/** 等这一轮结束（停止按钮消失）之后再看日志和气泡。 */
async function waitTurnEnd(page: import('@playwright/test').Page): Promise<void> {
  await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 180_000 });
}

/** 空库里走首次使用页（T008 流程）：选 MiniMax（国内站）→ 填 key → 接上 → 连接可用 → 开始。 */
async function connectMinimax(page: import('@playwright/test').Page): Promise<void> {
  await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 60_000 });
  await page.getByRole('button', { name: 'MiniMax（国内站）' }).click();
  await page.locator('#first-use-key').fill(process.env.MINIMAX_API_KEY ?? '');
  await page.getByRole('button', { name: '接上', exact: true }).click();
  await expect(page.getByText('连接可用', { exact: false })).toBeVisible({ timeout: 180_000 });
  await page.getByRole('button', { name: '开始', exact: true }).click();
  // 进空对话；若 RootHost 还没恢复，页面是「内核不可用」说明页 → 点「现在重试」。
  const composerInput = page.locator('[data-kind="composer-input"]');
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (await composerInput.isVisible().catch(() => false)) return;
    const retry = page.getByRole('button', { name: '现在重试' });
    if (await retry.count()) {
      await retry.first().click().catch(() => undefined);
    }
    await page.waitForTimeout(1500);
  }
  await expect(composerInput).toBeVisible({ timeout: 30_000 });
}

const send = async (page: import('@playwright/test').Page, text: string) => {
  await page.locator('[data-kind="composer-input"]').fill(text);
  await page.locator('[data-kind="send"]').click();
};

/// DoD2①：@ 文件问内容——回答说得对，而且回答之前 read 过 notes.md。
test('DoD2①：@ 文件问内容（回答之前 read notes.md）', async ({ page }) => {
  const fixture = await startT013DodFixture('1-at-file');
  const lines: string[] = [];
  const tag = '① @ 文件';
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await connectMinimax(page);
    const box = page.locator('[data-kind="composer-input"]');
    await box.fill('@notes.md');
    await page.locator('[data-kind="at-menu"]').getByRole('button', { name: /notes\.md/ }).first().click();
    // F2：问句里不写标记原文，标记只在文件里；先读这个文件是测试提示，不是系统提示。
    await box.fill('@notes.md 先读这个文件，再把它的全文原样告诉我。');
    await page.locator('[data-kind="send"]').click();
    const answer = page.locator('[data-kind="answer"]').last();
    await expect(answer).toContainText('DOD13-NOTES', { timeout: 180_000 });
    await waitTurnEnd(page);
    await assertReadBeforeAnswer({ log: sessionLog(fixture.dataDir), marker: 'DOD13-NOTES', needle: 'notes.md', tag, lines });
  } finally {
    jqExamples(sessionLog(fixture.dataDir), tag, lines);
    recordCase('1-at-file', lines);
    await fixture.close();
  }
});

/// DoD2②：拖文件问内容——回答说得对，而且回答之前 read 过副本路径。
test('DoD2②：拖文件问内容（回答之前 read 副本路径）', async ({ page }) => {
  const fixture = await startT013DodFixture('2-dropped-file');
  const lines: string[] = [];
  const tag = '② 拖文件';
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await connectMinimax(page);
    await page.locator('[data-kind="composer"]').evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['DOD13-DROP：拖进来的文件内容' + String.fromCharCode(10)], 'dropped.txt', { type: 'text/plain' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 20_000 });
    await send(page, '拖进来的这个文件里写了什么？原样告诉我。');
    const answer = page.locator('[data-kind="answer"]').last();
    await expect(answer).toContainText('DOD13-DROP', { timeout: 180_000 });
    await waitTurnEnd(page);
    // 副本路径：<数据目录>/attachments/v1/files/…（spec：文件块给副本的绝对路径）。
    await assertReadBeforeAnswer({
      log: sessionLog(fixture.dataDir),
      marker: 'DOD13-DROP',
      needle: 'attachments/v1/files',
      tag,
      lines,
    });
  } finally {
    jqExamples(sessionLog(fixture.dataDir), tag, lines);
    recordCase('2-dropped-file', lines);
    await fixture.close();
  }
});

/// DoD2③：粘贴 >1000 字符问内容——回答说得对，气泡上是卡片（F1）。
test('DoD2③：大段粘贴问内容（气泡上是卡片）', async ({ page }) => {
  const fixture = await startT013DodFixture('3-large-paste');
  const lines: string[] = [];
  const tag = '③ 大段粘贴';
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await connectMinimax(page);
    const marker = 'DOD13-PASTE-MARKER';
    const content = marker + String.fromCharCode(10) + 'x'.repeat(1200);
    await page.locator('[data-kind="composer-input"]').evaluate((element, value) => {
      const data = new DataTransfer();
      data.setData('text/plain', value);
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    }, content);
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await send(page, '我粘贴的这段文字里第一行有什么标记？原样告诉我。');
    const answer = page.locator('[data-kind="answer"]').last();
    await expect(answer).toContainText(marker, { timeout: 180_000 });
    await waitTurnEnd(page);
    // F1：发出去以后，气泡上面是一张粘贴卡，正文不是一大段字。
    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await expect(row.locator('[data-kind="user-bubble"]')).not.toContainText('x'.repeat(40));
  } finally {
    jqExamples(sessionLog(fixture.dataDir), tag, lines);
    recordCase('3-large-paste', lines);
    await fixture.close();
  }
});

/// DoD2④：派一个写任务改文件——diff 卡片出来，刷新后还在。
test('DoD2④：改文件（diff 卡片，刷新后还在）', async ({ page }) => {
  const fixture = await startT013DodFixture('4-edit-file');
  const lines: string[] = [];
  const tag = '④ 改文件';
  let taskId = '';
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await connectMinimax(page);
    await send(page, '派一个写任务：把 notes.md 里的 DOD13-NOTES 改成 DOD13-EDITED，只改这一处，别做别的。');
    taskId = await taskIdOf(fixture.dataDir);
    await waitTurnEnd(page);
    // Kind 没有 edit：写和改由写任务的 ThinAgent 做，diff 卡在任务对话的过程里。
    await page.goto(fixture.baseUrl + '/#taskchat:' + taskId);
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    const process = pane.locator('[data-kind="process"]').first();
    await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 180_000 });
    await process.locator('button').first().click();
    const groups = process.locator('[data-kind="process-group"] button');
    for (let index = 0; index < (await groups.count()); index += 1) {
      if ((await groups.nth(index).getAttribute('aria-expanded')) !== 'true') {
        await groups.nth(index).click();
      }
    }
    const diffRow = process
      .locator(
        '[data-kind="tool-row"][data-tool="edit"], [data-kind="tool-row"][data-tool="apply_patch"], [data-kind="tool-row"][data-tool="write"]',
      )
      .first();
    await expect(diffRow, '工作过程里有写/改文件那一行').toHaveCount(1);
    await diffRow.locator('button').first().click();
    // diff 卡片（「编辑 notes.md +N -N」）出现在展开的工作过程里。
    await expect(process.getByText('notes.md').first()).toBeVisible({ timeout: 30_000 });
    // 日志里的差异事实：任务会话的 tool/result.meta.diffs 有新文本。
    await expect
      .poll(async () => {
        try {
          // jq 输出的是 diffs 数组本身；判空 + 新文本才算真的改到了文件。
          const output = execFileSync(
            'jq',
            ['-c', 'select(.type=="tool/result") | .data.meta.diffs', taskSessionLog(fixture.dataDir, taskId)],
            { encoding: 'utf8' },
          );
          const hasDiffs = output
            .split(String.fromCharCode(10))
            .map((line) => line.trim())
            .filter((line) => line.length > 0 && line !== 'null')
            .length;
          return hasDiffs > 0 && output.includes('DOD13-EDITED') ? 1 : 0;
        } catch {
          return 0;
        }
      }, { timeout: 180_000 })
      .toBeGreaterThanOrEqual(1);
    // 刷新后还在。
    await page.reload();
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 60_000 });
    const process2 = page.locator('[data-pane="task-conversation"] [data-kind="process"]').first();
    await expect(process2.locator('button').first()).toContainText(/已完成，用时/, { timeout: 60_000 });
    await process2.locator('button').first().click();
    const groups2 = process2.locator('[data-kind="process-group"] button');
    for (let index = 0; index < (await groups2.count()); index += 1) {
      if ((await groups2.nth(index).getAttribute('aria-expanded')) !== 'true') {
        await groups2.nth(index).click();
      }
    }
    await expect(process2.getByText('notes.md').first()).toBeVisible({ timeout: 30_000 });
  } finally {
    if (taskId) {
      jqExamples(rootLog(fixture.dataDir), tag + ' 根会话', lines);
      try {
        jqExamples(taskSessionLog(fixture.dataDir, taskId), tag + ' 任务会话', lines);
      } catch {
        // 任务没开出来时只留根会话证据。
      }
    } else {
      jqExamples(sessionLog(fixture.dataDir), tag, lines);
    }
    recordCase('4-edit-file', lines);
    await fixture.close();
  }
});
