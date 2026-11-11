import { test, expect, type Page } from '@playwright/test';
import { spawn } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T029.3：重启接续的页面链路（真 serve + 回放，中途杀服务再起）。
 *
 * 任务 id 由服务在运行中生出来，Kind 的重启卡选项必须带真实 id；所以测试在拿到 id 之后、
 * 服务重启之前换一份回放路由（同一端口），再起服务。任务流这时已经停了。
 */

const FIXTURE_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures');
const REPO_ROOT = path.resolve(FIXTURE_DIR, '../../..');
const TITLE_A = '任务甲：查判重规则';
const TITLE_B = '任务乙：查重复链接';
const TITLE_C = '任务丙：查超时设置';
const TITLE_D = '任务丁：核对版本号';
const RECOVERY_NOTE = 'AIOS 在这个任务进行中重启了';
const RECOVERY_TAIL = '已经做完的外部动作不要再做一遍';

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function taskPane(page: Page) {
  return page.locator('[data-pane="task-conversation"]');
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

async function rootLog(fixture: KernelFixture): Promise<string> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  return readFile(path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'), 'utf8');
}

async function rootEvents(fixture: KernelFixture): Promise<{ type: string; data?: Record<string, any> }[]> {
  const text = await rootLog(fixture);
  return text
    .trim()
    .split(String.fromCharCode(10))
    .slice(1)
    .map((line) => JSON.parse(line));
}

async function taskIdsOf(fixture: KernelFixture, count: number): Promise<string[]> {
  return waitFor('日志里 ' + count + ' 个任务 id', async () => {
    const text = await rootLog(fixture);
    const ids = [...text.matchAll(/已派出任务 ([0-9a-f]+)。/g)].map((match) => match[1]);
    return ids.length >= count ? ids.slice(0, count) : null;
  });
}

async function waitTaskRunning(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  taskId: string,
) {
  await waitFor('任务跑起来：' + taskId, async () => {
    const response = await apiPost(request, fixture, { method: 'task-read', taskId });
    if (!response.ok()) return null;
    return (await response.json()).task.running === true ? true : null;
  });
}

async function waitTaskReported(fixture: KernelFixture, taskId: string) {
  await waitFor('任务报回：' + taskId, async () => {
    const text = await rootLog(fixture);
    return text.includes('task-report:' + taskId + ':') ? true : null;
  });
}

interface StoredTask {
  runs: { attempts: { id: number; conversation?: string | null; trigger?: string; end?: string | null }[] }[];
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

function latestAttempt(task: StoredTask) {
  const attempts = task.runs.flatMap((run) => run.attempts);
  return { count: attempts.length, attempt: attempts[attempts.length - 1] };
}

async function taskReadHeader(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  taskId: string,
) {
  const response = await apiPost(request, fixture, { method: 'task-read', taskId });
  expect(response.ok()).toBeTruthy();
  return (await response.json()).task as {
    attempts: number;
    interrupted: boolean;
    restartPending: boolean;
    running: boolean;
  };
}

async function writeRestartRoutes(
  fixture: KernelFixture,
  scenario: 'main' | 'single',
  ids: string[],
): Promise<string> {
  const templates: Record<string, { id: string; title: string }[]> = {
    main: [
      { id: ids[0], title: TITLE_A },
      { id: ids[1], title: TITLE_B },
    ],
    single: [{ id: ids[0], title: TITLE_A }],
  };
  let template = await readFile(
    path.join(FIXTURE_DIR, 't029-kind-restart-' + scenario + '.template.jsonl'),
    'utf8',
  );
  template = template
    .replaceAll('__TASK_A_ID__', templates[scenario][0].id)
    .replaceAll('__TASK_A_TITLE__', templates[scenario][0].title)
    .replaceAll(
      '__TASK_TITLES__',
      templates[scenario].map((item) => item.title).join('、'),
    );
  if (templates[scenario][1]) {
    template = template
      .replaceAll('__TASK_B_ID__', templates[scenario][1].id)
      .replaceAll('__TASK_B_TITLE__', templates[scenario][1].title);
  }
  const restartPath = path.join(fixture.dataDir, 't029-kind-restart.jsonl');
  await writeFile(restartPath, template);
  const base = JSON.parse(
    await readFile(path.join(FIXTURE_DIR, 't029-routes-' + scenario + '.json'), 'utf8'),
  ) as { routes: { contains: string; file: string; paceMs?: number }[] };
  const routes = [
    { contains: 'tasks_interrupted', file: restartPath, paceMs: 120 },
    ...base.routes.map((route) => ({
      ...route,
      file: path.join(FIXTURE_DIR, route.file),
    })),
  ];
  const routesPath = path.join(fixture.dataDir, 't029-routes-generated.json');
  await writeFile(routesPath, JSON.stringify({ routes }, null, 2));
  return routesPath;
}

/** 再起一个回放（新路由）并把连接的 endpoint 换过去；返回新回放地址。 */
async function switchReplay(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  routesFile: string,
): Promise<string> {
  const switchReplay = fixture.switchReplay;
  if (!switchReplay) throw new Error('夹具不支持换回放');
  const url = await switchReplay(routesFile);
  const saved = await apiPost(request, fixture, {
    method: 'connection-update',
    id: 'deepseek',
    endpoint: url,
    model: 'deepseek-flash',
  });
  expect(saved.ok()).toBeTruthy();
  return url;
}

async function lastRequestOn(url: string): Promise<unknown> {
  const response = await fetch(url + '/__replay/last-request');
  const body = (await response.json()) as { request?: unknown };
  return body.request ?? null;
}

async function launchTasks(
  page: Page,
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  count: number,
  message: string,
) {
  await page.goto(urlOf(fixture));
  await sendKind(page, message);
  return taskIdsOf(fixture, count);
}

async function restartMessages(fixture: KernelFixture) {
  return waitFor('Kind 收到重启汇报', async () => {
    const events = await rootEvents(fixture);
    const found: { rpcId: string; body: any }[] = [];
    for (const event of events) {
      if (event.type !== 'user/message') continue;
      const message = event.data?.message as
        | { source?: { kind?: string; rpcId?: string }; content?: { type?: string; text?: string }[] }
        | undefined;
      const rpcId = message?.source?.rpcId;
      if (message?.source?.kind !== 'user' || typeof rpcId !== 'string' || !rpcId.startsWith('restart:')) continue;
      const text = (message.content ?? [])
        .map((block) => (block.type === 'text' ? block.text ?? '' : ''))
        .join('');
      try {
        found.push({ rpcId, body: JSON.parse(text) });
      } catch {
        // 不是重启 JSON 的用户消息，不算。
      }
    }
    return found.length > 0 ? found : null;
  });
}

async function kindTurnCount(fixture: KernelFixture): Promise<number> {
  const text = await rootLog(fixture);
  return text.split('"type":"turn/start"').length - 1;
}

/** 按真实任务 id 生成重启汇报的回放，换连接后再重启同一端口的 serve。 */
async function restartAfterInterruption(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  scenario: 'main' | 'single',
  ids: string[],
) {
  const routes = await writeRestartRoutes(fixture, scenario, ids);
  const replayUrl = await switchReplay(request, fixture, routes);
  await fixture.restart?.();
  return { reports: await restartMessages(fixture), replayUrl };
}

test('重启接续：汇报与每任务一项的卡，勾一项接续、没勾的停着；ready 与待答不受影响', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ t029Restart: 'main' });
  try {
    // 四个任务：甲/乙跑长流，丙报 ready，丁带问题报回。席位放开，四个都开跑。
    await apiPost(request, fixture, { method: 'settings-update', seats: 8 });
    const ids = await launchTasks(page, request, fixture, 4, '帮我派四个任务');
    const [taskA, taskB, taskC, taskD] = ids;
    await Promise.all([
      waitTaskRunning(request, fixture, taskA),
      waitTaskRunning(request, fixture, taskB),
      waitTaskReported(fixture, taskC),
      waitTaskReported(fixture, taskD),
    ]);


    // 中途重启：甲/乙这次 Attempt 没结束；丙/丁已经交过报告。
    const beforeA = latestAttempt(taskRow(fixture, taskA));
    const beforeB = latestAttempt(taskRow(fixture, taskB));
    const restarted = await restartAfterInterruption(request, fixture, 'main', ids);
    const reports = restarted.reports;
    expect(reports).toHaveLength(1);
    expect(reports[0].body.kind).toBe('tasks_interrupted');
    const listed = reports[0].body.tasks as { id: string; title: string; attempts: number }[];
    expect(listed.map((task) => task.title).sort()).toEqual([TITLE_A, TITLE_B].sort());
    expect(listed.every((task) => task.attempts === 1)).toBe(true);
    expect(listed.some((task) => task.id === taskC || task.id === taskD)).toBe(false);

    // Kind 对话：两张「被打断了」结果卡，每张都能打开任务对话；丙/丁不在里面。
    await page.goto(urlOf(fixture));
    const interrupted = page.locator('[data-kind="task-result"][data-state="interrupted"]');
    await expect(interrupted).toHaveCount(2, { timeout: 30_000 });
    for (const title of [TITLE_A, TITLE_B]) {
      const card = interrupted.filter({ hasText: title }).first();
      await expect(card).toContainText('被打断了');
      await expect(card.getByRole('button', { name: /打开任务对话/ })).toBeVisible();
    }
    await expect(interrupted.filter({ hasText: TITLE_C })).toHaveCount(0);
    await expect(interrupted.filter({ hasText: TITLE_D })).toHaveCount(0);
    const questionCard = page
      .locator('[data-kind="task-result"][data-state="blocked"]')
      .filter({ hasText: TITLE_D });
    await expect(questionCard).toHaveCount(1);
    await expect(questionCard).toContainText('用哪个版本？');

    // Kind 出接续卡：两个选项，事实行固定；没勾之前任务在页头仍是「被打断了」。
    const card = page.locator('[data-kind="confirm-card"]').last();
    await expect(card).toHaveAttribute('data-state', 'pending', { timeout: 30_000 });
    const options = card.locator('[data-action="card-option"]');
    await expect(options).toHaveCount(2);
    for (const option of await options.all()) {
      await expect(option.locator('[data-kind="confirm-fact"]')).toContainText(
        '被打断了 → 新的 Attempt，接着做',
      );
    }

    const paneA = page.locator('[data-pane="task-conversation"]');
    await page.goto(urlOf(fixture, '#taskchat:' + taskA));
    await expect(paneA.locator('[data-kind="task-full-title"]')).toHaveText(TITLE_A, {
      timeout: 20_000,
    });
    await expect(paneA.locator('[data-kind="task-state"]')).toHaveText('被打断了');
    // 汇报已经送到：待核实横幅消失，但状态还在。
    await expect(paneA.locator('[data-kind="await-kind"]')).toHaveCount(0);

    // 没勾的乙也停着。
    await page.goto(urlOf(fixture, '#taskchat:' + taskB));
    await expect(taskPane(page).locator('[data-kind="task-state"]')).toHaveText('被打断了');

    // 回 Kind 勾甲、确认：回执「已执行」，乙仍是「被打断了」。
    await page.goto(urlOf(fixture));
    const cardAgain = page.locator('[data-kind="confirm-card"]').last();
    await expect(cardAgain).toHaveAttribute('data-state', 'pending');
    await cardAgain.locator('[data-action="card-option"]').filter({ hasText: TITLE_A }).click();
    await cardAgain.locator('[data-action="card-confirm"]').click();
    await expect(cardAgain).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(cardAgain.locator('[data-kind="confirm-receipt"]')).toContainText('已执行');

    // 库里：甲的 Attempt +1、trigger resume、会话 id 没变；乙还是 1 次。
    const afterA = latestAttempt(taskRow(fixture, taskA));
    expect(afterA.count).toBe(2);
    expect(afterA.attempt.trigger).toBe('resume');
    expect(afterA.attempt.conversation).toBe(beforeA.attempt.conversation);
    expect(latestAttempt(taskRow(fixture, taskB)).count).toBe(1);

    // 甲的任务对话：第 2 次 Attempt，第一条输入是恢复说明（先看现场、外部动作不重做）。
    await page.goto(urlOf(fixture, '#taskchat:' + taskA));
    const paneAfter = taskPane(page);
    await expect(paneAfter.locator('[data-kind="task-attempt"]')).toHaveText('第 2 次 Attempt', {
      timeout: 20_000,
    });
    const note = paneAfter.locator('[data-kind="user-bubble"]').filter({ hasText: RECOVERY_NOTE });
    await expect(note).toHaveCount(1, { timeout: 20_000 });
    await expect(note).toContainText(RECOVERY_TAIL);
    await expect(note).toContainText('先看当前的文件和状态');

    // 新的结果卡回到 Kind。
    await page.goto(urlOf(fixture));
    await expect(
      page
        .locator('[data-kind="task-result"][data-state="ready"]')
        .filter({ hasText: '接着之前的结果做完了' }),
    ).toHaveCount(1, { timeout: 30_000 });

    // 故事5：重启前已交 ready 的丙阶段不变、不在卡上；页头没有「被打断了」。
    await page.goto(urlOf(fixture, '#taskchat:' + taskC));
    await expect(taskPane(page).locator('[data-kind="task-full-title"]')).toHaveText(TITLE_C);
    await expect(taskPane(page).locator('[data-kind="task-state"]')).toHaveCount(0);
    await expect(taskPane(page).locator('[data-kind="task-attempt"]')).toHaveCount(0);

    // 故事6：丁的待答问题还在 Kind 的结果卡上；在任务对话回答后开第 2 次 Attempt。
    await page.goto(urlOf(fixture, '#taskchat:' + taskD));
    const paneD = taskPane(page);
    await expect(paneD.locator('[data-kind="task-full-title"]')).toHaveText(TITLE_D);
    await paneD.locator('[data-kind="composer-input"]').fill('用 2.1 版');
    await paneD.locator('[data-kind="composer-input"]').press('Enter');
    await expect(paneD.locator('[data-kind="task-attempt"]')).toHaveText('第 2 次 Attempt', {
      timeout: 30_000,
    });
    await expect(
      paneD.locator('[data-kind="user-bubble"]').filter({ hasText: '用 2.1 版' }),
    ).toHaveCount(1);
    expect(latestAttempt(taskRow(fixture, taskD)).count).toBe(2);

    // 乙还是被打断了，没被别人的接续带跑。
    await page.goto(urlOf(fixture, '#taskchat:' + taskB));
    await expect(taskPane(page).locator('[data-kind="task-state"]')).toHaveText('被打断了');
    expect(latestAttempt(taskRow(fixture, taskB)).count).toBe(1);

    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('任务对话里先说「继续」：开新 Attempt，请求里恢复说明在用户话前；再点卡上那项回「已经接着做了」', async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t029Restart: 'single' });
  try {
    await apiPost(request, fixture, { method: 'settings-update', seats: 8 });
    const [taskA] = await launchTasks(page, request, fixture, 1, '帮我派一个任务');
    await waitTaskRunning(request, fixture, taskA);
    const before = latestAttempt(taskRow(fixture, taskA));
    await restartAfterInterruption(request, fixture, 'single', [taskA]);

    // 卡在；先去任务对话说一句（也算同意）。
    await page.goto(urlOf(fixture));
    const card = page.locator('[data-kind="confirm-card"]').last();
    await expect(card).toHaveAttribute('data-state', 'pending', { timeout: 30_000 });
    await expect(card.locator('[data-action="card-option"]')).toHaveCount(1);

    await page.goto(urlOf(fixture, '#taskchat:' + taskA));
    const pane = taskPane(page);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('被打断了', {
      timeout: 20_000,
    });
    await pane.locator('[data-kind="composer-input"]').fill('把剩下的补完');
    await pane.locator('[data-kind="composer-input"]').press('Enter');
    await expect(pane.locator('[data-kind="task-attempt"]')).toHaveText('第 2 次 Attempt', {
      timeout: 30_000,
    });
    const said = pane.locator('[data-kind="user-bubble"]').filter({ hasText: '把剩下的补完' });
    await expect(said).toHaveCount(1);
    await expect(said).toContainText(RECOVERY_NOTE);

    // 日志里是同一条消息：恢复说明块在前、用户的话在后。
    const read = await apiPost(request, fixture, { method: 'task-read', taskId: taskA });
    expect(read.ok()).toBeTruthy();
    const events = (await read.json()).events as {
      type: string;
      data: { message?: { source?: { rpcId?: string }; content?: { type?: string; text?: string }[] } };
    }[];
    const resumed = events.find((event) => {
      const rpcId = event.data.message?.source?.rpcId;
      return event.type === 'user/message' && typeof rpcId === 'string' && rpcId.startsWith('resume:');
    });
    expect(resumed, '任务日志里有接续的那条输入').toBeTruthy();
    const blocks = (resumed?.data.message?.content ?? []).map((block) => block.text ?? '');
    expect(blocks[0]).toContain(RECOVERY_NOTE);
    expect(blocks[0]).toContain(RECOVERY_TAIL);
    expect(blocks.slice(1).join('')).toContain('把剩下的补完');

    const afterSay = latestAttempt(taskRow(fixture, taskA));
    expect(afterSay.count).toBe(2);
    expect(afterSay.attempt.conversation).toBe(before.attempt.conversation);

    // 再点卡上那一项：reconcile 回「已经接着做了」，不再开第三次。
    await page.goto(urlOf(fixture));
    const pending = page.locator('[data-kind="confirm-card"]').last();
    await expect(pending).toHaveAttribute('data-state', 'pending');
    await pending.locator('[data-action="card-option"]').click();
    await pending.locator('[data-action="card-confirm"]').click();
    await expect(pending).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(pending.locator('[data-kind="confirm-receipt"]')).toContainText('已经接着做了');
    expect(latestAttempt(taskRow(fixture, taskA)).count).toBe(2);
  } finally {
    await fixture.close();
  }
});

