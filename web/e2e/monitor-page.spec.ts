import { test, expect, type Page } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('监控：消耗、在跑、诊断三块走服务端快照，只读', async ({ page }) => {
  test.setTimeout(180_000);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const snapshotCalls: string[] = [];
  const sockets: string[] = [];
  page.on('request', (request) => {
    if (request.method() !== 'POST' || !request.url().endsWith('/api')) return;
    try {
      const body = request.postDataJSON() as { method?: string };
      if (body.method === 'monitor-snapshot') snapshotCalls.push(request.url());
    } catch {
      // 不是 JSON 的请求不算。
    }
  });
  page.on('websocket', (socket) => sockets.push(socket.url()));
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const result = page.locator('[data-kind="task-result"]').last();
    await expect(result).toBeVisible({ timeout: 60_000 });

    await page.goto(urlOf(fixture, '#monitor'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('heading', { name: '监控' })).toBeVisible();
    await expect(page.locator('[data-tab-key="cost"]')).toBeVisible();
    await expect(page.locator('[data-tab-key="live"]')).toBeVisible();
    await expect(page.locator('[data-kind="cost-daily"]')).toBeVisible();
    await expect(page.locator('[data-kind="kpis"]')).toBeVisible();
    // 项目筛：没建项目时仍有「全部项目 / 个人」；任务归属显示「个人」。
    await expect(page.locator('[data-kind="project-filter"]')).toContainText('全部项目');
    await expect(page.locator('[data-kind="project-filter"]')).toBeVisible();
    await expect(page.locator('[data-kind="task-project"]').first()).toContainText('个人');
    // 缺数据画斜线，不拿 0 顶。
    await expect(page.locator('[data-kind="missing"]').first()).toBeVisible();
    // 监控 WS 已连；打开后不在固定间隔里拉快照。
    expect(sockets.some((url) => url.includes('monitor=1'))).toBeTruthy();
    await page.waitForTimeout(3000);
    const callsBefore = snapshotCalls.length;
    await page.waitForTimeout(1500);
    expect(snapshotCalls.length).toBe(callsBefore);

    // 已知任务出现在任务卡；点开进入诊断。
    const card = page.locator('[data-kind="task-card"]').filter({ hasText: '判重改成按' }).first();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await card.click();
    await expect(page.locator('[data-page="monitor-task"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="kpis"]')).toBeVisible();
    await expect(page.locator('[data-kind="ctx-card"]')).toBeVisible();
    await expect(page.locator('[data-kind="trace"]')).toBeVisible();
    await expect(page.locator('[data-kind="event-table"]')).toBeVisible();
    // 只读：没有停止按钮。
    await expect(page.getByRole('button', { name: '立即停' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: '收尾停' })).toHaveCount(0);

    // 在跑页：服务端快照的席位/在跑/排队/异常事实都在；没有在跑任务时显示空态。
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-tab="live"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="kpis"]')).toBeVisible();

    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('在跑页的异常事实列表只显示服务端来的 detail', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture();
  try {
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method !== 'monitor-snapshot') {
        await route.continue();
        return;
      }
      const response = await route.fetch();
      const body = (await response.json()) as {
        anomalies?: { kind: string; taskId?: string; detail: string }[];
      };
      body.anomalies = [
        { kind: 'request-longest', detail: '本次 14分钟（在飞），此前最长 21秒' },
        { kind: 'same-command', detail: '连跑 2 次，结果一样：cargo test -q' },
      ];
      await route.fulfill({ response, json: body });
    });
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    const facts = page.locator('[data-kind="monitor-fact"]');
    await expect(facts).toHaveCount(2, { timeout: 20_000 });
    await expect(facts.nth(0)).toContainText('本次 14分钟（在飞），此前最长 21秒');
    await expect(facts.nth(1)).toContainText('连跑 2 次，结果一样：cargo test -q');
  } finally {
    await fixture.close();
  }
});

