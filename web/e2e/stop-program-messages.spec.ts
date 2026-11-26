import { test, expect, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

/**
 * T095.1：停止只清用户自己排队的话；程序消息留在队列照旧送。
 * 说明：e2e 走页面 /api 的 send 只能造「用户发送」；生产里的程序消息由服务内部
 * （host.send_message，带 source.rpcId 前缀）注入。本用例验：停止后输入框只放回用户话、
 * 程序请求入过队列且不被 canceled；程序消息下一轮恰好一次由 agent/service 测试覆盖。
 */

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('停止只清用户排队消息，程序消息留在队列', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t059LongStream: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await sendKind(page, '你现在都能做什么卡片');
    await expect(page.locator('[data-kind="stop"]')).toBeVisible({ timeout: 30_000 });

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
        content: '任务报回了',
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
    const input = page.locator('[data-kind="composer-input"]');
    await expect(input).toHaveValue(/用户排队的话/, { timeout: 30_000 });
    await expect(input).not.toHaveValue(/任务报回了/);

    // 日志：两条消息都进过队列；取消只落用户消息那一段的 canceled splice。
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
