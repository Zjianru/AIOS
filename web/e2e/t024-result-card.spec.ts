import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';
const TRUNCATED_TITLE = [...TITLE].slice(0, 20).join('');
const READY_SUMMARY = '判重用的是原始链接；加了跟踪参数的同一个链接会重复进简报。';
const BLOCKED_SUMMARY = '缺一份只有你能给的版本号，拿到才能继续比对归档。';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

function resultCard(page: Page) {
  return page.locator('[data-kind="task-result"]').last();
}

async function send(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 两段文字的连续重合长度（「Kind 不复述」的判据；允许短引用，>20 字算复述）。 */
function longestCommonRun(left: string, right: string): number {
  const a = [...left];
  const b = [...right];
  let best = 0;
  let previous = new Array<number>(b.length + 1).fill(0);
  for (let i = 1; i <= a.length; i += 1) {
    const current = new Array<number>(b.length + 1).fill(0);
    for (let j = 1; j <= b.length; j += 1) {
      if (a[i - 1] === b[j - 1]) {
        current[j] = previous[j - 1] + 1;
        if (current[j] > best) best = current[j];
      }
    }
    previous = current;
  }
  return best;
}

test('结果卡 ready：有结果、标题截到 20 字、刷新还在、Kind 不复述', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ taskResult: 'ready' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');

    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toHaveAttribute('data-state', 'ready');
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('有结果');
    await expect(card.getByText('调查任务')).toBeVisible();
    await expect(card.locator('[data-kind="task-title"]')).toHaveText(TRUNCATED_TITLE);
    await expect(card.locator('[data-kind="task-title"]')).not.toHaveText(TITLE);
    await expect(card).toContainText(READY_SUMMARY);
    await expect(card.getByRole('button', { name: /打开任务对话/ })).toBeVisible();
    await expect(card).not.toContainText('完成');

    // relay 之后 Kind 的回答不能把 relay JSON 或摘要整段复述出来。
    const answer = page.locator('[data-kind="assistant-answer"]').last();
    await expect(answer).toContainText('具体内容看卡片', { timeout: 20_000 });
    const answerText = await answer.innerText();
    expect(answerText).not.toContain('task_report');
    expect(answerText).not.toContain('task-report:');
    expect(longestCommonRun(answerText, READY_SUMMARY)).toBeLessThanOrEqual(20);

    // 刷新浏览器：卡从会话日志重建，仍然在。
    await page.reload();
    const again = resultCard(page);
    await expect(again).toBeVisible({ timeout: 20_000 });
    await expect(again.locator('[data-kind="task-state"]')).toHaveText('有结果');
    await expect(again.locator('[data-kind="task-title"]')).toHaveText(TRUNCATED_TITLE);
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('结果卡 blocked：卡住了，问你的话在正文槽上面', async ({ page }) => {
  const fixture = await startKernelFixture({ taskResult: 'blocked' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');

    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toHaveAttribute('data-state', 'blocked');
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('卡住了');
    await expect(card).toContainText(BLOCKED_SUMMARY);
    const question = card.locator('[data-kind="task-question"]');
    await expect(question).toContainText('它问你');
    await expect(question).toContainText('用哪个版本？');
    await expect(card).not.toContainText('完成');
  } finally {
    await fixture.close();
  }
});

test('结果卡 failed：失败，摘要照报告显示', async ({ page }) => {
  const fixture = await startKernelFixture({ taskResult: 'failed' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');

    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toHaveAttribute('data-state', 'failed');
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('失败');
    await expect(card).toContainText('文档站打不开');
    await expect(card).not.toContainText('完成');
  } finally {
    await fixture.close();
  }
});

test('结果卡 waiting：等报告，摘要用 ThinAgent 最后那句话', async ({ page }) => {
  const fixture = await startKernelFixture({ taskResult: 'waiting' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');

    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toHaveAttribute('data-state', 'waiting');
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('等报告');
    await expect(card).toContainText('我还在查，别急。');
  } finally {
    await fixture.close();
  }
});

test('结果卡 缺项：没开跑，摘要由程序写、写明缺什么', async ({ page }) => {
  const fixture = await startKernelFixture({ taskResult: 'missing' });
  try {
    await page.goto(urlOf(fixture));
    const card = resultCard(page);
    await expect(card).toBeVisible({ timeout: 20_000 });
    await expect(card).toHaveAttribute('data-state', 'blocked');
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('没开跑');
    await expect(card).toContainText('任务包缺：deepseek-flash 的凭证');
    await expect(card.locator('[data-kind="task-question"]')).toHaveCount(0);
    await expect(card).not.toContainText('完成');
  } finally {
    await fixture.close();
  }
});

test('结果卡 坏 JSON：退回原始文本，不出卡', async ({ page }) => {
  const fixture = await startKernelFixture({ taskResult: 'bad-json' });
  try {
    await page.goto(urlOf(fixture));
    await expect(page.getByText('这条消息解析不了，照原始文本显示。')).toBeVisible({
      timeout: 20_000,
    });
    await expect(resultCard(page)).toHaveCount(0);
    const bubble = page.locator('[data-kind="user-bubble"]').filter({ hasText: 'task_report' });
    await expect(bubble).toHaveCount(1);
    await expect(bubble).toContainText('{"kind":"task_report",');
  } finally {
    await fixture.close();
  }
});
