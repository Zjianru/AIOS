import { test, expect, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startT015DodFixture } from './service-fixture';
import { sessionLog } from './dod-log';

/**
 * T015 DoD2（真 key，MiniMax）：画布四件事——新建、用 CDN 图表库重画、用 edits 改一处、
 * 一句话的问题不画。一条用例走同一个会话（②③ 的第 2/3 版依赖上一版），每条事实都从
 * 会话日志用 jq 算，页面断言只认画布块和版本表。
 *
 * key：只在运行时从环境变量读（由 /tmp/run-t015-dod.cjs 从 ~/.zshrc 取，不打印、不进 argv）。
 * 数据目录：/tmp/aios-t015-dod/dod2/（跑完不清，jq 的证据写在 WORK/T015/evidence/）。
 */

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? path.join(repoRoot, '../aios-work/T015/evidence');
const NL = String.fromCharCode(10);

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（T015.6 真 key DoD）');
test.setTimeout(900_000);

function renderCommand(args: string[]): string {
  return args
    .map((arg) => (arg.includes(' ') || arg.includes(NL) ? "'" + arg + "'" : arg))
    .join(' ');
}

function jq(args: string[], log: string, label: string, lines: string[]): string {
  const out = execFileSync('jq', [...args, log], { encoding: 'utf8' }).trim();
  lines.push('$ jq ' + renderCommand([...args, log]) + '  # ' + label + NL + out);
  return out;
}

/** 日志是 turn/end 之后才落；等它稳定到期望的条数，别在日志还没写完时读。 */
async function pollJq(log: string, probe: string[], expected: number): Promise<void> {
  await expect
    .poll(() => Number(execFileSync('jq', [...probe, log], { encoding: 'utf8' }).trim()), {
      timeout: 30_000,
    })
    .toBe(expected);
}

function record(name: string, lines: string[]): void {
  mkdirSync(EVIDENCE, { recursive: true });
  const head = execFileSync('git', ['-C', repoRoot, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  writeFileSync(
    path.join(EVIDENCE, 'T015.6-dod2-jq-' + name + '.txt'),
    ['# frozen sha: ' + head, ...lines].join(NL + NL) + NL,
  );
}

/** 空库里走首次使用页（T008 流程）：选 MiniMax（国内站）→ 填 key → 接上 → 已保存到 Keyring → 开始。 */
async function connectMinimax(page: Page): Promise<void> {
  await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 60_000 });
  await page.getByRole('button', { name: 'MiniMax（国内站）' }).click();
  await page.locator('#first-use-key').fill(process.env.MINIMAX_API_KEY ?? '');
  await page.getByRole('button', { name: '接上', exact: true }).click();
  await expect(page.getByText('已保存到 Keyring', { exact: false })).toBeVisible({ timeout: 180_000 });
  await page.getByRole('button', { name: '开始', exact: true }).click();
  const composerInput = page.locator('[data-kind="composer-input"]');
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (await composerInput.isVisible().catch(() => false)) return;
    const retry = page.getByRole('button', { name: '现在重试' });
    if (await retry.count()) await retry.first().click().catch(() => undefined);
    await page.waitForTimeout(1500);
  }
  await expect(composerInput).toBeVisible({ timeout: 30_000 });
}

async function send(page: Page, text: string): Promise<void> {
  await page.locator('[data-kind="composer-input"]').fill(text);
  await page.locator('[data-kind="send"]').click();
}

/** 等这一轮结束（停止按钮消失）之后再看日志和页面。 */
async function waitTurnEnd(page: Page): Promise<void> {
  await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 240_000 });
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

const canvasCallsProbe = [
  '-c', '-s',
  '[ .[] | select(.type=="tool/call" and .data.name=="canvas") ] | length',
];

const canvasVersionCountProbe = [
  '-c', '-s',
  '[ .[] | select(.type=="tool/result") | .data.meta.canvas? // empty ] | length',
];

