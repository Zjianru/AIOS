#!/usr/bin/env node
/**
 * shoot.mjs —— 第 1 期截图（app-v14-brief.md §三.6）。
 *
 * 产出到 ../proto-shots/v14-p1/ 下的三个目录：
 *   v14      这一期原型
 *   v13      同状态的旧原型（对照；流程编辑器对照的是 v0 编辑器）
 *   compare  两张并排的对照图
 * 截图清单写进 perf/p1-shots.md。
 *
 * 截图不计时，不受测速锁限制。
 * 用法：node shoot.mjs [--url=http://127.0.0.1:4184] [--only=前缀]
 */

import { chromium } from '/Users/codez/develop/aios/web/node_modules/playwright/index.mjs';
import { mkdirSync, writeFileSync, rmSync } from 'node:fs';

const OUT = '/Users/codez/Documents/aios-design/proto-shots/v14-p1';
const V14 = (process.argv.find((a) => a.startsWith('--url=')) || '--url=http://127.0.0.1:4184').slice(6);
const V13 = 'file:///Users/codez/Documents/aios-design/prototype/v13.html';
const V0 = 'file:///Users/codez/Documents/aios-design/prototype/strategy-editor-v0.html';
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const NL = String.fromCharCode(10);
const only = (process.argv.find((a) => a.startsWith('--only=')) || '').slice(7);

const rows = [];
const errors = [];
const v14Errors = () => errors.filter((e) => e.startsWith('v14'));
const oldErrors = () => errors.filter((e) => !e.startsWith('v14'));
const want = (name) => !only || name.startsWith(only);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function ensureDirs() {
  for (const dir of ['v14', 'v13', 'compare']) mkdirSync(OUT + '/' + dir, { recursive: true });
}

