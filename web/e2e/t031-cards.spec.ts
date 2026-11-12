import { test, expect, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture, type KernelFixture } from './service-fixture';

const SENT = '帮我派两个任务';
const TITLE_A = '任务甲：查判重规则';
const TITLE_B = '任务乙：查归档范围';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function send(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 根会话日志的事件（测试断言「同一轮」用，不靠 DOM 猜）。 */
async function rootEvents(fixture: KernelFixture): Promise<Record<string, any>[]> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  return text
    .split(String.fromCharCode(10))
    .filter((line) => line.trim() !== '')
    .map((line) => JSON.parse(line) as Record<string, any>);
}

function turnOfUserMessage(event: Record<string, any>): string | null {
  const content = event.data?.message?.content;
  if (!Array.isArray(content)) return null;
  return content.map((block: any) => block?.text ?? '').join('');
}

test('一轮两张结果卡：按到达顺序、气泡在前、刷新同序', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ taskResult: 'two' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, SENT);

    const cards = page.locator('[data-kind="task-result"]');
    await expect(cards).toHaveCount(2, { timeout: 30_000 });
    await expect(cards.nth(0).locator('[data-kind="task-title"]')).toHaveText(TITLE_A);
    await expect(cards.nth(1).locator('[data-kind="task-title"]')).toHaveText(TITLE_B);
    // relay 自己的 JSON 气泡不出现（两张卡代替了它们）。
    await expect(
      page.locator('[data-kind="user-bubble"]').filter({ hasText: 'task_report' }),
    ).toHaveCount(0);

    // 同轮：两条 relay 的 turn 相同，而且就是用户这句所在的轮。
    // T038：派发卡在派出时就出现，卡片到齐不再隐含 relay 已到；这里等日志里真的两条 relay。
    let events = await rootEvents(fixture);
    let relayTurns: number[] = [];
    for (let attempt = 0; attempt < 600; attempt += 1) {
      events = await rootEvents(fixture);
      relayTurns = events
        .filter(
          (event) =>
            event.type === 'user/message' &&
            String(event.data?.message?.source?.rpcId ?? '').startsWith('task-report:'),
        )
        .map((event) => event.data.turn as number);
      if (relayTurns.length === 2) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const userTurns = events
      .filter(
        (event) =>
          event.type === 'user/message' &&
          String(event.data?.message?.source?.rpcId ?? '').startsWith('task-report:') === false &&
          turnOfUserMessage(event) === SENT,
      )
      .map((event) => event.data.turn as number);
    expect(relayTurns, '两个任务各一条 relay').toHaveLength(2);
    expect(relayTurns[0], '两条 relay 在同一轮').toBe(relayTurns[1]);
    expect(userTurns, '用户那句也在同一轮').toContain(relayTurns[0]);

    // 卡在用户气泡后面（同一轮里气泡照旧、卡跟在后面）。
    const bubble = page.locator('[data-kind="user-bubble"]').filter({ hasText: SENT }).first();
    await expect(bubble).toBeVisible();
    const bubbleBox = await bubble.boundingBox();
    const cardBox = await cards.nth(0).boundingBox();
    expect(bubbleBox && cardBox && bubbleBox.y < cardBox.y, '气泡在卡片上面').toBe(true);

    // 刷新：还是两张，顺序不变。
    await page.reload();
    await expect(cards).toHaveCount(2, { timeout: 20_000 });
    await expect(cards.nth(0).locator('[data-kind="task-title"]')).toHaveText(TITLE_A);
    await expect(cards.nth(1).locator('[data-kind="task-title"]')).toHaveText(TITLE_B);
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});
