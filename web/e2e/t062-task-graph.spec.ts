import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T092：任务图照原型（下拉切图 / 卡三行 / 线 / 右侧详情 / 缩放平移 / 空图）。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash: string) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

const pickGraph = async (page: import('@playwright/test').Page, title: string) => {
  await page.locator('[data-kind="graph-pick"]').click();
  await page.getByRole('menuitemradio', { name: new RegExp(title) }).click();
};

test('任务图：真实夹具的切图/卡三行/详情/关闭/打开任务页', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    const pane = page.locator('[data-kind="task-graph-pane"]');
    await expect(pane).toBeVisible({ timeout: 30_000 });
    const firstZoom = await page.locator('[data-kind="graph-zoom"]').innerText();

    // 没关完的在前；有门那张第一张，一张图一次只画一张。
    await expect(page.locator('[data-kind="graph-pick"]')).toContainText('有门等你判的图');
    await expect(page.locator('[data-kind="graph-pick"]')).toContainText('3 个任务');
    await expect(page.locator('[data-kind="graph-node"]')).toHaveCount(3);
    await expect(page.locator('[data-kind="graph-edge"]')).toHaveCount(2);
    await expect(page.locator('[data-kind="graph-edge"][data-state="judge"]')).toHaveCount(1);

    // 卡三行：序号·类型 / 标题 / 状态点+看板原状态词；不写 ThinAgent。
    const firstNode = page.locator('[data-kind="graph-node"]').first();
    const nodeText = await firstNode.innerText();
    expect(nodeText).toMatch(/^\d{2}\s*·/);
    await expect(firstNode).not.toContainText('ThinAgent');
    await expect(page.locator('[data-kind="graph-node"][data-shape="milestone"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-node"] .graph-flag')).toHaveCount(1);

    // 下拉：三张图，全部关了的排最后并写「都关了」。
    await page.locator('[data-kind="graph-pick"]').click();
    const menu = page.getByRole('menuitemradio');
    await expect(menu).toHaveCount(3);
    await expect(menu.nth(2)).toContainText('全部关了的图');
    await expect(menu.nth(2)).toContainText('都关了');
    await page.getByRole('menuitemradio', { name: /两个上游汇到一个下游的图/ }).click();

    // 第二张：写任务左边写色条、里程碑小旗、只读普通框；线的数据来自事实。
    await expect(page.locator('[data-kind="graph-pick"]')).toContainText('两个上游汇到一个下游的图');
    await expect(page.locator('[data-kind="graph-node"]')).toHaveCount(4);
    await expect(page.locator('[data-kind="graph-node"][data-shape="write"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-node"][data-shape="write"] .graph-write-bar')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-node"][data-shape="read"]').first()).toBeVisible();
    await expect(page.locator('[data-kind="graph-node"][data-shape="milestone"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="task-graph-pane"] .graph-legend-box')).toHaveCount(4);
    await expect(page.locator('[data-kind="graph-single-count"]')).toContainText('另有 1 个单派的任务');

    // 详情栏：320px、五段、选中前后亮其余淡；Esc/空白/关闭按钮都能收。
    const detail = page.locator('[data-kind="graph-detail"]');
    const closeDetail = async () => {
      await expect(detail).toHaveCount(0);
    };
    await page.locator('[data-kind="graph-node"][data-title="查 A"]').click();
    await expect(detail).toBeVisible();
    const box = await detail.boundingBox();
    expect(box?.width ?? 0).toBeGreaterThan(300);
    expect(box?.width ?? 0).toBeLessThanOrEqual(322);
    await expect(detail.locator('[data-detail]')).toHaveCount(5);
    await expect(detail.locator('[data-detail="requirement"]')).toContainText('查 A', { timeout: 20_000 });
    await expect(detail.locator('[data-detail="upstream"]')).toContainText('不等谁');
    await expect(detail.locator('[data-detail="downstream"]')).toContainText('核查 A');
    await expect(page.locator('[data-kind="graph-node"][data-title="查 B"]')).toHaveAttribute('data-dim', 'true');
    await expect(page.locator('[data-kind="graph-node"][data-title="核查 A"]')).toHaveAttribute('data-dim', 'false');
    await page.keyboard.press('Escape');
    await closeDetail();

    await page.locator('[data-kind="graph-node"][data-title="查 A"]').click();
    await expect(detail).toBeVisible();
    await page.locator('[data-kind="graph-viewport"]').click({ position: { x: 6, y: 6 } });
    await closeDetail();

    await page.locator('[data-kind="graph-node"][data-title="查 A"]').click();
    await expect(detail).toBeVisible();
    await detail.locator('[data-kind="graph-detail-close"]').click();
    await closeDetail();

    // 切图复位选中；右栏能打开任务页。
    await page.locator('[data-kind="graph-node"][data-title="查 A"]').click();
    await expect(detail).toBeVisible();
    await pickGraph(page, '有门等你判的图');
    await expect(page.locator('[data-kind="graph-pick"]')).toContainText('有门等你判的图');
    await closeDetail();
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText(firstZoom);

    await page.locator('[data-kind="graph-node"]').first().click();
    await expect(detail).toBeVisible();
    await detail.locator('[data-kind="graph-open-task"]').click();
    await expect(page).toHaveURL(/#taskchat:/, { timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('任务图：四种外形、线实虚/动效/payload/图例，减少动效箭头', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    const now = Date.now();
    const task = (id: string, title: string, extra: Record<string, unknown> = {}) => ({
      id, title, stage: 'running', version: 1, attempts: 1, createdAt: now, running: true,
      after: [], afterWaiting: false, gate: false, program: false, programCommands: 0, ...extra,
    });
    const tasks = [
      task('r', '只读一', { parent: 'g1', running: false, stage: 'requirement_met', latest: { reportId: 'p1', state: 'ready', at: now, summary: '查完了' } }),
      task('w', '写一', { parent: 'g1', writeScope: 'src/a.ts' }),
      task('m', '门一', { parent: 'g1', gate: true }),
      task('p', '程序门一', { parent: 'g1', program: true, programCommands: 2, gate: true }),
      task('single', '单派一', { running: false }),
    ];
    const work = {
      generatedAt: now,
      tasks,
      wait: [],
      graphs: [{ id: 'g1', title: '四张形状图', edges: [
        { from: 'r', to: 'w', kind: 'needs', state: 'handed', program: false, live: 'flow', payload: '第 1 份 · 1 文件' },
        { from: 'w', to: 'm', kind: 'guards', state: 'handed', program: false },
        { from: 'm', to: 'p', kind: 'release', state: 'fix', program: true, live: 'back', payload: '打回第 1 份' },
        { from: 'r', to: 'p', kind: 'needs', state: 'waiting', program: false },
      ] }],
    };
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method === 'work') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(work) });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    await expect(page.locator('[data-kind="task-graph-pane"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="graph-node"]')).toHaveCount(4);
    await expect(page.locator('[data-kind="graph-node"][data-shape="read"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-node"][data-shape="write"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-node"][data-shape="milestone"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-node"][data-shape="program"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-node"][data-shape="write"]')).toContainText('写 src/a.ts');
    await expect(page.locator('[data-kind="graph-node"][data-shape="program"]')).toContainText('程序门');
    const mono = page.locator('[data-kind="graph-node"][data-shape="program"] .font-mono').first();
    await expect(mono).toBeVisible();
    expect((await mono.evaluate((el) => getComputedStyle(el).fontFamily)).toLowerCase()).toContain('mono');

    // 实虚 + 动效 + payload。
    await expect(page.locator('[data-kind="graph-edge"][data-state="handed"]')).toHaveCount(2);
    await expect(page.locator('[data-kind="graph-edge"][data-state="waiting"]')).toHaveCount(1);
    const dashed = page.locator('[data-kind="graph-edge"][data-state="waiting"] path').first();
    expect(await dashed.evaluate((el) => getComputedStyle(el).strokeDasharray)).toContain('5');
    await expect(page.locator('[data-kind="graph-edge"][data-live="flow"] path.graph-dash-flow')).toHaveCount(1);
    await expect(page.locator('[data-kind="graph-edge"][data-live="back"] path.graph-dash-back')).toHaveCount(1);
    await expect(page.getByText('第 1 份 · 1 文件', { exact: true })).toBeVisible();
    await expect(page.getByText('打回 · 打回第 1 份', { exact: true })).toBeVisible();
    await expect(page.locator('[data-kind="graph-legend"]')).toContainText('拿到了');
    await expect(page.locator('[data-kind="graph-legend"]')).toContainText('还在等');
    await expect(page.locator('[data-kind="graph-legend"]')).toContainText('在流（往那边走）');
    await expect(page.locator('[data-kind="graph-single-count"]')).toContainText('另有 1 个单派的任务');

    // 减少动效：线不走、中间一个方向箭头。
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.reload();
    await expect(page.locator('[data-kind="graph-node"]')).toHaveCount(4, { timeout: 30_000 });
    await expect(page.locator('[data-kind="graph-reduced-arrow"]')).toHaveCount(2);
    const flowPath = page.locator('[data-kind="graph-edge"][data-live="flow"] path').first();
    expect(await flowPath.evaluate((el) => getComputedStyle(el).animationName)).toBe('none');
  } finally {
    await fixture.close();
  }
});

