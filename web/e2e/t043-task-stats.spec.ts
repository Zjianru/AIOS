import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T043/T051：任务对话输入框下的任务级统计条只显示服务端 task-read 的 stats
 * （和 Kind status 同一个 session_stats 函数、任务冻结的模型、当前 Attempt 全部事件）。
 * 页面上不允许出现 Kind 全局数，也不允许页面自己按模型显示名匹配价格/窗口。
 */

/** 和 web/src/lib/numbers.ts 的 formatCost 同一规则（币种、汇率、unpriced 的「+」）。 */
function formatCost(usd: number, currency: string, cnyPerUsd: number, partial: boolean): string {
  let amount = usd;
  let sign = '$';
  if (currency === 'cny' && cnyPerUsd > 0) {
    amount = usd * cnyPerUsd;
    sign = '¥';
  }
  const text = amount >= 100 ? amount.toFixed(0) : amount >= 1 ? amount.toFixed(2) : amount.toFixed(3);
  return sign + text + (partial ? '+' : '');
}

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function taskPane(page: import('@playwright/test').Page) {
  return page.locator('[data-pane="task-conversation"]');
}

function statsDock(page: import('@playwright/test').Page) {
  return taskPane(page).locator('[data-kind="task-stats"]');
}

async function taskIdOf(fixture: KernelFixture, count = 1): Promise<string[]> {
  for (let attempt = 0; attempt < 600; attempt += 1) {
    const root = JSON.parse(
      await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
    ) as { sessionId: string };
    const text = await readFile(
      path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
      'utf8',
    );
    const ids = [...text.matchAll(/已派出任务 ([0-9a-f]+)。/g)].map((match) => match[1]);
    if (ids.length >= count) return ids.slice(0, count);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('等不到任务 id');
}

/** 任务当前 Attempt 的会话日志（跳过 Kind 根会话）。 */
async function taskLogText(fixture: KernelFixture): Promise<string> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const files = await readdir(path.join(fixture.dataDir, 'sessions'));
  for (const file of files) {
    if (file === root.sessionId + '.jsonl') continue;
    const text = await readFile(path.join(fixture.dataDir, 'sessions', file), 'utf8');
    if (text.includes('"rpcId":"assignment:')) return text;
  }
  throw new Error('找不到任务会话日志');
}

interface LogCounts {
  turns: number;
  steps: number;
  input: number;
  output: number;
}

function countsOf(log: string): LogCounts {
  let turns = 0;
  let steps = 0;
  let input = 0;
  let output = 0;
  for (const line of log.trim().split(String.fromCharCode(10)).slice(1)) {
    const row = JSON.parse(line) as { type?: string; data?: { usage?: { inputTokens?: number; outputTokens?: number } } };
    if (row.type === 'turn/start') turns += 1;
    if (row.type === 'step/start') steps += 1;
    if (row.type === 'assistant/message') {
      input += row.data?.usage?.inputTokens ?? 0;
      output += row.data?.usage?.outputTokens ?? 0;
    }
  }
  return { turns, steps, input, output };
}

