import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** 任务图：三张已确认图（有门等你判 / 两上游汇一下游 / 全部关了）+ 一个单派任务。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash: string) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

const graphBlock = (page: import('@playwright/test').Page, title: string) =>
  page.locator('[data-kind="task-graph"]').filter({ hasText: title });

test('任务图：三张图的块/站/线/选中/表，路由直达和切换保留选中', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    // 路由直达：#work?wv=graph。
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    const pane = page.locator('[data-kind="task-graph-pane"]');
    await expect(pane).toBeVisible({ timeout: 30_000 });

    // 块数：三张；全部关了的排最后。
    const blocks = page.locator('[data-kind="task-graph"]');
    await expect(blocks).toHaveCount(3, { timeout: 20_000 });
    await expect(blocks.nth(0)).toHaveAttribute('data-closed', 'false');
    await expect(blocks.nth(1)).toHaveAttribute('data-closed', 'false');
    await expect(blocks.nth(2)).toHaveAttribute('data-closed', 'true');
    await expect(blocks.nth(2)).toContainText('全部关了的图');

    // 图一：3 站 3 线；有门等你判（guards handed、release judge）。
    const judgeGraph = graphBlock(page, '有门等你判的图');
    await expect(judgeGraph.locator('[data-kind="graph-node"]')).toHaveCount(3);
    await expect(judgeGraph.locator('[data-kind="graph-edge"]')).toHaveCount(4);
    await expect(judgeGraph.locator('[data-kind="graph-edge"][data-state="judge"]')).toHaveCount(1);
    await expect(judgeGraph.locator('[data-kind="graph-edge"][data-edge-kind="guards"][data-state="handed"]')).toHaveCount(1);

    // 图二：4 站；两个上游汇到一个下游（一条 handed、一条 waiting）。
    const mergeGraph = graphBlock(page, '两个上游汇到一个下游的图');
    const mergeNodes = mergeGraph.locator('[data-kind="graph-node"]');
    await expect(mergeNodes).toHaveCount(4);
    const cId = await mergeGraph.locator('[data-kind="graph-node"][data-title="写 C"]').getAttribute('data-node-id');
    const incoming = mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"]');
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"][data-edge-kind="needs"][data-state="handed"]')).toHaveCount(1);
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"][data-edge-kind="needs"][data-state="waiting"]')).toHaveCount(1);
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"][data-edge-kind="release"][data-state="handed"]')).toHaveCount(1);

    // 单派任务只在页尾报数。
    await expect(page.locator('[data-kind="graph-single-count"]')).toContainText('另有 1 个单派的任务');

    // 选中「查 A」：前后变亮（门和 C 亮，B 暗），右栏五样 + 要求读一次。
    const nodeA = mergeGraph.locator('[data-kind="graph-node"][data-title="查 A"]');
    await nodeA.click();
    await expect(nodeA).toHaveAttribute('data-selected', 'true');
    const detail = page.locator('[data-kind="graph-detail"]');
    await expect(detail).toBeVisible();
    await expect(detail.locator('[data-detail]')).toHaveCount(5);
    await expect(detail.locator('[data-detail="requirement"]')).toContainText('查 A：要做到可核对', { timeout: 20_000 });
    await expect(mergeGraph.locator('[data-kind="graph-node"][data-title="查 B"]')).toHaveAttribute('data-dim', 'true');
    await expect(mergeGraph.locator('[data-kind="graph-node"][data-title="核查 A"]')).toHaveAttribute('data-dim', 'false');
    await expect(mergeGraph.locator('[data-kind="graph-node"][data-title="写 C"]')).toHaveAttribute('data-dim', 'false');
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-edge-id*="guards"]')).toHaveAttribute('data-lit', 'true');

    // 表和图条数一致。
    await judgeGraph.locator('[data-kind="graph-table-toggle"]').click();
    await expect(judgeGraph.locator('[data-kind="graph-table"]')).toBeVisible();
    await expect(judgeGraph.locator('[data-kind="graph-row"]')).toHaveCount(3);
    await judgeGraph.locator('[data-kind="graph-table-toggle"]').click();

    // 切看板再切回任务图：选中还在。
    await page.locator('[data-kind="work-view"][data-view="board"]').click();
    await expect(page.locator('[data-kind="work-board"]')).toBeVisible();
    await page.locator('[data-kind="work-view"][data-view="graph"]').click();
    await expect(pane).toBeVisible();
    await expect(mergeGraph.locator('[data-kind="graph-node"][data-title="查 A"]')).toHaveAttribute('data-selected', 'true');

    // 右栏能打开任务页。
    await detail.locator('[data-kind="graph-open-task"]').click();
    await expect(page).toHaveURL(/#taskchat:/, { timeout: 20_000 });
    await expect(page.locator('[data-kind="task-full-title"]')).toContainText('查 A', { timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('任务图不新起轮询：打开图后 getWork 仍按 workStore 两秒一次', async ({ page }) => {
  test.setTimeout(60_000);
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    const calls: number[] = [];
    page.on('request', (request) => {
      if (
        request.method() === 'POST' &&
        request.url().endsWith('/api') &&
        (request.postData() ?? '').includes('"method":"work"')
      ) {
        calls.push(Date.now());
      }
    });
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    await expect(page.locator('[data-kind="task-graph-pane"]')).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(1_000);
    calls.length = 0;
    await page.waitForTimeout(4_200);
    // 一个 2 秒轮询：4.2 秒内 2 次（最多 3 次抖动）；两个 interval 会到 4 次。
    expect(calls.length).toBeLessThanOrEqual(3);
  } finally {
    await fixture.close();
  }
});