const canvasVersionsProbe = [
  '-c', '-s',
  '[ .[] | select(.type=="tool/result") | .data.meta.canvas? // empty | {id,n,title,note,htmlLength:(.html|length)} ]',
];

const canvasArgsProbe = [
  '-c', '-s',
  [
    '[ .[] | select(.type=="tool/call" and .data.name=="canvas")',
    '  | (.data.arguments | fromjson)',
    '  | {id:(.id // null), title:(.title // null), note, hasHtml:has("html"), edits:(.edits // null)} ]',
  ].join(NL),
];

const cdnProbe = [
  '-c', '-s',
  [
    '[ .[] | select(.type=="tool/result") | .data.meta.canvas? // empty | select(.n==2)',
    '  | {n, cdn:(',
    '      (.html | contains("cdn.jsdelivr.net"))',
    '      or (.html | contains("cdnjs.cloudflare.com"))',
    '      or (.html | contains("unpkg.com"))',
    '      or (.html | contains("cdn.tailwindcss.com"))',
    '      or (.html | contains("code.jquery.com"))',
    '    )} ]',
  ].join(NL),
];

/// DoD2 四件事：① 画布出来 ② CDN 图表库重画 ③ edits 改标题颜色 ④ 一句话不画。
test('DoD2：真 key 四件事（MiniMax）', async ({ page }) => {
  const fixture = await startT015DodFixture('dod2');
  const logOf = () => sessionLog(fixture.dataDir);
  const all: string[] = [];
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await connectMinimax(page);

    // ① 新建画布：日志里有 canvas 调用；页面显示画布。
    const first: string[] = [];
    await send(page, '每月存 3000、年化 3%、存十年，画出来。');
    await expect(page.locator('[data-kind="canvas"]')).toBeVisible({ timeout: 240_000 });
    await expect(page.locator('[data-kind="canvas-title"]')).not.toHaveText('', { timeout: 30_000 });
    const iframe1 = page.locator('iframe[data-kind="canvas-frame"]');
    await expect(iframe1).toHaveCount(1);
    expect(await iframe1.evaluate((element) => element.getBoundingClientRect().height)).toBeGreaterThan(0);
    await waitTurnEnd(page);
    await pollJq(logOf(), canvasVersionCountProbe, 1);
    const calls1 = Number(jq(canvasCallsProbe, logOf(), '① 新建：canvas 调用数', first));
    expect(calls1).toBeGreaterThanOrEqual(1);
    jq(canvasVersionsProbe, logOf(), '① 新建：各画布版本的 meta', first);
    await page.screenshot({ path: path.join(EVIDENCE, 'T015.6-dod2-1-new.png'), fullPage: true });
    record('1-new', first);

    // ② 用清单里的 CDN 图表库重画：第 2 版出现，页面有图表，html 引用清单地址。
    const second: string[] = [];
    await send(
      page,
      '用 CDN 上的图表库（Chart.js，从 cdn.jsdelivr.net 加载）把这张图重画成柱状图，显示每一年的余额。' +
        '用 canvas 工具改同一个画布，出第 2 版，note 写「改用 Chart.js 画柱状图」。',
    );
    await expect(page.locator('[data-kind="canvas-versions"]')).toContainText('第 2 版', { timeout: 240_000 });
    await expect(page.locator('[data-kind="canvas-stub"]')).toHaveCount(1);
    const frame2 = await canvasFrame(page);
    const chart = frame2.locator('canvas, svg');
    await expect.poll(async () => chart.count(), { timeout: 30_000 }).toBeGreaterThan(0);
    const chartBox = await chart.first().boundingBox();
    expect(chartBox && chartBox.width > 0 && chartBox.height > 0).toBeTruthy();
    await waitTurnEnd(page);
    await pollJq(logOf(), canvasVersionCountProbe, 2);
    const cdnRaw = jq(cdnProbe, logOf(), '② 第 2 版 html 是否引用清单里的 CDN', second);
    const cdnVec = JSON.parse(cdnRaw) as { n: number; cdn: boolean }[];
    expect(cdnVec.length).toBe(1);
    expect(cdnVec[0].cdn).toBeTruthy();
    jq(canvasVersionsProbe, logOf(), '② 第 2 版 meta', second);
    await page.screenshot({ path: path.join(EVIDENCE, 'T015.6-dod2-2-cdn.png'), fullPage: true });
    record('2-cdn', second);

    // ③ 改一处：第 3 版用 edits，页面显示第 3 版、前两版缩成一行。
    const third: string[] = [];
    await send(
      page,
      '把画布的标题颜色改成紫色（#7C3AED），别的都不要动。必须用 canvas 工具的 edits 局部改，不要整份重写；' +
        'note 写「标题改成紫色」。',
    );
    await expect(page.locator('[data-kind="canvas-versions"]')).toContainText('第 3 版', { timeout: 240_000 });
    await expect(page.locator('[data-kind="canvas-stub"]')).toHaveCount(2);
    await waitTurnEnd(page);
    await pollJq(logOf(), canvasVersionCountProbe, 3);
    const argsRaw = jq(canvasArgsProbe, logOf(), '③ 每次 canvas 调用的参数（第 3 版应只有 edits，没有 html）', third);
    const args = JSON.parse(argsRaw) as { note: string; hasHtml: boolean; edits: unknown }[];
    // 真 key 的模型可能先漏 title 被 bad-args 拒一次再重试；只认最后一次调用和成功出来的第 3 版。
    const editsCalls = args.filter((item) => !item.hasHtml && Array.isArray(item.edits) && item.edits.length > 0);
    expect(editsCalls.length).toBeGreaterThanOrEqual(1);
    const lastCall = args[args.length - 1];
    expect(lastCall.hasHtml).toBeFalsy();
    expect(Array.isArray(lastCall.edits) && (lastCall.edits as unknown[]).length > 0).toBeTruthy();
    const versionsRaw = jq(canvasVersionsProbe, logOf(), '③ 第 3 版 meta', third);
    const versions = JSON.parse(versionsRaw) as { n: number; htmlLength: number }[];
    expect(versions[versions.length - 1].n).toBe(3);
    expect(versions[versions.length - 1].htmlLength).toBeGreaterThan(0);
    await page.screenshot({ path: path.join(EVIDENCE, 'T015.6-dod2-3-edits.png'), fullPage: true });
    record('3-edits', third);

    // ④ 一句话的问题：回答出来，没有 canvas 调用，页面不新增画布。
    const fourth: string[] = [];
    const callsBefore = Number(jq(canvasCallsProbe, logOf(), '④ 一句话之前 canvas 调用数', fourth));
    await send(page, '1+1 等于几？只回答数字，不要画任何东西，也不要调用工具。');
    await expect(page.locator('[data-kind="answer"]').last()).toContainText('2', { timeout: 240_000 });
    await waitTurnEnd(page);
    const callsRaw = jq(canvasCallsProbe, logOf(), '④ 一句话之后 canvas 调用数（应与之前相同）', fourth);
    expect(Number(callsRaw)).toBe(callsBefore);
    await pollJq(logOf(), canvasVersionCountProbe, 3);
    await expect(page.locator('[data-kind="canvas"]')).toHaveCount(1);
    await page.screenshot({ path: path.join(EVIDENCE, 'T015.6-dod2-4-one-liner.png'), fullPage: true });
    record('4-one-liner', fourth);
  } finally {
    try {
      all.push('# 数据目录：' + fixture.dataDir);
      all.push('# 会话日志：' + sessionLog(fixture.dataDir));
    } catch {
      all.push('# 数据目录：' + fixture.dataDir);
    }
    record('run', all);
    await fixture.close();
  }
});