test('任务图：缩放/适合窗口/紧凑/平移/多图复位/空图', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    const now = Date.now();
    const node = (id: string, title: string) => ({
      id, title, stage: 'running', version: 1, attempts: 1, createdAt: now, running: true,
      after: [], afterWaiting: false, gate: false, program: false, programCommands: 0, parent: 'big',
    });
    const bigNodes = Array.from({ length: 10 }, (_, index) => node('b' + index, '长链任务 ' + index));
    const bigEdges = bigNodes.slice(0, -1).map((item, index) => ({
      from: item.id, to: bigNodes[index + 1].id, kind: 'needs', state: 'waiting', program: false,
    }));
    const work = {
      generatedAt: now,
      tasks: [...bigNodes, { ...node('one', '第二张图的任务'), parent: 'small' }, { id: 'single', title: '单派', stage: 'pending', version: 1, attempts: 0, createdAt: now, running: false, after: [], afterWaiting: false, gate: false, program: false, programCommands: 0 }],
      wait: [],
      graphs: [
        { id: 'big', title: '十张长链图', edges: bigEdges },
        { id: 'small', title: '第二张图', edges: [{ from: 'one', to: 'one', kind: 'needs', state: 'waiting', program: false }] },
      ],
    };
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method === 'work') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(work) });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    await expect(page.locator('[data-kind="task-graph-pane"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="graph-node"]')).toHaveCount(10);

    // 放不下：停在 50%，提示共几个；节点还不是紧凑。
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('50%');
    await expect(page.locator('[data-kind="graph-many"]')).toContainText('共 10 个，拖动看其余的；「适合窗口」看全貌');
    await expect(page.locator('[data-kind="graph-node"][data-compact="true"]')).toHaveCount(0);

    // 适合窗口 30% → 紧凑只剩序号/状态点/标题，线上不写字。
    await page.locator('[data-kind="graph-fit"]').click();
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('30%');
    await expect(page.locator('[data-kind="graph-node"][data-compact="true"]')).toHaveCount(10);
    await expect(page.locator('[data-kind="graph-payload"]')).toHaveCount(0);

    // 缩放 + / -、平移按钮、方向键、Ctrl+滚轮。
    await page.locator('[data-kind="graph-zoom-in"]').click();
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('45%');
    await page.locator('[data-kind="graph-zoom-out"]').click();
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('30%');
    const stage = page.locator('[data-kind="graph-stage"]');
    const beforePan = await stage.getAttribute('style');
    await page.getByLabel('任务图向右平移').click();
    expect(await stage.getAttribute('style')).not.toBe(beforePan);
    const beforeKey = await stage.getAttribute('style');
    await page.locator('[data-kind="graph-viewport"]').press('ArrowRight');
    expect(await stage.getAttribute('style')).not.toBe(beforeKey);
    const beforeWheel = await page.locator('[data-kind="graph-zoom"]').innerText();
    await page.locator('[data-kind="graph-viewport"]').hover();
    await page.keyboard.down('Control');
    await page.mouse.wheel(0, -120);
    await page.keyboard.up('Control');
    await expect.poll(async () => page.locator('[data-kind="graph-zoom"]').innerText()).not.toBe(beforeWheel);

    // 切第二张图：选中/缩放复位。先适合窗口把节点放回视野。
    await page.locator('[data-kind="graph-fit"]').click();
    await page.locator('[data-kind="graph-node"]').first().click();
    await expect(page.locator('[data-kind="graph-detail"]')).toBeVisible();
    await pickGraph(page, '第二张图');
    await expect(page.locator('[data-kind="graph-pick"]')).toContainText('第二张图');
    await expect(page.locator('[data-kind="graph-detail"]')).toHaveCount(0);
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText(/^(50|65|100)%$/);
  } finally {
    await fixture.close();
  }
});

