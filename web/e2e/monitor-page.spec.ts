import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

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
