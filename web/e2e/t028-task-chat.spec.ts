import { test, expect, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function taskPane(page: Page) {
  return page.locator('[data-pane="task-conversation"]');
}

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 根会话日志里的 task_accept 回执带任务 id。 */
async function taskIdOf(fixture: KernelFixture): Promise<string> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const root = JSON.parse(
        await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
      ) as { sessionId: string };
      const text = await readFile(
        path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
        'utf8',
      );
      const match = /已派出任务 ([0-9a-f]+)。/.exec(text);
      if (match) return match[1];
    } catch {
      // 会话文件还没写出来；继续等。
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error('根会话日志里等不到 task_accept 的任务 id');
}

async function waitUntilTaskReadable(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  taskId: string,
) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const response = await apiPost(request, fixture, { method: 'task-read', taskId });
    if (response.ok()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('TaskRead 一直打不开：' + taskId);
}

async function openTaskChat(
  page: Page,
  fixture: KernelFixture,
  taskId: string,
  title = TITLE,
) {
  await page.goto(urlOf(fixture, '#taskchat:' + taskId));
  const pane = taskPane(page);
  await expect(pane).toBeVisible({ timeout: 20_000 });
  await expect(pane.locator('[data-kind="task-full-title"]')).toHaveText(title, {
    timeout: 20_000,
  });
  return pane;
}

/** 根会话日志里所有 task_accept 回执带的任务 id（按派出顺序）。 */
async function taskIdsOf(fixture: KernelFixture, count: number): Promise<string[]> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const root = JSON.parse(
        await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
      ) as { sessionId: string };
      const text = await readFile(
        path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
        'utf8',
      );
      const ids = [...text.matchAll(/已派出任务 ([0-9a-f]+)。/g)].map((match) => match[1]);
      if (ids.length >= count) return ids.slice(0, count);
    } catch {
      // 会话文件还没写出来；继续等。
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('根会话日志里等不到 ' + count + ' 个任务 id');
}

/** 等一个任务真的在跑（席位放开后它才开始）。 */
async function waitTaskRunning(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  taskId: string,
) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const response = await apiPost(request, fixture, { method: 'task-read', taskId });
      if (response.ok() && (await response.json()).task.running === true) return;
    } catch {
      // 还没开出来；继续等。
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('任务一直没跑起来：' + taskId);
}

/** Kind 根会话里的轮数（抄送只用 inject，不能叫醒 Kind 开新的轮）。 */
async function kindTurns(fixture: KernelFixture): Promise<number> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  return text.split('"type":"turn/start"').length - 1;
}

async function launchTask(page: Page, fixture: KernelFixture, request: Parameters<typeof apiPost>[0]) {
  await page.goto(urlOf(fixture));
  await sendKind(page, '帮我查一下新闻归档里的判重规则');
  const taskId = await taskIdOf(fixture);
  await waitUntilTaskReadable(request, fixture, taskId);
  return taskId;
}

