import { test, expect, type Page } from '@playwright/test';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import type { AddressInfo } from 'node:net';
import path from 'node:path';
import { startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T015.1（画布外壳）：画布显示、主题变量、420px、刷新回初始、/canvas-frame 的 CSP 和自检、
 * 地址清单和隔离、死循环不卡页面。
 * T015.2（版本）：第 2 版用 edits 出、旧版缩成一行、点了跳到最新、标题栏切版本、刷新回最新；
 * 页面切到旧版后发一句，发给模型的画布仍是最新版。
 *
 * 画布那一轮由真内核执行 canvas 工具产生（夹具只是假模型的回放脚本），所以页面看到的
 * HTML 一定来自日志里的 tool/result.meta.canvas。
 */

/** strategy 第 5 节的 CSP（五个 CDN 按 spec 实现决定展开）。 */
const CANVAS_FRAME_CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline' cdnjs.cloudflare.com cdn.jsdelivr.net/npm/ unpkg.com cdn.tailwindcss.com code.jquery.com",
  "style-src 'unsafe-inline' fonts.googleapis.com cdnjs.cloudflare.com cdn.jsdelivr.net/npm/ unpkg.com cdn.tailwindcss.com code.jquery.com",
  'font-src data: fonts.gstatic.com',
  'img-src data: blob:',
  'media-src data: blob:',
  'connect-src fonts.googleapis.com fonts.gstatic.com',
  "frame-src 'none'",
  "object-src 'none'",
  "base-uri 'none'",
  "frame-ancestors 'self'",
].join('; ');

const urlOf = (fixture: KernelFixture) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function ask(page: Page, text = '每月存 3000，十年后有多少，画出来') {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 等画布那一轮出现（画布标题栏里有标题）。 */
async function waitCanvas(page: Page, title: string) {
  await expect(page.locator('[data-kind="canvas"]')).toContainText(title, { timeout: 40_000 });
}

/** 画布 iframe 的 frame（sandbox 没给 allow-same-origin，是跨源 frame，Playwright 仍能进去）。 */
async function canvasFrame(page: Page) {
  const locator = page.locator('iframe[data-kind="canvas-frame"]').first();
  await expect(locator).toBeAttached();
  const element = await locator.elementHandle();
  if (!element) throw new Error('画布 iframe 不在页面上');
  let frame = await element.contentFrame();
  for (let attempt = 0; attempt < 100 && !frame; attempt += 1) {
    await page.waitForTimeout(50);
    frame = await element.contentFrame();
  }
  if (!frame) throw new Error('画布 iframe 没有 contentFrame');
  return frame;
}

/** 会话日志里的全部事件（root 会话的文件）。 */
async function sessionEvents(fixture: KernelFixture) {
  const root = JSON.parse(await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8')) as {
    sessionId: string;
  };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  return text
    .split('\n')
    .filter((line) => line.trim())
    .map((line) => JSON.parse(line) as { type: string; data: Record<string, any> });
}

/** 画布文档里的某个 CSS 变量值。 */
async function canvasVariable(page: Page, name: string): Promise<string> {
  const frame = await canvasFrame(page);
  return frame.evaluate(
    (variable) => getComputedStyle(document.documentElement).getPropertyValue(variable).trim(),
    name,
  );
}

test('画布显示：HTML 从日志 meta 来，模型只收到一句', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');

    // 沙箱：只给脚本和表单，不给同源、弹窗、下载。
    const iframe = page.locator('iframe[data-kind="canvas-frame"]');
    await expect(iframe).toHaveAttribute('sandbox', 'allow-scripts allow-forms');

    // 画布真的跑起来了：外壳报高度后 iframe 有高度，里面的内容渲染出来。
    const frame = await canvasFrame(page);
    await expect(frame.locator('#total')).toHaveText('¥378,000');
    expect(await iframe.evaluate((element) => element.getBoundingClientRect().height)).toBeGreaterThan(0);

    // 内核侧：tool/result.meta.canvas 是全文，给模型的内容只有一句。
    const events = await sessionEvents(fixture);
    const result = events.find((event) => event.type === 'tool/result' && event.data.meta?.canvas);
    expect(result, '日志里要有带 meta.canvas 的 tool/result').toBeTruthy();
    const canvas = result!.data.meta.canvas as Record<string, unknown>;
    expect(canvas.title).toBe('每月存钱算一算');
    expect(canvas.n).toBe(1);
    expect(String(canvas.id)).toMatch(/^cv_/);
    expect(String(canvas.html)).toContain('id="total"');
    const text = (result!.data.message.content as { text?: string }[])
      .map((block) => block.text ?? '')
      .join('');
    expect(text).toBe('画布「每月存钱算一算」第 1 版已显示（' + String(canvas.id) + '）');
  } finally {
    await fixture.close();
  }
});

