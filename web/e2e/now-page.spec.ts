import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

/**
 * T098.3：现在页照原型（D124 / reply s 第 2 节）。
 * 顶部通知说明、三组空状态、页尾两句；有任务时按总任务一张卡（站点链/卡在哪句/模型·已用·还需·token·花费，
 * 「做到几成」「还需」空着）；等你决定摘要不露 Markdown；监控没有事实时不编 0。
 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash: string) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

test('现在页：顶部通知说明、空状态、页尾两句', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-scene-page="now"]')).toBeVisible({ timeout: 30_000 });

    const notify = page.locator('[data-kind="now-notify"]');
    await expect(notify).toBeVisible();
    await notify.click();
    await expect(page.locator('[data-kind="now-notify-body"]')).toContainText('等你决定和出问题会提醒');
    await notify.click();

    await expect(page.locator('[data-kind="now-empty"]')).toContainText('都在正常推进');
    await expect(page.locator('[data-kind="now-week-done"]')).toBeVisible();
    await expect(page.getByText('AIOS 概念 · 现在', { exact: false })).toBeVisible();
    await expect(page.getByText('「还需」带 ≈ 的是估算', { exact: false })).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('现在页：在推进卡/问题卡字段、摘要纯文字、监控没事实不编 0', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    const now = Date.now();
    const taskBase = {
      version: 1,
      attempts: 1,
      createdAt: now - 60_000,
      running: false,
      gate: false,
      program: false,
      programCommands: 0,
      after: [] as string[],
      afterWaiting: false,
    };
    const runningTask = {
      ...taskBase,
      id: 'task-run-1',
      title: '正在跑的排版检查',
      stage: 'running',
      running: true,
      runningSince: now - 30_000,
    };
    const blockedTask = {
      ...taskBase,
      id: 'task-block-1',
      title: '抓取数据核对',
      stage: 'needs_conditions',
      attempts: 2,
      latest: {
        reportId: 'report-block-1',
        state: 'blocked',
        at: now - 60_000,
        summary: '**卡住**：等来源恢复再继续。',
        question: null,
      },
    };
    const waitItem = {
      kind: 'answer',
      title: '要你定一件事',
      need: '选一个来源',
      body: '**重点**：先按旧源继续，来源恢复再换。',
      at: now - 30_000,
    };
    const work = {
      method: 'work',
      generatedAt: now,
      tasks: [runningTask, blockedTask],
      wait: [waitItem],
      graphs: [],
    };
    // 这个用例只拦 work：monitor-snapshot 走真 serve（没有这两个任务的事实行），
    // 用来验证页面在没采到时不 reduce 出 $0.00 / 0 分钟 / 0 token。
    await page.route('**/api', async (route) => {
      const data = route.request().postDataJSON() as { method?: string } | null;
      if (data?.method === 'work') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(work) });
        return;
      }
      await route.continue();
    });

    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-scene-page="now"]')).toBeVisible({ timeout: 30_000 });

    // 在推进：站点链、卡在哪句、模型/已用/还需—/token/花费、展开·去看；监控无事实写「没采到」。
    const advancing = page.locator('[data-kind="now-advancing-card"]');
    await expect(advancing).toHaveCount(1);
    await expect(advancing.locator('[data-kind="now-station"]')).toHaveCount(1);
    await expect(advancing.locator('[data-kind="now-stuck"]')).toContainText('卡在：');
    const meta = advancing.locator('[data-kind="now-advancing-meta"]');
    await expect(meta).toContainText('已用 没采到');
    await expect(meta).toContainText('还需 —');
    await expect(meta).toContainText('token 没采到');
    await expect(meta).toContainText('花费没采到');
    await expect(advancing.locator('[data-kind="now-advancing-open"]')).toContainText('展开 · 去看');
    await expect(page.getByText('$0.00')).toHaveCount(0);

    // 问题卡：Origin「项目 › 任务标题」、正文「出了什么事」、处理状态、Kind 的处理、头行右侧时间；
    // 类别（LLM/AIOS）内核没有事实：不冒充，页面上不出现 LLM 问题/AIOS 问题标，阶段词也撤了。
    const problem = page.locator('[data-kind="now-problem"]');
    await expect(problem).toHaveCount(1);
    await expect(problem.locator('[data-kind="now-problem-origin"]')).toHaveText('个人 › 抓取数据核对');
    await expect(problem.locator('[data-kind="now-problem-what"]')).toContainText('卡住：等来源恢复再继续。');
    await expect(problem).toContainText('处理中');
    await expect(problem.locator('[data-kind="now-problem-time"]')).not.toHaveText('');
    await expect(problem.locator('[data-kind="now-problem-handling"]')).toContainText('Kind 的处理');
    await expect(problem).not.toContainText('待补足条件');
    await expect(problem).not.toContainText('LLM 问题');
    await expect(problem).not.toContainText('AIOS 问题');

    // 等你决定：摘要不露 Markdown（fixture 保证正文里有 ** 与标题）。
    await expect(page.locator('[data-kind="now-decisions"] [data-kind="wait-item"]')).toContainText('重点');
    const text = await page.locator('[data-scene-page="now"]').innerText();
    expect(text).not.toContain('**');
    expect(text).not.toMatch(/^#{1,6} /m);
  } finally {
    await fixture.close();
  }
});
test('现在页：花费固定按美元显示，不再有币种设置', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    const now = Date.now();
    const work = {
      method: 'work',
      generatedAt: now,
      tasks: [
        {
          id: 'task-run-2',
          title: '美元显示检查',
          stage: 'running',
          running: true,
          runningSince: now - 30_000,
          version: 1,
          attempts: 1,
          createdAt: now - 60_000,
          gate: false,
          program: false,
          programCommands: 0,
          after: [],
          afterWaiting: false,
        },
      ],
      wait: [],
      graphs: [],
    };
    const snapshot = {
      range: 'today',
      timezone: 'Asia/Shanghai',
      generatedAt: now,
      days: [],
      tasks: [
        {
          taskId: 'task-run-2',
          title: '美元显示检查',
          state: 'running',
          model: 'MiniMax-M3.1-Flash',
          running: true,
          startedAt: now - 60_000,
          requests: 1,
          inputTokens: 1000,
          outputTokens: 500,
          costUsd: 1,
          unpriced: false,
          toolCalls: 0,
          contextUsed: 0,
          contextWindow: 200000,
        },
      ],
      projects: [],
      models: [],
      tools: [],
      commands: [],
      rings: { costUsd: 1, unpriced: false, busyMs: 0, requestMs: 0, toolMs: 0, tokens: 0 },
      anomalies: [],
      running: { tasks: 1, seats: 1 },
      queue: [],
    };
    await page.route('**/api', async (route) => {
      const data = route.request().postDataJSON() as { method?: string } | null;
      if (data?.method === 'work') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(work) });
        return;
      }
      if (data?.method === 'monitor-snapshot') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(snapshot) });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#now'));
    const meta = page.locator('[data-kind="now-advancing-meta"]');
    await expect(meta).toContainText('$', { timeout: 30_000 });
    await expect(meta).not.toContainText('¥');
  } finally {
    await fixture.close();
  }
});
