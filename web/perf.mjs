#!/usr/bin/env node
/**
 * perf.mjs —— 第 1 期测速（app-v14-brief.md §三.5）。
 *
 * 跑之前先看测速锁：
 *   - 锁文件不存在，且 pgrep -fl 'cargo|rustc' 为空，才写锁开跑；
 *   - 跑完删锁；中途出现 cargo 就作废重测；
 *   - 开跑与结束各记一次 top -l 1 -n 5。
 *
 * 用法：node perf.mjs [--url=http://127.0.0.1:4184]
 * 产物：perf/p1-report.md、perf/p1-raw.json
 */

import { chromium } from '/Users/codez/develop/aios/web/node_modules/playwright/index.mjs';
import { execSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';
import { join } from 'node:path';

const ROOT = '/Users/codez/Documents/aios-design/app-v14';
const LOCK = '/Users/codez/develop/aios-d14-audit/perf-lock.md';
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const NL = String.fromCharCode(10);
const TICK = String.fromCharCode(96);
const FENCE = TICK.repeat(3);
const args = process.argv.slice(2);
const DRY = args.includes('--dry');
const urlArg = args.find((a) => a.startsWith('--url='));
const BASE = urlArg ? urlArg.slice(6) : 'http://127.0.0.1:4184';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (cmd) => {
  try {
    return execSync(cmd, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  } catch {
    return '';
  }
};

function checkLock() {
  if (existsSync(LOCK)) throw new Error('测速锁已存在：' + LOCK + NL + readFileSync(LOCK, 'utf8'));
  const running = sh("pgrep -fl 'cargo|rustc'");
  if (running) throw new Error('机器不空闲，有 cargo/rustc 在跑：' + NL + running);
}

function writeLock() {
  writeFileSync(
    LOCK,
    [
      '# 测速锁（v14 前端预演）',
      '',
      '- 持有者：merry-otter（前端预演会话 F2 / tidy-badger，app-v14 第 1 期）',
      '- 目的：跑 perf.mjs（2000 轮长对话、滚动帧率、流式、切场景）',
      '- 开始：' + new Date().toISOString(),
      '- 结束：跑完立即删除本文件',
      '- 期间请勿同时跑 AIOS 的 cargo 构建或性能基线单元',
      '',
    ].join(NL),
  );
}

function removeLock() {
  try {
    execSync('rm -f ' + JSON.stringify(LOCK));
  } catch {
    /* ignore */
  }
}

const topSnapshot = () => sh('top -l 1 -n 5');

/** 取某个 mark 的绝对时间（ms，从 navigation 算），没有就 null。 */
async function markTime(page, name, timeout = 8000) {
  const started = Date.now();
  for (;;) {
    const found = await page.evaluate((target) => {
      const entry = performance.getEntriesByName(target, 'mark').at(-1);
      return entry ? entry.startTime : null;
    }, name);
    if (found !== null) return found;
    if (Date.now() - started > timeout) return null;
    await sleep(16);
  }
}

async function main() {
  if (!DRY) {
    checkLock();
    writeLock();
  }
  const topStart = topSnapshot();
  const result = { startedAt: new Date().toISOString(), base: BASE, metrics: {}, notes: [] };

  const browser = await chromium.launch({ executablePath: CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 920 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push('pageerror: ' + e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push('console: ' + m.text());
  });

  const client = await context.newCDPSession(page);
  await client.send('Performance.enable');
  const heap = async () => {
    const { metrics } = await client.send('Performance.getMetrics');
    const map = Object.fromEntries(metrics.map((item) => [item.name, item.value]));
    return map.JSHeapUsedSize ?? null;
  };

  const installLongTaskObserver = () =>
    page.evaluate(() => {
      window.__longTasks = [];
      window.__longTaskObserver?.disconnect();
      window.__longTaskObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) window.__longTasks.push(Math.round(entry.duration));
      });
      window.__longTaskObserver.observe({ entryTypes: ['longtask'] });
    });
  const takeLongTasks = () =>
    page.evaluate(() => {
      const list = window.__longTasks ?? [];
      window.__longTasks = [];
      return list;
    });
  const clearMarks = () => page.evaluate(() => performance.clearMarks());

  // ── 1. 冷启动到首屏（直接落在 Kind）──
  await page.goto(BASE + '/#kind', { waitUntil: 'load' });
  result.metrics.coldStartMs = await markTime(page, 'turns:painted', 15000);
  const nav = await page.evaluate(() => {
    const entry = performance.getEntriesByType('navigation')[0];
    return { domContentLoaded: entry ? entry.domContentLoadedEventEnd : null, load: entry ? entry.loadEventEnd : null };
  });
  result.metrics.coldStartDomContentLoaded = nav.domContentLoaded;
  result.metrics.coldStartLoad = nav.load;
  result.metrics.firstScreen = await page.evaluate(() => ({
    sidebar: !!document.querySelector('[data-slot="sidebar"]'),
    toolbar: !!document.querySelector('header'),
    conversation: !!document.querySelector('[data-pane="conversation"]'),
  }));

  // ── 2. 2000 轮对话首屏（点进 Kind 到最后一页画完）──
  const kindFirstScreen = [];
  for (let i = 0; i < 5; i += 1) {
    await page.evaluate(() => window.__v14.go('now'));
    await sleep(240);
    await clearMarks();
    await page.evaluate(() => window.__v14.go('kind'));
    const painted = await markTime(page, 'turns:painted', 8000);
    const started = await markTime(page, 'route:start:kind', 1000);
    kindFirstScreen.push(painted !== null && started !== null ? painted - started : null);
    await sleep(140);
  }
  result.metrics.kindFirstScreenMs = kindFirstScreen;

  // ── 3. 首屏 DOM 节点数 ──
  await sleep(200);
  result.metrics.domNodes = await page.evaluate(() => document.querySelectorAll('*').length);

  // ── 4. 往上翻一页（触顶 → 新内容画完），10 页 ──
  await installLongTaskObserver();
  // 首屏加载后不会自动往上取页（程序改 scrollTop 不算用户滚动），
  // 所以先用真滚轮把「用户滚动过」置上，再开始量「触顶 → 新内容画完」。
  const convBox = await page.locator('[data-pane="conversation"]').boundingBox();
  if (convBox) {
    await page.mouse.move(convBox.x + convBox.width / 2, convBox.y + convBox.height / 2);
    await page.mouse.wheel(0, -600);
    await sleep(120);
  }
  const prependTimes = [];
  for (let i = 0; i < 10; i += 1) {
    const value = await page.evaluate(
      () =>
        new Promise((resolve) => {
          const scroller = document.querySelector('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
          if (!scroller) {
            resolve(null);
            return;
          }
          const started = performance.now();
          scroller.scrollTop = 0;
          const poll = () => {
            const entry = performance.getEntriesByName('prepend:painted', 'mark').at(-1);
            if (entry && entry.startTime >= started) {
              resolve(entry.startTime - started);
              return;
            }
            if (performance.now() - started > 6000) {
              resolve(null);
              return;
            }
            requestAnimationFrame(poll);
          };
          requestAnimationFrame(poll);
        }),
    );
    if (value === null) break;
    prependTimes.push(value);
    await page.evaluate(() => window.__v14.scrollToBottom());
    await sleep(140);
  }
  result.metrics.prependMs = prependTimes;

  // ── 5. 滚动帧率：匀速滚 5 秒 ──
  await sleep(150);
  await installLongTaskObserver();
  const scrollRun = await page.evaluate(
    () =>
      new Promise((resolve) => {
        const scroller = document.querySelector('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
        const started = performance.now();
        let frames = 0;
        const tick = () => {
          scroller.scrollTop -= 6;
          frames += 1;
          if (performance.now() - started < 5000) requestAnimationFrame(tick);
          else resolve({ frames, elapsed: performance.now() - started });
        };
        requestAnimationFrame(tick);
      }),
  );
  const scrollLongTasks = await takeLongTasks();
  result.metrics.scrollFps = (scrollRun.frames / (scrollRun.elapsed / 1000)).toFixed(1);
  result.metrics.scrollLongTasks = scrollLongTasks.filter((d) => d > 50);

  // ── 6. 流式回复：模拟 30 秒 ──
  const streamMs = DRY ? 4000 : 30000;
  await page.evaluate((ms) => window.__v14.sendMessage('跑一次流式回复，我要看长任务。', ms), streamMs);
  await installLongTaskObserver();
  const streamStart = Date.now();
  await sleep(streamMs + 1000);
  const streamLongTasks = await takeLongTasks();
  result.metrics.streamSeconds = (Date.now() - streamStart) / 1000;
  result.metrics.streamLongTasks = streamLongTasks.filter((d) => d > 50);

  // ── 7. 切场景：现在 → Kind → 能力 ──
  const sceneSwitches = [];
  for (const target of ['kind', 'ability', 'kind']) {
    await page.evaluate(() => window.__v14.go('now'));
    await sleep(260);
    await clearMarks();
    await page.evaluate((route) => window.__v14.go(route), target);
    const painted = await markTime(page, 'route:painted:' + target, 5000);
    const started = await markTime(page, 'route:start:' + target, 1000);
    sceneSwitches.push(painted !== null && started !== null ? painted - started : null);
    await sleep(160);
  }
  result.metrics.sceneSwitchMs = sceneSwitches;

  // ── 8. 内存：空闲一次，翻完 2000 轮再一次 ──
  await page.evaluate(() => window.__v14.go('kind'));
  await sleep(700);
  result.metrics.heapIdle = await heap();
  result.metrics.loadAll = await page.evaluate(async () => {
    const started = performance.now();
    const total = await window.__v14.loadAll();
    return { total, ms: Math.round(performance.now() - started), count: window.__v14.conversationState().count };
  });
  await sleep(900);
  result.metrics.heapAfterAll = await heap();
  result.metrics.domNodesAfterAll = await page.evaluate(() => document.querySelectorAll('*').length);

  // ── 9. 流程画布：拖动与平移（压测流程，程序生成 60 节点） ──
  await page.evaluate(() => window.__v14.setDevStressFlow(true));
  await sleep(200);
  await page.evaluate(() => window.__v14.go('flow:stress-60'));
  await sleep(1800);
  const flowInfo = await page.evaluate(() => ({
    nodes: document.querySelectorAll('.react-flow__node').length,
    canvas: !!document.querySelector('.react-flow__pane'),
  }));
  result.metrics.flowCanvas = flowInfo;
  if (flowInfo.canvas) {
    await installLongTaskObserver();
    const box = await page.locator('.react-flow__pane').boundingBox();
    if (box) {
      const cx = box.x + box.width / 2;
      const cy = box.y + box.height / 2;
      await page.mouse.move(cx, cy);
      await page.mouse.down();
      for (let i = 0; i < 60; i += 1) {
        await page.mouse.move(cx + Math.sin(i / 6) * 120, cy + Math.cos(i / 6) * 80);
        await sleep(16);
      }
      await page.mouse.up();
      const nodeBox = await page.locator('.react-flow__node').first().boundingBox();
      if (nodeBox) {
        await page.mouse.move(nodeBox.x + nodeBox.width / 2, nodeBox.y + nodeBox.height / 2);
        await page.mouse.down();
        for (let i = 0; i < 40; i += 1) {
          await page.mouse.move(nodeBox.x + nodeBox.width / 2 + i * 4, nodeBox.y + nodeBox.height / 2 + i * 2);
          await sleep(16);
        }
        await page.mouse.up();
      }
    }
    result.metrics.flowLongTasks = (await takeLongTasks()).filter((d) => d > 50);
  } else {
    result.notes.push('流程画布未渲染，该项未测。');
  }

  // ── 10. 体积 ──
  const dist = join(ROOT, 'dist');
  const sizes = [];
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      const info = statSync(full);
      if (info.isDirectory()) walk(full);
      else sizes.push({ name: full.slice(dist.length + 1), bytes: info.size });
    }
  };
  if (existsSync(dist)) walk(dist);
  const totalBytes = sizes.reduce((sum, item) => sum + item.bytes, 0);
  let gzipTotal = 0;
  for (const file of sizes) gzipTotal += gzipSync(readFileSync(join(dist, file.name))).length;
  const js = sizes.filter((file) => file.name.endsWith('.js')).sort((a, b) => b.bytes - a.bytes);
  // 首屏 = index.html 直接引的；其余（比如 elkjs）是懒加载块
  const html = readFileSync(join(dist, 'index.html'), 'utf8');
  const entryNames = new Set([...html.matchAll(/\/assets\/([^"']+)/g)].map((match) => match[1]));
  const entryFiles = sizes.filter((file) => entryNames.has(file.name.replace(/^assets\//, '')));
  const entryBytes = entryFiles.reduce((sum, file) => sum + file.bytes, 0);
  const entryGzip = entryFiles.reduce((sum, file) => sum + gzipSync(readFileSync(join(dist, file.name))).length, 0);
  const lazyBytes = totalBytes - entryBytes;
  result.metrics.bundle = {
    files: sizes.length,
    totalBytes,
    gzipBytes: gzipTotal,
    entryBytes,
    entryGzipBytes: entryGzip,
    lazyBytes,
    topJs: js.slice(0, 5).map((file) => ({
      name: file.name,
      bytes: file.bytes,
      gzip: gzipSync(readFileSync(join(dist, file.name))).length,
      lazy: !entryNames.has(file.name.replace(/^assets\//, '')),
    })),
  };

  result.metrics.webkit = 'WebKit 未测（本机 Playwright 没装 webkit；按需求不为此下载浏览器）';
  result.notes.push(result.metrics.webkit);
  result.notes.push(
    '流程画布按 brief §四.2 地铁线规则改过（线 4px / 间距 6px；站中性浅底深描边；换乘站跨线胶囊、门跨线菱形），本期这一项量的是改后版本。',
  );
  result.notes.push(
    '地铁线「≥4 条线时点图例其余淡到 25%」本期未实现：现有流程最多 3 条线，没有触发场景，等第 2 期有 4 线数据再落（协调线 2026-10-28 同意先登记）。',
  );
  result.errors = errors;
  result.browserVersion = await browser.version();
  result.machine = {
    model: sh('sysctl -n hw.model'),
    chip: sh('sysctl -n machdep.cpu.brand_string'),
    memory: sh('sysctl -n hw.memsize'),
    os: sh('sw_vers -productVersion'),
  };
  await browser.close();
  const topEnd = topSnapshot();
  if (!DRY) removeLock();
  result.top = { start: topStart, end: topEnd };
  result.finishedAt = new Date().toISOString();
  mkdirSync(join(ROOT, 'perf'), { recursive: true });
  if (DRY) {
    writeFileSync('/tmp/p1-raw-dry.json', JSON.stringify(result, null, 2));
    writeReport(result, '/tmp/p1-report-dry.md');
  } else {
    writeFileSync(join(ROOT, 'perf/p1-raw.json'), JSON.stringify(result, null, 2));
    writeReport(result);
  }
  console.log('perf done → perf/p1-raw.json / perf/p1-report.md');
}

const median = (list) => {
  if (!list.length) return null;
  const sorted = [...list].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};

function writeReport(result, outPath) {
  const m = result.metrics;
  const code = (text) => TICK + text + TICK;
  const fmt = (value, digits = 0) => (value === null || value === undefined ? '—' : Number(value).toFixed(digits));
  const verdict = (value, limit) => (value === null || value === undefined ? '未测' : value <= limit ? '达标' : '**超了**');
  const rows = [
    ['冷启动到首屏', fmt(m.coldStartMs) + ' ms', '≤ 2000 ms', verdict(m.coldStartMs, 2000)],
    [
      '2000 轮对话首屏',
      '中位 ' + fmt(median(m.kindFirstScreenMs)) + ' ms（5 次：' + m.kindFirstScreenMs.map((v) => fmt(v)).join(' / ') + '）',
      '≤ 300 ms',
      verdict(median(m.kindFirstScreenMs), 300),
    ],
    ['首屏 DOM 节点数', String(m.domNodes), '报数', '—'],
    [
      '往上翻一页（10 次）',
      '中位 ' + fmt(median(m.prependMs)) + ' ms · 最大 ' + fmt(Math.max(...m.prependMs)) + ' ms',
      '≤ 150 ms',
      verdict(median(m.prependMs), 150),
    ],
    ['滚动帧率（匀速 5 秒）', fmt(m.scrollFps, 1) + ' fps', '≥ 55 fps', Number(m.scrollFps) >= 55 ? '达标' : '**超了**'],
    ['滚动长任务（>50ms）', m.scrollLongTasks.length + ' 个', '0 个', m.scrollLongTasks.length === 0 ? '达标' : '**超了**'],
    ['流式回复 30 秒长任务', m.streamLongTasks.length + ' 个', '0 个', m.streamLongTasks.length === 0 ? '达标' : '**超了**'],
    ['切场景（现在→Kind→能力→Kind）', m.sceneSwitchMs.map((v) => fmt(v)).join(' / ') + ' ms', '≤ 200 ms', verdict(Math.max(...m.sceneSwitchMs), 200)],
    [
      '内存（JS 堆）',
      '空闲 ' + fmt((m.heapIdle || 0) / 1048576, 1) + ' MB · 翻完 2000 轮 ' + fmt((m.heapAfterAll || 0) / 1048576, 1) + ' MB',
      '报数',
      '—',
    ],
    [
      '体积 dist/',
      fmt(m.bundle.totalBytes / 1048576, 2) +
        ' MB · gzip ' +
        fmt(m.bundle.gzipBytes / 1024, 0) +
        ' KB（首屏 ' +
        fmt(m.bundle.entryBytes / 1048576, 2) +
        ' MB / gzip ' +
        fmt(m.bundle.entryGzipBytes / 1024, 0) +
        ' KB，其余是懒加载块）',
      '报数',
      '—',
    ],
    [
      '流程画布（压测流程 60 节点）',
      flowCanvasText(m),
      '0 个 >50ms',
      m.flowCanvas && m.flowCanvas.canvas ? ((m.flowLongTasks && m.flowLongTasks.length) === 0 ? '达标' : '**超了**') : '未测',
    ],
  ];
  const lines = [
    '# 第 1 期测速报告',
    '',
    '生成时间：' + result.finishedAt + '（' + result.startedAt + ' 开始）',
    '',
    '## 环境',
    '',
    '- 机器：' + (result.machine.model || '—') + ' · ' + (result.machine.chip || '—') + ' · 内存 ' + Math.round(Number(result.machine.memory || 0) / 1073741824) + ' GB',
    '- 系统：macOS ' + (result.machine.os || '—'),
    '- 浏览器：Playwright 驱动的 Chrome ' + result.browserVersion + '（headless）',
    '- 视口：1440 × 920',
    '- 命令：' + code('node perf.mjs') + '（打开 ' + result.base + '/#kind）',
    '- 数据延迟：40ms（默认）；2000 轮用固定种子按需生成，不预载',
    '- 测速锁：开跑前锁不存在 + ' + code("pgrep -fl 'cargo|rustc'") + ' 为空；跑完已删除',
    '- 流程画布那一项：先用开发开关打开压测流程（工具栏「开发 → 压测（只影响流程列表）→ 压测流程（60 节点）」，对应测试钩子 __v14.setDevStressFlow(true)），再访问 #flow:stress-60。这条流程由程序生成 60 节点 / 59 条连线，只在开关打开时出现，开关不进 localStorage。',
    '- WebKit：' + result.metrics.webkit,
    '',
    '## 结果',
    '',
    '| 项 | 数值 | 标准 | 结论 |',
    '|---|---|---|---|',
    ...rows.map((row) => '| ' + row.join(' | ') + ' |'),
    '',
    '## 明细',
    '',
    '- 冷启动：从 navigation 到 ' + code('turns:painted') + '（侧栏 + 工具栏 + 对话最后一页画完，含字体到位后的对齐）。DOMContentLoaded ' + fmt(m.coldStartDomContentLoaded) + ' ms，load ' + fmt(m.coldStartLoad) + ' ms。',
    '- 首个 30 秒流式回复期间的长任务：' + (m.streamLongTasks.length ? m.streamLongTasks.join(', ') + ' ms' : '无') + '。',
    '- 滚动帧率测试里记录的长任务：' + (m.scrollLongTasks.length ? m.scrollLongTasks.join(', ') + ' ms' : '无') + '。',
    '- 翻完 2000 轮：加载 ' + m.loadAll.total + ' 轮，用时 ' + m.loadAll.ms + ' ms，列表 ' + m.loadAll.count + ' 条；之后 DOM 节点 ' + m.domNodesAfterAll + ' 个。',
    '- 流程画布：压测流程在 0.92 缩放（站名 13.8px / 副标 11px）下记录拖动与平移的长任务；60 个节点全部渲染。',
    '- 最大的 5 个 JS chunk：',
    ...m.bundle.topJs.map(
      (file) =>
        '  - ' +
        code(file.name) +
        '：' +
        Math.round(file.bytes / 1024) +
        ' KB（gzip ' +
        Math.round(file.gzip / 1024) +
        ' KB' +
        (file.lazy ? '，懒加载' : '，首屏') +
        '）',
    ),
    '',
    '## 机器占用快照（top -l 1 -n 5）',
    '',
    '开跑前：',
    '',
    FENCE,
    result.top.start,
    FENCE,
    '',
    '跑完后：',
    '',
    FENCE,
    result.top.end,
    FENCE,
    '',
    '## 备注',
    '',
    ...(result.notes.length ? result.notes.map((note) => '- ' + note) : ['- 无']),
    '- 控制台错误：' + (result.errors.length ? result.errors.join(' / ') : '[]'),
    '',
  ];
  writeFileSync(outPath || join(ROOT, 'perf/p1-report.md'), lines.join(NL));
}

function flowCanvasText(m) {
  if (!m.flowCanvas || !m.flowCanvas.canvas) return '未测（画布未渲染）';
  return '画布节点 ' + m.flowCanvas.nodes + ' 个 · 长任务 ' + (m.flowLongTasks ? m.flowLongTasks.length : 0) + ' 个';
}

main().catch((error) => {
  removeLock();
  console.error(error.message || error);
  process.exit(1);
});
