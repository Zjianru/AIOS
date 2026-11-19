import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

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
    await expect(judgeGraph.locator('[data-kind="graph-edge"]')).toHaveCount(2);
    await expect(judgeGraph.locator('[data-kind="graph-edge"][data-edge-kind="needs"]')).toHaveCount(0);
    await expect(judgeGraph.locator('[data-kind="graph-edge"][data-state="judge"]')).toHaveCount(1);
    await expect(judgeGraph.locator('[data-kind="graph-edge"][data-edge-kind="guards"][data-state="handed"]')).toHaveCount(1);

    // 图二：4 站；两个上游汇到一个下游（一条 handed、一条 waiting）。
    const mergeGraph = graphBlock(page, '两个上游汇到一个下游的图');
    const mergeNodes = mergeGraph.locator('[data-kind="graph-node"]');
    await expect(mergeNodes).toHaveCount(4);
    const cId = await mergeGraph.locator('[data-kind="graph-node"][data-title="写 C"]').getAttribute('data-node-id');
    const incoming = mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"]');
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"][data-edge-kind="needs"]')).toHaveCount(1);
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"][data-edge-kind="needs"][data-state="waiting"]')).toHaveCount(1);
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-to="' + cId + '"][data-edge-kind="release"][data-state="handed"]')).toHaveCount(1);
    // 被门把关的上游不再画平行 needs。
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-edge-kind="needs"]')).toHaveCount(1);

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
    // 写 C 只和门（release）连，和查 A 不再有平行 needs，所以选中查 A 时它是暗的。
    await expect(mergeGraph.locator('[data-kind="graph-node"][data-title="写 C"]')).toHaveAttribute('data-dim', 'true');
    await expect(mergeGraph.locator('[data-kind="graph-edge"][data-edge-id*="guards"]')).toHaveAttribute('data-lit', 'true');

    // 表「等谁」显示门那条：写 C 的等谁只有核查 A（门），没有平行的查 A 结果线。
    await mergeGraph.locator('[data-kind="graph-table-toggle"]').click();
    const cRow = mergeGraph.locator('[data-kind="graph-row"][data-node-id="' + cId + '"]');
    await expect(cRow.locator('td').nth(3)).toContainText('核查 A');
    await mergeGraph.locator('[data-kind="graph-table-toggle"]').click();

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

test('T066 任务图：框型、状态环、线动效、详情关闭、减少动效', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    const judgeGraph = graphBlock(page, '有门等你判的图');
    await expect(judgeGraph.locator('[data-kind="graph-node"]')).toHaveCount(3, { timeout: 30_000 });
    await expect(judgeGraph.locator('[data-kind="graph-node"][data-box="gate"]')).toHaveCount(1);
    await expect(judgeGraph.locator('[data-kind="graph-node"][data-box="read"]').first()).toBeVisible();
    await expect(page.locator('[data-kind="graph-node"][data-box="write"]').first()).toBeVisible();
    const writeBox = page.locator('[data-kind="graph-node"][data-box="write"]').first();
    expect(await writeBox.evaluate((el) => getComputedStyle(el).borderLeftWidth)).toBe('3px');
    // 状态环：等你的门环转得更慢；减少动效前有 animation。
    const ring = judgeGraph.locator('.graph-node-ring[data-slow="true"]').first();
    await expect(ring).toBeVisible();
    const spinning = await ring.evaluate((el) => getComputedStyle(el).animationName);
    expect(spinning).not.toBe('none');
    // 线的 live 数据来自服务；减少动效后动画名是 none。
    const liveEdge = page.locator('[data-kind="graph-edge"][data-live="flow"], [data-kind="graph-edge"][data-live="make"], [data-kind="graph-edge"][data-live="back"], [data-kind="graph-edge"][data-live="judge"]').first();
    await expect(liveEdge).toBeVisible();
    const animatedLine = page.locator('.graph-edge-live-flow, .graph-edge-live-make, .graph-edge-live-back').first();
    await expect(animatedLine).toHaveCount(1);
    expect(await animatedLine.evaluate((el) => getComputedStyle(el).animationName)).not.toBe('none');
    expect(await animatedLine.evaluate((el) => getComputedStyle(el).strokeDasharray)).toContain('6');

    // × 关闭。
    const nodeA = graphBlock(page, '两个上游汇到一个下游的图').locator('[data-kind="graph-node"][data-title="查 A"]');
    await nodeA.click();
    const detail = page.locator('[data-kind="graph-detail"]');
    await expect(detail).toBeVisible();
    await detail.locator('[data-kind="graph-detail-close"]').click();
    await expect(detail).toHaveCount(0);
    // Esc 关闭。
    await nodeA.click();
    await expect(detail).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(detail).toHaveCount(0);
    // 点图空白关闭。
    await nodeA.click();
    await expect(detail).toBeVisible();
    await page.locator('[data-kind="task-graph-pane"]').getByText('下游开跑时拿到了').click();
    await expect(detail).toHaveCount(0);

    // 减少动效：环/线都不动。
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.reload();
    await expect(page.locator('[data-kind="task-graph"]').first()).toBeVisible({ timeout: 30_000 });
    const ringAfter = page.locator('.graph-node-ring').first();
    await expect(ringAfter).toBeVisible();
    expect(await ringAfter.evaluate((el) => getComputedStyle(el).animationName)).toBe('none');
    const motionEdge = page.locator('[data-kind="graph-edge"][data-live]').first();
    if (await motionEdge.count()) {
      expect(await motionEdge.locator('path').first().evaluate((el) => getComputedStyle(el).animationName)).toBe('none');
    }
  } finally {
    await fixture.close();
  }
});

test('任务图不另起轮询：打开图后空闲不请求 work', async ({ page }) => {
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
    // 首屏可能有几次初始读；稳定后不再轮询。
    await page.waitForTimeout(2_500);
    calls.length = 0;
    await page.waitForTimeout(4_200);
    expect(calls.length).toBe(0);
  } finally {
    await fixture.close();
  }
});

test('工作页只挂一根 live：两个 pane 不重复轮询', async ({ page }) => {
  test.setTimeout(60_000);
  const work = await readFile(
    fileURLToPath(new URL('../src/pages/work/WorkScene.tsx', import.meta.url)),
    'utf8',
  );
  const graph = await readFile(
    fileURLToPath(new URL('../src/pages/work/GraphPane.tsx', import.meta.url)),
    'utf8',
  );
  expect(work.match(/useWorkPolling\(\)/g)?.length ?? 0).toBe(1);
  const root = work.slice(work.indexOf('export function WorkScene'));
  expect(root).toContain('useWorkPolling()');
  expect(graph).not.toContain('useWorkPolling');

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
    // 等待页（工作场景两个 pane 都挂着）：稳定后不再按固定间隔读。
    await page.goto(urlOf(fixture, '#work'));
    await expect(page.locator('[data-pane="waiting"]')).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(2_500);
    calls.length = 0;
    await page.waitForTimeout(4_200);
    expect(calls.length).toBe(0);
  } finally {
    await fixture.close();
  }
});