test('画布跟着 AIOS 的深浅色：主题变量换一套', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');

    const raisedOf = () =>
      page.evaluate(() =>
        getComputedStyle(document.documentElement).getPropertyValue('--raised').trim(),
      );
    // 深色：画布拿到的背景变量和页面自己的 --raised 一样。
    await expect.poll(() => canvasVariable(page, '--color-background-primary')).toBe(
      (await raisedOf()),
    );
    const dark = await canvasVariable(page, '--color-background-primary');

    await page.getByRole('button', { name: '外观', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await expect.poll(() => canvasVariable(page, '--color-background-primary')).not.toBe(dark);
    expect(await canvasVariable(page, '--color-background-primary')).toBe(await raisedOf());
  } finally {
    await fixture.close();
  }
});

test('拖滑块、点按钮当场变；刷新以后画布还在、滑块回初始', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');

    const frame = await canvasFrame(page);
    await expect(frame.locator('#total')).toHaveText('¥378,000');
    // 画布自己处理输入：拖到最大，输出当场变。
    await frame.evaluate(() => {
      const slider = document.getElementById('m') as HTMLInputElement;
      slider.value = '10000';
      slider.dispatchEvent(new Event('input'));
    });
    await expect(frame.locator('#total')).toHaveText('¥1,260,000');
    await frame.locator('#label').click();
    await expect(frame.locator('#label')).toHaveText('已经换了');

    // 刷新：画布和版本从日志重建，画布里的状态回到初始。
    await page.reload();
    await waitCanvas(page, '每月存钱算一算');
    const reloaded = await canvasFrame(page);
    await expect(reloaded.locator('#m')).toHaveValue('3000');
    await expect(reloaded.locator('#total')).toHaveText('¥378,000');
  } finally {
    await fixture.close();
  }
});

test('内容超过 420px：封顶 420、标题栏出「展开」，点了展开再收起', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  try {
    // 展开以后画布比一屏（720）高；用高一点的视口，标题栏不会被列表的「钉到底」挤出去，
    // 点「收起」时按钮也不会在 mousedown 和 mouseup 之间移动。
    await page.setViewportSize({ width: 1280, height: 1600 });
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');

    const iframe = page.locator('iframe[data-kind="canvas-frame"]');
    await expect(iframe).toBeVisible();
    await expect(page.getByRole('button', { name: '展开', exact: true })).toBeVisible();
    const capped = await iframe.evaluate((element) => element.getBoundingClientRect().height);
    expect(Math.round(capped)).toBe(420);

    await page.getByRole('button', { name: '展开', exact: true }).click();
    await expect
      .poll(async () => Math.round(await iframe.evaluate((element) => element.getBoundingClientRect().height)))
      .toBeGreaterThan(420);

    await page.getByRole('button', { name: '收起', exact: true }).click();
    await expect
      .poll(async () => Math.round(await iframe.evaluate((element) => element.getBoundingClientRect().height)))
      .toBe(420);
  } finally {
    await fixture.close();
  }
});

