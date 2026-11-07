import { test, expect, type Page } from '@playwright/test';
import { createServer } from 'node:http';
import { copyFile, readFile } from 'node:fs/promises';
import type { AddressInfo } from 'node:net';
import path from 'node:path';
import { startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T015.1（画布外壳）：画布显示、主题变量、420px、刷新回初始、/canvas-frame 的 CSP 和自检、
 * 地址清单和隔离、死循环不卡页面。
 * T015.2（版本）：第 2 版用 edits 出、旧版缩成一行、点了跳到最新、标题栏切版本、刷新回最新；
 * 页面切到旧版后发一句，发给模型的画布仍是最新版。
 * T015.4（来自画布）：点画布里的按钮只把话放进输入框（带可移除标记，不替用户发）；
 * 用户发出去以后，气泡、日志块、请求体三处都带「来自画布」。
 * T015.5（正在画、出错、全屏、存成文件）：流式时有「正在画 · 标题」、日志里没有 tool-call-delta、
 * ready 帧协议 7；三种错误都出「出错了」；全屏能开能关；存下的 .html 单独打开能显示、清单外被拦。
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

/**
 * 存文件用的 CSP：默认同响应头，但 frame-ancestors 在 meta 里会被忽略（去掉）；
 * file:// 文档里无 scheme 的主机源会按 file: 继承，所以 CDN/字体源要显式写 https://。
 */
const CANVAS_FILE_CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net/npm/ https://unpkg.com https://cdn.tailwindcss.com https://code.jquery.com",
  "style-src 'unsafe-inline' https://fonts.googleapis.com https://cdnjs.cloudflare.com https://cdn.jsdelivr.net/npm/ https://unpkg.com https://cdn.tailwindcss.com https://code.jquery.com",
  'font-src data: https://fonts.gstatic.com',
  'img-src data: blob:',
  'media-src data: blob:',
  'connect-src https://fonts.googleapis.com https://fonts.gstatic.com',
  "frame-src 'none'",
  "object-src 'none'",
  "base-uri 'none'",
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

/* ───────── T015.4：画布里的话进输入框，带「来自画布」 ───────── */

/**
 * 日志里你自己发的每条 user/message 的块（按出现顺序）。
 * 同一个事件类型还有 pre-step 注入的 AGENTS.md/skill 消息（source 不是 user），不算。
 */
async function userMessageBlocks(fixture: KernelFixture) {
  const events = await sessionEvents(fixture);
  return events
    .filter(
      (event) =>
        event.type === 'user/message' &&
        (event.data.message?.source as { kind?: string } | undefined)?.kind === 'user',
    )
    .map(
      (event) =>
        (event.data.message?.content ?? []) as {
          type?: string;
          text?: string;
          canvas?: { id?: string; title?: string };
        }[],
    );
}

/** 日志里那一版画布的 id（tool/result.meta.canvas.id）。 */
async function canvasIdOf(fixture: KernelFixture): Promise<string> {
  const events = await sessionEvents(fixture);
  const result = events.find((event) => event.type === 'tool/result' && event.data.meta?.canvas);
  if (!result) throw new Error('日志里还没有画布');
  return String((result.data.meta as { canvas: { id: string } }).canvas.id);
}

/**
 * T015.4 清单 7（故事 5、16）：点画布里的按钮，话进真输入框、带可移除的「来自画布」；
 * 点完不替用户发；接着写再发以后，气泡、日志块（canvas 恰好 {id,title}）、请求体三处都带标记。
 */
test('画布按钮：话进输入框带标记、点完不自动发；接着写再发，气泡/日志/请求体都带标记', async ({
  page,
}) => {
  const fixture = await startKernelFixture({ canvas: 'send' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(
      '画好了：拖滑块看结果。',
      { timeout: 40_000 },
    );

    const frame = await canvasFrame(page);
    await frame.locator('#save').click();

    // 话真进了输入框，带一个能移除的「来自画布：标题」。
    const input = page.getByPlaceholder('和 Kind 说…');
    await expect(input).toHaveValue('按这个存：每月 3000');
    const chip = page.locator('[data-kind="composer"] [data-kind="from-canvas"]');
    await expect(chip).toContainText('来自画布');
    await expect(chip).toContainText('每月存钱算一算');

    // 点完以后、用户发送之前：话只在输入框里，没有开始跑，也没有新的 user/message（故事 16）。
    // 不睡固定时长：发送会在同一个消息处理器里同步起一轮，此刻「停止」在不在、日志条数
    // 都能直接看；真被替发的话，下面发送后的日志会多出第三条。
    await expect(page.getByRole('button', { name: '停止' })).toHaveCount(0);
    expect((await userMessageBlocks(fixture)).length, '点完按钮就替用户发了').toBe(1);

    // 接着写一句再发（同一句话后面接，不是替换）。
    await input.fill('按这个存：每月 3000，再存五年');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(
      '收到，我按画布里的数接着算。',
      { timeout: 40_000 },
    );

    // 气泡上留着标记。
    await expect(
      page.locator('[data-kind="user-message-row"]').last().locator('[data-kind="from-canvas"]'),
    ).toContainText('每月存钱算一算');

    // 日志：标记块排在最前，块上 canvas 恰好是 {id, title}，正文块是打的字。
    const canvasId = await canvasIdOf(fixture);
    const blocks = await userMessageBlocks(fixture);
    expect(blocks.length).toBe(2);
    expect(blocks[1]).toEqual([
      {
        type: 'text',
        text: '（来自画布「每月存钱算一算」）',
        canvas: { id: canvasId, title: '每月存钱算一算' },
      },
      { type: 'text', text: '按这个存：每月 3000，再存五年' },
    ]);

    // 请求体：模型看到的是「（来自画布「标题」）」。
    const request = await fixture.lastRequest();
    expect(request, '要拿到第 2 轮的请求体').toBeTruthy();
    expect(JSON.stringify(request)).toContain('（来自画布「每月存钱算一算」）');
    expect(JSON.stringify(request)).toContain('按这个存：每月 3000，再存五年');
  } finally {
    await fixture.close();
  }
});

/**
 * T015.4 清单 7（故事 5）：草稿里的标记刷新后还在；移除标记再发，日志里没有 canvas 块。
 */
test('草稿里的「来自画布」刷新后还在；移除标记再发，日志里没有 canvas 块', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'send' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(
      '画好了：拖滑块看结果。',
      { timeout: 40_000 },
    );

    const frame = await canvasFrame(page);
    await frame.locator('#save').click();
    await expect(page.locator('[data-kind="composer"] [data-kind="from-canvas"]')).toBeVisible();

    // 草稿（文字 + 标记）写进了 localStorage。
    const draftCanvas = () =>
      page.evaluate(() => {
        const key = Object.keys(localStorage).find((name) => name.startsWith('aios.kind.draft.'));
        if (!key) return null;
        try {
          const value = JSON.parse(localStorage.getItem(key) ?? '{}') as { canvas?: unknown };
          return value.canvas ?? null;
        } catch {
          return null;
        }
      });
    await expect.poll(draftCanvas).toEqual({ id: expect.any(String), title: '每月存钱算一算' });

    // 刷新以后文字和标记都在（草稿从 localStorage 回来）。
    await page.reload();
    const input = page.getByPlaceholder('和 Kind 说…');
    await expect(input).toHaveValue('按这个存：每月 3000', { timeout: 15_000 });
    const chip = page.locator('[data-kind="composer"] [data-kind="from-canvas"]');
    await expect(chip).toContainText('每月存钱算一算');

    // 移除标记再发：日志里没有 canvas 块（当成你自己写的）。
    await page.getByRole('button', { name: '移除来自画布的标记' }).click();
    await expect(chip).toHaveCount(0);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(
      '收到，我按画布里的数接着算。',
      { timeout: 40_000 },
    );
    const blocks = await userMessageBlocks(fixture);
    expect(blocks.length).toBe(2);
    expect(blocks[1]).toEqual([{ type: 'text', text: '按这个存：每月 3000' }]);
  } finally {
    await fixture.close();
  }
});