test('任务图：没有图时写文案并报单派数', async ({ page }) => {
  test.setTimeout(60_000);
  const fixture = await startKernelFixture({});
  try {
    const now = Date.now();
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method === 'work') {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            generatedAt: now,
            tasks: [{ id: 'single', title: '单派一', stage: 'pending', version: 1, attempts: 0, createdAt: now, running: false, after: [], afterWaiting: false, gate: false, program: false, programCommands: 0 }],
            wait: [],
            graphs: [],
          }),
        });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    const empty = page.locator('[data-kind="task-graph-empty"]');
    await expect(empty).toBeVisible({ timeout: 30_000 });
    await expect(empty).toContainText('还没有任务图。Kind 把一件事拆成几步、你确认了，这里就有一张。');
    await expect(empty).toContainText('另有 1 个单派的任务，没有前后关系，在看板里。');
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
      if (request.method() === 'POST' && request.url().endsWith('/api') && (request.postData() ?? '').includes('"method":"work"')) calls.push(Date.now());
    });
    await page.goto(urlOf(fixture, '#work?wv=graph'));
    await expect(page.locator('[data-kind="task-graph-pane"]')).toBeVisible({ timeout: 30_000 });
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
  const work = await readFile(fileURLToPath(new URL('../src/pages/work/WorkScene.tsx', import.meta.url)), 'utf8');
  const graphPane = await readFile(fileURLToPath(new URL('../src/pages/work/GraphPane.tsx', import.meta.url)), 'utf8');
  expect(work.match(/useWorkPolling\(\)/g)?.length ?? 0).toBe(1);
  expect(work.slice(work.indexOf('export function WorkScene'))).toContain('useWorkPolling()');
  expect(graphPane).not.toContain('useWorkPolling');

  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    const calls: number[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && request.url().endsWith('/api') && (request.postData() ?? '').includes('"method":"work"')) calls.push(Date.now());
    });
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