test('模型不可用时：页头「被打断了 · 待 Kind 核实，尚未接续」，Kind 没请求、任务不自己跑；恢复后汇报，暂不后不再追问', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const fixture = await startKernelFixture({ t029Restart: 'single' });
  try {
    await apiPost(request, fixture, { method: 'settings-update', seats: 8 });
    const [taskA] = await launchTasks(page, request, fixture, 1, '帮我派一个任务');
    await waitTaskRunning(request, fixture, taskA);

    // 先把连接指到新回放；再把 Keyring 拿走：服务起得来，但根会话/模型不可用。
    const { rm } = await import('node:fs/promises');
    const routes = await writeRestartRoutes(fixture, 'single', [taskA]);
    const replayUrl = await switchReplay(request, fixture, routes);
    await rm(path.join(fixture.dataDir, 'keyring.age'), { force: true });
    await fixture.restart?.();
    await new Promise((resolve) => setTimeout(resolve, 500));

    // 任务对话能打开：页头固定「被打断了」，横幅「待 Kind 核实」。
    await page.goto(urlOf(fixture, '#taskchat:' + taskA));
    const pane = taskPane(page);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('被打断了', {
      timeout: 20_000,
    });
    const banner = pane.locator('[data-kind="await-kind"]');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('待 Kind 核实，尚未接续');
    await expect(
      pane.locator('[data-kind="task-attempt"]'),
    ).toHaveCount(0);
    const header = await taskReadHeader(request, fixture, taskA);
    expect(header.attempts).toBe(1);
    expect(header.interrupted).toBe(true);
    expect(header.restartPending).toBe(true);
    // 模型不可用：新的回放进程没收到请求；任务也没自己跑。
    await new Promise((resolve) => setTimeout(resolve, 800));
    expect(await lastRequestOn(replayUrl)).toBeNull();
    expect((await taskReadHeader(request, fixture, taskA)).attempts).toBe(1);

    // 把 Keyring 放回去、recheck：根会话接管，restart 汇报送出去。
    const bin = process.env.AIOS_BIN ?? path.join(REPO_ROOT, 'target/debug/aios-dev');
    await new Promise<void>((resolve, reject) => {
      const child = spawn(
        bin,
        [
          'keyring',
          '--data-dir',
          path.join(fixture.dataDir),
          '--identity',
          path.join(fixture.dataDir, 'identity.txt'),
          'set',
          '--id',
          'deepseek',
          '--name',
          'DeepSeek',
          '--preset',
          'deepseek',
          '--model',
          'deepseek-flash',
          '--endpoint',
          replayUrl,
        ],
        { stdio: ['pipe', 'pipe', 'pipe'] },
      );
      child.stdin.end('test-key' + String.fromCharCode(10));
      child.on('exit', (code) => (code === 0 ? resolve() : reject(new Error('keyring set 失败：' + code))));
      child.on('error', reject);
    });
    await apiPost(request, fixture, { method: 'recheck' });
    await restartMessages(fixture);
    await waitFor('restartPending 转假', async () => {
      const next = await taskReadHeader(request, fixture, taskA);
      return next.restartPending === false ? true : null;
    });

    // Kind 出卡；先「不理」一会儿：不再有新的汇报、也不叫模型。
    await page.goto(urlOf(fixture));
    const card = page.locator('[data-kind="confirm-card"]').last();
    await expect(card).toHaveAttribute('data-state', 'pending', { timeout: 30_000 });
    const messagesBefore = (await restartMessages(fixture)).length;
    const lastRequest = await lastRequestOn(replayUrl);
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect((await restartMessages(fixture)).length).toBe(messagesBefore);
    expect(await lastRequestOn(replayUrl)).toEqual(lastRequest);

    // 点暂不：任务一直停着、不过期，也不再追问。
    await card.locator('[data-action="card-dismiss"]').click();
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(card.locator('[data-kind="confirm-receipt"]')).toContainText('暂不');
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect((await restartMessages(fixture)).length).toBe(messagesBefore);
    expect(await lastRequestOn(replayUrl)).toEqual(lastRequest);
    const after = await taskReadHeader(request, fixture, taskA);
    expect(after.attempts).toBe(1);
    expect(after.interrupted).toBe(true);

    // 横幅在汇报送到后消失；页头状态还在。
    await page.goto(urlOf(fixture, '#taskchat:' + taskA));
    await expect(taskPane(page).locator('[data-kind="task-state"]')).toHaveText('被打断了');
    await expect(taskPane(page).locator('[data-kind="await-kind"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
