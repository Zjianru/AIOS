import { test, expect, type Page } from '@playwright/test';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture, type KernelFixture } from './service-fixture';

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

function resultCard(page: Page) {
  return page.locator('[data-kind="task-result"]').last();
}

function taskPane(page: Page) {
  return page.locator('[data-pane="task-conversation"]');
}

async function send(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 任务的 ThinAgent 会话：root 状态文件里那一条之外的唯一会话日志。 */
async function thinSessionFile(fixture: KernelFixture): Promise<string> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const dir = path.join(fixture.dataDir, 'sessions');
  const files = (await readdir(dir)).filter(
    (name) => name.endsWith('.jsonl') && !name.startsWith(root.sessionId),
  );
  expect(files).toHaveLength(1);
  return path.join(dir, files[0]);
}

/**
 * 等任务会话的写者收工：终态报告落库之后，执行者还要写最后一段回答和 turn/end。
 * 对着还在长的日志追加分页夹具，会和服务的 seq 撞车（日志变成 19..1668 后接 19），
 * 之后任何 task-read 都报 seq 不连续——这是全量偶发红的真原因，不是页面丢滚动。
 */
async function waitSessionQuiet(file: string): Promise<void> {
  let previous = -1;
  let stable = 0;
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const raw = await readFile(file, 'utf8');
    const lines = raw.trimEnd().split(String.fromCharCode(10));
    let ended = false;
    try {
      ended = (JSON.parse(lines[lines.length - 1]) as { type?: string }).type === 'turn/end';
    } catch {
      ended = false;
    }
    stable = raw.length === previous ? stable + 1 : 0;
    previous = raw.length;
    // 这一轮已经 turn/end，而且 200ms 没有新字节：写者收工，可以补夹具了。
    if (ended && stable >= 4) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('任务会话一直没写完，不能补分页夹具');
}

/** 在真实任务会话后面追加若干轮回答（分页夹具；只动测试现场日志）。 */
async function appendTurns(file: string, fromTurn: number, count: number): Promise<void> {
  const raw = await readFile(file, 'utf8');
  let seq = 0;
  let time = 0;
  for (const line of raw.trimEnd().split('\n')) {
    const row = JSON.parse(line) as { seq?: number; time?: number };
    if (typeof row.seq === 'number') seq = Math.max(seq, row.seq);
    if (typeof row.time === 'number') time = Math.max(time, row.time);
  }
  const rows: string[] = [];
  for (let index = 0; index < count; index += 1) {
    const turn = fromTurn + index;
    seq += 1;
    rows.push(JSON.stringify({ type: 'turn/start', seq, time: time + seq, data: { turn } }));
    seq += 1;
    rows.push(
      JSON.stringify({
        type: 'assistant/message',
        seq,
        time: time + seq,
        data: {
          turn,
          step: 1,
          message: {
            id: 'msg-syn-' + String(turn),
            role: 'assistant',
            source: { kind: 'model', provider: 'deepseek', model: 'deepseek-flash' },
            content: [{ type: 'text', text: '第 ' + String(turn) + ' 步完成。' }],
          },
          stream: [],
          usage: { inputTokens: 10, outputTokens: 3 },
        },
      }),
    );
    seq += 1;
    rows.push(
      JSON.stringify({
        type: 'turn/end',
        seq,
        time: time + seq,
        data: { turn, reason: { kind: 'completed' } },
      }),
    );
  }
  await writeFile(file, raw + rows.join('\n') + '\n');
}

