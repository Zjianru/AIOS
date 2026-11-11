import { test, expect, type Page } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { realpath } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T030.3：写任务从 Kind 派到 ThinAgent，再回到结果卡上接受；bash 开关和工作树。
 *
 * 全部用真 serve + 按请求体路由的回放：Kind 和 ThinAgent 各走自己的脚本，工具真的执行。
 * 任务 id 由服务生成；只有 expand 的第二段（task_update 要带真实任务 id）在拿到 id 之后
 * 换一份回放再重启服务，和 T029 的做法一致。
 */

const FIXTURE_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures');
const TITLE = '把 hello.txt 的判重改成按 url';
const WRITE_SCOPE = 'hello.txt';
const ORIGINAL = 'hello from fixture\n';
const CHANGED = 'hello from fixture\nurl: https://example.com/1\n';
const BASH_NOTE = '这个任务开了 bash，bash 的改动不在清单里';
const NOT_A_REPO = '这个目录不是 git 仓库，开不了工作树';

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function resultCards(page: Page) {
  return page.locator('[data-kind="task-result"]');
}

function reviewCard(page: Page) {
  return page.locator('[data-kind="task-result"][data-state="review"]').last();
}

function blockedCard(page: Page) {
  return page.locator('[data-kind="task-result"][data-state="blocked"]').last();
}

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

