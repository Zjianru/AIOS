import { test, expect, type Locator, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/** Kind 的上下文；T023 夹具的最终答案。 */
const KIND_PRETEXT = '我先确认一下你系统里对卡片的定义，再回答你。';
const KIND_ANSWER = '查完了。真正能发出的卡片有五类：画布卡、提问卡、确认卡、待办卡，还有回答卡。';
/** 任务对话夹具：ThinAgent 先说明再读文件。 */
const THIN_PRETEXT = '我先看一下 hello.txt，再回答你。';
const THIN_ANSWER = '看完了。hello.txt 里就一行 hello from fixture。';
const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

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

/** 过程收起（默认）时工具前说明就在；展开后它在工具步前面。 */
async function expectSayBeforeTool(
  scope: Locator,
  pretext: string,
  tool: string,
) {
  const process = scope.locator('[data-kind="process"]').last();
  const say = process.locator('[data-kind="interim-say"]');
  await expect(say).toHaveText(pretext);
  await expect(say).toBeVisible();
  await expect(process).toHaveAttribute('data-open', 'false');
  await process.locator('button').first().click();
  // 组默认也收着：把每一组的行展开，才看得到工具行。
  const groups = process.locator('[data-kind="process-group"]');
  for (let index = 0; index < (await groups.count()); index += 1) {
    await groups.nth(index).locator('button').first().click();
  }
  const row = process.locator('[data-kind="tool-row"][data-tool="' + tool + '"]');
  await expect(row).toHaveCount(1);
  const sayBox = await say.boundingBox();
  const rowBox = await row.boundingBox();
  expect(sayBox && rowBox && sayBox.y < rowBox.y, '工具前说明在工具步上面').toBe(true);
}

test('Kind：工具前说明在过程里、收起可见、回答区没有；刷新一样', async ({ page }) => {
  const fixture = await startKernelFixture({ pretextTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '你现在都能做什么卡片');
    const answer = page.locator('[data-kind="assistant-answer"]').last();
    await expect(answer).toHaveText(KIND_ANSWER, { timeout: 30_000 });
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 30_000 });
    expect(await answer.textContent()).not.toContain(KIND_PRETEXT);

    await expectSayBeforeTool(page, KIND_PRETEXT, 'read');
    expect(await answer.textContent()).not.toContain(KIND_PRETEXT);

    // 刷新：从日志重放，过程里还是在、还是收起可见、回答区还是没有。
    await page.reload();
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toHaveText(KIND_ANSWER, {
      timeout: 30_000,
    });
    await expectSayBeforeTool(page, KIND_PRETEXT, 'read');
    expect(await page.locator('[data-kind="assistant-answer"]').last().textContent()).not.toContain(
      KIND_PRETEXT,
    );
  } finally {
    await fixture.close();
  }
});

test('任务对话：工具前说明在过程里、收起可见、回答区没有；刷新一样', async ({
  page,
  request,
}) => {
  const fixture = await startKernelFixture({ taskResult: 'pretext' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '帮我查一下 hello.txt');
    const taskId = await taskIdOf(fixture);
    await waitUntilTaskReadable(request, fixture, taskId);

    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
      timeout: 20_000,
    });
    const answer = pane.locator('[data-kind="assistant-answer"]').last();
    await expect(answer).toHaveText(THIN_ANSWER, { timeout: 30_000 });
    expect(await answer.textContent()).not.toContain(THIN_PRETEXT);

    await expectSayBeforeTool(pane, THIN_PRETEXT, 'read');
    expect(await answer.textContent()).not.toContain(THIN_PRETEXT);

    await page.reload();
    await expect(pane.locator('[data-kind="assistant-answer"]').last()).toHaveText(THIN_ANSWER, {
      timeout: 20_000,
    });
    await expectSayBeforeTool(pane, THIN_PRETEXT, 'read');
    expect(
      await pane.locator('[data-kind="assistant-answer"]').last().textContent(),
    ).not.toContain(THIN_PRETEXT);
  } finally {
    await fixture.close();
  }
});
