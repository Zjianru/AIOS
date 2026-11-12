import { test, expect, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { apiPost, startT016DodFixture } from './service-fixture';
import { sessionLog } from './dod-log';

/**
 * DoD2（真 key，MiniMax 国内站）：四件事——
 * ① 获取模型：存下的 live 列表文件在；第一次和再获取一次都一个不标新；
 * ② 让 Kind 写长内容，跑着时用 disabled-models-set 屏蔽当前模型：turn/end aborted + hook model-disabled，
 *    监控里打断后没有发给这个模型的请求；
 * ③ 再发一句：turn/end error MODEL_DISABLED，监控里这一轮 0 次模型请求；
 * ④ 解除屏蔽再发：正常回答，assistant/message 的 provider 是 minimax-cn（国内站；用户 key 只在
 *    api.minimaxi.com 有效，国际站 401）。
 *
 * key：只在运行时从环境变量读（/tmp/run-t016-dod.cjs 从 ~/.zshrc 取，不打印、不进 argv）。
 * 数据目录：/tmp/aios-t016-dod/dod2/（跑完不清，jq 证据写在 WORK/T016/evidence/）。
 */

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const DOD_ROOT = process.env.AIOS_T016_DOD_ROOT ?? '/tmp/aios-t016-dod';
const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? path.join(repoRoot, '../aios-work/T016/evidence');
const NL = String.fromCharCode(10);

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（T016.7 真 key DoD）');
test.setTimeout(900_000);

function renderCommand(args: string[]): string {
  return args
    .map((arg) => (arg.includes(' ') || arg.includes(NL) ? "'" + arg + "'" : arg))
    .join(' ');
}

function jq(args: string[], file: string, label: string, lines: string[]): string {
  const out = execFileSync('jq', [...args, file], { encoding: 'utf8' }).trim();
  lines.push('$ jq ' + renderCommand([...args, file]) + '  # ' + label + NL + out);
  return out;
}

function record(name: string, lines: string[]): void {
  mkdirSync(EVIDENCE, { recursive: true });
  const head = execFileSync('git', ['-C', repoRoot, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  writeFileSync(
    path.join(EVIDENCE, 'T016.7-dod2-jq-' + name + '.txt'),
    ['# frozen sha: ' + head, ...lines].join(NL + NL) + NL,
  );
}

/** 空库里走首次使用页（T008/T015 流程）：选 MiniMax（国内站）→ 填 key → 接上 → 开始。 */
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

/** 一轮结束：停止按钮消失。 */
async function waitTurnEnd(page: Page): Promise<void> {
  await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 300_000 });
}

function monitorLog(dataDir: string): string {
  const dir = path.join(dataDir, 'monitor');
  const file = readdirSync(dir)
    .filter((name) => name.endsWith('.jsonl'))
    .sort()
    .pop();
  if (!file) throw new Error('没有监控日志');
  return path.join(dir, file);
}

interface LiveEntry {
  fetchedAt: number;
  models: { id: string }[];
}

/// DoD2 四件事：获取、跑着时屏蔽、再发被拒、解除后正常回答。
test('DoD2：真 key 四件事（MiniMax）', async ({ page, request }) => {
  const fixture = await startT016DodFixture('dod2');
  const all: string[] = [];
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await connectMinimax(page);

    // 当前模型：首次使用保存连接时写的主模型（连接模型行的第一行）。
    const settingsResponse = await apiPost(request, fixture, { method: 'model-settings' });
    expect(settingsResponse.ok()).toBeTruthy();
    const settingsBody = await settingsResponse.json();
    const connection = settingsBody.settings.connections.find(
      (item: { id: string }) => item.id === 'minimax-cn',
    );
    expect(connection, '首次使用页建好 minimax-cn 连接').toBeTruthy();
    const currentModel: string = connection.model;

    // ① 获取两次：第一次落盘、没有上一次列表可比，第二次也没有新增；两次都不标新。
    const first = await apiPost(request, fixture, {
      method: 'available-models',
      id: 'minimax-cn',
      mode: 'fetch',
    });
    expect(first.ok()).toBeTruthy();
    const firstBody = await first.json();
    const fetch1Path = path.join(DOD_ROOT, 'dod2', 'fetch-1.json');
    writeFileSync(fetch1Path, JSON.stringify(firstBody, null, 2) + NL);
    expect(firstBody.live, '厂商 /v1/models 成功').toBeTruthy();
    const livePath = path.join(fixture.dataDir, 'catalog', 'models', 'minimax-cn.json');
    expect(existsSync(livePath), '存下的 live 列表文件存在：' + livePath).toBeTruthy();
    const live = JSON.parse(readFileSync(livePath, 'utf8')) as LiveEntry;
    expect(live.models.length).toBeGreaterThan(0);
    const freshFirst = (firstBody.models as { fresh: boolean }[]).filter((row) => row.fresh).length;
    expect(freshFirst, '第一次获取没有上一次的列表，一条都不标新').toBe(0);

    const fetchStep: string[] = [];
    jq(['-c', '{live, models:(.models | length), fresh:([.models[] | select(.fresh)] | length)}'], fetch1Path, '① 第一次获取：厂商列表成功、条数、标新的条数', fetchStep);
    jq(['-c', '{fetchedAt, models:(.models | length), ids:[.models[].id]}'], livePath, '① 存下的 live 列表文件', fetchStep);

    const second = await apiPost(request, fixture, {
      method: 'available-models',
      id: 'minimax-cn',
      mode: 'fetch',
    });
    expect(second.ok()).toBeTruthy();
    const secondBody = await second.json();
    const fetch2Path = path.join(DOD_ROOT, 'dod2', 'fetch-2.json');
    writeFileSync(fetch2Path, JSON.stringify(secondBody, null, 2) + NL);
    jq(['-c', '{live, models:(.models | length), fresh:([.models[] | select(.fresh)] | length)}'], fetch2Path, '① 第二次获取：没有新的，fresh 应为 0', fetchStep);
    expect(secondBody.live, '第二次也拿到厂商列表').toBeTruthy();
    const freshSecond = (secondBody.models as { fresh: boolean }[]).filter((row) => row.fresh).length;
    expect(freshSecond, '第二次获取一个都不标新').toBe(0);
    record('1-fetch', fetchStep);

    // 当前模型必须在厂商这次给的列表里；不在就选第一个 live 模型（页面的选择也走同一接口）。
    const liveIds = live.models.map((row) => row.id);
    let targetModel = currentModel;
    if (!liveIds.includes(targetModel)) {
      targetModel = liveIds[0];
      const selected = await apiPost(request, fixture, {
        method: 'session-model-select',
        connection: 'minimax-cn',
        model: targetModel,
        effortWanted: 'low',
      });
      expect(selected.ok()).toBeTruthy();
    }

    // ② 让 Kind 写一段长的东西；跑着的时候屏蔽当前模型。
    const turnTwo: string[] = [];
    await send(
      page,
      '写一篇很长的技术科普文章，主题是「浏览器事件循环」，至少 3000 字，分 12 节，每节至少 3 段。' +
        '一次写完，不要调用任何工具。',
    );
    await expect(page.locator('[data-kind="stop"]')).toBeVisible({ timeout: 60_000 });
    // 等正文真的流出来再屏蔽：只停在思考阶段的话「已流出的字留着」没有对象。
    // 空的时候 Answer 里是「正在想…」占位，必须等真正渲染正文的 assistant-answer。
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toBeVisible({ timeout: 180_000 });
    // 写出一段再屏蔽：被保留的前缀有实际内容，不是刚开头的一两个字。
    await expect
      .poll(
        async () => (await page.locator('[data-kind="assistant-answer"]').last().textContent())?.length ?? 0,
        { timeout: 120_000 },
      )
      .toBeGreaterThan(180);
    expect(await page.locator('[data-kind="stop"]').count(), '屏蔽时这一轮还在跑').toBe(1);
    const maskRule = 'minimax-cn/' + targetModel;
    const masked = await apiPost(request, fixture, {
      method: 'disabled-models-set',
      models: [maskRule],
    });
    expect(masked.ok()).toBeTruthy();
    const maskedBody = await masked.json();
    expect(maskedBody.models).toEqual([maskRule]);

    const failure = page.locator('[data-kind="failure"]');
    await expect(failure).toBeVisible({ timeout: 60_000 });
    await expect(failure).toContainText('模型在设置里被屏蔽了');
    await waitTurnEnd(page);
    await page.screenshot({
      path: path.join(EVIDENCE, 'T016.7-dod2-1-aborted.png'),
      fullPage: true,
    });

    const log = sessionLog(fixture.dataDir);
    const monitor = monitorLog(fixture.dataDir);
    jq(['-s', '-c', '[ .[] | select(.type=="turn/end") | {turn:.data.turn, reason:.data.reason} ]'], log, '② 全部 turn/end', turnTwo);
    jq(['-s', '-c', '[ .[] | select(.type=="turn/end" and .data.reason.kind=="aborted") | .data.reason.reason ]'], log, '② 被打断那一轮的 turn/end 原因（kind=hook，reason=model-disabled）', turnTwo);
    jq(['-s', '-c', '[ .[] | select(.type=="assistant/message" and .data.interrupted==true) | {turn:.data.turn, textChars:([.data.message.content[]? | select(.type=="text") | .text] | join("") | length)} ]'], log, '② 打断时保留的已流出文本', turnTwo);
    jq(['-s', '-c', '[ .[] | select(.type=="assistant/attempt") ] | length'], log, '② 会话里没有无消息的模型尝试', turnTwo);
    const abortedEndSeq = Number(
      execFileSync('jq', ['-s', '-r', '[ .[] | select(.type=="turn/end" and .data.reason.kind=="aborted") ][0].seq', log], {
        encoding: 'utf8',
      }).trim(),
    );
    expect(abortedEndSeq).toBeGreaterThan(0);
    record('2-aborted', turnTwo);

    // ③ 再发一句：发出去之前就被拒，这一轮没有模型请求。
    const turnThree: string[] = [];
    await send(page, '屏蔽还没解除，再发一句试试。');
    await expect(failure).toHaveCount(2, { timeout: 60_000 });
    await waitTurnEnd(page);
    jq(['-s', '-c', '[ .[] | select(.type=="turn/end") ][-1].data.reason.error'], log, '③ 再发一句的 turn/end error（code=MODEL_DISABLED）', turnThree);
    jq(['-s', '-c', '[ .[] | select(.data.turn==2 and (.type=="assistant/attempt" or .type=="assistant/message")) ] | length'], log, '③ 这一轮会话里的模型请求数（应为 0）', turnThree);
    const errorEndSeq = Number(
      execFileSync('jq', ['-s', '-r', '[ .[] | select(.type=="turn/end") ][-1].seq', log], { encoding: 'utf8' }).trim(),
    );
    jq(['-s', '-c', '--argjson', 'a', String(abortedEndSeq), '--argjson', 'b', String(errorEndSeq), '[ .[] | select(.channel=="ledger" and (.attributes["event.seq"] > $a) and (.attributes["event.seq"] <= $b) and ((.attributes["event.type"]=="assistant/attempt") or (.attributes["event.type"]=="assistant/message"))) ] | length'], monitor, '③ 监控（ledger）里打断之后到这次再发之间发给模型的请求数（应为 0）', turnThree);
    record('3-rejected', turnThree);

    // ④ 解除屏蔽再发：正常回答，assistant/message 的 provider 是 minimax-cn。
    const turnFour: string[] = [];
    const unmasked = await apiPost(request, fixture, { method: 'disabled-models-set', models: [] });
    expect(unmasked.ok()).toBeTruthy();
    await send(page, '1+1 等于几？只回答数字，不要调用工具。');
    const lastAnswer = page.locator('[data-kind="answer"]').last();
    await expect(lastAnswer).toContainText('2', { timeout: 300_000 });
    await waitTurnEnd(page);
    jq(['-s', '-c', '[ .[] | select(.type=="turn/end") ][-1].data.reason'], log, '④ 解除后这一轮的 turn/end（completed）', turnFour);
    jq(['-s', '-c', '[ .[] | select(.type=="assistant/message") ][-1].data.message.source'], log, '④ 最后一条 assistant/message 的来源（provider=minimax-cn，真实厂商不是 deepseek）', turnFour);
    jq(['-s', '-c', '[ .[] | select(.type=="assistant/message") | .data.message.source.provider ] | unique'], log, '④ 整段会话用过的 provider', turnFour);
    jq(['-s', '-c', '[ .[] | select(.channel=="ledger") | select(.attributes["event.type"]=="assistant/attempt" or .attributes["event.type"]=="assistant/message") | {seq:.attributes["event.seq"], type:.attributes["event.type"], turn:.body.data.turn} ]'], monitor, '④ 监控里的模型请求一览（只有被打断的那次和解除后的一次）', turnFour);
    record('4-unmasked', turnFour);

    all.push('# 数据目录：' + fixture.dataDir);
    all.push('# 会话日志：' + log);
    all.push('# 监控日志：' + monitor);
  } finally {
    try {
      all.push('# 数据目录：' + fixture.dataDir);
    } catch {
      all.push('# 数据目录：' + fixture.dataDir);
    }
    record('run', all);
    await fixture.close();
  }
});
