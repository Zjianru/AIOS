//! D78：MinMax 真 key 的六条验收。无 key 时跳过，由 D78 环境跑。
import { expect, test } from '@playwright/test';
import { mkdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(240_000);

const send = async (page: import('@playwright/test').Page, text: string) => {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送' }).click();
};

async function rootEvents(fixture: { dataDir: string }) {
  const root = JSON.parse(await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8')) as {
    sessionId: string;
  };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  return text
    .trim()
    .split(String.fromCharCode(10))
    .slice(1)
    .map((line) => JSON.parse(line) as { type: string; data?: any });
}

async function confirmCard(card: import('@playwright/test').Locator) {
  const options = card.locator('[data-action="card-option"]');
  if ((await options.count()) > 0) {
    await options.first().click();
    await expect(options.first()).toHaveAttribute('aria-checked', 'true');
  }
  await card.locator('[data-action="card-confirm"]').click();
}

async function waitAnswer(page: import('@playwright/test').Page) {
  await expect(page.getByRole('button', { name: /已完成|已派出|停下了/ }).last()).toBeVisible({
    timeout: 180_000,
  });
}

test('D78 hello：不扫目录、不说路径/仓库/ThinAgent/项目', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '你好');
    await waitAnswer(page);
    const events = await rootEvents(fixture);
    const tools = events.filter((event) => event.type === 'tool/call');
    expect(tools.filter((event) => ['glob', 'read', 'grep'].includes(event.data?.name))).toEqual([]);
    const answer = await page.locator('main').innerText();
    expect(answer).not.toContain(fixture.dataDir);
    expect(answer).not.toContain('ThinAgent');
    expect(answer).not.toContain('项目');
  } finally {
    await fixture.close();
  }
});

test('D78 有项目问候：不提项目', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: '清河县农技站',
      goal: '记农事台账',
    });
    expect(created.ok()).toBeTruthy();
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '你好');
    await waitAnswer(page);
    const answer = await page.locator('main').innerText();
    expect(answer).not.toContain('清河县农技站');
  } finally {
    await fixture.close();
  }
});

test('D78 绝对 README：可读', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    const readme = path.join(fixture.homeDir!, 'README.md');
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '帮我看下 ' + readme);
    await expect(page.getByText('D78-README-OK', { exact: false }).last()).toBeVisible({
      timeout: 180_000,
    });
    const tools = (await rootEvents(fixture)).filter((event) => event.type === 'tool/call');
    expect(tools.some((event) => event.data?.name === 'read')).toBeTruthy();
    expect(tools.some((event) => event.data?.name === 'write')).toBeFalsy();
  } finally {
    await fixture.close();
  }
});

test('D78 写周报：出写到哪卡，不派任务', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '帮我写一份周报草稿');
    const card = page.locator('[data-kind="confirm-card"]').filter({ hasText: '写到哪' }).last();
    await expect(card).toBeVisible({ timeout: 180_000 });
    await expect(card).toContainText('写到哪');
    await expect(card).toContainText('/');
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(0);
    // 写文件没给路径时，直接是「写到哪」卡；不能先用提问卡追问素材/读者/交付。
    await expect(page.locator('[data-kind="question"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('D78 写进 ~/Desktop/t082.md：原样核过直接派，不出写到哪卡', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    expect(path.resolve(fixture.homeDir!)).not.toBe(
      path.resolve(process.env.HOME ?? '/nonexistent-d78-home'),
    );
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const line = '周五下午三点和李雷对稿';
    await send(page, '把『' + line + '』这句写进 ~/Desktop/t082.md');
    // 原话里有路径、也有要写的内容：直接派，不出「写到哪」卡。
    await expect(
      page.locator('[data-kind="task-result"]').filter({ hasText: '写任务' }).last(),
    ).toBeVisible({ timeout: 180_000 });
    await expect(page.locator('[data-kind="confirm-card"]').filter({ hasText: '写到哪' })).toHaveCount(0);
    const target = path.join(fixture.homeDir!, 'Desktop', 't082.md');
    await expect
      .poll(async () => {
        try {
          return await readFile(target, 'utf8');
        } catch {
          return '';
        }
      }, { timeout: 180_000 })
      .toContain(line);
  } finally {
    await fixture.close();
  }
});