test('在跑时在任务对话里说话：任务出现用户气泡，Kind 出现抄送行，Kind 不回答', async ({
  page,
  request,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ taskResult: 'say' });
  try {
    const taskId = await launchTask(page, fixture, request);
    const pane = await openTaskChat(page, fixture, taskId);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('执行中', {
      timeout: 20_000,
    });
    // 固定说明行在输入框下面。
    await expect(pane.locator('[data-kind="task-footer"]')).toContainText(
      '你在这里说的直接给 ThinAgent，同时抄送 Kind（不叫醒它）。',
    );
    const before = await kindTurns(fixture);

    const said = '先只看新闻归档，别看测试目录';
    await pane.locator('[data-kind="composer-input"]').fill(said);
    // 在跑时输入框旁是「停下这一轮」，发送走 Enter（和 Kind 对话一样）。
    await expect(pane.locator('[data-kind="task-stop-turn"]')).toBeVisible();
    await pane.locator('[data-kind="composer-input"]').press('Enter');

    // 任务的第 1 次 Attempt 不变；用户气泡是正面断言。
    await expect(
      pane.locator('[data-kind="user-bubble"]').filter({ hasText: said }),
    ).toHaveCount(1, { timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-attempt"]')).toHaveCount(0);

    // Kind 对话里是一行虚线抄送（不是用户气泡），Kind 没有因为抄送发请求。
    await page.goto(urlOf(fixture));
    const cc = page.locator('[data-kind="task-cc"]').last();
    await expect(cc).toBeVisible({ timeout: 20_000 });
    await expect(cc).toContainText('你对 ' + TITLE + ' 说：');
    await expect(cc).toContainText(said);
    await expect(cc).toContainText('抄送 Kind，不叫醒它');
    await expect(page.locator('[data-kind="task-cc"] [data-kind="user-bubble"]')).toHaveCount(0);
    expect(await kindTurns(fixture), '抄送不能叫醒 Kind').toBe(before);

    // 点抄送行回到任务对话。
    await cc.locator('[data-kind="task-cc-open"]').click();
    await expect(page).toHaveURL(new RegExp('#taskchat:' + taskId));
    await expect(taskPane(page).locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
      timeout: 20_000,
    });
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('在跑时只发粘贴、没有文字：任务对话出现粘贴卡，日志里是 paste 块，Kind 抄送不空', async ({
  page,
  request,
}) => {
  const fixture = await startKernelFixture({ taskResult: 'say' });
  try {
    const taskId = await launchTask(page, fixture, request);
    const pane = await openTaskChat(page, fixture, taskId);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('执行中', {
      timeout: 20_000,
    });

    // 粘贴超过 1000 字：折成粘贴卡，textarea 里没有字。
    const pasted = '第一行\n' + '字'.repeat(1100);
    await pane.locator('[data-kind="composer-input"]').evaluate((element, value) => {
      const data = new DataTransfer();
      data.setData('text/plain', value);
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    }, pasted);
    await expect(pane.locator('[data-kind="paste-card"]')).toHaveCount(1);
    // 只有粘贴、没有文字：Enter 照样发得出去（不报空输入）。
    await pane.locator('[data-kind="composer-input"]').press('Enter');

    // 任务对话里这一句以粘贴卡呈现，不画空用户气泡。
    await expect(
      pane.locator('[data-kind="user-message-row"] [data-kind="paste-card"]'),
    ).toHaveCount(1, { timeout: 20_000 });
    await expect(pane.locator('[data-kind="user-bubble"]')).toHaveCount(0);

    // 日志里是 paste 块（不是文字块），说明服务把它当成一句话送给了 ThinAgent。
    const read = await apiPost(request, fixture, { method: 'task-read', taskId });
    expect(read.ok()).toBe(true);
    const events = (await read.json()).events as {
      type: string;
      data: { message?: { content?: { type?: string; text?: string; paste?: boolean }[] } };
    }[];
    const saidMessage = events.find(
      (event) =>
        event.type === 'user/message' &&
        (event.data.message?.content ?? []).some((block) => block.paste === true),
    );
    expect(saidMessage, '任务日志里有一条粘贴块的用户消息').toBeTruthy();
    expect(saidMessage?.data.message?.content?.some((block) => block.text === pasted)).toBe(true);

    // Kind 里的抄送行不空：没有文字时给一句固定说明。
    await page.goto(urlOf(fixture));
    const cc = page.locator('[data-kind="task-cc"]').last();
    await expect(cc).toBeVisible({ timeout: 20_000 });
    await expect(cc).toContainText('你对 ' + TITLE + ' 说：');
    await expect(cc).toContainText('（没有文字，只有粘贴或附件）');
  } finally {
    await fixture.close();
  }
});

test('停了以后说「接着做吧」：第 2 次 Attempt，原会话接着做，Kind 出新卡', async ({
  page,
  request,
}) => {
  const fixture = await startKernelFixture({ taskResult: 'resume' });
  try {
    const taskId = await launchTask(page, fixture, request);
    await page.goto(urlOf(fixture));
    const first = page.locator('[data-kind="task-result"]').last();
    await expect(first).toBeVisible({ timeout: 30_000 });
    await expect(first).toHaveAttribute('data-state', 'ready');

    const pane = await openTaskChat(page, fixture, taskId);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveCount(0);
    await pane.locator('[data-kind="composer-input"]').fill('接着做吧');
    await pane.locator('[data-kind="task-send"]').click();

    await expect(
      pane.locator('[data-kind="user-bubble"]').filter({ hasText: '接着做吧' }),
    ).toHaveCount(1, { timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-attempt"]')).toHaveText('第 2 次 Attempt', {
      timeout: 20_000,
    });

    // 第二个 Attempt 的新结果卡回到 Kind 对话。
    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(2, {
      timeout: 30_000,
    });
    await expect(page.locator('[data-kind="task-result"]').last()).toContainText(
      '照你在任务对话里说的补上了',
    );
  } finally {
    await fixture.close();
  }
});

test('暂停横幅与排队；草稿按任务分开，切走回来还在', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'resume' });
  try {
    const taskId = await launchTask(page, fixture, request);
    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({
      timeout: 30_000,
    });
    const paused = await apiPost(request, fixture, { method: 'settings-update', paused: true });
    expect(paused.ok()).toBe(true);

    const pane = await openTaskChat(page, fixture, taskId);
    await expect(pane.locator('[data-kind="paused"]')).toContainText(
      '全局暂停中：你的消息会先排着',
    );

    // 草稿：写一半切走，再回来还在（草稿键按任务存）。
    const draft = '这句话先排着，等开闸';
    const input = pane.locator('[data-kind="composer-input"]');
    await input.fill(draft);
    await pane.getByRole('button', { name: /回 Kind 对话/ }).click();
    await expect(page).toHaveURL(/#kind/);
    const again = await openTaskChat(page, fixture, taskId);
    await expect(again.locator('[data-kind="composer-input"]')).toHaveValue(draft);

    // 发送：受理后以排队的样式留在对话里，输入框清空。
    await again.locator('[data-kind="task-send"]').click();
    await expect(again.locator('[data-kind="task-queued"]')).toContainText(draft, {
      timeout: 20_000,
    });
    await expect(again.locator('[data-kind="composer-input"]')).toHaveValue('');
  } finally {
    await fixture.close();
  }
});

test('Esc 停下这一轮：任务不算停、不出新卡，输入框还在', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'say' });
  try {
    const taskId = await launchTask(page, fixture, request);
    const pane = await openTaskChat(page, fixture, taskId);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('执行中', {
      timeout: 20_000,
    });
    await page.keyboard.press('Escape');
    await expect(pane.getByText(/停下了这一轮/)).toBeVisible({ timeout: 20_000 });

    // 没有新卡：Kind 对话里一张结果卡都没有。
    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(0);
    // 任务没被记成停止：任务对话还有输入框，菜单两项可用状态由页头事实决定。
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const again = taskPane(page);
    await expect(again.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 20_000 });
    await expect(again.locator('[data-kind="task-state"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('ThinAgent 交回：Kind 出现「卡住了」的卡，带固定问题', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'handback' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我查一下新闻归档里的判重规则');
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toHaveAttribute('data-state', 'blocked');
    await expect(card).toContainText('卡住了');
    await expect(card).toContainText('这超出了任务范围，要不要改任务？');
  } finally {
    await fixture.close();
  }
});

