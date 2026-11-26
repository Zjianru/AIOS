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
