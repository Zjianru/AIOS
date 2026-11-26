import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T095.1：停止只清用户自己排队的话；程序消息（带前缀 requestId 经 /api 发）留在队列，
 * 之后单独开一轮送到对话区。用 InboxSpliced 的 start/removedCount 折叠断言取消只覆盖
 * 用户消息那一格，程序消息那格没被覆盖。
 */

interface LoggedEvent {
  seq: number;
  type: string;
  data?: Record<string, unknown>;
}

function messageText(message: unknown): string {
  const content = (message as { content?: Array<{ type?: string; text?: string }> })?.content ?? [];
  return content.map((block) => (block.type === 'text' ? block.text ?? '' : '')).join('');
}

/** 折叠 next-turn 队列：inserted 按 start 插入，removedCount 删掉；返回还在排队的文本。 */
function pendingNextTurn(events: LoggedEvent[]): string[] {
  const pending: string[] = [];
  for (const event of events) {
    if (event.type !== 'agent/inbox/spliced' || event.data?.target !== 'next-turn') continue;
    const start = Number(event.data.start ?? 0);
    const removedCount = Number(event.data.removedCount ?? 0);
    const inserted = (event.data.inserted as unknown[] | undefined) ?? [];
    pending.splice(start, removedCount, ...inserted.map(messageText));
  }
  return pending;
}

async function events(request: APIRequestContext, fixture: KernelFixture): Promise<LoggedEvent[]> {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  const body = (await response.json()) as { events?: LoggedEvent[] };
  return body.events ?? [];
}

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('停止只清用户排队消息，程序消息留在队列并照旧送到对话区', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t059LongStream: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await sendKind(page, '你现在都能做什么卡片');
    await expect(page.locator('[data-kind="stop"]')).toBeVisible({ timeout: 30_000 });

    // 跑着时排队：一条程序消息（前缀 requestId，正常页面是 uuid 才是用户消息）+ 一条用户消息。
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
        requestId: 'appointment-wake:e2e:queued',
        content: '定时唤醒到了',
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
    await expect(input).not.toHaveValue(/定时唤醒到了/);

    // 取消只覆盖用户消息那一格；程序消息还在 pending 队列里等着。
    await expect
      .poll(async () => pendingNextTurn(await events(request, fixture)).join('|'), { timeout: 20_000 })
      .toContain('定时唤醒到了');
    expect(pendingNextTurn(await events(request, fixture))).not.toContain('用户排队的话');

    // 发一句新的唤醒：程序消息单独开一轮，对话区要出现它自己的气泡。
    await input.fill('接着');
    await page.keyboard.press('Enter');
    await expect(
      page.locator('[data-pane="conversation"]').getByText('定时唤醒到了', { exact: false }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(input).not.toHaveValue(/定时唤醒到了/);
  } finally {
    await fixture.close();
  }
});
