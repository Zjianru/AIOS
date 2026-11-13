import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T043：任务对话输入框下补任务级统计（只统计当前任务当前 Attempt，不串 Kind 根会话）。
 * 数字以任务会话日志为准（测试自己数日志），界面上不允许出现 Kind 全局数。
 */

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

    // 任务自己的会话日志数出来的数（当前 Attempt）。
    const counts = countsOf(await taskLogText(fixture));
    const activity = dock.locator('[data-kind="stats-activity"]');
    await expect(numbers(activity)).toHaveText([String(counts.turns), String(counts.steps)]);
    const usage = dock.locator('[data-kind="stats-usage"]');
    const usageNumbers = await numbers(usage).allTextContents();
    expect(usageNumbers[0]).toBe(String(counts.input));
    expect(usageNumbers[1]).toBe(String(counts.output));

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

test('模型价格/窗口匹配不到：对应项隐藏，只留可靠统计', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('查一下新闻归档里重复链接的判重规则和影响范围');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskId] = await taskIdOf(fixture);
    await waitUntilReadable(request, fixture, taskId);

    // 任务头只给模型显示名：把 task-read 回包里的名字换成目录里没有的，模拟「匹配不到」。
    // 其余字段原样放行（轮/步/token 仍来自真实事件）。
    await page.route('**/api', async (route) => {
      const postData = route.request().postData() ?? '';
      if (!postData.includes('"method":"task-read"')) {
        await route.continue();
        return;
      }
      try {
        const response = await route.fetch();
        const json = (await response.json()) as { task?: { model?: string } };
        if (json.task) json.task.model = '不存在的模型（T043 测试）';
        await route.fulfill({ response, json });
      } catch {
        // 测试收尾时夹具已关：把还没回来的请求直接断掉，不让它把用例判红。
        await route.abort().catch(() => undefined);
      }
    });
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    await expect(statsDock(page)).toBeVisible({ timeout: 20_000 });
    const dock = statsDock(page);
    await expect(dock.locator('[data-kind="stats-activity"]')).toBeVisible();
    await expect(dock.locator('[data-kind="stats-usage"]')).toBeVisible();
    await expect(dock.locator('[data-kind="stats-cost"]')).toHaveCount(0);
    await expect(dock.locator('[data-kind="context-meter"]')).toHaveCount(0);
    await page.unroute('**/api');
  } finally {
    await fixture.close();
  }
});