async function waitFor<T>(label: string, check: () => Promise<T | null | false>): Promise<T> {
  let lastError: unknown = null;
  for (let attempt = 0; attempt < 600; attempt += 1) {
    try {
      const value = await check();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('等不到：' + label + (lastError ? '（最后错误：' + String(lastError) + '）' : ''));
}

interface RootEvent {
  type: string;
  seq: number;
  data?: Record<string, any>;
}

async function rootLogText(fixture: KernelFixture): Promise<string> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  return readFile(path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'), 'utf8');
}

async function rootEvents(fixture: KernelFixture): Promise<RootEvent[]> {
  return (await rootLogText(fixture))
    .trim()
    .split(String.fromCharCode(10))
    .slice(1)
    .map((line) => JSON.parse(line) as RootEvent);
}

/** 根会话里 task_accept 回执带的任务 id（回执是 tool/result，不是 user/message）。 */
async function taskIdOf(fixture: KernelFixture): Promise<string> {
  return waitFor('Kind 日志里派出任务', async () => {
    const match = /已派出任务 ([0-9a-f]+)。/.exec(await rootLogText(fixture));
    return match ? match[1] : null;
  });
}

interface StoredTask {
  write_scope?: string;
  bash?: boolean;
  worktree?: boolean;
  worktree_state?: { root: string; repo: string; cwd: string } | null;
  versions?: unknown[];
  acceptance?: { version: number; report_id: string } | null;
  reports: { id: string; state: string; version?: number }[];
  runs: { attempts: { id: string; trigger?: string; ended_at?: number | null }[] }[];
  tools: string[];
}

function taskRow(fixture: KernelFixture, taskId: string): StoredTask {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const row = db.prepare('SELECT value FROM task WHERE id = ?').get(taskId) as
      | { value: string }
      | undefined;
    if (!row) throw new Error('任务库里没有 ' + taskId);
    return JSON.parse(row.value) as StoredTask;
  } finally {
    db.close();
  }
}

async function taskHeader(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  taskId: string,
) {
  const response = await apiPost(request, fixture, { method: 'task-read', taskId });
  expect(response.ok()).toBeTruthy();
  return (await response.json()).task as {
    id: string;
    cwd: string;
    tools: string[];
    attempts: number;
  };
}

/** Kind 日志里这个任务最后一张结果卡的 relay 正文（接受之后阶段仍是 ready_for_review 的来源）。 */
async function relayOf(fixture: KernelFixture, taskId: string): Promise<Record<string, any>> {
  return waitFor('Kind 日志里的结果卡', async () => {
    const events = await rootEvents(fixture);
    for (let index = events.length - 1; index >= 0; index -= 1) {
      const event = events[index];
      if (event.type !== 'user/message') continue;
      const rpcId = event.data?.message?.source?.rpcId;
      if (typeof rpcId !== 'string' || !rpcId.startsWith('task-report:' + taskId + ':')) continue;
      const text = (event.data?.message?.content ?? [])
        .map((block: { type?: string; text?: string }) =>
          block.type === 'text' ? block.text ?? '' : '',
        )
        .join('');
      return JSON.parse(text) as Record<string, any>;
    }
    return null;
  });
}

async function readText(file: string): Promise<string> {
  return readFile(file, 'utf8');
}

test('写任务：点名直接派，卡「待你核」+清单只该文件；接受→「你已接受」，阶段仍待检查、无「完成」', async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');

    const card = reviewCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card.getByText('写任务')).toBeVisible();
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('待你核');
    const changed = card.locator('[data-kind="task-changed"]');
    await expect(changed).toContainText('改了 1 个文件（写入范围 hello.txt）');
    await expect(changed.locator('.process-path')).toHaveCount(1);
    await expect(changed.locator('.process-path')).toHaveText('hello.txt');
    await expect(card).not.toContainText('完成');
    await expect(card.getByRole('button', { name: '接受', exact: true })).toBeVisible();

    const taskId = await taskIdOf(fixture);
    // Kind 派的时候带上了写入范围。
    const events = await rootEvents(fixture);
    const acceptCall = events.find(
      (event) => event.type === 'tool/call' && event.data?.name === 'task_accept',
    );
    expect(acceptCall?.data?.arguments ?? '').toContain('"write_scope":"hello.txt"');

    // ThinAgent 的工具表正好 11 件；文件真的改成按 url。
    const header = await taskHeader(request, fixture, taskId);
    expect(header.tools).toHaveLength(11);
    expect(header.tools).toContain('write');
    expect(header.tools).toContain('edit');
    expect(header.tools).not.toContain('bash');
    expect(await readText(path.join(fixture.dataDir, 'hello.txt'))).toBe(CHANGED);

    // 接受：按钮变「你已接受」；刷新后从日志重建仍是已接受。
    await card.getByRole('button', { name: '接受', exact: true }).click();
    await expect(card.locator('[data-kind="task-accepted"]')).toHaveText(/你已接受/, {
      timeout: 20_000,
    });
    await expect(card).not.toContainText('完成');
    await page.reload();
    const again = reviewCard(page);
    await expect(again).toBeVisible({ timeout: 20_000 });
    await expect(again.locator('[data-kind="task-accepted"]')).toHaveText(/你已接受/);
    await expect(again).not.toContainText('完成');

    // 接受记录绑当前版本和这份报告；阶段仍是 ready_for_review（卡上叫「待检查」）。
    const row = taskRow(fixture, taskId);
    expect(row.acceptance).toBeTruthy();
    expect(row.acceptance?.version).toBe(1);
    expect(row.reports.some((report) => report.id === row.acceptance?.report_id)).toBe(true);
    const relay = await relayOf(fixture, taskId);
    expect(relay.task.stage).toBe('ready_for_review');
    expect(relay.card_state).toBe('review');
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('写任务：没说改哪里时 Kind 先问并给具体路径；同意前不派，同意后才派', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t030: 'ask' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把判重的写法改一下');

    const question = page.locator('[data-kind="question"]');
    await expect(question).toBeVisible({ timeout: 30_000 });
    await expect(question).toContainText('你是想改 hello.txt 吗？');
    await expect(question).toContainText('对，改 hello.txt');
    // 还没同意：日志里没有派出任务。
    const before = (await rootEvents(fixture)).filter(
      (event) => event.type === 'tool/call' && event.data?.name === 'task_accept',
    );
    expect(before).toHaveLength(0);

    await page.getByRole('radio', { name: /对，改 hello.txt/ }).click();
    await page.getByRole('button', { name: '提交回答' }).click();
    const card = reviewCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card.getByText('写任务')).toBeVisible();
    await expect(card.locator('[data-kind="task-changed"]')).toContainText(
      '改了 1 个文件（写入范围 hello.txt）',
    );

    // 同意之后只派了一次，而且带的就是问出来的那个路径。
    const after = (await rootEvents(fixture)).filter(
      (event) => event.type === 'tool/call' && event.data?.name === 'task_accept',
    );
    expect(after).toHaveLength(1);
    expect(after[0].data?.arguments ?? '').toContain('"write_scope":"hello.txt"');
    const taskId = await taskIdOf(fixture);
    expect((await taskHeader(request, fixture, taskId)).tools).toHaveLength(11);
  } finally {
    await fixture.close();
  }
});