test('监控 WS：开页后新任务 TaskEvent，双任务 seq 差大重连不漏不重', async ({ page, browser }) => {
  test.setTimeout(180_000);
  // t029 的就绪夹具：一条 Kind 会话里 4 个 task_accept，甲乙跑长流；不新造夹具。
  const fixture = await startKernelFixture({ t029Restart: 'main', t075TwoTasks: true });
  const frames: { taskId: string; seq: number }[] = [];
  page.on('websocket', (socket) => {
    socket.on('framereceived', (received) => {
      try {
        const raw = String(received.payload);
        const frame = JSON.parse(raw) as { type?: string; taskId?: string; event?: { seq?: number } };
        if (frame.type === 'task-event' && frame.taskId && typeof frame.event?.seq === 'number') {
          frames.push({ taskId: frame.taskId, seq: frame.event.seq });
        }
      } catch {
        // 非 JSON 帧不管。
      }
    });
  });
  const ids = () => [...new Set(frames.map((frame) => frame.taskId))];
  try {
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    const kindPage = await browser.newPage();
    try {
      await kindPage.goto(urlOf(fixture, '#kind'));
      await sendKind(kindPage, '帮我派四个任务');
      await expect.poll(() => ids().length, { timeout: 60_000 }).toBeGreaterThanOrEqual(2);
      const taskIds = ids().slice(0, 2);
      expect(taskIds.length).toBe(2);
      // 两任务都跑出若干事件再断线；甲乙长流让 seq 继续拉大。
      await expect
        .poll(() => taskIds.every((taskId) => frames.filter((frame) => frame.taskId === taskId).length >= 3), {
          timeout: 60_000,
        })
        .toBe(true);
      const before = frames.length;
      await page.reload();
      await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
      // 页面重载关掉旧 WS；新开一条真实 WS 从各任务 0 补，收集 3 秒。
      const reconnected = await page.evaluate(async () => {
        const protocol = location.protocol === 'https:' ? 'wss' : 'ws';
        const socket = new WebSocket(protocol + '://' + location.host + '/ws?monitor=1&lastSeq=0');
        const got: { taskId: string; seq: number }[] = [];
        await new Promise<void>((resolve) => {
          const done = setTimeout(() => {
            socket.close();
            resolve();
          }, 3000);
          socket.onmessage = (message) => {
            try {
              const frame = JSON.parse(String(message.data)) as {
                type?: string;
                taskId?: string;
                event?: { seq?: number };
              };
              if (frame.type === 'task-event' && frame.taskId && typeof frame.event?.seq === 'number') {
                got.push({ taskId: frame.taskId, seq: frame.event.seq });
              }
            } catch {
              // 非 JSON 帧不管。
            }
          };
          void done;
        });
        return got;
      });
      for (const taskId of taskIds) {
        const pre = frames.slice(0, before).filter((frame) => frame.taskId === taskId).map((frame) => frame.seq);
        const after = reconnected.filter((frame) => frame.taskId === taskId).map((frame) => frame.seq);
        expect(pre.length, taskId + ' 首连要有帧').toBeGreaterThan(0);
        expect(after.length, taskId + ' 重连要补到').toBeGreaterThan(0);
        expect(after[0], taskId + ' 从 0 补').toBe(1);
        expect(new Set(after).size, taskId + ' 重连连接内重复').toBe(after.length);
        for (const seq of pre) expect(after, taskId + ' 重连补齐首连 seq ' + seq).toContain(seq);
      }
    } finally {
      await kindPage.close();
    }
  } finally {
    await fixture.close();
  }
});
test('监控跟着任务走：在跑卡不用刷新出现，诊断链路自己长', async ({ page, browser }) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ taskResult: 'live', slowMs: 2500 });
  try {
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    const kindPage = await browser.newPage();
    try {
      await kindPage.goto(urlOf(fixture, '#kind'));
      await sendKind(kindPage, '帮我查一下新闻归档里的判重规则');
      // 页面在任务开始前就开着：卡出现=快照跟事件刷了，没有手动刷新。
      const card = page.locator('[data-kind="live-card"]').first();
      await expect(card).toBeVisible({ timeout: 90_000 });
      await expect(page.locator('[data-kind="kpis"]')).toContainText('在跑');

      // 诊断页跟同一路 WS：事件表行数自己变多。
      await card.click();
      await expect(page.locator('[data-page="monitor-task"]')).toBeVisible({ timeout: 30_000 });
      const rows = page.locator('[data-kind="event-table"] tbody tr');
      await expect(rows.first()).toBeVisible({ timeout: 30_000 });
      const rowCount = await rows.count();
      await expect
        .poll(async () => rows.count(), { timeout: 90_000 })
        .toBeGreaterThan(rowCount);
    } finally {
      await kindPage.close();
    }
  } finally {
    await fixture.close();
  }
});