async function main() {
  ensureDirs();
  const browser = await chromium.launch({ executablePath: CHROME, headless: true });

  const mkPage = async (viewport, label) => {
    const context = await browser.newContext({ viewport });
    const page = await context.newPage();
    page.on('pageerror', (e) => errors.push(label + ' pageerror: ' + e.message));
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(label + ' console: ' + m.text());
    });
    page.on('response', (response) => {
      if (response.status() === 404) errors.push(label + ' 404: ' + response.url());
    });
    return page;
  };

  const page = await mkPage({ width: 1440, height: 920 }, 'v14');
  const pageNarrow = await mkPage({ width: 900, height: 860 }, 'v14-narrow');
  const pageWide = await mkPage({ width: 1920, height: 1080 }, 'v14-1920');
  const page1024 = await mkPage({ width: 1024, height: 800 }, 'v14-1024');
  const v13Page = await mkPage({ width: 1440, height: 920 }, 'v13');
  const v0Page = await mkPage({ width: 1440, height: 920 }, 'v0');

  const goto14 = async (target, route) => {
    const url = V14 + '/#' + route;
    // 同一个 URL 再 goto 不会真的重新加载，流式/滚动状态会残留；这种情况强制 reload。
    if (target.url() === url) await target.reload({ waitUntil: 'load' });
    else await target.goto(url, { waitUntil: 'load' });
    await sleep(700);
  };
  const go = async (target, route) => {
    await target.evaluate((r) => window.__v14.go(r), route);
    await sleep(600);
  };
  const setTheme = async (target, theme) => {
    await target.evaluate((t) => window.__v14.setTheme(t), theme);
    await sleep(160);
  };
  const shot = async (target, dir, name) => {
    // 需求 §一：页面不许横向滚动
    const overflow = await target.evaluate(() => ({
      doc: document.documentElement.scrollWidth - window.innerWidth,
      body: document.body.scrollWidth - window.innerWidth,
    }));
    if (dir === 'v14' && (overflow.doc > 1 || overflow.body > 1)) {
      errors.push('v14 横向滚动：' + name + ' ' + JSON.stringify(overflow));
    }
    await target.screenshot({ path: OUT + '/' + dir + '/' + name + '.png' });
  };

  const goto13 = async (route, theme) => {
    const stamp = Math.floor(Math.random() * 1e6);
    await v13Page.goto(V13 + '?theme=' + (theme || 'dark') + '&n=' + stamp + '#' + route, { waitUntil: 'load' });
    await sleep(750);
  };

  const compare = async (name) => {
    const html =
      '<html><body style="margin:0;background:#111;display:flex">' +
      '<img src="file://' + OUT + '/v14/' + name + '.png" style="width:50%;height:auto;display:block">' +
      '<img src="file://' + OUT + '/v13/' + name + '.png" style="width:50%;height:auto;display:block">' +
      '</body></html>';
    const tmp = '/tmp/v14-compare-' + name + '.html';
    writeFileSync(tmp, html);
    const cmp = await browser.newPage({ viewport: { width: 1440, height: 920 } });
    await cmp.goto('file://' + tmp, { waitUntil: 'load' });
    await sleep(350);
    await cmp.screenshot({ path: OUT + '/compare/' + name + '.png', fullPage: true });
    await cmp.close();
    try {
      rmSync(tmp);
    } catch {
      /* ignore */
    }
  };

  const pair = async (name, note, v14Action, v13Action, target, compareTarget) => {
    if (!want(name)) return;
    await v14Action();
    await shot(target || page, 'v14', name);
    await v13Action();
    await shot(compareTarget || v13Page, 'v13', name);
    await compare(name);
    rows.push('| ' + name + ' | ' + note + ' | v14/' + name + '.png | v13/' + name + '.png | compare/' + name + '.png |');
  };

  // ══ A. 每个场景的默认布局（深色）══
  const scenes = [
    ['scene-now', 'now', '现在（第 2 期占位）'],
    ['scene-kind', 'kind', 'Kind（第 1 期：对话 | 在做的事）'],
    ['scene-work', 'work', '工作（第 2 期占位）'],
    ['scene-project', 'project:wsp', '项目页（第 2 期占位）'],
    ['scene-task', 'task:root', '任务详情（第 2 期占位）'],
    ['scene-team', 'team', '团队（第 3 期占位）'],
    ['scene-ability', 'ability', '能力（含「流程」分组）'],
    ['scene-lib', 'lib', '资料（第 3 期占位）'],
    ['scene-memory', 'memory', '记忆（第 3 期占位）'],
  ];
  await goto14(page, 'now');
  for (const item of scenes) {
    await pair(
      item[0],
      item[2],
      async () => {
        await go(page, item[1]);
        await sleep(300);
      },
      async () => {
        await goto13(item[1].split(':')[0], 'dark');
      },
    );
  }

  // ══ B. 深浅两套 ══
  for (const theme of ['dark', 'light']) {
    await pair(
      'theme-kind-' + theme,
      'Kind 场景 · ' + (theme === 'dark' ? '深色' : '浅色'),
      async () => {
        await setTheme(page, theme);
        await go(page, 'kind');
        await sleep(420);
      },
      async () => {
        await goto13('kind', theme);
      },
    );
  }

  // ══ C. 三种宽度 ══
  const widthCases = [
    ['width-1024-kind', page1024, 'Kind 场景 @ 1024'],
    ['width-1440-kind', page, 'Kind 场景 @ 1440'],
    ['width-1920-kind', pageWide, 'Kind 场景 @ 1920'],
  ];
  for (const item of widthCases) {
    await pair(
      item[0],
      item[2],
      async () => {
        await setTheme(page, 'dark');
        await goto14(item[1], 'kind');
        await sleep(400);
      },
      async () => {
        await goto13('kind', 'dark');
      },
      item[1],
    );
  }

  // ══ D. 窄窗自动收成标签页，放宽后恢复 ══
  await setTheme(page, 'dark');
  await pair(
    'narrow-tabs',
    '窗口太窄时自动收成标签页（900 宽）',
    async () => {
      await goto14(pageNarrow, 'kind');
      await sleep(900);
    },
    async () => {
      await goto13('kind', 'dark');
    },
    pageNarrow,
  );
  await pair(
    'narrow-restored',
    '放宽后原样恢复（同一页面回到 1440）',
    async () => {
      await page.setViewportSize({ width: 1440, height: 920 });
      await sleep(900);
    },
    async () => {
      await goto13('kind', 'dark');
    },
  );

  // ══ D2. 侧栏收起成 56px 图标列 ══
  await pair(
    'sidebar-collapsed',
    '侧栏收到 56px 图标列',
    async () => {
      await setTheme(page, 'dark');
      await page.evaluate(() => window.__v14.go('kind'));
      await sleep(400);
      const trigger = page.locator('[data-sidebar="trigger"]').first();
      if (await trigger.count()) {
        await trigger.click();
        await sleep(600);
      }
    },
    async () => {
      await goto13('kind', 'dark');
    },
  );
  // 展开回来，后面的截图按默认布局来
  await page.evaluate(() => window.__v14.setSidebarCollapsed(false));
  await sleep(500);

  // ══ E. Kind 的四个状态 ══
  await goto14(page, 'kind');
  await sleep(900);
  await pair(
    'kind-top',
    'Kind · 首屏（最后一页 5 轮）',
    async () => {
      await page.evaluate(() => window.__v14.scrollToBottom());
      await sleep(500);
    },
    async () => {
      await goto13('kind', 'dark');
    },
  );
  await pair(
    'kind-mid-2000',
    'Kind · 加载完 2000 轮并翻到中间',
    async () => {
      await page.evaluate(async () => {
        await window.__v14.loadAll();
        window.__v14.scrollToFraction(0.5);
      });
      await sleep(1200);
    },
    async () => {
      await goto13('kind', 'dark');
      await v13Page.evaluate(() => {
        const view = document.querySelector('.view:not([hidden])');
        if (view) view.scrollTop = 1400;
      });
      await sleep(300);
    },
  );
  await pair(
    'kind-streaming',
    'Kind · 流式回复进行中',
    async () => {
      await page.evaluate(() => {
        window.__v14.scrollToBottom();
        window.__v14.sendMessage('把多引擎实测的结论写成一段，我发给同事看。', 12000);
      });
      await sleep(1800);
      await page.evaluate(() => window.__v14.scrollToBottom());
      await sleep(300);
    },
    async () => {
      await goto13('kind', 'dark');
    },
  );
  await pair(
    'kind-back-to-latest',
    'Kind · 不在底部时出现「回到最新」',
    async () => {
      await goto14(page, 'kind');
      await sleep(1000);
      await page.evaluate(() => window.__v14.sendMessage('再跑一次，我要看两次的差别。', 20000));
      await sleep(1200);
      let visible = false;
      for (const fraction of [0.8, 0.68, 0.56, 0.44]) {
        await page.evaluate((f) => window.__v14.scrollToFraction(f), fraction);
        await sleep(700);
        visible = await page.evaluate(() =>
          [...document.querySelectorAll('button')].some((b) => b.textContent && b.textContent.includes('回到最新')),
        );
        if (visible) break;
      }
      if (!visible) {
        errors.push('v14 截图状态不对：kind-back-to-latest 没出现「回到最新」');
      }
    },
    async () => {
      await goto13('kind', 'dark');
    },
  );

  // ══ F. v13 Kind 页各类卡片，一个个截 ══
  const cardTypes = [
    'routeDraft',
    'task',
    'askDecision',
    'askApproval',
    'askWeb',
    'artifact',
    'memory',
    'wake',
    'roleJoin',
    'planChanged',
    'milestone',
    'assignRole',
    'quotaAlert',
    'problem',
  ];
  await goto14(page, 'kind');
  await sleep(700);
  await page.evaluate(() => window.__v14.loadAll());
  await sleep(700);
  const indexes = await page.evaluate(() => window.__v14.cardIndexes());
  for (const type of cardTypes) {
    const index = indexes[type];
    if (index === undefined) {
      errors.push('没找到卡片类型：' + type);
      continue;
    }
    await pair(
      'card-' + type,
      'Kind 卡片类型：' + type,
      async () => {
        await page.evaluate((i) => window.__v14.scrollToDataIndex(i), index);
        await sleep(700);
      },
      async () => {
        await goto13('kind', 'dark');
      },
    );
  }

  // ══ G. 流程编辑器（对照 v0 编辑器）══
  const v0Shot = async (theme) => {
    await v0Page.goto(V0 + '?theme=' + theme, { waitUntil: 'load' });
    await sleep(700);
  };
  await pair(
    'flow-default',
    '流程编辑器 · 默认（内容运营）',
    async () => {
      await setTheme(page, 'dark');
      await goto14(page, 'flow:content-ops');
      await sleep(1600);
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );
  await pair(
    'flow-light',
    '流程编辑器 · 浅色',
    async () => {
      await setTheme(page, 'light');
      await sleep(500);
    },
    async () => {
      await v0Shot('light');
    },
    page,
    v0Page,
  );
  await pair(
    'flow-aios',
    '流程编辑器 · AIOS 产品（多价值流网络）',
    async () => {
      await setTheme(page, 'dark');
      await go(page, 'flow:aios-product');
      await sleep(1600);
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );
  const flowStates = [
    [
      'flow-select-node',
      '选中一站（右侧详情栏）',
      async () => {
        const node = page.locator('.react-flow__node').nth(1);
        if (await node.count()) {
          await node.click();
          await sleep(600);
        }
      },
    ],
    [
      'flow-arrange-after',
      '整理之后（elkjs 从左到右分层）',
      async () => {
        const button = page.locator('button', { hasText: /^整理$/ }).first();
        if (await button.count()) {
          await button.click();
          await sleep(1200);
        }
      },
    ],
    [
      'flow-proposal-rejected',
      'Kind 拒绝「发布改成一天同意一次」',
      async () => {
        const chip = page.locator('button', { hasText: '发布改成一天同意一次' }).first();
        if (await chip.count()) {
          await chip.click();
          await sleep(900);
        }
      },
    ],
    [
      'flow-instance',
      '实例态（版本固定、在跑的车）',
      async () => {
        const group = page.locator('[aria-label="看流程还是看实例"]');
        const button = group.locator('button', { hasText: '实例' }).first();
        if (await button.count()) {
          await button.click();
          await sleep(1000);
        }
      },
    ],
    [
      'flow-new-dialog',
      '新建流程：五种来源',
      async () => {
        const button = page.locator('button', { hasText: '新建流程' }).first();
        if (await button.count()) {
          await button.click();
          await sleep(700);
        }
      },
    ],
    [
      'flow-more-menu',
      '更多菜单（在用的只能归档）',
      async () => {
        await page.keyboard.press('Escape');
        await sleep(250);
        const button = page.locator('[aria-label="更多"]').first();
        if (await button.count()) {
          await button.click();
          await sleep(500);
        }
      },
    ],
  ];
  for (const item of flowStates) {
    await pair(
      item[0],
      '流程编辑器 · ' + item[1],
      async () => {
        await page.keyboard.press('Escape');
        await sleep(250);
        await goto14(page, 'flow:content-ops');
        await sleep(1400);
        await item[2]();
      },
      async () => {
        await v0Shot('dark');
      },
      page,
      v0Page,
    );
  }

  // ══ G1b. 提议对比（绿加 / 红去 / 蓝改）与接受后（版本推进到 v4）══
  const openSuggestion = async () => {
    await goto14(page, 'flow:content-ops');
    await sleep(1500);
    const chip = page.locator('button', { hasText: '去掉抖音' }).first();
    if (await chip.count()) {
      await chip.click();
      await sleep(900);
    }
  };
  await pair(
    'flow-proposal',
    '修改提议对比：绿加 / 红去 / 蓝改三态（只读）',
    async () => {
      await openSuggestion();
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );
  await pair(
    'flow-accepted',
    '接受提议，版本推进到 v4，画布是改后的样子',
    async () => {
      await openSuggestion();
      const button = page.locator('button', { hasText: '接受，存为' }).first();
      if (await button.count()) {
        await button.click();
        await sleep(1100);
      }
      const hasV4 = await page.evaluate(() => document.body.innerText.includes('v4'));
      if (!hasV4) errors.push('v14 截图状态不对：flow-accepted 里没有出现 v4');
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );

  // ══ G2. 拖零件、对齐线、提炼草稿、全部补上 ══
  const canvasCenter = async () => {
    const box = await page.locator('.react-flow__pane').first().boundingBox();
    return box ? { x: box.x + box.width / 2, y: box.y + box.height / 2, box } : null;
  };

  await pair(
    'flow-drag-part-dragging',
    '拖零件到线上（拖动中，能放的线高亮）',
    async () => {
      await goto14(page, 'flow:content-ops');
      await sleep(1500);
      const part = page.getByRole('button', { name: '门', exact: true }).first();
      const start = await part.boundingBox();
      const center = await canvasCenter();
      if (start && center) {
        await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
        await page.mouse.down();
        for (let i = 1; i <= 12; i += 1) {
          await page.mouse.move(
            start.x + (center.x - start.x) * (i / 12),
            start.y + (center.y - start.y) * (i / 12),
          );
          await sleep(30);
        }
        await sleep(400);
      }
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );
  await pair(
    'flow-drag-part-dropped',
    '拖零件到线上（放下后，线一分为二）',
    async () => {
      await page.mouse.up();
      await sleep(900);
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );

  await pair(
    'flow-align-guides',
    '对齐线出现（拖节点靠近另一个的中心）',
    async () => {
      await goto14(page, 'flow:content-ops');
      await sleep(1500);
      const nodes = page.locator('.react-flow__node');
      const from = await nodes.nth(1).boundingBox();
      const to = await nodes.nth(2).boundingBox();
      if (from && to) {
        await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
        await page.mouse.down();
        for (let i = 1; i <= 10; i += 1) {
          await page.mouse.move(
            from.x + from.width / 2 + ((to.x - from.x) * i) / 10,
            from.y + from.height / 2 + ((to.y - from.y) * i) / 10 + 2,
          );
          await sleep(40);
        }
        await sleep(400);
      }
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );

  const openDistilDraft = async () => {
    await goto14(page, 'flow:content-ops');
    await sleep(1400);
    const newButton = page.locator('button', { hasText: '新建流程' }).first();
    if (!(await newButton.count())) return;
    await newButton.click();
    await sleep(500);
    const distil = page.locator('button', { hasText: '从做过的事提炼' }).first();
    if (!(await distil.count())) return;
    await distil.click();
    await sleep(400);
    const weekly = page.locator('button', { hasText: '周报' }).first();
    if (!(await weekly.count())) return;
    await weekly.click();
    await sleep(900);
  };
  await pair(
    'flow-distil-draft',
    '提炼出来的草稿（2 处要补）',
    openDistilDraft,
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );
  await pair(
    'flow-fix-all',
    '全部按建议补上',
    async () => {
      await page.evaluate(() => window.__v14.go('flow:content-ops'));
      await sleep(1200);
      await openDistilDraft();
      const fixAll = page.locator('button', { hasText: '全部按建议补上' }).first();
      if (await fixAll.count()) {
        await fixAll.click();
        await sleep(700);
      }
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );

  // ══ G3. 画布缩放（下限）与多线并排特写 ══
  const readZoom = () =>
    page.evaluate(() => {
      const vp = document.querySelector('.react-flow__viewport');
      if (!vp) return null;
      const matrix = new DOMMatrixReadOnly(getComputedStyle(vp).transform);
      return Math.round(matrix.a * 100) / 100;
    });

  await pair(
    'flow-zoom-fit',
    '画布默认适配缩放：下限 0.92（站名 13.8px / 副标 11px），放不下就横向平移',
    async () => {
      await goto14(page, 'flow:aios-product');
      await sleep(1800);
      const zoom = await readZoom();
      if (zoom !== null && zoom < 0.91) errors.push('v14 截图状态不对：flow-zoom-fit 缩放 ' + zoom + ' 低于下限 0.92');
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );

  await pair(
    'flow-zoom-narrow',
    '1024 宽同样不缩到看不清（保持 0.92，横向平移）',
    async () => {
      await page1024.evaluate(() => window.__v14.go('flow:aios-product'));
      await sleep(2000);
      const zoom = await page1024.evaluate(() => {
        const vp = document.querySelector('.react-flow__viewport');
        if (!vp) return null;
        return Math.round(new DOMMatrixReadOnly(getComputedStyle(vp).transform).a * 100) / 100;
      });
      if (zoom !== null && zoom < 0.91) errors.push('v14 截图状态不对：flow-zoom-narrow 缩放 ' + zoom + ' 低于下限 0.92');
    },
    async () => {
      await v0Shot('dark');
    },
    page1024,
    v0Page,
  );

  await pair(
    'flow-interchange',
    '多线共用路段并排画 + 共用站是换乘胶囊（全图）',
    async () => {
      await goto14(page, 'flow:aios-product');
      await sleep(1800);
    },
    async () => {
      await v0Shot('dark');
    },
    page,
    v0Page,
  );
  // 再压一张特写（不参与 pair，直接存）
  {
    const box = await page.locator('.react-flow__pane').first().boundingBox();
    if (box) {
      const clipWidth = 620;
      const clipHeight = 300;
      await page.screenshot({
        path: OUT + '/v14/flow-interchange-closeup.png',
        clip: {
          x: Math.max(0, box.x + box.width - clipWidth - 20),
          y: Math.max(0, box.y + box.height / 2 - clipHeight / 2),
          width: clipWidth,
          height: clipHeight,
        },
      });
    }
  }

  // ══ H. 减少动效 ══
  await pair(
    'reduced-motion',
    '减少动效（开）',
    async () => {
      await page.evaluate(() => window.__v14.setReducedMotion(true));
      await setTheme(page, 'dark');
      await go(page, 'kind');
      await sleep(500);
    },
    async () => {
      await goto13('kind', 'dark');
    },
  );
  await page.evaluate(() => window.__v14.setReducedMotion(null));

  await browser.close();

  const lines = [
    '# 第 1 期截图清单',
    '',
    '目录：/Users/codez/Documents/aios-design/proto-shots/v14-p1/',
    '',
    '- v14/：本期原型；v13/：同状态的旧原型截图（流程编辑器对照的是 v0 编辑器 prototype/strategy-editor-v0.html）；',
    '- compare/：上面两张并排的对照图（左 v14、右 v13）。',
    '- 浏览器：Playwright 驱动的 Chrome；控制台与 pageerror 必须为空（见下）。',
    '',
    '| 名字 | 状态 | v14 | v13 对照 | 并排 |',
    '|---|---|---|---|---|',
    ...rows,
    '',
    '## 控制台',
    '',
    v14Errors().length ? v14Errors().map((e) => '- ' + e).join(NL) : '- v14（含窄窗/1920/1024 三个页面）：console error 与 pageerror 都是 []',
    '- v13 / v0（对照用的旧原型）：' + (oldErrors().length ? oldErrors().length + ' 条，见下（旧原型自己的问题，不计入本期交付）' : '[]'),
    ...(oldErrors().length ? ['', ...oldErrors().map((e) => '  - ' + e)] : []),
    '',
    '## 说明',
    '',
    '- 「窄窗收成标签页」在 900 宽触发（面板最小宽度 420 + 280 + 分隔条 = 706，扣掉侧栏 220 与窗口外边距后放不下）。',
    '- v13 没有分页/虚拟列表，也没有 dock 分栏；Kind 的中间态、流式、「回到最新」这些状态在 v13 里没有对应物，',
    '  对照图给的是 v13 Kind 页整体，作视觉基准用。',
    '- 卡片类型的对照图同样是 v13 Kind 页：v13 的这些卡片散在对话里，没法逐个定位。',
    '',
  ];
  mkdirSync('/Users/codez/Documents/aios-design/app-v14/perf', { recursive: true });
  writeFileSync('/Users/codez/Documents/aios-design/app-v14/perf/p1-shots.md', lines.join(NL) + NL);
  console.log('shots: ' + rows.length + ' 组；console errors: ' + errors.length);
  if (errors.length) console.log(errors.slice(0, 5).join(NL));
}

main().catch((error) => {
  console.error(error.message || error);
  process.exit(1);
});