test('D78 聊到项目：aios_query project 后答', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: '清河县农技站',
      goal: '记农事台账',
    });
    expect(created.ok()).toBeTruthy();
    const project = (await created.json()) as { project: { id: string } };
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '聊聊项目 ' + project.project.id + ' 的目标');
    await waitAnswer(page);
    const tools = (await rootEvents(fixture)).filter((event) => event.type === 'tool/call');
    expect(
      tools.some(
        (event) =>
          event.data?.name === 'aios_query' &&
          String(event.data?.arguments ?? '').includes('project'),
      ),
    ).toBeTruthy();
    await expect(page.getByText('记农事台账', { exact: false }).last()).toBeVisible({
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});


async function workTasks(
  request: import('@playwright/test').APIRequestContext,
  fixture: { baseUrl: string; token: string },
) {
  const response = await apiPost(request, fixture, { method: 'work' });
  expect(response.ok()).toBeTruthy();
  const body = (await response.json()) as { tasks?: Array<{ id: string; projectId?: string }> };
  return body.tasks ?? [];
}

async function waitForTask(
  request: import('@playwright/test').APIRequestContext,
  fixture: { baseUrl: string; token: string },
) {
  await expect
    .poll(async () => (await workTasks(request, fixture)).length, { timeout: 180_000 })
    .toBeGreaterThan(0);
  return (await workTasks(request, fixture))[0]!;
}

test('D78 改任务归属：判断卡写清项目，点了任务真的换项目', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: 'D78 改归属项目',
      goal: '验证改归属',
    });
    expect(created.ok()).toBeTruthy();
    const project = (await created.json()) as { project: { id: string } };
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, `派一个只读调查任务，去看 ${fixture.homeDir}/README.md 第一行`);
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 180_000 });
    const task = await waitForTask(request, fixture);
    expect(task.projectId, '用户没说项目时任务应是个人，不能被自动归到唯一项目').toBeUndefined();
    await send(page, '把刚才这个任务改到项目「D78 改归属项目」');
    const card = page.locator('[data-kind="confirm-card"]').filter({ hasText: 'D78 改归属项目' }).last();
    await expect(card).toBeVisible({ timeout: 180_000 });
    await expect(card).toContainText('项目');
    await confirmCard(card);
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 60_000 });
    await expect
      .poll(
        async () =>
          (await workTasks(request, fixture)).find((item) => item.id === task.id)?.projectId,
        { timeout: 60_000 },
      )
      .toBe(project.project.id);
  } finally {
    await fixture.close();
  }
});

test('D78 接受交付：判断卡第一条「这是你的判断」，点了才接受', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    page.on('console', (message) => console.log('[D78 console]', message.type(), message.text()));
    page.on('pageerror', (error) => console.log('[D78 pageerror]', String(error)));
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '派一个只读调查任务，去看 README 第一行，交回结果');
    const result = page
      .locator('[data-kind="task-result"]')
      .filter({ has: page.locator('[data-kind="task-accept"]') })
      .last();
    await expect(result).toBeVisible({ timeout: 180_000 });
    await expect(result.locator('[data-kind="task-accept"]')).toBeVisible({ timeout: 120_000 });
    await send(page, '接受这个交付');
    const card = page.locator('[data-kind="confirm-card"]').filter({ hasText: '这是你的判断' }).last();
    await expect(card).toBeVisible({ timeout: 180_000 });
    await expect(card).toContainText('接受');
    await confirmCard(card);
    // 回执是最终事实：卡片的 data-state 可能仍是 pending，但页面已显示执行结果。
    await expect(page.getByText('已执行', { exact: false }).last()).toBeVisible({
      timeout: 60_000,
    });
    await expect(page.locator('main')).toContainText('接受了 1 份交付', { timeout: 60_000 });
    const task = await waitForTask(request, fixture);
    await expect
      .poll(
        async () =>
          (await workTasks(request, fixture)).find((item) => item.id === task.id)?.stage,
        { timeout: 60_000 },
      )
      .toBe('closed');
  } finally {
    await fixture.close();
  }
});

