//! D78：MinMax 真 key 的六条验收。无 key 时跳过，由 D78 环境跑。
import { expect, test } from '@playwright/test';
import { mkdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(240_000);

const send = async (page: import('@playwright/test').Page, text: string) => {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送' }).click();
};

async function rootEvents(fixture: { dataDir: string }) {
  const root = JSON.parse(await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8')) as {
    sessionId: string;
  };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  return text
    .trim()
    .split(String.fromCharCode(10))
    .slice(1)
    .map((line) => JSON.parse(line) as { type: string; data?: any });
}

async function waitAnswer(page: import('@playwright/test').Page) {
  await expect(page.getByRole('button', { name: /已完成|已派出|停下了/ }).last()).toBeVisible({
    timeout: 180_000,
  });
}

test('D78 hello：不扫目录、不说路径/仓库/ThinAgent/项目', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '你好');
    await waitAnswer(page);
    const events = await rootEvents(fixture);
    const tools = events.filter((event) => event.type === 'tool/call');
    expect(tools.filter((event) => ['glob', 'read', 'grep'].includes(event.data?.name))).toEqual([]);
    const answer = await page.locator('main').innerText();
    expect(answer).not.toContain(fixture.dataDir);
    expect(answer).not.toContain('ThinAgent');
    expect(answer).not.toContain('项目');
  } finally {
    await fixture.close();
  }
});

test('D78 有项目问候：不提项目', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: '清河县农技站',
      goal: '记农事台账',
    });
    expect(created.ok()).toBeTruthy();
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '你好');
    await waitAnswer(page);
    const answer = await page.locator('main').innerText();
    expect(answer).not.toContain('清河县农技站');
  } finally {
    await fixture.close();
  }
});

test('D78 绝对 README：可读', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    const readme = path.resolve(fixture.dataDir, '..', 'work');
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '帮我看下 ' + readme + '/README.md');
    await expect(page.getByText('D78-README-OK', { exact: false }).last()).toBeVisible({
      timeout: 180_000,
    });
    const tools = (await rootEvents(fixture)).filter((event) => event.type === 'tool/call');
    expect(tools.some((event) => event.data?.name === 'read')).toBeTruthy();
    expect(tools.some((event) => event.data?.name === 'write')).toBeFalsy();
  } finally {
    await fixture.close();
  }
});

test('D78 写周报：出写到哪卡，不派任务', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '帮我写一份周报草稿');
    const card = page.locator('[data-kind="confirm-card"]').filter({ hasText: '写到哪' }).last();
    await expect(card).toBeVisible({ timeout: 180_000 });
    await expect(card).toContainText('写到哪');
    await expect(card).toContainText('/');
    await expect(page.locator('[data-kind="task-result"]')).toHaveCount(0);
    // 写文件没给路径时，直接是「写到哪」卡；不能先用提问卡追问素材/读者/交付。
    await expect(page.locator('[data-kind="question"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('D78 写进 ~/Desktop/t082.md：原样核过直接派，不出写到哪卡', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '把这段写进 ~/Desktop/t082.md');
    // 按 spec 这条只要求：原话里有路径就直接派、不要「写到哪」卡；任务后续缺正文可以卡住，
    // 不要求它成功，所以不等「已派出」，只等写任务卡出现。
    await expect(
      page.locator('[data-kind="task-result"]').filter({ hasText: '写任务' }).last(),
    ).toBeVisible({ timeout: 180_000 });
    await expect(page.locator('[data-kind="confirm-card"]').filter({ hasText: '写到哪' })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('D78 聊到项目：aios_query project 后答', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: '清河县农技站',
      goal: '记农事台账',
    });
    expect(created.ok()).toBeTruthy();
    const project = (await created.json()) as { project: { id: string } };
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '聊聊项目 ' + project.project.id + ' 的目标');
    await waitAnswer(page);
    const tools = (await rootEvents(fixture)).filter((event) => event.type === 'tool/call');
    expect(
      tools.some(
        (event) =>
          event.data?.name === 'aios_query' &&
          String(event.data?.arguments ?? '').includes('project'),
      ),
    ).toBeTruthy();
    await expect(page.getByText('记农事台账', { exact: false }).last()).toBeVisible({
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});