test('写任务：越界卡住、文件没改；同意扩大后版本 2 在新写入范围里写成', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'expand' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');

    // 越界写被程序拦住；卡是「卡住了」，Kind 问要不要扩大写入范围。
    const blocked = blockedCard(page);
    await expect(blocked).toBeVisible({ timeout: 40_000 });
    await expect(blocked.locator('[data-kind="task-state"]')).toHaveText('卡住了');
    await expect(blocked).toContainText('想改 other.txt');
    await expect(blocked.locator('[data-kind="task-question"]')).toContainText(
      '这超出了任务范围，要不要改任务？',
    );
    const ask = page.locator('[data-kind="assistant-answer"]').last();
    await expect(ask).toContainText('要不要把写入范围扩大', { timeout: 30_000 });
    // 文件没被改：范围外的 other.txt 不存在，hello.txt 还是原样。
    await expect.poll(() => readText(path.join(fixture.dataDir, 'hello.txt'))).toBe(ORIGINAL);
    const other = await readFile(path.join(fixture.dataDir, 'other.txt'), 'utf8').catch(
      () => null,
    );
    expect(other).toBeNull();

    const taskId = await taskIdOf(fixture);
    const before = taskRow(fixture, taskId);
    expect(before.versions).toHaveLength(1);
    expect(before.runs.flatMap((run) => run.attempts)).toHaveLength(1);

    // 拿到真实任务 id 后再生成 task_update 那一段回放，换回放、换连接、重启同一端口的 serve。
    let template = await readFile(
      path.join(FIXTURE_DIR, 't030-kind-write2.template.jsonl'),
      'utf8',
    );
    template = template.split('__TASK_ID__').join(taskId);
    const kind2 = path.join(fixture.dataDir, 't030-kind-write2.jsonl');
    await writeFile(kind2, template);
    const routesPath = path.join(fixture.dataDir, 't030-routes-expand2.json');
    await writeFile(
      routesPath,
      JSON.stringify({
        routes: [
          { contains: '"name":"task_accept"', file: kind2 },
          {
            contains: '同意把写入范围改成',
            file: path.join(FIXTURE_DIR, 't030-thin-write2.jsonl'),
            paceMs: 60,
          },
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
    await expect(blockedCard(page)).toBeVisible({ timeout: 30_000 });

    // 用户同意扩大；Kind 调 task_update，新要求版本在原会话开新 Attempt。
    await sendKind(page, '好，把写入范围扩大到整个目录，继续做。');
    const card = reviewCard(page);
    await expect(card).toBeVisible({ timeout: 40_000 });
    await expect(card.getByText('写任务')).toBeVisible();
    await expect(card.locator('[data-kind="task-changed"]')).toContainText(
      '改了 1 个文件（写入范围 .）',
    );
    await expect(card.locator('[data-kind="task-changed"]')).toContainText('other.txt');
    expect(await readText(path.join(fixture.dataDir, 'other.txt'))).toBe(
      'url: https://example.com/2\n',
    );
    // 原来的文件仍是原样；失败的那次没写进去。
    expect(await readText(path.join(fixture.dataDir, 'hello.txt'))).toBe(ORIGINAL);

    const after = taskRow(fixture, taskId);
    expect(after.versions).toHaveLength(2);
    // 任务行存解析后的绝对路径；卡上写的是相对任务 cwd 的「.」。
    expect(after.write_scope).toBe(await realpath(fixture.dataDir));
    const attempts = after.runs.flatMap((run) => run.attempts);
    expect(attempts).toHaveLength(2);
    expect(attempts[1].trigger).toBe('requirement_changed');
    const reviewRelay = await relayOf(fixture, taskId);
    expect(reviewRelay.write_scope).toBe('.');
    expect(reviewRelay.changed).toEqual(['other.txt']);
    // 旧卡仍是「卡住了」。
    await expect(blockedCard(page)).toContainText('卡住了');
  } finally {
    await fixture.close();
  }
});

test('bash 开关：打开后刷新还在；写任务 15 件、卡上说明；关掉不影响已派的任务', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const fixture = await startKernelFixture({ t030: 'bash' });
  try {
    await page.goto(urlOf(fixture, '#settings:exec'));
    const section = page.locator('[data-kind="exec-settings"]');
    await expect(section).toBeVisible({ timeout: 20_000 });
    const toggle = section.locator('[data-kind="bash-toggle"]');
    await expect(toggle).toHaveAttribute('aria-checked', 'false');
    // 提醒照 spec 原文。
    await expect(section).toContainText(
      '打开以后，写任务的 ThinAgent 能跑任意命令，写入范围管不住 bash。改动和后果由你自己承担。',
    );
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
    await page.reload();
    await expect(page.locator('[data-kind="exec-settings"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="bash-toggle"]')).toHaveAttribute(
      'aria-checked',
      'true',
    );
    const view = await apiPost(request, fixture, { method: 'settings-update' });
    expect(view.ok()).toBeTruthy();
    expect((await view.json()).exec.bash).toBe(true);

    // 开着 bash 派的写任务：15 件工具、卡上固定说明；bash 写的文件不在清单里。
    await page.goto(urlOf(fixture));
    await sendKind(page, '用 bash 看一下 hello.txt，再把判重改成按 url');
    const card = reviewCard(page);
    await expect(card).toBeVisible({ timeout: 40_000 });
    await expect(card).toContainText(BASH_NOTE);
    const changed = card.locator('[data-kind="task-changed"]');
    await expect(changed).toContainText('改了 1 个文件（写入范围 hello.txt）');
    await expect(changed.locator('.process-path')).toHaveCount(1);
    await expect(changed).not.toContainText('bash-output.txt');
    expect(await readText(path.join(fixture.dataDir, 'bash-output.txt'))).toBe('ran by bash\n');

    const taskId = await taskIdOf(fixture);
    const header = await taskHeader(request, fixture, taskId);
    expect(header.tools).toHaveLength(15);
    for (const tool of ['bash', 'job_output', 'job_list', 'job_kill']) {
      expect(header.tools).toContain(tool);
    }
    expect(taskRow(fixture, taskId).bash).toBe(true);

    // 派完再关：设置持久化关掉，已派的任务仍是 15 件（创建时冻结），卡上说明还在。
    await page.goto(urlOf(fixture, '#settings:exec'));
    await expect(page.locator('[data-kind="bash-toggle"]')).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await page.locator('[data-kind="bash-toggle"]').click();
    await expect(page.locator('[data-kind="bash-toggle"]')).toHaveAttribute(
      'aria-checked',
      'false',
    );
    await page.reload();
    await expect(page.locator('[data-kind="bash-toggle"]')).toHaveAttribute(
      'aria-checked',
      'false',
    );
    expect((await taskHeader(request, fixture, taskId)).tools).toHaveLength(15);
    await page.goto(urlOf(fixture));
    await expect(reviewCard(page)).toContainText(BASH_NOTE);
  } finally {
    await fixture.close();
  }
});

test('工作树：ThinAgent 在工作树里改，原目录不动，卡上写工作树路径', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t030: 'worktree' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '开个工作树，把 hello.txt 的判重改成按 url');
    const card = reviewCard(page);
    await expect(card).toBeVisible({ timeout: 40_000 });
    await expect(card.locator('[data-kind="task-changed"]')).toContainText(
      '改了 1 个文件（写入范围 hello.txt）',
    );

    const taskId = await taskIdOf(fixture);
    const root = await realpath(fixture.dataDir);
    const worktree = path.join(root, 'worktrees', taskId);
    await expect(card.locator('[data-kind="task-worktree"]')).toContainText('工作树：' + worktree);
    // 会话的 cwd 在工作树里，原目录的文件没被改。
    const row = taskRow(fixture, taskId);
    expect(row.worktree_state?.root).toBe(worktree);
    expect(row.worktree_state?.cwd).toBe(worktree);
    expect(await readText(path.join(worktree, 'hello.txt'))).toBe(CHANGED);
    expect(await readText(path.join(fixture.dataDir, 'hello.txt'))).toBe(ORIGINAL);
    expect((await taskHeader(request, fixture, taskId)).tools).toHaveLength(11);
  } finally {
    await fixture.close();
  }
});