/* ───────── T015.5：正在画、出错、全屏、存成文件 ───────── */

/** 另开一条 WS 取第一帧 ready（页面自己那条已经在用）。 */
async function readReady(page: Page): Promise<Record<string, unknown>> {
  return page.evaluate(
    () =>
      new Promise<Record<string, unknown>>((resolve, reject) => {
        const socket = new WebSocket('ws://' + location.host + '/ws');
        const timer = setTimeout(() => {
          socket.close();
          reject(new Error('等 ready 帧超时'));
        }, 5000);
        socket.onmessage = (event) => {
          const frame = JSON.parse(String(event.data)) as Record<string, unknown>;
          if (frame.type !== 'ready') return;
          clearTimeout(timer);
          socket.close();
          resolve(frame);
        };
        socket.onerror = () => {
          clearTimeout(timer);
          reject(new Error('WS 打不开'));
        };
      }),
  );
}

/** 验收：ready 帧协议版本 7（T015.5），旧的 root/context 字段照旧。 */
test('ready 帧：协议版本 7，旧的 root/context 字段不变', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  try {
    await page.goto(urlOf(fixture));
    const ready = await readReady(page);
    expect(ready.protocolVersion).toBe(7);
    expect(ready.root).toEqual({ state: 'ok' });
    expect(ready.context).toEqual(expect.objectContaining({ window: expect.any(Number) }));
  } finally {
    await fixture.close();
  }
});

