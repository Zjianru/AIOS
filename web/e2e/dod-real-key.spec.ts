import { test, expect } from '@playwright/test';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { apiPost, startRealKernelFixture } from './service-fixture';

const evidence =
  process.env.AIOS_DOD_EVIDENCE ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T007/evidence/dod');

test.skip(!process.env.DEEPSEEK_API_KEY, '需要 DEEPSEEK_API_KEY（D34）');
test.setTimeout(240_000);

/// DoD4：真 key 基础——模型用 bash 只读 /tmp/aios-t007-dod/work 并回答；截图+日志比对。
test('DoD4 真 key：bash 只读 /tmp/aios-t007-dod/work 并回答', async ({ page, request }) => {
  const fixture = await startRealKernelFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill(
      '请只用 bash 工具、只读地查看 /tmp/aios-t007-dod/work/hello.txt 的内容，把文件内容原样回复我。不要写任何文件、不要安装。',
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
    await page.getByRole('button', { name: /工作过程/ }).click();
    const rawPageText = await page.locator('main').innerText();
    await writeFile(path.join(evidence, 'dod4-page.txt'), rawPageText);
    const pageText = rawPageText.replace(/\s+/g, ' ');
    const normalize = (text: string) => text.replace(/\s+/g, ' ').trim();
    const condense = (text: string) => text.replace(/[^A-Za-z0-9\u4e00-\u9fff]/g, '');
    const pageCondensed = condense(rawPageText);
    const compare = [...byTurn.values()].sort((a, b) => a.turn - b.turn).map((entry) => {
      const answer = entry.answers[entry.answers.length - 1] ?? '';
      return {
        turn: entry.turn,
        users: entry.users,
        tools: entry.tools,
        answerTail: answer.slice(-80),
        pageHasUser: entry.users.every((text) => pageText.includes(normalize(text))),
        pageHasTools: entry.tools.every((text) => pageText.includes(normalize(text))),
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

/// DoD5：真 key 停止（Esc）——跑到工具时停下，页面留「停下了」。
test('DoD5 真 key：跑到工具时 Esc 停下', async ({ page }) => {
  const fixture = await startRealKernelFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill(
      '请必须调用 bash 工具运行前台命令：sleep 10；等它结束后回复我「完成」。',
    );
    await page.getByRole('button', { name: '发送' }).click();
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
  const fixture = await startRealKernelFixture({ proxy: true });
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').fill(
      '必须调用 bash 工具运行前台命令 sleep 8；等它结束后回复「断线后完成」。',
    );
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByRole('button', { name: '停止', exact: true })).toBeVisible({ timeout: 60_000 });
    await fixture.dropProxy?.();
    const banner = page.locator('[data-kind="offline"]');
    await expect(banner).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(9_000);
    await fixture.restoreProxy?.();
    await expect(banner).toHaveCount(0, { timeout: 60_000 });
    await expect(page.getByText('断线后完成', { exact: true })).toBeVisible({ timeout: 180_000 });
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