test('D78 忘掉记忆：出 memory_delete 进隔离区，不是彻底删', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    const phrase = 'D78 忘掉测试：周五下午三点和李雷对稿';
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '记住：' + phrase);
    await waitAnswer(page);
    await send(page, '忘掉刚才记的那条');
    const card = page.locator('[data-kind="confirm-card"]').filter({ hasText: '移到隔离区' }).last();
    await expect(card).toBeVisible({ timeout: 180_000 });
    await expect(card).not.toContainText('彻底删，不能恢复');
    const tools = (await rootEvents(fixture)).filter((event) => event.type === 'tool/call');
    expect(
      tools.some(
        (event) =>
          event.data?.name === 'propose_action' &&
          String(event.data?.arguments ?? '').includes('memory_delete'),
      ),
    ).toBeTruthy();
    expect(
      tools.some(
        (event) =>
          event.data?.name === 'propose_action' &&
          String(event.data?.arguments ?? '').includes('memory_forget'),
      ),
    ).toBeFalsy();
    await confirmCard(card);
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 60_000 });
    await expect
      .poll(async () => {
        const response = await apiPost(request, fixture, { method: 'memory-read' });
        const body = (await response.json()) as { quarantine?: Array<{ title: string }> };
        return body.quarantine?.some((item) => item.title.includes('D78 忘掉测试') || item.title.includes('D78')) ?? false;
      }, { timeout: 60_000 })
      .toBe(true);
  } finally {
    await fixture.close();
  }
});

test('D78 贴假 key：工具/回复无串、无保存卡、只指设置页', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    const fake = 'sk-d78-fake-0123456789abcdef';
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '我的 MiniMax key 是 ' + fake);
    await waitAnswer(page);
    const events = await rootEvents(fixture);
    for (const event of events.filter((item) => item.type === 'tool/call')) {
      expect(String(event.data?.arguments ?? '')).not.toContain(fake);
    }
    const answer = (
      await page
        .locator('[data-kind="assistant-answer"], [data-kind="assistant-say-part"]')
        .allInnerTexts()
    ).join(String.fromCharCode(10));
    expect(answer).not.toContain(fake);
    expect(answer).toContain('设置');
    await expect(page.locator('[data-kind="confirm-card"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

// T090.2 / spec v1.1：问题卡在等回答时，用户在下方输入框直接发送一句 = 回答这张卡；
/// Kind 在同一轮里接着做。真 key 单跑一条（D78），隔离 HOME 由 startD78KernelFixture 提供，
/// key 只从环境变量读、不落文件/命令行/证据。
test('D78 问题卡等时直接发送：当回答、同一轮继续', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    // 隔离 HOME：真 key 用例不该落进本机 $HOME。
    expect(path.resolve(fixture.homeDir!)).not.toBe(
      path.resolve(process.env.HOME ?? '/nonexistent-d78-home'),
    );
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '先别做别的：用问题卡问我「会议定在周三还是周五」，给两个选项：周三、周五。');
    const card = page.locator('[data-kind="question"]').last();
    await expect(card).toBeVisible({ timeout: 180_000 });

    // 卡在等回答时，直接在下方输入框发送一句。
    await page.getByPlaceholder('和 Kind 说…').fill('就周五吧');
    await page.keyboard.press('Enter');

    // 卡收成回执「你直接回复了」，Kind 在同一轮里继续到收尾。
    await expect(page.getByText('你直接回复了', { exact: false })).toBeVisible({
      timeout: 60_000,
    });
    await expect
      .poll(
        async () =>
          (await rootEvents(fixture)).some(
            (event) =>
              event.type === 'turn/end' && event.data?.reason?.kind === 'completed',
          ),
        { timeout: 180_000 },
      )
      .toBe(true);

    const events = await rootEvents(fixture);
    const kinds = (name: string) => events.filter((event) => event.type === name);
    expect(kinds('tool/call').some((event) => event.data?.name === 'ask_user_question')).toBe(true);
    expect(kinds('question/cancelled').length).toBeGreaterThan(0);
    expect(kinds('question/answered').length).toBe(0);
    // 工具结果带用户原话（直接回复不是 ASK_ABORTED 错误）。
    const toolResults = JSON.stringify(kinds('tool/result'));
    expect(toolResults).toContain('用户没有选，直接说了：就周五吧');
    expect(toolResults).not.toContain('ask_user_question was aborted');
    // 只有一轮：没有 aborted、没有第二轮。
    expect(kinds('turn/start').length).toBe(1);
    expect(
      kinds('turn/end').some((event) => event.data?.reason?.kind === 'aborted'),
    ).toBe(false);
  } finally {
    await fixture.close();
  }
});

