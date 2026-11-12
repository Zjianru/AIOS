import { test, expect, type Page } from '@playwright/test';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { apiPost, startRealKernelFixture } from './service-fixture';

const evidence =
  process.env.AIOS_DOD_EVIDENCE ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T007/evidence/dod');

test.skip(!process.env.DEEPSEEK_API_KEY, '需要 DEEPSEEK_API_KEY（D34）');
test.setTimeout(240_000);

/** 根会话日志里 task_accept 回执带的任务 id（Kind 派出的任务）。 */
async function taskIdOf(fixture: { dataDir: string }): Promise<string> {
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
      // 根会话文件还没写出来；继续等。
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error('根会话日志里等不到 task_accept 的任务 id');
}

/** 写任务要用 bash：先在设置里打开开关。 */
async function enableBash(page: Page, fixture: { baseUrl: string; token: string }): Promise<void> {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#settings:exec`);
  await expect(page.locator('[data-kind="exec-settings"]')).toBeVisible({ timeout: 20_000 });
  const toggle = page.locator('[data-kind="bash-toggle"]');
  if ((await toggle.getAttribute('aria-checked')) !== 'true') {
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
  }
}

/// DoD4：真 key 基础——模型用 read 只读 /tmp/aios-t007-dod/work 并回答；截图+日志比对。
test('DoD4 真 key：read 只读 /tmp/aios-t007-dod/work 并回答', async ({ page, request }) => {
  const fixture = await startRealKernelFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill(
      '请只用 read 工具、只读地查看 /tmp/aios-t007-dod/work/hello.txt 的内容，把文件内容原样回复我。不要写任何文件、不要安装。',
    );
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByText('DOD-4-OK', { exact: false }).last()).toBeVisible({ timeout: 180_000 });
    // 等这一轮的 turn/end 落到日志（流式文本先到，日志后写），再做截图与比对。
    const followFor = async () => {
      const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
      return (await response.json()).events as { type: string }[];
    };
    await expect
      .poll(async () => (await followFor()).filter((event) => event.type === 'turn/end').length, {
        timeout: 90_000,
      })
      .toBeGreaterThanOrEqual(1);
    await page.screenshot({ path: path.join(evidence, 'dod4-answer.png'), fullPage: true });
    const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
    const events = (await response.json()).events as {
      type: string;
      data?: Record<string, any>;
    }[];
    const textOf = (message: any): string => {
      const content = message?.content;
      if (!Array.isArray(content)) return '';
      return content.filter((b: any) => b?.type === 'text').map((b: any) => String(b.text ?? '')).join('');
    };
    const byTurn = new Map<number, { users: string[]; tools: string[]; answers: string[]; turn: number }>();
    for (const event of events) {
      const turn = event.data?.turn;
      if (typeof turn !== 'number') continue;
      let entry = byTurn.get(turn);
      if (!entry) {
        entry = { turn, users: [], tools: [], answers: [] };
        byTurn.set(turn, entry);
      }
      if (event.type === 'user/message') {
        const text = textOf(event.data?.message).trim();
        if (text && !text.includes('<system-reminder>')) entry.users.push(text);
      } else if (event.type === 'tool/call') {
        entry.tools.push(String(event.data?.name ?? ''));
      } else if (event.type === 'assistant/message') {
        const text = textOf(event.data?.message).trim();
        if (text) entry.answers.push(text);
      }
    }
    // 过程默认收起：逐个展开整轮和分组，按 data-tool 取实际渲染出来的工具名。
    // 行标题是中文（读取/运行命令），页面上没有英文工具名，不能拿 innerText 比。
    const processes = page.locator('[data-kind="process"]');
    for (let index = 0; index < (await processes.count()); index += 1) {
      const process = processes.nth(index);
      if ((await process.getAttribute('data-open')) !== 'true') {
        await process.locator('button').first().click();
      }
      const groups = process.locator('[data-kind="process-group"] button');
      for (let group = 0; group < (await groups.count()); group += 1) {
        if ((await groups.nth(group).getAttribute('aria-expanded')) !== 'true') {
          await groups.nth(group).click();
        }
      }
    }
    const rawPageText = await page.locator('main').innerText();
    await writeFile(path.join(evidence, 'dod4-page.txt'), rawPageText);
    const pageText = rawPageText.replace(/\s+/g, ' ');
    const normalize = (text: string) => text.replace(/\s+/g, ' ').trim();
    const condense = (text: string) => text.replace(/[^A-Za-z0-9\u4e00-\u9fff]/g, '');
    const pageCondensed = condense(rawPageText);
    const pageTools = new Set(
      await page
        .locator('[data-kind="tool-row"]')
        .evaluateAll((rows) => rows.map((row) => row.getAttribute('data-tool') ?? '')),
    );
    const compare = [...byTurn.values()].sort((a, b) => a.turn - b.turn).map((entry) => {
      const answer = entry.answers[entry.answers.length - 1] ?? '';
      return {
        turn: entry.turn,
        users: entry.users,
        tools: entry.tools,
        answerTail: answer.slice(-80),
        pageHasUser: entry.users.every((text) => pageText.includes(normalize(text))),
        pageHasTools: entry.tools.every((text) => pageTools.has(text)),
        pageHasAnswer: !answer || pageCondensed.includes(condense(answer).slice(0, 40)),
      };
    });
    const allMatched = compare.every((row) => row.pageHasUser && row.pageHasTools && row.pageHasAnswer);
    const summary = { turns: compare, allMatched, token: '<redacted>' };
    await writeFile(path.join(evidence, 'dod4-compare.json'), JSON.stringify(summary, null, 2));
    expect(allMatched).toBeTruthy();
  } finally {
    await fixture.close();
  }
});

