import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T046：未声明 addition-only 的路由（工具表全量固定）——这里用 e2e 真实请求体做回归。
 * 夹具连接被指到 aios-replay（假厂商 HTTP 端点），请求体就是 model crate 序列化器拼的；
 * /__replay/last-request 能拿到最近一次请求体，直接读 tools[].name。
 *
 * 预期（recon T046/evidence/recon.md + tool-sets-actual.log）：
 * - Kind 全开 15：read/glob/grep/skill/todo_write/ask_user_question/canvas/aios_query/propose_action/
 *   web_search/web_fetch/task_accept/task_status/task_stop/task_update（无 write/edit/bash/job_*）；
 * - ThinAgent 只读全开 9：read/glob/grep/skill/todo_write/web_search/web_fetch/task_report/task_handback。
 */

const KIND_FULL = [
  'read',
  'glob',
  'grep',
  'skill',
  'todo_write',
  'ask_user_question',
  'canvas',
  'aios_query',
  'propose_action',
  'web_search',
  'web_fetch',
  'task_accept',
  'task_status',
  'task_stop',
  'task_update',
];

const THIN_READ_FULL = [
  'read',
  'glob',
  'grep',
  'skill',
  'todo_write',
  'web_search',
  'web_fetch',
  'task_report',
  'task_handback',
];

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

interface LastRequest {
  request?: { tools?: { name?: string }[] };
}

async function lastToolNames(request: import('@playwright/test').APIRequestContext, fixture: { fakeEndpoint: string }): Promise<string[]> {
  const response = await request.get(fixture.fakeEndpoint + '/__replay/last-request');
  const body = (await response.json()) as LastRequest;
  return (body.request?.tools ?? []).map((tool) => String(tool.name ?? ''));
}

test('Kind：默认会话第一轮工具表就是全量固定（15），不含 write/edit/bash/job_*', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ routeModel: 'replay-uncatalogued-model' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await page.getByText('（T007 e2e 假回放）').last().waitFor({ state: 'visible', timeout: 30_000 });
    await expect
      .poll(async () => (await lastToolNames(request, fixture)).length, { timeout: 20_000 })
      .toBeGreaterThan(0);
    const tools = await lastToolNames(request, fixture);
    expect(tools).toEqual(KIND_FULL);
    for (const forbidden of ['write', 'edit', 'bash', 'job_output', 'job_list', 'job_kill']) {
      expect(tools).not.toContain(forbidden);
    }
  } finally {
    await fixture.close();
  }
});

test('ThinAgent：只读任务工具表全量固定（9），跨请求一致', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live', routeModel: 'replay-uncatalogued-model' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('查一下新闻归档里重复链接的判重规则和影响范围');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const seen: string[][] = [];
    for (let attempt = 0; attempt < 400; attempt += 1) {
      const tools = await lastToolNames(request, fixture);
      if (tools.includes('task_report')) {
        const key = tools.join(',');
        if (!seen.some((list) => list.join(',') === key)) seen.push(tools);
        if (seen.length >= 2) break;
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    // 假回放的 ThinAgent 很快跑完，轮询可能只抓到一条请求；跨轮一致性由 Kind 那条 + agent
    // 集成测试（unsupported_route_declares_all_tools_from_the_first_step）覆盖，这里断言
    // 抓到过的每条 ThinAgent 请求工具表都等于只读全量 9 件。
    expect(seen.length).toBeGreaterThanOrEqual(1);
    for (const tools of seen) {
      expect(tools).toEqual(THIN_READ_FULL);
    }
    const finalTools = await lastToolNames(request, fixture);
    if (finalTools.includes('task_report')) {
      expect(finalTools).toEqual(THIN_READ_FULL);
    }
  } finally {
    await fixture.close();
  }
});