/**
 * 清单 9（故事 8）：流式过程中工作过程里有「正在画 · 标题」，结束后那一行消失、画布出现；
 * 会话日志里没有 tool-call-delta。
 */
test('正在画：流式过程中有「正在画 · 标题」，结束后变成画布；日志没有 tool-call-delta', async ({
  page,
}) => {
  const fixture = await startKernelFixture({ canvas: 'streaming' });
  try {
    await page.goto(urlOf(fixture));
    const drawing = page.locator('[data-kind="canvas-drawing"]');
    await ask(page);
    await expect(drawing).toContainText('正在画 · 每月存钱算一算', { timeout: 30_000 });
    await waitCanvas(page, '每月存钱算一算');
    await expect(drawing).toHaveCount(0);
    await expect(page.locator('iframe[data-kind="canvas-frame"]')).toHaveCount(1);

    // 不落日志：日志里没有 tool-call-delta 这种事件/帧（只有 T006.14 的 tool-call-chunks）。
    const events = await sessionEvents(fixture);
    expect(events.some((event) => event.type === 'tool-call-delta')).toBe(false);
    expect(JSON.stringify(events)).not.toContain('tool-call-delta');
  } finally {
    await fixture.close();
  }
});

/**
 * 清单 12（故事 12）：脚本报错、未处理 rejection、securitypolicyviolation 三种都让标题栏出
 * 「出错了」，点了把错误放进输入框（带「来自画布」标记，由用户发）。
 */
test('出错：脚本报错、未处理的 rejection、CSP 违规都出「出错了」，点了错误进输入框', async ({
  page,
}) => {
  const fixture = await startKernelFixture({ canvas: 'errors' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '出错测试画布');
    const frame = await canvasFrame(page);
    const input = page.getByPlaceholder('和 Kind 说…');
    const errorButton = page.getByRole('button', { name: '出错了' });

    const tellKind = async (trigger: string, expected: string) => {
      await frame.locator('#' + trigger).click();
      await expect(errorButton).toBeVisible({ timeout: 15_000 });
      // 清掉上次放进来的文字，再点「出错了」；断言这次是新的那种错误。
      await input.fill('');
      await errorButton.click();
      await expect(input).toHaveValue(new RegExp(expected));
      await expect(page.locator('[data-kind="composer"] [data-kind="from-canvas"]')).toContainText(
        '出错测试画布',
      );
    };

    await tellKind('script-error', '脚本报错');
    await tellKind('rejection', '未处理的 rejection');
    await tellKind('csp', 'CSP 违规');
  } finally {
    await fixture.close();
  }
});

/**
 * 审计 T015.5 #1（故事 12）：画布顶层脚本一打开就 throw，错误监听必须已经装好。
 * 走真外壳（iframe src=/canvas-frame，响应头带 CSP），标题栏出「出错了」；点了进输入框。
 */
