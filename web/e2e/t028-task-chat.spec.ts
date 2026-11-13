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
    // T051：交过 ready 报告、不在跑 → 页头用 taskStatusOf 的词（只读任务 ready = 有结果）。
    await expect(pane.locator('[data-kind="task-state"]')).toContainText('有结果');
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

    // 发送：受理后以排队的样式留在对话里，输入框清空；真暂停时队列写明原因。
    await again.locator('[data-kind="task-send"]').click();
    const queued = again.locator('[data-kind="task-queued"]');
    await expect(queued).toContainText(draft, { timeout: 20_000 });
    await expect(queued).toHaveAttribute('data-state', 'paused');
    await expect(queued).toContainText('排队中：全局暂停，开闸后送到');
    await expect(again.locator('[data-kind="composer-input"]')).toHaveValue('');
  } finally {
    await fixture.close();
  }
});

test('没暂停时受理的消息是「发送中…」，不写全局暂停', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'say' });
  try {
    const taskId = await launchTask(page, fixture, request);
    // 掐掉任务对话的实时帧：受理后的消息不会立刻出现在页面上，排队行停在「发送中」。
    // （exec.paused 为假；这一行只是在等日志。）
    await page.routeWebSocket(/\/ws\?task=/, () => {});
    const pane = await openTaskChat(page, fixture, taskId);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('执行中', {
      timeout: 20_000,
    });

    await pane.locator('[data-kind="composer-input"]').fill('这句在送');
    await pane.locator('[data-kind="composer-input"]').press('Enter');

    const queued = pane.locator('[data-kind="task-queued"]');
    await expect(queued).toHaveCount(1, { timeout: 20_000 });
    await expect(queued).toHaveAttribute('data-state', 'sending');
    await expect(queued).toContainText('这句在送');
    await expect(queued).toContainText('发送中…');
    await expect(queued).not.toContainText('排队中');
    await expect(queued).not.toContainText('全局暂停');
    await expect(pane.locator('[data-kind="paused"]')).toHaveCount(0);
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
    // 任务没被记成停止：任务对话还有输入框；T039 后页头任何时刻都有状态词，
    // 但不能是「已停止」（Esc 只停这一轮）。
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const again = taskPane(page);
    await expect(again.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 20_000 });
    const state = again.locator('[data-kind="task-state"]');
    await expect(state).toHaveCount(1);
    await expect(state).not.toContainText('已停止');
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

test('抄送行跨分页只一行：spliced 和领取后的 user/message 分在两页', async ({ page }) => {
  const fixture = await startKernelFixture({ kindHistory: 'cc-pages' });
  try {
    await page.goto(urlOf(fixture));
    // 初始页（尾页）只有领取后的 user/message：一行抄送。
    const cc = page.locator('[data-kind="task-cc"]');
    await expect(cc).toHaveCount(1, { timeout: 20_000 });
    await expect(cc).toContainText('这条抄送跨两页');
    await expect(page.getByText('第 12 步完成。')).toBeVisible();

    // 往上翻到第 1 轮：spliced 在它后面、上一页末尾，同一行不能变成两行。
    await page.locator('[data-pane="conversation"]').hover();
    await expect
      .poll(
        async () => {
          if ((await page.getByText('第一轮好了。').count()) > 0) return true;
          await page.mouse.wheel(0, -3000);
          return false;
        },
        { timeout: 30_000, intervals: [200, 400] },
      )
      .toBe(true);
    await expect(page.getByText('第一轮好了。')).toBeVisible();
    await expect(cc).toHaveCount(1);
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
    // T040：不再锁死「总共 2 张卡」。按任务/内容断言语义：
    // - 此时只有甲有卡（乙还在跑）：卡标题都是甲；
    // - 甲第 1 次 Attempt 可能 1 张，也可能 2 张（T038 已记 known：同一 Attempt 的 waiting→ready
    //   两条 relay 各出一张卡，需 relay 带 attempt/version 才能彻底修，后续票）；
    // - 第 2 次 Attempt 的新卡必须在：总卡数 2..=3；
    // - 真重复卡仍要抓：同一任务的卡不允许出现两张正文完全一样的。
    const cards = page.locator('[data-kind="task-result"]');
    // 回 Kind 后连接/视图会重挂：每次轮询重新读卡片正文，等甲的卡稳定在 2 张以上（known 双卡时 3 张），
    // 避免「刚数到数量、视图又被重挂清空」的竞态。
    const jiaTexts = async () =>
      (await cards.allInnerTexts()).filter((text) => text.includes('任务甲'));
    // 用例真正要证明的点：乙还在跑时，甲第 2 次 Attempt 的新结果卡已经出来（不是被 Later 压住）。
    // 先等这张卡出现（attempt2 relay 的固定正文），再做数量/唯一性断言——否则两只卡正好是
    // known waiting→ready 双卡时，会漏掉 attempt2 根本没出来的回归。
    await expect
      .poll(async () => (await jiaTexts()).some((text) => text.includes('接着之前的结果做完')), {
        timeout: 30_000,
        intervals: [200, 500],
      })
      .toBe(true);
    // T051.4：先等证明点再数——回 Kind 后视图会重挂，乙的卡可能还没渲染出来；
    // 轮询到「至少一张任务乙的卡」再统一数（旧断言 <=2 对 0 张也放行，等于没证明乙有卡）。
    await expect
      .poll(async () => (await cards.allInnerTexts()).filter((text) => text.includes('任务乙')).length, {
        timeout: 30_000,
        intervals: [200, 500],
      })
      .toBeGreaterThanOrEqual(1);
    const texts = await cards.allInnerTexts();
    // 这张用例的证明点是甲：第 1 次 Attempt 的卡（正常 1 张；T038 known 双卡时 2 张）+ 第 2 次 Attempt
    // 的新卡必须在。乙的卡可能已收尾出现（也可能还在跑），不锁总卡数、不要求此时只有甲。
    const jia = texts.filter((text) => text.includes('任务甲'));
    const yi = texts.filter((text) => text.includes('任务乙'));
    expect(jia.length, '甲：第 1 次 Attempt 1 张（known 双卡时 2 张）+ 第 2 次 Attempt 1 张').toBeGreaterThanOrEqual(2);
    expect(jia.length).toBeLessThanOrEqual(3);
    expect(yi.length, '乙：每张被唤醒的任务都有卡').toBeGreaterThanOrEqual(1);
    expect(yi.length, '乙：known 双卡时最多 2 张').toBeLessThanOrEqual(2);
    expect(texts.length, '每张卡都能归到甲或乙').toBe(jia.length + yi.length);
    expect(new Set(texts).size, '同一任务不能出现两张内容完全相同的卡').toBe(texts.length);
  } finally {
    await fixture.close();
  }
});
