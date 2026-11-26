import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T092.2：原型 5 个状态落点（切图复位/里程碑与程序门字/空图加单派/长标题两行/紧凑选中描边）。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash: string) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

test('任务图 .2 原型状态：长标题两行、紧凑选中描边、里程碑/程序门字', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    const now = Date.now();
    const base = {
      stage: 'running', version: 1, attempts: 1, createdAt: now, running: true,
      after: [], afterWaiting: false, gate: false, program: false, programCommands: 0, parent: 'g1',
    };
    const work = {
      generatedAt: now,
      tasks: [
        { ...base, id: 'long', title: '一个特别长的标题要能折成两行而不是一直往右边顶出去把卡撑坏并且线上小字也要能看得见', writeScope: 'src/very-long.ts' },
        { ...base, id: 'm', title: '里程碑', gate: true },
        { ...base, id: 'p', title: '程序门', gate: true, program: true, programCommands: 2 },
      ],
      wait: [],
      graphs: [{ id: 'g1', title: '原型状态图', edges: [
        { from: 'long', to: 'm', kind: 'needs', state: 'waiting', program: false },
        { from: 'm', to: 'p', kind: 'release', state: 'handed', program: true },
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

    // 长标题两行：标题元素带 line-clamp-2。
    const longTitle = page.locator('[data-kind="graph-node"][data-title*="一个特别长的标题"] strong');
    await expect(longTitle).toHaveClass(/line-clamp-2/);
    expect(await longTitle.evaluate((el) => getComputedStyle(el).webkitLineClamp)).toBe('2');

    // 里程碑/程序门字。
    await expect(page.locator('[data-kind="graph-node"][data-shape="milestone"]')).toContainText('里程碑');
    await expect(page.locator('[data-kind="graph-node"][data-shape="program"]')).toContainText('程序门');

    // 缩到 <50% 后紧凑选中描边。
    await page.locator('[data-kind="graph-zoom-out"]').click();
    await page.locator('[data-kind="graph-zoom-out"]').click();
    await page.locator('[data-kind="graph-node"]').first().click();
    await expect(page.locator('[data-kind="graph-node"][data-title*="一个特别长的标题"]')).toHaveAttribute('data-compact', 'true');
    const selected = page.locator('[data-kind="graph-node"][data-selected="true"]');
    await expect(selected).toHaveCount(1);
    expect(await selected.evaluate((el) => getComputedStyle(el).outlineWidth)).not.toBe('0px');
  } finally {
    await fixture.close();
  }
});
test('任务图 .2 交互：50% 镜头对最左未完成/留前置、拖动/滚轮/两种缩放/边界', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    const now = Date.now();
    const nodes = Array.from({ length: 10 }, (_, index) => ({
      id: 'n' + index, title: '长链任务 ' + index,
      stage: index < 5 ? 'closed' : 'running', version: 1, attempts: 1, createdAt: now,
      running: index >= 5, after: [], afterWaiting: false, gate: false, program: false, programCommands: 0, parent: 'g1',
    }));
    const work = {
      generatedAt: now,
      tasks: nodes,
      wait: [],
      graphs: [{ id: 'g1', title: '十节点长链', edges: nodes.slice(0, -1).map((item, index) => ({
        from: item.id, to: nodes[index + 1].id, kind: 'needs', state: 'waiting', program: false,
      })) }],
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
    const stage = page.locator('[data-kind="graph-stage"]');
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('50%', { timeout: 30_000 });
    const viewport = page.locator('[data-kind="graph-viewport"]');
    const box = await viewport.boundingBox();
    const styleOf = async () => (await stage.getAttribute('style')) ?? '';
    const parse = (style: string) => {
      const match = style.replace(/\s/g, '').match(/translate\((-?[\d.]+)px,(-?[\d.]+)px\)scale\(([\d.]+)\)/);
      return match ? { x: Number(match[1]), y: Number(match[2]), scale: Number(match[3]) } : null;
    };
    const initial = parse(await styleOf());
    expect(initial?.scale).toBe(0.5);

    // 50% 放不下：镜头对准最左未完成（第 5 个，第 2 列），左边留一列前置。
    const target = page.locator('[data-kind="graph-node"][data-title="长链任务 5"]');
    const relative = (item: { x: number; width: number } | null) => ({
      x: (item?.x ?? 0) - (box?.x ?? 0),
      right: (item?.x ?? 0) + (item?.width ?? 0) - (box?.x ?? 0),
    });
    const targetBox = relative(await target.boundingBox());
    expect(targetBox.x).toBeGreaterThan(80);
    expect(targetBox.x).toBeLessThan(260);
    const predecessorBox = relative(
      await page.locator('[data-kind="graph-node"][data-title="长链任务 4"]').boundingBox(),
    );
    expect(predecessorBox.right).toBeGreaterThan(0);
    expect(predecessorBox.x).toBeGreaterThan(0);
    const firstDoneBox = relative(
      await page.locator('[data-kind="graph-node"][data-title="长链任务 0"]').boundingBox(),
    );
    expect(firstDoneBox.right).toBeLessThan(0);

    // 普通滚轮平移。
    const beforeWheel = parse(await styleOf());
    await viewport.hover();
    await page.mouse.wheel(0, 120);
    await page.waitForTimeout(250);
    const afterWheel = parse(await styleOf());
    expect(afterWheel?.y).not.toBe(beforeWheel?.y);
    expect(afterWheel?.y ?? 0).toBeCloseTo((beforeWheel?.y ?? 0) - 120, 0);

    // 拖背景平移。
    const beforeDrag = await styleOf();
    await page.mouse.move((box?.x ?? 0) + 8, (box?.y ?? 0) + (box?.height ?? 0) - 12);
    await page.mouse.down();
    await page.mouse.move((box?.x ?? 0) + 48, (box?.y ?? 0) + (box?.height ?? 0) + 18, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(80);
    expect(await styleOf()).not.toBe(beforeDrag);

    // Ctrl+滚轮、⌘+滚轮按指针缩放。
    const percent = async () => page.locator('[data-kind="graph-zoom"]').innerText();
    await viewport.hover();
    const beforeCtrl = await percent();
    await page.keyboard.down('Control');
    await page.mouse.wheel(0, -120);
    await page.keyboard.up('Control');
    await expect.poll(percent).not.toBe(beforeCtrl);
    const beforeMeta = await percent();
    await page.keyboard.down('Meta');
    await page.mouse.wheel(0, -120);
    await page.keyboard.up('Meta');
    await expect.poll(percent).not.toBe(beforeMeta);

    // 适合窗口 30% 与缩放边界 0.3–1.5。
    await page.locator('[data-kind="graph-fit"]').click();
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('30%');
    for (let i = 0; i < 3; i += 1) await page.locator('[data-kind="graph-zoom-out"]').click();
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('30%');
    for (let i = 0; i < 12; i += 1) await page.locator('[data-kind="graph-zoom-in"]').click();
    await expect(page.locator('[data-kind="graph-zoom"]')).toHaveText('150%');
  } finally {
    await fixture.close();
  }
});