test('初始化脚本报错：一打开就出「出错了」，点了错误进输入框', async ({ page, request }) => {
  const fixture = await startKernelFixture({ canvas: 'init-error' });
  try {
    // 真外壳：/canvas-frame 的响应头带 CSP（不是静态 fallback 的裸 HTML）。
    const shell = await request.get(fixture.baseUrl + '/canvas-frame');
    expect(shell.headers()['content-security-policy']).toBe(CANVAS_FRAME_CSP);

    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '初始化就报错');
    const errorButton = page.getByRole('button', { name: '出错了' });
    await expect(errorButton).toBeVisible({ timeout: 15_000 });

    const frame = await canvasFrame(page);
    expect(new URL(frame.url()).pathname).toBe('/canvas-frame');
    // throw 只打断那一段脚本，后面的 HTML 照常解析。
    await expect(frame.locator('#still')).toHaveText('还在');

    await errorButton.click();
    const input = page.getByPlaceholder('和 Kind 说…');
    await expect(input).toHaveValue(/脚本报错/);
    expect(await input.inputValue()).toContain('init-boom');
    await expect(page.locator('[data-kind="composer"] [data-kind="from-canvas"]')).toContainText(
      '初始化就报错',
    );
  } finally {
    await fixture.close();
  }
});

/** 清单 10（故事 9）：全屏能开能关（按钮和 Esc 都行），画布还是同一个 iframe。 */
test('全屏：能开能关（按钮和 Esc）', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'interactive' });
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '每月存钱算一算');
    const block = page.locator('[data-kind="canvas"]');

    await page.getByRole('button', { name: '全屏' }).click();
    await expect(block).toHaveAttribute('data-fullscreen', 'true');
    // 真的铺满窗口，不只是加了个属性（虚拟列表的祖先可能有 transform）。
    const viewport = page.viewportSize();
    const box = await block.boundingBox();
    if (!viewport || !box) throw new Error('拿不到全屏画布的位置');
    expect(Math.round(box.x)).toBe(0);
    expect(Math.round(box.y)).toBe(0);
    expect(Math.round(box.width)).toBe(viewport.width);
    expect(Math.round(box.height)).toBe(viewport.height);
    await expect(page.locator('iframe[data-kind="canvas-frame"]')).toHaveCount(1);
    await expect((await canvasFrame(page)).locator('#total')).toHaveText('¥378,000');

    await page.getByRole('button', { name: '退出全屏' }).click();
    await expect(block).toHaveAttribute('data-fullscreen', 'false');

    await page.getByRole('button', { name: '全屏' }).click();
    await expect(block).toHaveAttribute('data-fullscreen', 'true');
    await page.keyboard.press('Escape');
    await expect(block).toHaveAttribute('data-fullscreen', 'false');
  } finally {
    await fixture.close();
  }
});

/**
 * 清单 10（故事 10）：存成 .html 由页面生成，文件里有外壳、画布 HTML、CSP meta；
 * 用新页面单独打开能显示，清单外的请求被拦（CSP meta 生效，不是「请求被拦」冒充 CSP 违规）。
 */