test('/canvas-frame：CSP 头逐字对上，直接打开报错，非父窗口的消息被忽略', async ({ page, request }) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  try {
    // 不要 cookie，只要 Host 对。两条路径都要带同一份 CSP：
    // /canvas-frame.html 是 public 里的同名文件，曾被静态 fallback 原样 200（无 CSP）。
    for (const path of ['/canvas-frame', '/canvas-frame.html']) {
      const response = await request.get(fixture.baseUrl + path);
      expect(response.status(), path).toBe(200);
      expect(response.headers()['content-security-policy'], path).toBe(CANVAS_FRAME_CSP);
      expect(response.headers()['content-security-policy'], path).toContain(
        "frame-ancestors 'self'",
      );
    }

    // 大小写变体也不能被静态 fallback 原样发出来（macOS 的文件系统不分大小写）：
    // 要么 404，要么和官方路径一样带 CSP，不能 200 + 无头。
    const caseVariant = await request.get(fixture.baseUrl + '/CANVAS-FRAME.HTML');
    expect(
      caseVariant.status() === 404 ||
        caseVariant.headers()['content-security-policy'] === CANVAS_FRAME_CSP,
      '大小写变体走了静态 fallback 且没有 CSP',
    ).toBe(true);

    // 直接打开（不在 iframe 里）：外壳自检报错。
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto(fixture.baseUrl + '/canvas-frame');
    await expect.poll(() => errors.length).toBeGreaterThan(0);
    expect(errors.join('\n')).toContain('画布只能嵌在对话里');

    // 非父窗口发来的消息被忽略：另外建一个 iframe，让它给画布发一份假 HTML。
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');
    await page.evaluate(() => {
      const helper = document.createElement('iframe');
      helper.id = 't015-helper';
      helper.style.display = 'none';
      helper.srcdoc = '<body></body>';
      document.body.append(helper);
    });
    const helperElement = await page.locator('#t015-helper').elementHandle();
    if (!helperElement) throw new Error('helper iframe 不在页面上');
    let helperFrame = await helperElement.contentFrame();
    for (let attempt = 0; attempt < 100 && !helperFrame; attempt += 1) {
      await page.waitForTimeout(50);
      helperFrame = await helperElement.contentFrame();
    }
    if (!helperFrame) throw new Error('helper iframe 没有 contentFrame');
    // 先确认这个「外来窗口」真的拿到了画布 iframe —— 否则这条断言等于空转。
    const found = await helperFrame.evaluate(() => {
      const target = parent.document.querySelector('iframe[data-kind="canvas-frame"]') as
        | HTMLIFrameElement
        | null;
      target?.contentWindow?.postMessage(
        { aiosHost: true, type: 'render', html: '<div id="pwned">pwned</div>' },
        '*',
      );
      return Boolean(target?.contentWindow);
    });
    expect(found).toBe(true);
    await page.waitForTimeout(300);

    const frame = await canvasFrame(page);
    await expect(frame.locator('#total')).toHaveText('¥378,000');
    expect(await frame.locator('#pwned').count()).toBe(0);
  } finally {
    await fixture.close();
  }
});

test('地址清单：清单里的 CDN 能加载，清单外的脚本、图片、fetch 被拦；/api、/ws、parent 都被拒', async ({
  page,
}) => {
  const fixture = await startKernelFixture({ canvas: 'cdn' });
  try {
    let offListRequests = 0;
    await page.route('https://evil.example/**', async (route) => {
      offListRequests += 1;
      await route.fulfill({ status: 200, body: 'x' });
    });
    await page.route('https://cdn.jsdelivr.net/npm/t015-fake-lib/index.js', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'text/javascript',
        body: 'window.__t015Lib = true;',
      });
    });

    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, 'CDN 与隔离');
    const frame = await canvasFrame(page);

    // 清单里的库：Playwright 拦下来的假库真的执行了。
    await expect(frame.locator('#marker')).toHaveText('lib:1', { timeout: 20_000 });
    // 清单外的脚本、图片、fetch：CSP 先拦掉，请求根本没发出去。
    await expect(frame.locator('#fetch')).toHaveText('fetch:blocked');
    await expect.poll(() => offListRequests).toBe(0);
    expect(await frame.evaluate(() => (document.getElementById('extimg') as HTMLImageElement).naturalWidth)).toBe(0);

    // 画布拿不到 AIOS 的东西：/api、/ws、parent.document 都失败。
    await expect(frame.locator('#api')).toHaveText('api:blocked');
    await expect(frame.locator('#ws')).toHaveText('ws:blocked');
    await expect(frame.locator('#parent')).toHaveText('parent:blocked');
  } finally {
    await fixture.close();
  }
});