test('慢的旧快照不会盖掉新数据（代次丢弃）', async ({ page, browser }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  let snapshotCalls = 0;
  try {
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method !== 'monitor-snapshot') {
        await route.continue();
        return;
      }
      snapshotCalls += 1;
      if (snapshotCalls === 1) {
        // 第一次快照故意慢 1500ms，并带一个一眼能认出的旧值；第二次及以后是真实新值。
        await new Promise((resolve) => setTimeout(resolve, 1500));
        const response = await route.fetch();
        const body = (await response.json()) as {
          tasks?: { costUsd?: number | null }[];
          running?: { tasks?: { costUsd?: number | null }[] };
        };
        for (const task of body.tasks ?? []) task.costUsd = 999.999;
        for (const task of body.running?.tasks ?? []) task.costUsd = 999.999;
        await route.fulfill({ response, json: body });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    const kindPage = await browser.newPage();
    try {
      await kindPage.goto(urlOf(fixture, '#kind'));
      await sendKind(kindPage, '帮我查一下新闻归档里的判重规则');
      await expect(page.locator('[data-kind="live-card"]').first()).toBeVisible({ timeout: 90_000 });
      // 第一次慢快照在第二次之后回来；代次丢弃后页面不能出现旧值。
      await page.waitForTimeout(2500);
      await expect(page.locator('[data-kind="live-card"]').first()).toBeVisible();
      await expect(page.getByText('999.999')).toHaveCount(0);
    } finally {
      await kindPage.close();
    }
  } finally {
    await fixture.close();
  }
});
test('看旧 Attempt：新事件不把页面换成最新 Attempt', async ({ page, browser }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live', slowMs: 2500 });
  try {
    await page.route('**/api', async (route) => {
      let requestBody: { method?: string; attemptId?: number } = {};
      try {
        requestBody = route.request().postDataJSON() as { method?: string; attemptId?: number };
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (requestBody.method !== 'monitor-task') {
        await route.continue();
        return;
      }
      const response = await route.fetch();
      const json = (await response.json()) as Record<string, unknown>;
      const requested = requestBody.attemptId;
      json.attempts = [
        { attemptId: 1, startedAt: 1_700_000_000_000, endedAt: 1_700_000_100_000, requests: 1, inputTokens: 10, outputTokens: 5, unpriced: false },
        { attemptId: 2, startedAt: 1_700_000_200_000, requests: 1, inputTokens: 20, outputTokens: 5, unpriced: false },
      ];
      json.todo = [
        {
          content: requested === 1 ? '旧 Attempt 的待办' : '新 Attempt 的待办',
          status: 'in_progress',
        },
      ];
      await route.fulfill({ response, json });
    });
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    const kindPage = await browser.newPage();
    try {
      await kindPage.goto(urlOf(fixture, '#kind'));
      await sendKind(kindPage, '帮我查一下新闻归档里的判重规则');
      const card = page.locator('[data-kind="live-card"]').first();
      await expect(card).toBeVisible({ timeout: 90_000 });
      await card.click();
      await expect(page.locator('[data-page="monitor-task"]')).toBeVisible({ timeout: 30_000 });
      const oldAttempt = page.locator('[data-kind="diagnose-attempt"][data-attempt-id="1"]');
      await expect(oldAttempt).toBeVisible({ timeout: 30_000 });
      await oldAttempt.click();
      await expect(page.getByText('旧 Attempt 的待办')).toBeVisible({ timeout: 20_000 });
      await page.waitForTimeout(4000);
      await expect(page.getByText('旧 Attempt 的待办')).toBeVisible();
      await expect(page.getByText('新 Attempt 的待办')).toHaveCount(0);
      await expect(oldAttempt).toHaveClass(/bg-active/);
    } finally {
      await kindPage.close();
    }
  } finally {
    await fixture.close();
  }
});


test('在跑卡的花费/token/上下文跟着任务事件出现，不用刷新', async ({ page, browser }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live', slowMs: 8000 });
  try {
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method === 'monitor-snapshot') {
        const response = await route.fetch();
        const json = (await response.json()) as {
          tasks?: { running?: boolean; costUsd?: number | null; inputTokens?: number; outputTokens?: number; contextUsed?: number; contextWindow?: number }[];
          running?: { tasks?: { costUsd?: number | null; inputTokens?: number; outputTokens?: number; contextUsed?: number; contextWindow?: number }[] };
        };
        const running = json.running?.tasks ?? [];
        for (const task of json.tasks ?? []) {
          if (!task.running) continue;
          task.costUsd = 1.5;
          task.inputTokens = 1500;
          task.outputTokens = 500;
          task.contextUsed = 1234;
          task.contextWindow = 200_000;
        }
        for (const task of running) {
          task.costUsd = 1.5;
          task.inputTokens = 1500;
          task.outputTokens = 500;
          task.contextUsed = 1234;
          task.contextWindow = 200_000;
        }
        await route.fulfill({ response, json });
        return;
      }
      if (method === 'monitor-task') {
        const response = await route.fetch();
        const json = (await response.json()) as Record<string, unknown>;
        json.todo = [{ content: '事件里新增的待办', status: 'in_progress' }];
        json.contextParts = {
          fixedBlock: 100,
          toolDefinitions: 200,
          taskBookMessages: 300,
          modelReplies: 400,
          toolResults: 234,
        };
        await route.fulfill({ response, json });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    // 页面在任务开始前就开着：此刻没有在跑卡/数字。
    await expect(page.locator('[data-kind="live-card"]')).toHaveCount(0);
    const kindPage = await browser.newPage();
    try {
      await kindPage.goto(urlOf(fixture, '#kind'));
      await sendKind(kindPage, '帮我查一下新闻归档里的判重规则');
      const card = page.locator('[data-kind="live-card"]').first();
      // 任务事件到了以后卡和数字出现（页面没刷新、没手动重取）。
      await expect(card).toBeVisible({ timeout: 90_000 });
      await expect(card.locator('[data-kind="live-cost"]')).toHaveText('$1.500');
      await expect(card.locator('[data-kind="live-token"]')).toContainText('2K');
      await expect(card.locator('[data-kind="live-context"]')).toContainText('1.2K');
    } finally {
      await kindPage.close();
    }
  } finally {
    await fixture.close();
  }
});
test('监控照原型：五卡、项目标签、日柱叠色、两环、排行按项目、模型回复悬浮', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({});
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: '项目甲',
      goal: '看监控',
    });
    const body = (await created.json()) as { project: { id: string } };
    const projectId = body.project.id;
    const now = Date.now();
    const task = {
      taskId: 'task-1',
      title: '任务一',
      state: 'running',
      model: 'deepseek-flash',
      projectId,
      attemptId: 1,
      running: true,
      startedAt: now - 60_000,
      requests: 2,
      inputTokens: 1_000,
      outputTokens: 100,
      costUsd: 1.5,
      unpriced: false,
      toolCalls: 1,
      contextUsed: 1_000,
      contextWindow: 100_000,
    };
    const snapshot = {
      range: 'today',
      timezone: 'UTC',
      generatedAt: now,
      days: [
        {
          day: new Date().toLocaleDateString('sv-SE'),
          costUsd: 2,
          unpriced: 0,
          inputTokens: 1_000,
          outputTokens: 100,
          requests: 2,
          toolCalls: 1,
          commandCalls: 1,
          groups: [
            { projectId, label: '项目甲', costUsd: 1.5, unpriced: 0 },
            { label: 'Kind 对话', costUsd: 0.5, unpriced: 0 },
          ],
        },
        {
          day: new Date(Date.now() - 86_400_000).toLocaleDateString('sv-SE'),
          costUsd: 1,
          unpriced: 0,
          inputTokens: 0,
          outputTokens: 0,
          requests: 1,
          toolCalls: 0,
          commandCalls: 0,
          groups: [{ projectId, label: '项目甲', costUsd: 1, unpriced: 0 }],
        },
      ],
      tasks: [task],
      projects: [],
      models: [
        { key: 'deepseek-flash', label: 'deepseek-flash', projectId, requests: 2, inputTokens: 1_000, outputTokens: 100, cacheReadTokens: 500, cacheWriteTokens: 0, costUsd: 1.5, unpriced: false },
      ],
      tools: [{ key: 'read', label: 'read', projectId, requests: 1, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, costUsd: null, unpriced: false }],
      commands: [{ key: 'ls', label: 'ls', projectId, requests: 1, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, costUsd: null, unpriced: false }],
      rings: { costUsd: 2, unpriced: false, busyMs: 120_000, requestMs: 90_000, toolMs: 20_000, commandMs: 10_000, tokens: 1_100 },
      anomalies: [],
      running: { seats: 2, running: 1, queue: 0, anomalous: 0, tasks: [task] },
      queue: [],
    };
    const detail = {
      task: { taskId: 'task-1', title: '任务一', state: 'running', model: 'deepseek-flash', projectId, running: true },
      attempts: [{ attemptId: 1, startedAt: now - 60_000, requests: 2, inputTokens: 1_000, outputTokens: 100, costUsd: 1.5, unpriced: false }],
      contextParts: { fixedBlock: 10, toolDefinitions: 20, taskBookMessages: 30, modelReplies: 1_234, toolResults: 40 },
      todo: [{ content: '做一半', status: 'in_progress' }],
      events: [],
    };
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method === 'monitor-snapshot') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(snapshot) });
        return;
      }
      if (method === 'monitor-task') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(detail) });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#monitor'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });

    // 顶上五张卡。
    const kpis = page.locator('[data-kind="kpis"] [data-kind="kpi"]');
    await expect(kpis).toHaveCount(5);
    for (const label of ['花费', 'Token', '缓存命中', '模型请求', '在忙']) {
      await expect(kpis.filter({ hasText: label })).toHaveCount(1);
    }
    await expect(page.locator('[data-kind="kpi"]').filter({ hasText: '缓存命中' })).toContainText('50%');
    await expect(page.locator('[data-kind="kpi"]').filter({ hasText: '在忙' })).toContainText('02:00');

    // 项目筛选是一排标签。
    await expect(page.locator('[data-kind="monitor-project-tag"]').filter({ hasText: '全部项目' })).toBeVisible();
    await expect(page.locator('[data-kind="monitor-project-tag"]').filter({ hasText: '项目甲' })).toBeVisible();

    // 日柱按项目叠色（项目甲 + Kind），今天也画整月。
    await expect(page.locator('[data-kind="cost-day"]')).toHaveCount(2);
    await expect(
      page.locator('[data-kind="cost-bar-group"][data-project-id="' + projectId + '"]'),
    ).toHaveCount(2);
    await expect(page.locator('[data-kind="cost-bar-group"][data-project-id="kind"]')).toHaveCount(1);

    // 两环：钱按项目/Kind；时间含模型/工具/命令，缺的段照实写。
    await expect(page.getByText('钱花在哪')).toBeVisible();
    await expect(page.locator('[data-kind="donut"]').first()).toContainText('项目甲');
    await expect(page.locator('[data-kind="donut"]').first()).toContainText('Kind 对话');
    await expect(page.locator('[data-kind="time-donut"]')).toContainText('等首字');
    await expect(page.locator('[data-kind="time-donut"]')).toContainText('模型生成');
    await expect(page.locator('[data-kind="time-donut"]')).toContainText('工具');
    await expect(page.locator('[data-kind="time-donut"]')).toContainText('命令');
    await expect(page.locator('[data-kind="time-donut"]')).toContainText('没采到');
    await expect(page.locator('[data-kind="time-ring-missing"]')).toContainText('等首字');
    // 「在跑」页签带数目。
    await expect(page.locator('[data-tab-key="live"]')).toContainText('1');

    // 排行按项目拆：模型行带项目名。
    await expect(page.locator('[data-kind="rank-row"]').filter({ hasText: '项目甲 · deepseek-flash' }).first()).toBeVisible();

    // 模型回复从上下文蓝条移除，悬浮看约值。
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    const card = page.locator('[data-kind="live-card"]').first();
    await expect(card).toBeVisible({ timeout: 30_000 });
    const context = card.locator('[data-kind="live-context"]');
    await expect(context).toHaveAttribute('title', /模型回复 约 1\.2K/);
    await expect(context.locator('[data-kind="live-context-reply"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});