test('席位 1、另一个任务在跑时，叫醒停了的任务照样开跑', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'seats' });
  try {
    const saved = await apiPost(request, fixture, { method: 'settings-update', seats: 1 });
    expect(saved.ok()).toBe(true);
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我派两个任务');
    const [firstTask, secondTask] = await taskIdsOf(fixture, 2);

    // 甲先跑完：Kind 里出现第一张卡；席位放开后乙才开始跑。
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({
      timeout: 30_000,
    });
    await waitTaskRunning(request, fixture, secondTask);

    // 乙占着唯一的席位；去甲的任务对话说「接着做吧」——person 可以多占 1 个。
    const pane = await openTaskChat(page, fixture, firstTask, '任务甲：查判重规则');
    await pane.locator('[data-kind="composer-input"]').fill('接着做吧');
    await pane.locator('[data-kind="task-send"]').click();
    await expect(
      pane.locator('[data-kind="user-bubble"]').filter({ hasText: '接着做吧' }),
    ).toHaveCount(1, { timeout: 30_000 });
    await expect(pane.locator('[data-kind="task-attempt"]')).toHaveText('第 2 次 Attempt', {
      timeout: 30_000,
    });

    // 真正的证明：乙还在跑，甲的新结果卡已经出来（不是被 Later 压住）。
    const second = await apiPost(request, fixture, { method: 'task-read', taskId: secondTask });
    expect(second.ok()).toBe(true);
    expect((await second.json()).task.running).toBe(true);
    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(2, {
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});