test('死循环画布：页面计数器照走，画布能收起', async ({ page }) => {
  test.setTimeout(90_000);
  const fixture = await startKernelFixture({ canvas: 'loop' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '死循环画布');

    await page.evaluate(() => {
      (window as unknown as { __ticks: number }).__ticks = 0;
      setInterval(() => {
        const scope = window as unknown as { __ticks: number };
        scope.__ticks += 1;
      }, 50);
    });
    await page.waitForTimeout(1500);
    const ticks = await page.evaluate(() => (window as unknown as { __ticks: number }).__ticks);
    expect(ticks).toBeGreaterThan(10);

    await page.getByRole('button', { name: '收起画布', exact: true }).click();
    await expect(page.locator('iframe[data-kind="canvas-frame"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('另一个 localhost 端口嵌入：两条路径都被 frame-ancestors 挡住，注入 render 不生效', async ({
  page,
}) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  // 对手页：另一个 localhost 端口上的普通页面，负责嵌 iframe、发 render、收注入脚本的回执。
  const attacker = createServer((request, response) => {
    if (request.url !== '/') {
      response.writeHead(404);
      response.end();
      return;
    }
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    response.end(
      '<!doctype html><meta charset="utf-8"><body><script>' +
        'window.__pwned = 0;' +
        'addEventListener("message", function (event) {' +
        '  if (event.data && event.data.pwned) window.__pwned += 1;' +
        '});' +
        '</script></body>',
    );
  });
  try {
    await new Promise<void>((resolve) => attacker.listen(0, '127.0.0.1', resolve));
    const attackerPort = (attacker.address() as AddressInfo).port;
    expect(attackerPort).not.toBe(Number(new URL(fixture.baseUrl).port));
    await page.goto('http://127.0.0.1:' + String(attackerPort) + '/');

    for (const path of ['/canvas-frame', '/canvas-frame.html']) {
      const pwned = await page.evaluate(
        async ({ base, path }) => {
          const scope = window as unknown as { __pwned: number };
          scope.__pwned = 0;
          for (const old of Array.from(document.querySelectorAll('iframe'))) old.remove();
          const frame = document.createElement('iframe');
          frame.src = base + path;
          document.body.append(frame);
          const html =
            '<script>parent.postMessage({ pwned: true }, "*")<' +
            '/script><div id="pwned">pwned</div>';
          // 外壳脚本就绪前消息会丢，重发几次；被 frame-ancestors 挡住时一次也到不了。
          for (let attempt = 0; attempt < 20; attempt += 1) {
            await new Promise((resolve) => setTimeout(resolve, 100));
            frame.contentWindow?.postMessage({ aiosHost: true, type: 'render', html }, '*');
          }
          await new Promise((resolve) => setTimeout(resolve, 300));
          return scope.__pwned;
        },
        { base: fixture.baseUrl, path },
      );
      expect(pwned, path + ' 被另一个 localhost 端口嵌入并注入了 render').toBe(0);

      // 再用 Playwright 的跨源视角看 #pwned；被挡的 frame 可能根本拿不到文档。
      let nodes = 0;
      for (const frame of page.frames()) {
        if (frame === page.mainFrame()) continue;
        try {
          nodes += await frame.locator('#pwned').count();
        } catch {
          // frame 没有可访问的文档（被 frame-ancestors 挡住）。
        }
      }
      expect(nodes, path + ' 的注入内容进了 DOM').toBe(0);
    }
  } finally {
    await new Promise<void>((resolve) => attacker.close(() => resolve()));
    await fixture.close();
  }
});

/** 第 2 版：第 1 版那一轮缩成一行，点了跳到最新；标题栏切版本；刷新回最新。 */
test('第 2 版：旧版缩成一行、点它跳到最新；标题栏切版本，刷新回最新', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'versions' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');
    await expect((await canvasFrame(page)).locator('#v')).toHaveText('1');

    // 第二句：Kind 用 edits 给同一个画布出第 2 版。
    await ask(page, '把柱子换成折线');
    const stub = page.locator('[data-kind="canvas-stub"]');
    await expect(stub).toHaveCount(1, { timeout: 40_000 });
    await expect(stub).toContainText('画布 · 每月存钱算一算');
    await expect(stub).toContainText('第 1 版');
    await expect(stub).toContainText('第一版：三个滑块');
    await expect(stub).toContainText('已有第 2 版');

    // 第 2 版在最新那一轮展开；内容来自日志里的 edits 结果。
    await expect(page.locator('iframe[data-kind="canvas-frame"]')).toHaveCount(1);
    const live = await canvasFrame(page);
    await expect(live.locator('#v')).toHaveText('2');
    await expect(live.locator('#total')).toHaveText('¥1,260,000');

    // 点旧版那一行：滚到出最新版的那一轮（往下走）。
    const scroller = page.locator('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
    await stub.scrollIntoViewIfNeeded();
    const before = await scroller.evaluate((element) => element.scrollTop);
    await stub.click();
    await expect
      .poll(() => scroller.evaluate((element) => element.scrollTop), { timeout: 15_000 })
      .toBeGreaterThan(before);
    await expect(page.locator('[data-kind="canvas"]')).toBeInViewport();

    // 标题栏切到第 1 版：只在页面上切。
    const versions = page.locator('[data-kind="canvas-versions"]');
    await versions.click();
    await page.locator('[data-kind="canvas-version"][data-version="1"]').click();
    await expect(versions).toContainText('第 1 版');
    await expect(versions).toContainText('旧');
    await expect((await canvasFrame(page)).locator('#v')).toHaveText('1');
    await expect((await canvasFrame(page)).locator('#total')).toHaveText('¥378,000');

    // 刷新：回到最新版。
    await page.reload();
    await waitCanvas(page, '每月存钱算一算');
    await expect(page.locator('[data-kind="canvas-versions"]')).toContainText('第 2 版');
    await expect((await canvasFrame(page)).locator('#v')).toHaveText('2');
  } finally {
    await fixture.close();
  }
});