test('结果卡打开任务对话：工具调用和最后回答都在，刷新能重开', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ taskResult: 'chat' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });

    await card.getByRole('button', { name: /打开任务对话/ }).click();
    await expect(page).toHaveURL(/#taskchat:/);
    const pane = taskPane(page);
    await expect(pane).toBeVisible({ timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-full-title"]')).toHaveText(TITLE);
    await expect(pane).toContainText('调查任务 · 只读');
    // 只有写任务才有页头写入范围；调查任务页头不出现这条。
    await expect(pane.locator('[data-kind="task-write-scope"]')).toHaveCount(0);

    // 第一条 assignment 不画成用户气泡，任务要求只在头部。
    await expect(pane.locator('[data-kind="user-bubble"]')).toHaveCount(0);
    await expect(pane).not.toContainText('"task_id"');

    // 工作过程：点开这一轮和这一组才看到工具行（Kind 的渲染默认收起）。
    // 终态报告后执行者立刻关掉会话，日志没有 turn/end；页面据 reportedAt 把最后一轮收成已完成。
    const process = pane.locator('[data-kind="process"]').first();
    await expect(process).toContainText('已完成');
    await expect(pane).not.toContainText('工作中');
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();

    const readRow = pane.locator('[data-kind="tool-row"][data-tool="read"]');
    await expect(readRow).toBeVisible();
    await readRow.locator('button').first().click();
    await expect(pane.getByText('hello from fixture')).toBeVisible();

    // ThinAgent 的最后内容就是那份报告：在 task_report 这一行的卡片里能看到摘要。
    const reportRow = pane.locator('[data-kind="tool-row"][data-tool="task_report"]');
    await expect(reportRow).toBeVisible();
    await reportRow.locator('button').first().click();
    await expect(pane.getByText(/判重用的是原始链接/)).toBeVisible();

    // T028.5：任务对话现在有输入框（复用 Kind 的 Composer）；没在跑时没有「停下这一轮」。
    await expect(page.locator('[data-kind="composer"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="task-send"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="task-stop-turn"]')).toHaveCount(0);
    await expect(page.locator('[data-kind="task-footer"]')).toBeVisible();

    // 刷新浏览器：同一个路由重新拿一次 TaskRead，内容还在。
    await page.reload();
    const again = taskPane(page);
    await expect(again.locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
      timeout: 20_000,
    });
    await expect(again).toContainText('已完成');
    await expect(again).not.toContainText('工作中');
    await expect(again.locator('[data-kind="user-bubble"]')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('任务没在跑时静态可看：waiting 的最后一段回答', async ({ page }) => {
  const fixture = await startKernelFixture({ taskResult: 'waiting' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });

    await card.getByRole('button', { name: /打开任务对话/ }).click();
    const pane = taskPane(page);
    await expect(pane).toBeVisible({ timeout: 20_000 });
    // waiting 是执行者在一轮结束时合成的：这一轮有 turn/end，最后一段回答照日志显示。
    await expect(pane.locator('[data-kind="assistant-answer"]').last()).toContainText(
      '我还在查，别急。',
    );
    await expect(page.locator('[data-kind="composer"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="task-stop-turn"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('任务会话坏了：固定降级文案带原因，不空白不崩溃', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ taskResult: 'chat' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });

    // 把任务会话日志弄坏；serve 已把它收尾，不会再来写。
    await writeFile(await thinSessionFile(fixture), '{ 坏掉的会话\n');

    await card.getByRole('button', { name: /打开任务对话/ }).click();
    const fallback = page.locator('[data-kind="task-read-fallback"]');
    await expect(fallback).toBeVisible({ timeout: 20_000 });
    await expect(fallback).toContainText('任务对话打不开');
    await expect(fallback).toContainText('这个任务的对话现在打不开');
    await expect(fallback).toContainText('原因：');
    await expect(fallback).toContainText('会话日志');
    // 不是空白页，也不是崩溃：头部和回 Kind 的入口还在。
    await expect(page.getByRole('button', { name: /回 Kind 对话/ })).toBeVisible();
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('上翻 beforeSeq：更早的轮次按需加载', async ({ page }) => {
  const fixture = await startKernelFixture({ taskResult: 'chat' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });

    // 终态报告落库之后，执行者还要写最后一段回答和 turn/end：等会话真的收工再补分页夹具。
    const session = await thinSessionFile(fixture);
    await waitSessionQuiet(session);
    // 任务会话后面补 550 轮（每轮一条消息）：一页按 50 条消息/2 轮的对齐点往回走，
    // 所以第一页只到日志尾部，往上翻才能看到更早的轮次。
    await appendTurns(session, 2, 550);

    await card.getByRole('button', { name: /打开任务对话/ }).click();
    const pane = taskPane(page);
    await expect(pane.getByText('第 551 步完成。')).toBeVisible({ timeout: 20_000 });
    await expect(pane.getByText('第 500 步完成。')).toHaveCount(0);
    await expect(pane.getByText('第 2 步完成。')).toHaveCount(0);

    // 上翻：一次合成的 scroll 就该取回更早的一页（页面不丢加载期间收到的到顶信号）。
    await pane.locator('[data-kind="task-scroll"]').evaluate((element) => {
      element.scrollTop = 0;
      element.dispatchEvent(new Event('scroll', { bubbles: true }));
    });
    await expect(pane.getByText('第 500 步完成。')).toHaveCount(1, { timeout: 20_000 });
    await expect(pane.getByText('第 551 步完成。')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});

test('旧页面不调 task-read：Kind 对话照常收发', async ({ page }) => {
  const calls: string[] = [];
  page.on('request', (request) => {
    if (request.url().endsWith('/api')) calls.push(request.postData() ?? '');
  });
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await send(page, '你好');
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(
      '（T007 e2e 假回放）',
      { timeout: 20_000 },
    );
    expect(calls.filter((body) => body.includes('"method":"task-read"'))).toHaveLength(0);
  } finally {
    await fixture.close();
  }
});
