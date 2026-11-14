import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

/**
 * T046：未声明 addition-only 的路由（工具表全量固定）——这里用 e2e 真实请求体做回归。
 * 夹具连接被指到 aios-replay（假厂商 HTTP 端点），请求体就是 model crate 序列化器拼的；
 * /__replay/last-request 能拿到最近一次请求体，直接读 tools[].name。
 *
 * 预期（T046 + T050：全量固定，但 D73 的 key 条件仍在——没填 Exa key 时 web_* 不进表）：
 * - 无 Exa key：Kind 13（上面 15 去掉 web_search/web_fetch）、ThinAgent 只读 7（9 去掉两件 web）；
 * - 存了 Exa key：Kind 15、ThinAgent 9，含 web_search/web_fetch；
 * - 两种情况下都无 write/edit/bash/job_*（角色边界）；工具表跨轮、加载 skill 前后不变。
 */

// T050：没填 Exa key 时全量表里没有 web_*（D73）。
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
  'task_accept',
  'task_status',
  'task_stop',
  'task_update',
  'task_graph_propose',
  'task_continue',
];

// 存了 Exa key：T046 的全量 15 件 + T052 的 task_graph_propose + T053 的 task_continue = 17。
const KIND_FULL_WITH_KEY = [
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
  'task_graph_propose',
  'task_continue',
];

const THIN_READ_FULL = [
  'read',
  'glob',
  'grep',
  'skill',
  'todo_write',
  'task_report',
  'task_handback',
  'task_ledger_note',
  'task_ledger_history',
];

const THIN_READ_WITH_KEY = [
  'read',
  'glob',
  'grep',
  'skill',
  'todo_write',
  'web_search',
  'web_fetch',
  'task_report',
  'task_handback',
  'task_ledger_note',
  'task_ledger_history',
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

test('Kind：无 Exa key 时全量 15（不含 web_* / write / edit / bash / job_*）', async ({ page, request }) => {
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

test('ThinAgent：无 Exa key 时只读全量 9（不含 web_*），跨请求一致', async ({ page, request }) => {
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
    // 抓到过的每条 ThinAgent 请求工具表都等于只读全量 9 件（无 key，不含 web_*）。
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

test('存了 Exa key：Kind 17 / ThinAgent 11，含 web_*（D73 key 后出现）', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({
    taskResult: 'live',
    fakeExa: true,
    routeModel: 'replay-uncatalogued-model',
  });
  try {
    // 先存假 Exa key（设置页真实走的同一个服务方法）；下一轮起两件联网工具才进表。
    const saved = await apiPost(request, fixture, {
      method: 'exa-key-save',
      key: 'exa-t050-e2e-key',
    });
    expect(saved.ok()).toBe(true);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('查一下新闻归档里重复链接的判重规则和影响范围');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    // Kind 第一轮：全量 + web_*。
    await expect
      .poll(async () => (await lastToolNames(request, fixture)).length, { timeout: 30_000 })
      .toBeGreaterThan(0);
    expect(await lastToolNames(request, fixture)).toEqual(KIND_FULL_WITH_KEY);
    // ThinAgent（只读任务）：全量 + web_*。
    const seen: string[][] = [];
    for (let attempt = 0; attempt < 400; attempt += 1) {
      const tools = await lastToolNames(request, fixture);
      if (tools.includes('task_report')) {
        const key = tools.join(',');
        if (!seen.some((list) => list.join(',') === key)) seen.push(tools);
        if (seen.length >= 1) break;
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    expect(seen.length).toBeGreaterThanOrEqual(1);
    for (const tools of seen) {
      expect(tools).toEqual(THIN_READ_WITH_KEY);
    }
  } finally {
    await fixture.close();
  }
});