async function waitUntilReadable(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  taskId: string,
): Promise<void> {
  for (let attempt = 0; attempt < 600; attempt += 1) {
    const response = await apiPost(request, fixture, { method: 'task-read', taskId });
    if (response.ok()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('TaskRead 一直打不开');
}

function numbers(locator: import('@playwright/test').Locator) {
  return locator.locator('[data-kind="stat-number"]');
}

test('任务对话统计条：轮/步/token 与任务会话日志一致，不等于 Kind 根会话', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('查一下新闻归档里重复链接的判重规则和影响范围');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskId] = await taskIdOf(fixture);
    await waitUntilReadable(request, fixture, taskId);
    // 等任务真的收尾（收尾事件写全，统计才有完整数）。
    for (let attempt = 0; attempt < 800; attempt += 1) {
      const read = await apiPost(request, fixture, { method: 'task-read', taskId });
      if (read.ok()) {
        const body = (await read.json()) as { task: { reportedAt?: number | null; running: boolean } };
        if (body.task.reportedAt != null && !body.task.running) break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    await expect(taskPane(page)).toBeVisible({ timeout: 20_000 });
    const dock = statsDock(page);
    await expect(dock).toBeVisible({ timeout: 20_000 });

    // 页面显示的是服务端 task-read 的 stats；数字必须和它一致（不是页面自己数事件/匹配价格）。
    const serverRead = await apiPost(request, fixture, { method: 'task-read', taskId });
    const readBody = (await serverRead.json()) as {
      stats?: {
        turns: number;
        steps: number;
        inputTokens: number;
        outputTokens: number;
        cacheReadTokens: number;
        cacheWriteTokens: number;
        costUsd?: number | null;
        unpriced: boolean;
      };
      fx?: { rate: number } | null;
      currency?: string | null;
    };
    const serverStats = readBody.stats;
    if (!serverStats) throw new Error('task-read 要带 stats');
    const activity = dock.locator('[data-kind="stats-activity"]');
    await expect(numbers(activity)).toHaveText([
      String(serverStats.turns),
      String(serverStats.steps),
    ]);
    const usage = dock.locator('[data-kind="stats-usage"]');
    const usageNumbers = await numbers(usage).allTextContents();
    expect(usageNumbers[0]).toBe(
      String(
        serverStats.inputTokens +
          serverStats.cacheReadTokens +
          serverStats.cacheWriteTokens,
      ),
    );
    expect(usageNumbers[1]).toBe(String(serverStats.outputTokens));

    // 花费：服务端按目录标价算出来的 costUsd，页面用同一个 formatCost 显示（币种/汇率也来自服务端）。
    if (serverStats.costUsd == null) throw new Error('这个场景目录有价，stats.costUsd 不该为空');
    const unit = (readBody.currency ?? 'usd') === 'cny' ? 'cny' : 'usd';
    const expectedCost = formatCost(
      serverStats.costUsd,
      unit,
      readBody.fx?.rate ?? 7.2,
      serverStats.unpriced,
    );
    await expect(
      dock.locator('[data-kind="stats-cost"] [data-kind="stat-number"]'),
    ).toHaveText(expectedCost);

    // 日志数出来的数只用来做「不是 Kind 根会话全局数」的对照（页面不再按日志显示）。
    const counts = countsOf(await taskLogText(fixture));

    // 不等于 Kind 根会话的全局数（场景里 Kind 日志数出来跟任务不同）。
    const root = JSON.parse(
      await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
    ) as { sessionId: string };
    const rootCounts = countsOf(
      await readFile(path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'), 'utf8'),
    );
    expect(
      [counts.turns, counts.steps, counts.input, counts.output].join(','),
    ).not.toBe([rootCounts.turns, rootCounts.steps, rootCounts.input, rootCounts.output].join(','));

    // 统计条在输入框下面、说明文字上面；Kind 的统计条不在这页。
    await expect(taskPane(page).locator('[data-kind="composer-dock"]')).toHaveCount(0);
    await expect(taskPane(page).locator('[data-kind="task-footer"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('A/B 任务切换：统计数字跟着当前任务走', async ({ page, request }) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t029Restart: 'main' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('把四个任务都派出去');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const ids = await taskIdOf(fixture, 4);
    const [first, third] = [ids[0], ids[2]];
    await waitUntilReadable(request, fixture, first);
    await waitUntilReadable(request, fixture, third);
    // 等甲真的跑出几步、丙收尾：两边事件不同了再比（刚派出时都还是 1 轮 1 步）。
    for (let attempt = 0; attempt < 400; attempt += 1) {
      const read = await apiPost(request, fixture, { method: 'task-read', taskId: first });
      if (read.ok()) {
        const body = (await read.json()) as { events: { type: string }[] };
        if (body.events.filter((event) => event.type === 'step/start').length >= 2) break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    await page.goto(urlOf(fixture, '#taskchat:' + first));
    await expect(statsDock(page)).toBeVisible({ timeout: 20_000 });
    const firstText = (await statsDock(page).locator('[data-kind="stats-activity"]').innerText()).trim();
    const firstUsage = (await statsDock(page).locator('[data-kind="stats-usage"]').innerText()).trim();

    await page.goto(urlOf(fixture, '#taskchat:' + third));
    await expect(statsDock(page)).toBeVisible({ timeout: 20_000 });
    const thirdText = (await statsDock(page).locator('[data-kind="stats-activity"]').innerText()).trim();
    const thirdUsage = (await statsDock(page).locator('[data-kind="stats-usage"]').innerText()).trim();

    // 甲（长脚本）和丙（ready 脚本）事件不同：数字不能一样，也不能沿用上一个任务的。
    expect(firstText + '|' + firstUsage).not.toBe(thirdText + '|' + thirdUsage);
  } finally {
    await fixture.close();
  }
});

test('服务端没有价格/窗口（模型不在目录）：花费与占比隐藏，只留可靠统计', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  // 未声明路由的假模型不在目录里：task-read 的 stats 没有目录价、窗口查不到（0），
  // 页面只显示服务端给得出的轮/步/token；花费与占比隐藏由服务端数据驱动，不再靠页面按名字匹配。
  const fixture = await startKernelFixture({
    taskResult: 'live',
    routeModel: 'replay-uncatalogued-model',
  });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('查一下新闻归档里重复链接的判重规则和影响范围');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskId] = await taskIdOf(fixture);
    await waitUntilReadable(request, fixture, taskId);
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    await expect(statsDock(page)).toBeVisible({ timeout: 20_000 });
    const dock = statsDock(page);
    const read = await apiPost(request, fixture, { method: 'task-read', taskId });
    const body = (await read.json()) as { stats?: { costUsd?: number | null } };
    expect(body.stats?.costUsd ?? null).toBeNull();
    await expect(dock.locator('[data-kind="stats-activity"]')).toBeVisible();
    await expect(dock.locator('[data-kind="stats-usage"]')).toBeVisible();
    await expect(dock.locator('[data-kind="stats-cost"]')).toHaveCount(0);
    await expect(dock.locator('[data-kind="context-meter"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