/** 页面切到第 1 版以后发一句：发给模型的画布仍是第 2 版（日志 + 请求体各一条，故事 7）。 */
test('切到第 1 版以后发一句：请求里给模型的画布仍是第 2 版', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'versions' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');
    await ask(page, '把柱子换成折线');
    const versions = page.locator('[data-kind="canvas-versions"]');
    await expect(versions).toHaveCount(1, { timeout: 40_000 });

    // 页面上切到第 1 版。
    await versions.click();
    await page.locator('[data-kind="canvas-version"][data-version="1"]').click();
    await expect((await canvasFrame(page)).locator('#v')).toHaveText('1');

    // 第 3 轮：模型只回一句话（夹具第 5 条）。
    await ask(page, '那这样呢');
    await expect(
      page.locator('[data-kind="assistant-answer"]').last(),
    ).toContainText('好的。', { timeout: 30_000 });

    // 请求体：这一轮给模型的历史里，画布仍是第 2 版（页面切版本没有写回日志，也没有改请求）。
    const request = await fixture.lastRequest();
    expect(request, '要拿到第 3 轮的请求体').toBeTruthy();
    const body = JSON.stringify(request);
    expect(body).toContain('第 2 版已显示（cv_1）');
    expect(body).toContain('第二版：柱子换成折线');
    expect(body).toContain('¥1,260,000');

    // 日志：最后一条 tool/result.meta.canvas 是第 2 版，全文是改过的那份。
    const events = await sessionEvents(fixture);
    const canvases = events
      .filter((event) => event.type === 'tool/result' && event.data.meta?.canvas)
      .map((event) => event.data.meta!.canvas as Record<string, unknown>);
    expect(canvases).toHaveLength(2);
    expect(canvases[1].n).toBe(2);
    expect(canvases[1].note).toBe('第二版：柱子换成折线');
    expect(String(canvases[1].html)).toContain('<div id="v">2</div>');
    expect(String(canvases[1].html)).toContain('¥1,260,000');
  } finally {
    await fixture.close();
  }
});