/// DoD5：真 key 停止（Esc）——跑到提问工具停下，页面留「停下了」。
test('DoD5 真 key：跑到工具时 Esc 停下', async ({ page }) => {
  const fixture = await startRealKernelFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill(
      '请必须调用 ask_user_question 工具问我一个问题：问「继续吗？」，只问这一道、选项是/否，然后停下等我回答，不要做别的事。',
    );
    await page.getByRole('button', { name: '发送' }).click();
    // 提问工具在等回答，这一轮还开着：此时按 Esc 应当停下。
    await expect(
      page.locator('[data-kind="question"]').getByText('继续吗？', { exact: false }),
    ).toBeVisible({ timeout: 120_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toBeVisible({ timeout: 30_000 });
    await page.keyboard.press('Escape');
    await expect(page.getByText('停下了这一轮', { exact: false })).toBeVisible({
      timeout: 30_000,
    });
    await page.screenshot({ path: path.join(evidence, 'dod5-stop.png'), fullPage: true });
  } finally {
    await fixture.close();
  }
});

/// DoD5：真 key 两题整批作答，然后整批不答。
test('DoD5 真 key：两题整批作答 + 整批不答', async ({ page }) => {
  await rm('/tmp/aios-t007-dod', { recursive: true, force: true });
  const fixture = await startRealKernelFixture();
  try {
    page.on('pageerror', (error) => console.log('PAGEERROR ' + error.message));
    page.on('console', (message) => { if (message.type() === 'error') console.log('CONSOLE ' + message.text()); });
    await mkdir(evidence, { recursive: true });
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const input = page.getByPlaceholder('和 Kind 说…');
    const prompt = '调用 ask_user_question 工具，一次两题：q1 选项是/否问继续吗，q2 文本问名字。必须用工具，不要文字提问。调用后停下等我。';
    const pending = page.locator('[data-kind="question"]');
    await input.fill(prompt);
    await page.getByRole('button', { name: '发送' }).click();
    try {
      await expect(pending.getByText('继续吗？', { exact: false })).toBeVisible({ timeout: 120_000 });
    } catch (error) {
      await page.screenshot({ path: path.join(evidence, 'dod5-ask-fail.png'), fullPage: true });
      throw error;
    }
    if ((await pending.getByRole('radio').count()) > 0) {
      await pending.getByRole('radio').first().click();
    }
    const freeTexts = pending.getByPlaceholder('写下你的回答');
    const freeCount = await freeTexts.count();
    for (let index = 0; index < freeCount; index += 1) {
      await freeTexts.nth(index).fill(index === 0 ? '是' : '测试用户');
    }
    if ((await pending.getByPlaceholder('自己写').count()) > 0) {
      await pending.getByPlaceholder('自己写').first().fill('测试用户');
    }
    await page.getByRole('button', { name: '提交回答' }).click();
    await page.waitForTimeout(3000);
    await page.screenshot({ path: path.join(evidence, 'dod5-ask-answered.png'), fullPage: true });

    await input.fill(prompt);
    await page.getByRole('button', { name: '发送' }).click();
    await expect(pending.getByText('继续吗？', { exact: false })).toBeVisible({ timeout: 120_000 });
    await page.getByRole('button', { name: '不答' }).click();
    await page.waitForTimeout(2000);
    await page.screenshot({ path: path.join(evidence, 'dod5-ask-dismissed.png'), fullPage: true });
  } finally {
    await fixture.close();
  }
});


/// DoD6：真 key 断线重连——横幅出现又消失，重连后补齐、seq 连续不重复。
test('DoD6 真 key 断线重连', async ({ page, request }) => {
  // 上一次失败留下的卡片/会话会让「断线后才有卡」不成立：这条用例自己从干净目录起。
  await rm(process.env.AIOS_DOD_ROOT ?? '/tmp/aios-t007-dod', { recursive: true, force: true });
  const fixture = await startRealKernelFixture({ proxy: true });
  try {
    await mkdir(evidence, { recursive: true });
    // Kind 没有跑得久的工具：让写任务的 ThinAgent 跑 sleep 8，结果卡在断线期间回来。
    await enableBash(page, fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').fill(
      '派一个写任务，标题写「断线后完成」，写入范围就用当前目录：任务里只用 bash 工具运行前台命令 sleep 8，然后用 task_report 上报 state=ready、summary 写「断线后完成」；不要改任何文件，不要做别的事。',
    );
    await page.getByRole('button', { name: '发送' }).click();
    const taskId = await taskIdOf(fixture);
    expect(taskId, 'Kind 派出了任务').toMatch(/^[0-9a-f]+$/);
    const cards = page.locator('[data-kind="task-result"]');
    await expect(cards).toHaveCount(0);
    const card = cards.last();
    // 任务在服务端跑；Kind 这一轮派完就结束，页面只剩连线。
    await fixture.dropProxy?.();
    const banner = page.locator('[data-kind="offline"]');
    await expect(banner).toBeVisible({ timeout: 30_000 });
    // 服务端把任务结果写进根会话日志时页面还断着：等它落盘再恢复连线，卡才只能是补回来的。
    const root = JSON.parse(
      await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
    ) as { sessionId: string };
    const rootLog = path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl');
    await expect
      .poll(
        async () => {
          try {
            return (await readFile(rootLog, 'utf8')).includes('task-report:' + taskId + ':');
          } catch {
            return false;
          }
        },
        { timeout: 180_000 },
      )
      .toBe(true);
    await fixture.restoreProxy?.();
    await expect(banner).toHaveCount(0, { timeout: 60_000 });
    // 断线期间任务交回的结果卡，重连后要出现；结论在卡上。
    await expect(card).toBeVisible({ timeout: 180_000 });
    await expect(card).toContainText('断线后完成', { timeout: 30_000 });
    await expect(card.locator('[data-kind="task-state"]')).toHaveText(/有结果|待你核/);
    await page.screenshot({ path: path.join(evidence, 'dod6-reconnect.png'), fullPage: true });
    const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
    const events = (await response.json()).events as { seq: number }[];
    expect(events.length).toBeGreaterThan(0);
    for (let index = 1; index < events.length; index += 1) {
      expect(events[index].seq).toBeGreaterThan(events[index - 1].seq);
    }
  } finally {
    await fixture.close();
  }
});