test('存成文件：外壳 + 画布 HTML + CSP meta；单独打开能显示，清单外请求被拦', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'cdn' });
  try {
    await page.route('https://cdn.jsdelivr.net/npm/t015-fake-lib/index.js', (route) =>
      route.fulfill({ status: 200, contentType: 'text/javascript', body: 'window.__t015Lib = true;' }),
    );
    let offList = 0;
    await page.route('https://evil.example/**', async (route) => {
      offList += 1;
      await route.fulfill({ status: 200, body: 'x' });
    });

    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, 'CDN 与隔离');
    await expect.poll(() => canvasVariable(page, '--color-background-primary')).not.toBe('');
    const liveBackground = await canvasVariable(page, '--color-background-primary');

    const downloadPromise = page.waitForEvent('download');
    await page.getByRole('button', { name: '存成文件' }).click();
    const download = await downloadPromise;
    expect(download.suggestedFilename()).toBe('CDN 与隔离.html');
    // T015 fix B3：页面里留一个可点的「在浏览器打开」，文件名完整显示（blob URL 到组件卸载）。
    const openLink = page.locator('[data-kind="canvas-open-file"]');
    await expect(openLink).toBeVisible();
    await expect(openLink).toHaveAttribute('target', '_blank');
    expect(await openLink.getAttribute('href')).toMatch(/^blob:/);
    await expect(openLink).toContainText('在浏览器打开');
    const filePath = await download.path();
    if (!filePath) throw new Error('下载的文件没有落盘路径');
    // Playwright 把下载暂存成没有扩展名的文件；file:// 会按 text/plain 打开，复制一份 .html 再看。
    const htmlPath = filePath + '.html';
    await copyFile(filePath, htmlPath);
    const html = await readFile(htmlPath, 'utf8');

    expect(html.startsWith('<!doctype html>')).toBe(true);
    expect(html).toContain(
      '<meta http-equiv="Content-Security-Policy" content="' + CANVAS_FILE_CSP + '">',
    );
    expect(html).toContain('id="marker"');
    // 外壳的基础样式（不是只有画布 HTML 那一段）。
    expect(html).toContain('font-family:var(--font-sans)');
    // 标题和主题变量由父窗口写进去（审计整洁 #5）。
    expect(html).toContain('<title>CDN 与隔离</title>');
    expect(html).toContain('--color-background-primary:' + liveBackground);
    // 存下来的是能单独打开的页面，不是 iframe 自检外壳，也不带宿主运行时。
    expect(html).not.toContain('window.self === window.top');
    expect(html).not.toContain('data-aios-runtime');

    // 单独打开：清单里的库能加载，画布显示；主题变量在文件里真的生效；清单外的请求一个都没发出去。
    await page.goto('file://' + htmlPath);
    await expect(page.locator('#marker')).toHaveText('lib:1', { timeout: 20_000 });
    await expect(page.locator('#fetch')).toHaveText('fetch:blocked');
    expect(
      await page.evaluate(() =>
        getComputedStyle(document.documentElement)
          .getPropertyValue('--color-background-primary')
          .trim(),
      ),
    ).toBe(liveBackground);
    expect(offList, '清单外的请求发了出去').toBe(0);
  } finally {
    await fixture.close();
  }
});

/**
 * 审计 T015.5 #2（故事 10、16）：文件由父窗口生成，只认自己这份。
 * 不可信画布直接发 export（带自己的 HTML）时宿主不下载；点「存成文件」拿到的
 * 仍是日志里那一版 + 宿主自己的外壳/CSP，没有伪造内容。
 */
test('伪造 export：画布自己发 export 不产生下载；存下的文件仍由宿主生成', async ({ page }) => {
  const fixture = await startKernelFixture({ canvas: 'forged-export' });
  const downloads: string[] = [];
  page.on('download', (download) => downloads.push(download.suggestedFilename()));
  try {
    await page.goto(urlOf(fixture));
    await ask(page);
    await waitCanvas(page, '伪造导出的画布');

    const frame = await canvasFrame(page);
    await expect(frame.locator('#marker')).toHaveText('真的画布');
    // 让画布再伪造一次 export，然后发一条普通消息当回执：宿主收到了消息，却不应下载。
    await frame.locator('#forge').click();
    const input = page.getByPlaceholder('和 Kind 说…');
    await expect(input).toHaveValue('伪造完成');
    expect(downloads, '伪造的 export 触发了下载').toEqual([]);

    const downloadPromise = page.waitForEvent('download');
    await page.getByRole('button', { name: '存成文件' }).click();
    const download = await downloadPromise;
    expect(download.suggestedFilename()).toBe('伪造导出的画布.html');
    const filePath = await download.path();
    if (!filePath) throw new Error('下载的文件没有落盘路径');
    const html = await readFile(filePath, 'utf8');
    expect(html).toContain(
      '<meta http-equiv="Content-Security-Policy" content="' + CANVAS_FILE_CSP + '">',
    );
    expect(html).toContain('id="marker"');
    expect(html).not.toContain('id="pwned"');
    expect(downloads, '真正的存文件应恰好下载一次').toEqual(['伪造导出的画布.html']);
  } finally {
    await fixture.close();
  }
});
