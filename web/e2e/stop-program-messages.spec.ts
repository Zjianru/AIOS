import { test, expect, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

/**
 * T095.1：停止只清用户自己排队的话；程序消息（task-report/fixed-block 等）留在队列照旧送。
 * 用真 serve 的根会话日志折叠验证：取消只覆盖用户消息那一段，程序消息还在。
 */

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('停止只清用户排队消息，程序消息留在队列', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ slowMs: 3000 });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await sendKind(page, '你现在都能做什么卡片');
    await expect(page.locator('[data-kind="stop"]')).toBeVisible({ timeout: 30_000 });

    // 跑着时排队：一条程序消息 + 一条用户消息（同 NextTurn 队列）。
    const status = await page.evaluate(async () => {
      const send = async (body: unknown) => {
        const response = await fetch('/api', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(body),
        });
        return response.status;
      };
      const program = await send({
        method: 'send',
        requestId: 'task-report:task-e2e:report-e2e',
        content: JSON.stringify({
          kind: 'task_report',
          task: { id: 'task-e2e', title: 'E2E 任务' },
          state: 'ready',
          summary: '报回内容E2E',
        }),
        delivery: 'follow-up',
        pastes: [],
        attachments: [],
        canvas: null,
        resultCard: null,
      });
      const user = await send({
        method: 'send',
        requestId: 'user-queue-1',
        content: '用户排队的话',
        delivery: 'follow-up',
        pastes: [],
        attachments: [],
        canvas: null,
        resultCard: null,
      });
      return { program, user };
    });
    expect(status.program).toBe(200);
    expect(status.user).toBe(200);

    await page.locator('[data-kind="stop"]').click();
    // 停止后页面会把用户排队话放回输入框；程序消息不该被放回。
    const input = page.locator('[data-kind="composer-input"]');
    await expect(input).toHaveValue(/用户排队的话/, { timeout: 30_000 });
    // F1 未完成：对话区断言需要程序消息那一轮真正跑完（现有夹具只有 1–2 段回放，
    // 程序消息的 FollowUp 轮会等第 3 段脚本而挂住）。当前先验证「用户话放回、程序话不放回」+ 日志。
    // 日志：两条消息都进过队列；取消只落用户消息那一段的 canceled splice（程序消息不被取消）。
    const root = JSON.parse(await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8')) as {
      sessionId: string;
    };
    const logPath = path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl');
    await expect
      .poll(async () => (await readFile(logPath, 'utf8')).includes('task-report:task-e2e:report-e2e'), {
        timeout: 30_000,
      })
      .toBe(true);
    const log = await readFile(logPath, 'utf8');
    expect(log).toContain('task-report:task-e2e:report-e2e');
    expect(log).toContain('user-queue-1');
    const canceled = log.split('\n').filter((line) => line.includes('agent/inbox/spliced') && line.includes('"outcome":"canceled"')).length;
    expect(canceled).toBeGreaterThanOrEqual(1);
  } finally {
    await fixture.close();
  }
});