test('工作树：目录不是 git 仓库时待补足条件，卡上写固定说明', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t030: 'nonrepo' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '开个工作树，把 hello.txt 的判重改成按 url');
    const card = blockedCard(page);
    await expect(card).toBeVisible({ timeout: 40_000 });
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('没开跑');
    await expect(card).toContainText(NOT_A_REPO);
    await expect(reviewCard(page)).toHaveCount(0);
    // 没有开出会话，也没有写出任何文件。
    const taskId = await taskIdOf(fixture);
    const row = taskRow(fixture, taskId);
    expect(row.worktree).toBe(true);
    // 缺项的那次 Attempt 记成 not_opened：没有会话，也没有跑过模型。
    const attempts = row.runs.flatMap((run) => run.attempts);
    expect(attempts).toHaveLength(1);
    expect(attempts[0].conversation).toBeNull();
    expect(attempts[0].end).toBe('not_opened');
    expect(row.missing.join('；')).toContain(NOT_A_REPO);
    expect(await readText(path.join(fixture.dataDir, 'hello.txt'))).toBe(ORIGINAL);
  } finally {
    await fixture.close();
  }
});

test('Kind 没有写工具：让它改错字，它派写任务，文件由 ThinAgent 改', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t030: 'typo' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 里的错字改一下');
    const card = reviewCard(page);
    await expect(card).toBeVisible({ timeout: 40_000 });
    await expect(card.getByText('写任务')).toBeVisible();
    await expect(card.locator('[data-kind="task-changed"]')).toContainText(
      '改了 1 个文件（写入范围 hello.txt）',
    );
    expect(await readText(path.join(fixture.dataDir, 'hello.txt'))).toBe(
      'hello from the fixture\n',
    );

    // Kind 的工具表（工具注册记录）里没有写、改、跑命令和后台三件；它调的是 task_accept。
    const events = await rootEvents(fixture);
    const registry = new Set<string>();
    for (const event of events) {
      if (event.type !== 'tool/registry') continue;
      for (const addition of event.data?.additions ?? []) {
        if (typeof addition?.name === 'string') registry.add(addition.name);
      }
    }
    for (const forbidden of [
      'write',
      'edit',
      'apply_patch',
      'bash',
      'job_output',
      'job_list',
      'job_kill',
    ]) {
      expect(registry.has(forbidden), 'Kind 的工具表不该有 ' + forbidden).toBe(false);
    }
    expect(registry.has('task_accept')).toBe(true);
    const kindCalls = events
      .filter((event) => event.type === 'tool/call')
      .map((event) => event.data?.name);
    for (const forbidden of [
      'write',
      'edit',
      'apply_patch',
      'bash',
      'job_output',
      'job_list',
      'job_kill',
    ]) {
      expect(kindCalls).not.toContain(forbidden);
    }
    const acceptCall = events.find(
      (event) => event.type === 'tool/call' && event.data?.name === 'task_accept',
    );
    expect(acceptCall?.data?.arguments ?? '').toContain('"write_scope":"hello.txt"');

    // ThinAgent 才有写工具：它用 edit 改的那个文件。
    const taskId = await taskIdOf(fixture);
    const header = await taskHeader(request, fixture, taskId);
    expect(header.tools).toContain('edit');
    await expect(card.locator('[data-kind="task-changed"] .process-path')).toHaveText('hello.txt');
  } finally {
    await fixture.close();
  }
});
