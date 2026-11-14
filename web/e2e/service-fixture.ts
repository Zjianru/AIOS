import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { createServer, request as httpRequest, type Server } from 'node:http';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const FIXTURE_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures');

interface ReplaySelection {
  file: string;
  override?: string;
  paceMs?: number;
}

/** T010.2：场景 -> 录好的会话日志 + 覆盖文件（手写 SSE 已删除）。 */
function replaySelection(options: FixtureOptions): ReplaySelection {
  if (options.failure === 'credential') return { file: 'text', override: 'failure-credential' };
  if (options.failure === 'model') return { file: 'text', override: 'failure-model' };
  if (options.failure === 'retry-then-succeed') return { file: 'text', override: 'failure-retry' };
  if (options.failure === 'unknown') return { file: 'text', override: 'failure-unknown' };
  if (options.realTurn) return { file: 'real', paceMs: 20 };
  if (options.usageTurn) return { file: 'usage', paceMs: 400 };
  if (options.tinyPercent) return { file: 'usage-tiny', paceMs: 400 };
  if (options.usageReadOnly) return { file: 'usage-read-only', paceMs: 400 };
  if (options.questionDegrade) return { file: 'question-degrade' };
  if (options.askTurn) return { file: 'ask', paceMs: options.askTurnPaceMs };
  if (options.toolTurn) return { file: 'tool' };
  // T023：工具前说明 → read hello.txt → 最终答案；pace 150 让实时阶段能被观察到。
  if (options.pretextTurn) return { file: 't023-pretext', paceMs: 150 };
  // T023：同一轮放慢到 400ms/帧，设置页往返后还有一段实时流可观察。
  if (options.pretextSlowTurn) return { file: 't023-pretext-slow', paceMs: 400 };
  if (options.codeBlock) return { file: 'tool-code' };
  if (options.toolCards) return { file: 'tool-cards' };
  // 只保留 Kind 现有工具（read/glob/grep）：T030.2a 收窄后 Kind 没有 write/edit/bash。
  if (options.t013Interrupted) return { file: 'tool-interrupted-legal', paceMs: 800 };
  if (options.canvas) {
    // T015.5：streaming 把 canvas 的参数拆成几段慢慢流（页面画「正在画 · 标题」）；
    // errors 是三个错误触发按钮的画布。
    if (options.canvas === 'streaming') return { file: 'canvas-streaming', paceMs: 120 };
    return { file: options.canvas === 'interactive' ? 'canvas' : 'canvas-' + options.canvas };
  }
  if (options.localFilePath) return { file: 'local-file-text' };
  if (options.highUsage) return { file: 'high-usage' };
  if (options.t014Query) return { file: 't014-query' };
  if (options.t014Confirm) return { file: 't014-confirm' };
  if (options.t014Mask) return { file: 't014-mask' };
  if (options.t014Web) return { file: 't014-web' };
  if (options.t014Search) return { file: 't014-search' };
  if (options.t014Fetch) return { file: 't014-fetch' };
  return {
    file: 'text',
    paceMs: options.slowMs ? Math.max(1, Math.round(options.slowMs / 6)) : undefined,
  };
}

/** T029.3：重启接续夹具的初始路由文件（不含重启那一轮；测试拿到任务 id 后再生成完整路由）。 */
function restartRouteFile(options: FixtureOptions): string | null {
  if (!options.t029Restart) return null;
  return 't029-routes-' + options.t029Restart + '.json';
}

/** 起 aios-replay（打印一行 aios-replay: http://…）；每夹具一个进程。
 *
 * T029.3：显式 routesFile 和 port 用于服务重启前换回放脚本（同一个端口，连接不用改）。
 */
function startReplay(
  options: FixtureOptions,
  replayFile?: string,
  routesFile?: string | null,
): { child: ReturnType<typeof spawn>; url: Promise<string> } {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_REPLAY_BIN ?? path.join(repoRoot, 'target/debug/aios-replay');
  // 双会话（Kind + ThinAgent）按请求体各回各的脚本；路由文件里帧间隔逐条给。
  const routeFile =
    routesFile === undefined
      ? (restartRouteFile(options) ??
        t030RouteFile(options) ??
        t013RouteFile(options) ??
        t052RouteFile(options) ??
        t053RouteFile(options) ??
        taskResultRouteFile(options))
      : routesFile;
  const args: string[] = [];
  if (routeFile) {
    args.push('--routes', path.isAbsolute(routeFile) ? routeFile : path.join(FIXTURE_DIR, routeFile));
  } else {
    const selection = replaySelection(options);
    args.push('--file', replayFile ?? path.join(FIXTURE_DIR, selection.file + '.jsonl'));
    if (selection.override) args.push('--override', path.join(FIXTURE_DIR, selection.override + '.json'));
    if (selection.paceMs) args.push('--pace-ms', String(selection.paceMs));
  }
  // 假厂商也回一份 /v1/models：保存连接会来拉一次（默认按 DeepSeek 的真实模型配）。
  const vendorModels = options.vendorModels ?? ['deepseek-flash', 'deepseek-v4-pro'];
  if (vendorModels.length > 0) args.push('--vendor-models', vendorModels.join(','));
  args.push('--host', '127.0.0.1', '--port', '0');
  const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const url = new Promise<string>((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => reject(new Error('aios-replay 启动超时: ' + output)), 20_000);
    child.stdout?.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = /aios-replay: (http:\/\/\S+)/.exec(output);
      if (match) {
        clearTimeout(timer);
        resolve(match[1]);
      }
    });
    child.stderr?.on('data', (chunk: Buffer) => {
      output += chunk.toString();
    });
  });
  return { child, url };
}

/** 用 /connect 换到的 cookie 调 /api（e2e 不再带 query token）。 */
export async function apiPost(
  request: any,
  fixture: KernelFixture,
  data: unknown,
) {
  const connect = await request.get(
    fixture.baseUrl + '/connect?token=' + encodeURIComponent(fixture.token),
    { maxRedirects: 0 },
  );
  const setCookie = connect.headers()['set-cookie'];
  if (!setCookie) throw new Error('connect 没有返回 set-cookie');
  const cookie = setCookie.split(';', 1)[0];
  return request.post(fixture.baseUrl + '/api', {
    data,
    headers: { Cookie: cookie },
  });
}

export interface KernelFixture {
  baseUrl: string;
  token: string;
  dataDir: string;
  /** T008.4：假 DeepSeek 的 endpoint（页面连接会被 e2e 改写到它）。 */
  fakeEndpoint: string;
  /** T008.4：假 DeepSeek 最近一次请求体里的 model（下一轮生效断言用）。 */
  lastModel: () => Promise<string | null>;
  /** T012.9：假 DeepSeek 最近一次请求的完整请求体（断言思考档用）。 */
  lastRequest: () => Promise<Record<string, unknown> | null>;
  /** 路径显示用例：会话规范化后的工作目录，和它之外的一个临时目录。 */
  paths?: { cwd: string; outside: string };
  /** 真 key fixture：serve 进程的 stderr。 */
  stderr?: () => string;
  /** 设置页「联网搜索」用例：本地假 Exa 的根地址和收到的授权头。 */
  exa?: { baseUrl: string; requests: () => string[] };
  /** 读网页用例：本地假网页代理收到的目标 URL（本机地址不该出现在这里）。 */
  fetch?: { requests: () => string[] };
  /** T022：钉住汇率请求的本地代理，用来等 status 真的卡在 fx 的 await 里。 */
  fx?: FxGate;
  dropProxy?: () => Promise<void>;
  restoreProxy?: () => Promise<void>;
  /// T007 #1：页面开着时重启同一个端口的服务（新进程=新 cookie 钥匙，旧页面会话失效）。
  restart?: () => Promise<void>;
  /// T029.3：再起一个假模型回放（新端口、新路由文件），返回新地址；老进程留着继续服务在跑的任务。
  /// 测试把连接的 endpoint 换到新地址后再重启 serve——重启汇报的卡就能带真实任务 id。
  switchReplay?: (routesFile: string) => Promise<string>;
  close(): Promise<void>;
}

/** T022：兑换汇率（fx）请求的闸门。只拦 open.er-api.com；别的 HTTPS 立刻 502。 */
export interface FxGate {
  /** 第一次汇率请求到达（首屏 status 拉汇率）。 */
  firstConnect: Promise<void>;
  /** hold(true) 之后的下一次汇率请求已被扣住：服务正卡在 status 的 fx await 里。 */
  held: Promise<void>;
  hold(on: boolean): void;
  /** 放行被扣住的那次请求（回 502，服务走后备汇率），之后不再扣。 */
  release(): void;
}

export interface FixtureOptions {
  /// first request 回一次工具调用（read hello.txt），拿到工具结果后再回最终文本。
  toolTurn?: boolean;
  /// T023：第一步先流一段「工具前说明」再调 read hello.txt，第二步回最终答案。
  /// 回放 pace 150ms，让「工具之后、最终答案之前」的实时阶段能被 e2e 观察到。
  pretextTurn?: boolean;
  /// T023：同上，但 pace 400ms、最终答案 8 段，设置页往返后还有实时流可观察。
  pretextSlowTurn?: boolean;
  /// 路径显示：三步回放（cwd 内写、cwd 外写、最终文本）；夹具里的 __CWD__ / __OUTSIDE__ 起服务前换成真实目录。
  pathsTurn?: boolean;
  /// T013.6：最终回答里带一个代码块（复制按钮用）。
  codeBlock?: boolean;
  /// T013.5 修复：经写任务 ThinAgent 产生 write/edit/bash + task_report 一轮（工作过程页的
  /// 卡片用）。T030.2a 收窄后 Kind 没有写/改/跑命令，过程只能由 ThinAgent 合法产生。
  t013ProcessTask?: boolean;
  /// T017.3：一轮里 glob + 一个不认识的工具（搜索卡与通用输入/输出卡用）。
  toolCards?: boolean;
  /// T013.5：工具做完后被停下的一轮（工作过程页的「已停止」）。
  t013Interrupted?: boolean;
  /// T015.1：画布夹具。interactive 是拖滑块/换标签、内容超过 420px 的那张；cdn 验地址清单和隔离；
  /// loop 验死循环不卡页面。T015.2 加 versions：同一画布两版（第 2 版用 edits 改），再一条第三轮回答。
  /// T015.4 加 send：画布里一个「按这个存」按钮（aios.send），第 2 轮的回答另备一条。
  /// T015.5 加 streaming：参数拆成多段慢慢流（「正在画 · 标题」）；errors：脚本报错/
  /// 未处理 rejection/CSP 违规三个按钮。
  /// T015.5 审计修复加 init-error（画布顶层一打开就 throw）和 forged-export（画布自己发 export）。
  /// 每份都是手写的模型回放脚本（第一轮回 canvas 工具调用，第二轮回一句回答）。
  canvas?:
    | 'interactive'
    | 'cdn'
    | 'loop'
    | 'versions'
    | 'send'
    | 'streaming'
    | 'errors'
    | 'init-error'
    | 'forged-export';
  /// 每次模型回放延迟毫秒（T007.5 停止用例要在运行中按停止/Esc）。
  slowMs?: number;
  /// first request 回一次 ask_user_question（两题：一题选项、一题自己写）。
  askTurn?: boolean;
  /// T017.2：提问卡的降级回放（25 字小标题、空 label 选项、2233 字的问题）。
  questionDegrade?: boolean;
  /// T007.13：让 question/asked 分批到达（毫秒/帧）。
  askTurnPaceMs?: number;
  /// 模型失败脚本：前两次 500 后成功 / 一直 500 / 401 凭证错误。
  failure?: 'retry-then-succeed' | 'model' | 'credential' | 'unknown';
  /// 高上下文占用回放（配小 --context-window），用来触发自动压缩 / 让压缩按钮可用。
  highUsage?: boolean;
  /// serve 的 --context-window（自动压缩测试用 5000）。
  contextWindow?: number;
  /// 在页面和服务之间加一层可断开/恢复的 HTTP+WS 代理（断线重连用例）。
  proxy?: boolean;
  /// 启动前把根会话状态文件做成 lost / corrupt（说明页用例）。
  rootFault?: 'lost' | 'corrupt';
  /// 先起一个 holder serve 占住写锁，测试 serve 起来会报 busy（说明页用例）。
  busyRoot?: boolean;
  /// T008.4：不预置 Keyring 连接（首次使用用例）；serve 仍能起（T008.3b）。
  emptyKeyring?: boolean;
  /// T035.2：假厂商 /v1/models 回的模型列表（保存连接会拉一次）。默认 DeepSeek 的两条。
  vendorModels?: string[];
  /// T016.5：启动前把 settings.json 写成坏文件；serve 仍要能起（读设置是宽松的）。
  settingsFault?: 'corrupt';
  /// T009.5：serve 进程的 HOME；传 'dataDir' 用数据目录（页面路径缩写用例）。
  home?: 'dataDir' | string;
  /// T010.3：用带 reasoning/缓存读写的用量夹具（turn-usage 用例）。
  usageTurn?: boolean;
  /// T013.6：缓存命中不到 1%（usage 夹具的 cacheRead=1/input=1000）。
  tinyPercent?: boolean;
  /// T012 F1：用量夹具去掉缓存写（只填缓存读价的端到端用）。
  usageReadOnly?: boolean;
  /// T015 验收补 B2：回答里裸文本绝对路径 + `open <path>` 代码块（页面渲染成 /local-file 链接）。
  localFilePath?: boolean;
  /// T010.5：用真 key 录的夹具（多步工具/思考/缓存读/压缩）。
  realTurn?: boolean;
  /// T014.2：先加载 skill aios-settings，再查 AIOS 的设置和用量，最后一句回答（回放）。
  t014Query?: boolean;
  /// T014.4：两轮各出一张确认卡（三个选项一张、一个选项一张），回放只给模型流。
  t014Confirm?: boolean;
  /// T014.5：一张只有一个选项的屏蔽卡，回放只给模型流。
  t014Mask?: boolean;
  /// T014.6：先加载 skill web，再收尾一句回答（联网搜索的目录/工具表用）。
  t014Web?: boolean;
  /// T014.7：先加载 skill web，再 web_search 一次，最后回答引用来源 URL（联网搜索回放用）。
  t014Search?: boolean;
  /// T014.8：先加载 skill web，再读一个网页、读一个本机地址，最后收尾（读网页回放用）。
  t014Fetch?: boolean;
  /// T014.6：起一个本地假 Exa；serve 用 --exa-base-url 指过去。
  fakeExa?: boolean;
  /// T014.6：把 EXA_API_KEY 放进 serve 的环境变量，用来证明 key 不从环境变量读。
  envExaKey?: string;
  /// T014.2：起服务前先放一条旧格式（v16）的根会话日志，用来验旧日志照读。
  legacySession?: boolean;
  /// T022：把服务的汇率请求指到本地代理，测试可以等 status 进了 fx 的 await 再动作。
  slowFx?: boolean;
  /// T028.5：起服务前先写好根会话日志（翻页夹具）。cc-pages：同一条抄送的
  /// agent/inbox/spliced 落在上一页末尾、领取后的 user/message 落在下一页开头。
  kindHistory?: 'cc-pages';
  /// T024.4：任务结果卡。前几种跑真 Kind + ThinAgent 两个会话（按请求体路由）；
  /// missing / bad-json 用预写好的根会话日志直接验卡与退回文本。
  /// live：同一个任务脚本放慢帧间隔，供「在跑时打开任务对话」的实时用例观察。
  taskResult?:
    | 'ready'
    | 'blocked'
    | 'failed'
    | 'waiting'
    | 'missing'
    | 'bad-json'
    | 'chat'
    | 'live'
    /// T028.5：任务对话输入框/抄送/暂停/Attempt 的夹具。
    | 'say'
    | 'resume'
    | 'handback'
    /// T028.5：两个任务、席位 1，验证叫醒停任务可以多占 1 个。
    | 'seats'
    /// 两个任务在同一轮报回（两张结果卡、刷新同序）。
    | 'two'
    /// ThinAgent 先说话再调工具（任务对话的 say 步）。
    | 'pretext';
  /// T029.3：重启接续夹具。main 四个任务（两个被打断、一个 ready、一个待答），
  /// single 一个被打断的任务；重启那一轮的路由由测试拿到真实任务 id 后再生成。
  t029Restart?: 'main' | 'single';
  /// T037：serve 的启动参数（模拟 ds 预检单的 run.sh：`--connection`/`--model` 只当初始默认）。
  startupSelection?: { connection: string; model: string; effort?: string };
  /// T046：用 catalog 里不存在的模型 id 起会话，让 route_tool_update 返回 None（测工具表全量固定）。
  routeModel?: string;
  /// T030：写任务、接受、bash 开关、工作树、非仓库和改错字的真服务夹具。
  /// worktree 场景会在起服务前把数据目录做成一个 git 仓库；expand 的第二段路由由测试
  /// 拿到真实任务 id 后再生成。
  t030?: 'write' | 'ask' | 'expand' | 'bash' | 'worktree' | 'nonrepo' | 'typo' | 'scope';
  /// T052：任务图夹具——Kind 先 skill 再 task_graph_propose，再 propose_action 出确认卡；
  /// 卡上的 task_id 用 __GRAPH_ID__ 占位，由回放从请求体里换成真图 id。
  t052Graph?: boolean;
  /// T052：同一条任务图流程，但第一项 ThinAgent 报 failed（验证下游不自己跑）。
  t052Fail?: boolean;
  /// T053：任务图带只读里程碑门（先查 → 独立核查 → 通过后再写）。
  t053Gate?: boolean;
}

/** T024.4：双会话回放用哪份按请求体路由的文件；预写会话的两种不需要。 */
function taskResultRouteFile(options: FixtureOptions): string | null {
  const scenario = options.taskResult;
  if (!scenario || scenario === 'missing' || scenario === 'bad-json') return null;
  // T028.5：三份新场景各有自己的路由文件；其余沿用 T024 的。
  if (
    scenario === 'say' ||
    scenario === 'resume' ||
    scenario === 'handback' ||
    scenario === 'seats'
  ) {
    return 't028-routes-' + scenario + '.json';
  }
  // 同轮两张卡、任务对话的 say 步。
  if (scenario === 'two' || scenario === 'pretext') {
    return 't031-routes-' + scenario + '.json';
  }
  return 't024-routes-' + scenario + '.json';
}

/** T052：任务图场景的回放路由（Kind 建图/出卡，ThinAgent 报 ready）。 */
function t052RouteFile(options: FixtureOptions): string | null {
  if (options.t052Fail) return 't052-routes-graph-fail.json';
  if (!options.t052Graph) return null;
  return 't052-routes-graph.json';
}

/** T053：里程碑门场景的回放路由（Kind 提图/出卡，ThinAgent 按 B→门→C 报 ready）。 */
function t053RouteFile(options: FixtureOptions): string | null {
  if (!options.t053Gate) return null;
  return 't053-routes-graph.json';
}

/** T030：写任务/接受/bash/工作树这些场景的回放路由。 */
function t030RouteFile(options: FixtureOptions): string | null {
  if (!options.t030) return null;
  return 't030-routes-' + options.t030 + '.json';
}

/** T013.5 修复：写任务 ThinAgent 的合法生产回放（write/edit/bash + task_report）。 */
function t013RouteFile(options: FixtureOptions): string | null {
  if (!options.t013ProcessTask) return null;
  return 't013-routes-process.json';
}

/** 接缝 5：假 DeepSeek 脚本回放 + 真 aios-dev serve（页面也由它同源托管）。 */
export async function startKernelFixture(
  options: FixtureOptions = {},
): Promise<KernelFixture> {
  const dataDir = await mkdtemp(path.join(tmpdir(), 'aios-t007-'));
  // 路径夹具里的绝对路径要在起回放之前换成真实目录：工作目录内的那一份用规范化路径，
  // 和会话 header 的 cwd（serve 会 canonicalize）一致；之外的一份单独建一个临时目录。
  let paths: KernelFixture['paths'];
  let replayFile: string | undefined;
  if (options.pathsTurn) {
    const outside = await mkdtemp(path.join(tmpdir(), 'aios-t014-outside-'));
    const cwd = await realpath(dataDir);
    const template = await readFile(path.join(FIXTURE_DIR, 'tool-paths.jsonl'), 'utf8');
    replayFile = path.join(dataDir, 'replay-tool-paths.jsonl');
    await writeFile(
      replayFile,
      template.split('__CWD__').join(cwd).split('__OUTSIDE__').join(outside),
    );
    paths = { cwd, outside };
  }
  // 缺项和坏 JSON 的根会话日志直接写好（不发模型请求），serve 打开就能看卡/文本。
  if (options.taskResult === 'missing' || options.taskResult === 'bad-json') {
    const name = options.taskResult === 'missing' ? 't024-missing.jsonl' : 't024-bad-json.jsonl';
    const template = await readFile(path.join(FIXTURE_DIR, name), 'utf8');
    const cwd = await realpath(dataDir);
    const rows = template.split('__DATA_DIR__').join(cwd);
    const header = JSON.parse(rows.split(String.fromCharCode(10), 1)[0]) as { id: string };
    const sessions = path.join(dataDir, 'sessions');
    await mkdir(sessions, { recursive: true });
    await writeFile(path.join(sessions, header.id + '.jsonl'), rows);
    await writeFile(
      path.join(dataDir, 'kind-root.json'),
      JSON.stringify({ version: 1, sessionId: header.id }),
    );
  }
  const replay = startReplay(options, replayFile);
  const replayUrl = await replay.url;
  // T029.3：换路由时再起一个回放进程；老进程继续服务在跑的任务，收尾时一起杀。
  const extraReplayChildren: ReturnType<typeof spawn>[] = [];
  if (options.rootFault === 'corrupt') {
    await writeFile(path.join(dataDir, 'kind-root.json'), '{"version":1, ');
  } else if (options.rootFault === 'lost') {
    await writeFile(
      path.join(dataDir, 'kind-root.json'),
      JSON.stringify({ version: 1, sessionId: randomUUID() }),
    );
  }
  if (
    options.toolTurn ||
    options.t013ProcessTask ||
    options.pretextTurn ||
    options.pretextSlowTurn ||
    options.taskResult === 'chat' ||
    options.taskResult === 'live' ||
    options.taskResult === 'say' ||
    options.taskResult === 'resume' ||
    options.taskResult === 'handback' ||
    options.taskResult === 'seats' ||
    options.taskResult === 'two' ||
    options.taskResult === 'pretext' ||
    options.t029Restart !== undefined ||
    options.t030 !== undefined
  ) {
    await writeFile(path.join(dataDir, 'hello.txt'), 'hello from fixture\n');
    // 页头写入范围用例：任务目录里的另一个文件。
    if (options.t030 === 'scope') {
      await writeFile(path.join(dataDir, 'notes.md'), '# 项目笔记\n\n第一行：原样\n');
    }
    // T013.7：带空格的路径（@ 菜单插入要给引号用）。
    await writeFile(path.join(dataDir, 'with space.txt'), 'spaced file\n');
  }
  if (options.t030 === 'worktree') {
    // T030：工作树用例要一个真的 git 仓库；数据目录同时是任务 cwd（serve --cwd）。
    const gitEnv = {
      ...process.env,
      HOME: dataDir,
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_AUTHOR_NAME: 'AIOS e2e',
      GIT_AUTHOR_EMAIL: 'aios-e2e@local',
      GIT_COMMITTER_NAME: 'AIOS e2e',
      GIT_COMMITTER_EMAIL: 'aios-e2e@local',
    };
    for (const args of [['init', '-q'], ['add', 'hello.txt'], ['commit', '-qm', 'fixture']]) {
      const done = spawnSync('git', args, { cwd: dataDir, env: gitEnv, encoding: 'utf8' });
      if (done.status !== 0) {
        throw new Error('git ' + args.join(' ') + ' 失败：' + String(done.stderr));
      }
    }
  }
  if (options.toolCards) {
    // T017.3：让真实 glob 搜到两个文件（工具由内核执行，回放只重放模型流）。
    await writeFile(path.join(dataDir, 'a.ts'), 'export const a = 1;\n');
    await writeFile(path.join(dataDir, 'b.ts'), 'export const b = 2;\n');
  }
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');
  const identityPath = path.join(dataDir, 'identity.txt');
  if (!options.emptyKeyring) {
    const seed = spawnSync(
      bin,
      ['keyring', '--data-dir', dataDir, '--identity', identityPath, 'set', '--id', 'deepseek', '--name', 'DeepSeek', '--preset', 'deepseek', '--model', options.routeModel ?? 'deepseek-flash', '--endpoint', replayUrl],
      { input: 'test-key' + String.fromCharCode(10), encoding: 'utf8' },
    );
    if (seed.status !== 0) throw new Error('seed keyring failed: ' + String(seed.stderr));
  }
  // T013.5 修复：写任务要有 bash 才能产生「运行命令」步骤；任务创建时冻结这份设置。
  if (options.t013ProcessTask) {
    await writeFile(
      path.join(dataDir, 'settings.json'),
      JSON.stringify({ exec: { bash: true } }),
    );
  }
  // T016.5：坏文件要在 serve 起来之前就在；不预置 Keyring 时也照写（坏文件与连接无关）。
  if (options.settingsFault === 'corrupt') {
    await writeFile(path.join(dataDir, 'settings.json'), '{"lastSelection": ');
  }
  if (options.kindHistory === 'cc-pages') {
    // T028.5：抄送跨两页——spliced 在第 1 轮后面（上一页末尾），领取后的 user/message
    // 在第 2 轮开头（下一页第一项）；两处 request id 相同，页面只能画一行。
    const sessionId = '00000000-0000-4000-8000-000000000285';
    const sessions = path.join(dataDir, 'sessions');
    await mkdir(sessions, { recursive: true });
    const startedAt = Date.now();
    const cwd = await realpath(dataDir);
    const ccMessage = {
      id: 'msg-cc-pages',
      role: 'user',
      source: { kind: 'user', rpcId: 'task-cc:input-pages' },
      content: [
        {
          type: 'text',
          text: JSON.stringify({
            kind: 'task_cc',
            task: {
              id: 'task-cc-pages',
              title: '查一下新闻归档里重复链接的判重规则和影响范围',
            },
            text: '这条抄送跨两页',
          }),
        },
      ],
    };
    let seq = 0;
    let at = 0;
    const rows: Record<string, unknown>[] = [];
    const next = (type: string, data: Record<string, unknown>) => {
      seq += 1;
      at += 1;
      rows.push({ type, seq, time: startedAt + at, data });
    };
    const assistant = (turn: number, text: string) => {
      next('assistant/message', {
        turn,
        step: 1,
        message: {
          id: 'msg-syn-' + String(turn),
          role: 'assistant',
          source: { kind: 'model', provider: 'deepseek', model: 'deepseek-flash' },
          content: [{ type: 'text', text }],
        },
        stream: [],
        usage: { inputTokens: 10, outputTokens: 3 },
      });
    };
    next('turn/start', { turn: 1 });
    next('user/message', {
      turn: 1,
      step: 1,
      message: {
        id: 'msg-first',
        role: 'user',
        source: { kind: 'user' },
        content: [{ type: 'text', text: '先问一句' }],
      },
    });
    assistant(1, '第一轮好了。');
    next('turn/end', { turn: 1, reason: { kind: 'completed' } });
    next('agent/inbox/spliced', { target: 'next-turn', start: 0, inserted: [ccMessage] });
    next('turn/start', { turn: 2 });
    next('user/message', { turn: 2, step: 1, message: ccMessage });
    assistant(2, '第 2 步完成。');
    next('turn/end', { turn: 2, reason: { kind: 'completed' } });
    for (let turn = 3; turn <= 12; turn += 1) {
      next('turn/start', { turn });
      assistant(turn, '第 ' + String(turn) + ' 步完成。');
      next('turn/end', { turn, reason: { kind: 'completed' } });
    }
    const header = { type: 'session', version: 17, id: sessionId, createdAt: startedAt, cwd };
    await writeFile(
      path.join(sessions, sessionId + '.jsonl'),
      [header, ...rows].map((row) => JSON.stringify(row)).join(String.fromCharCode(10)) +
        String.fromCharCode(10),
    );
    await writeFile(
      path.join(dataDir, 'kind-root.json'),
      JSON.stringify({ version: 1, sessionId }),
    );
  }
  // T014.2：旧格式（v16）的根会话日志——读取器要照读，不拒绝、不迁移。
  if (options.legacySession) {
    const legacyId = '00000000-0000-4000-8000-000000000146';
    const sessions = path.join(dataDir, 'sessions');
    await mkdir(sessions, { recursive: true });
    const time = Date.now();
    const legacyCwd = await realpath(dataDir);
    const rows = [
      { type: 'session', version: 16, id: legacyId, createdAt: time, cwd: legacyCwd },
      {
        type: 'user/message',
        seq: 1,
        time,
        data: {
          turn: 1,
          step: 1,
          message: {
            id: 'msg-legacy-user',
            role: 'user',
            source: { kind: 'user' },
            content: [{ text: '这条对话是 16 版日志写的。', type: 'text' }],
          },
        },
      },
      {
        type: 'assistant/message',
        seq: 2,
        time,
        data: {
          turn: 1,
          step: 1,
          message: {
            id: 'msg-legacy-assistant',
            role: 'assistant',
            source: { kind: 'model', model: 'deepseek-flash', provider: 'deepseek' },
            content: [{ text: '（T014.2 旧日志）16 版写下的回答还在。', type: 'text' }],
          },
          stream: [
            { chunk: { blockType: 'text', index: 0, type: 'block-start' }, time, type: 'chunk' },
            { dt: [], index: 0, texts: ['（T014.2 旧日志）16 版写下的回答还在。'], time0: time, type: 'text-chunks' },
            { chunk: { block: { text: '（T014.2 旧日志）16 版写下的回答还在。', type: 'text' }, index: 0, type: 'block-end' }, time, type: 'chunk' },
            { chunk: { reason: { kind: 'stop' }, type: 'finish' }, time, type: 'chunk' },
          ],
          usage: { inputTokens: 10, outputTokens: 3 },
        },
      },
    ];
    await writeFile(
      path.join(sessions, legacyId + '.jsonl'),
      rows.map((row) => JSON.stringify(row)).join(String.fromCharCode(10)) + String.fromCharCode(10),
    );
    await writeFile(
      path.join(dataDir, 'kind-root.json'),
      JSON.stringify({ version: 1, sessionId: legacyId }),
    );
  }
  const serveArgs = ['serve', '--data-dir', dataDir, '--identity', identityPath, '--cwd', dataDir, '--web-dist', path.join(repoRoot, 'web/dist')];
  if (options.routeModel) {
    serveArgs.push('--connection', 'deepseek', '--model', options.routeModel);
  }
  if (options.contextWindow) {
    serveArgs.push('--context-window', String(options.contextWindow));
  }
  if (options.startupSelection) {
    serveArgs.push(
      '--connection',
      options.startupSelection.connection,
      '--model',
      options.startupSelection.model,
    );
    if (options.startupSelection.effort) {
      serveArgs.push('--effort', options.startupSelection.effort);
    }
  }
  const serviceEnv = { ...process.env };
  if (options.home) {
    serviceEnv.HOME = options.home === 'dataDir' ? dataDir : options.home;
  }
  // 设置页「联网搜索」：本地假 Exa（只认 /search；key 以 bad 开头就 401，照原型约定）。
  let exaServer: Server | null = null;
  let exaBaseUrl: string | null = null;
  const exaRequests: string[] = [];
  if (options.fakeExa) {
    exaServer = createServer((request, response) => {
      if (request.method !== 'POST' || request.url !== '/search') {
        response.writeHead(404);
        response.end();
        return;
      }
      let body = '';
      request.on('data', (chunk: Buffer) => {
        body += chunk.toString();
      });
      request.on('end', () => {
        const authorization = String(request.headers.authorization ?? '');
        exaRequests.push(authorization);
        if (authorization.includes('bad')) {
          response.writeHead(401, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ error: 'invalid api key' }));
          return;
        }
        // T014.7：搜索请求（numResults 8）回一条带摘要的结果；验证请求（numResults 1）照旧空结果。
        let numResults: unknown;
        try {
          numResults = (JSON.parse(body) as { numResults?: unknown }).numResults;
        } catch {
          numResults = undefined;
        }
        const results =
          numResults === 8
            ? [
                {
                  url: 'https://example.com/aios-search',
                  title: 'AIOS 原生工具',
                  highlights: ['AIOS 的联网搜索结果样例。'],
                  publishedDate: '2026-11-09',
                },
              ]
            : [];
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ results }));
      });
    });
    await new Promise<void>((resolve) => {
      exaServer!.listen(0, '127.0.0.1', () => resolve());
    });
    const port = (exaServer.address() as net.AddressInfo).port;
    exaBaseUrl = 'http://127.0.0.1:' + String(port);
    serveArgs.push('--exa-base-url', exaBaseUrl);
  }
  // 读网页：本地假网页代理。被测服务的 http_proxy 指过来，公网 IP 的请求由它回正文；
  // 本机地址在发请求之前就被拦下，不该到这里。
  let fetchServer: Server | null = null;
  const fetchRequests: string[] = [];
  if (options.t014Fetch) {
    fetchServer = createServer((request, response) => {
      const target = request.url ?? '';
      fetchRequests.push(target);
      let pathname = target;
      try {
        pathname = new URL(target).pathname;
      } catch {
        // 相对形式（不该出现）就按原样比。
      }
      if (request.method !== 'GET' || pathname !== '/aios-fetch') {
        response.writeHead(404);
        response.end();
        return;
      }
      response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      response.end(
        '<!doctype html><html><head><title>样例</title><script>window.alert("脚本不该出现")</script></head><body><h1>读网页正文</h1><p>T014.8 e2e 正文</p></body></html>',
      );
    });
    await new Promise<void>((resolve) => {
      fetchServer!.listen(0, '127.0.0.1', () => resolve());
    });
    const port = (fetchServer.address() as net.AddressInfo).port;
    const proxyUrl = 'http://127.0.0.1:' + String(port);
    serviceEnv.http_proxy = proxyUrl;
    serviceEnv.HTTP_PROXY = proxyUrl;
    for (const name of [
      'https_proxy',
      'HTTPS_PROXY',
      'all_proxy',
      'ALL_PROXY',
      'no_proxy',
      'NO_PROXY',
    ]) {
      serviceEnv[name] = '';
    }
  }
  if (options.envExaKey) {
    // 故意放一个值：产品不该读它（设置页仍是「还没有 key」）。
    serviceEnv.EXA_API_KEY = options.envExaKey;
  }
  // T022：把汇率请求钉在本地代理上。模型请求走 127.0.0.1，产品本来就绕过代理；
  // 这里只把 open.er-api.com 的 CONNECT 拦下来，测「status 正在 fx 的 await 里」这一态。
  let fxServer: Server | null = null;
  let fxGate: FxGate | null = null;
  if (options.slowFx) {
    let holding = false;
    let fxConnects = 0;
    let heldSocket: net.Socket | null = null;
    let firstResolve!: () => void;
    let heldResolve!: () => void;
    const firstConnect = new Promise<void>((resolve) => {
      firstResolve = resolve;
    });
    const held = new Promise<void>((resolve) => {
      heldResolve = resolve;
    });
    const server = createServer((_request, response) => {
      response.writeHead(502);
      response.end();
    });
    server.on('connect', (request, socket) => {
      socket.on('error', () => {});
      const host = (request.url ?? '').split(':', 1)[0];
      if (host !== 'open.er-api.com') {
        socket.end('HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n');
        return;
      }
      fxConnects += 1;
      if (fxConnects === 1) firstResolve();
      if (holding) {
        heldSocket = socket;
        heldResolve();
        return;
      }
      // 平时也让 fx 失败（不写缓存），这样每次 status 都会重新来拉。
      setTimeout(() => {
        if (!socket.destroyed) {
          socket.end('HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n');
        }
      }, 30);
    });
    await new Promise<void>((resolve) => {
      server.listen(0, '127.0.0.1', () => resolve());
    });
    const port = (server.address() as net.AddressInfo).port;
    const proxyUrl = 'http://127.0.0.1:' + String(port);
    serviceEnv.https_proxy = proxyUrl;
    serviceEnv.HTTPS_PROXY = proxyUrl;
    serviceEnv.all_proxy = '';
    serviceEnv.ALL_PROXY = '';
    serviceEnv.no_proxy = '';
    serviceEnv.NO_PROXY = '';
    fxServer = server;
    fxGate = {
      firstConnect,
      held,
      hold: (on) => {
        holding = on;
      },
      release: () => {
        holding = false;
        const socket = heldSocket;
        heldSocket = null;
        socket?.end('HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n');
      },
    };
  }
  let holder: ReturnType<typeof spawn> | null = null;
  if (options.busyRoot) {
    holder = spawn(bin, ['serve', '--data-dir', dataDir, '--identity', identityPath, '--cwd', dataDir], {
      env: serviceEnv,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const holderChild = holder;
    let holderOutput = '';
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('holder 启动超时: ' + holderOutput)), 30_000);
      holderChild.stdout?.on('data', (chunk: Buffer) => {
        holderOutput += chunk.toString();
        if (holderOutput.includes('aios-dev serve:')) {
          clearTimeout(timer);
          resolve();
        }
      });
      holderChild.on('exit', (code) => {
        clearTimeout(timer);
        reject(new Error('holder 退出: ' + String(code)));
      });
    });
  }
  const waitForUrl = (
    proc: ReturnType<typeof spawn>,
  ): Promise<{ baseUrl: string; token: string }> =>
    new Promise((resolve, reject) => {
      let output = '';
      const timer = setTimeout(() => reject(new Error(`serve 启动超时: ${output}`)), 30_000);
      proc.stdout?.on('data', (chunk: Buffer) => {
        output += chunk.toString();
        const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\/?\?token=([0-9a-f-]+)/);
        if (match) {
          clearTimeout(timer);
          resolve({ baseUrl: `http://127.0.0.1:${match[1]}`, token: match[2] });
        }
      });
      proc.on('exit', (code) => {
        clearTimeout(timer);
        reject(new Error(`serve 退出（${code}）: ${output}`));
      });
    });
  let child = spawn(bin, serveArgs, {
    env: serviceEnv,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const parsed = await waitForUrl(child);
  let token = parsed.token;
  const restart = async (): Promise<void> => {
    child.kill('SIGTERM');
    await new Promise<void>((resolve) => child.once('exit', () => resolve()));
    await new Promise((resolve) => setTimeout(resolve, 300));
    const port = Number(new URL(parsed.baseUrl).port);
    child = spawn(bin, [...serveArgs, '--port', String(port)], {
      env: serviceEnv,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const next = await waitForUrl(child);
    token = next.token;
  };
  /** T029.3：再起一个回放、换一份路由文件，返回新地址（老回放不杀，在跑的任务还连着它）。 */
  const switchReplay = async (routesFile: string): Promise<string> => {
    const next = startReplay(options, replayFile, routesFile);
    extraReplayChildren.push(next.child);
    return next.url;
  };
  let baseUrl = parsed.baseUrl;
  const proxyState: {
    server: Server | null;
    port: number;
    sockets: Set<net.Socket>;
  } = { server: null, port: 0, sockets: new Set() };
  const crlf = String.fromCharCode(13, 10);
  const targetPort = Number(new URL(parsed.baseUrl).port);
  const startProxy = async (port: number): Promise<number> =>
    new Promise((resolve) => {
      const server = createServer((req, res) => {
        const upstream = httpRequest(
          { host: '127.0.0.1', port: targetPort, path: req.url, method: req.method, headers: req.headers },
          (response) => {
            res.writeHead(response.statusCode ?? 502, response.headers);
            response.pipe(res);
          },
        );
        upstream.on('error', () => {
          if (!res.headersSent) res.writeHead(502);
          res.end();
        });
        req.pipe(upstream);
      });
      server.on('upgrade', (req, socket, head) => {
        const upstream = net.connect(targetPort, '127.0.0.1', () => {
          const headerLines: string[] = [];
          for (let index = 0; index < req.rawHeaders.length; index += 2) {
            headerLines.push(req.rawHeaders[index] + ': ' + req.rawHeaders[index + 1]);
          }
          upstream.write(req.method + ' ' + req.url + ' HTTP/1.1' + crlf + headerLines.join(crlf) + crlf + crlf);
          if (head.length > 0) upstream.write(head);
          socket.pipe(upstream);
          upstream.pipe(socket);
        });
        proxyState.sockets.add(socket);
        socket.on('close', () => proxyState.sockets.delete(socket));
        upstream.on('error', () => socket.destroy());
        socket.on('error', () => upstream.destroy());
      });
      server.listen(port, '127.0.0.1', () => {
        proxyState.server = server;
        proxyState.port = (server.address() as net.AddressInfo).port;
        resolve(proxyState.port);
      });
    });
  const dropProxy = async (): Promise<void> => {
    const server = proxyState.server;
    proxyState.server = null;
    if (server) {
      server.close();
    }
    for (const socket of proxyState.sockets) socket.destroy();
    proxyState.sockets.clear();
  };
  const restoreProxy = async (): Promise<void> => {
    if (!proxyState.server) await startProxy(proxyState.port);
  };
  if (options.proxy) {
    baseUrl = 'http://127.0.0.1:' + String(await startProxy(0));
  }
  return {
    ...parsed,
    baseUrl,
    // restart 会换新令牌：读属性要拿当前值，不能是启动时的快照。
    get token() {
      return token;
    },
    dataDir,
    paths,
    exa: exaBaseUrl ? { baseUrl: exaBaseUrl, requests: () => exaRequests } : undefined,
    fetch: fetchServer ? { requests: () => fetchRequests } : undefined,
    fx: fxGate ?? undefined,
    fakeEndpoint: replayUrl,
    lastModel: async () => {
      try {
        const response = await fetch(replayUrl + '/__replay/last-model');
        const body = (await response.json()) as { model?: string | null };
        return body.model ?? null;
      } catch {
        return null;
      }
    },
    lastRequest: async () => {
      try {
        const response = await fetch(replayUrl + '/__replay/last-request');
        const body = (await response.json()) as { request?: Record<string, unknown> | null };
        return body.request ?? null;
      } catch {
        return null;
      }
    },
    dropProxy,
    restoreProxy,
    restart,
    switchReplay,
    close: async () => {
      await dropProxy();
      fxGate?.release();
      fxServer?.close();
      exaServer?.close();
      fetchServer?.close();
      holder?.kill('SIGTERM');
      child.kill('SIGTERM');
      replay.child.kill('SIGTERM');
      for (const extra of extraReplayChildren) extra.kill('SIGTERM');
      await rm(dataDir, { recursive: true, force: true });
      if (paths) await rm(paths.outside, { recursive: true, force: true });
    },
  };
}


/** T007.11 真 key DoD：真 DeepSeek + /tmp/aios-t007-dod/work；无 key 时调用方 skip。 */

/** 页面 ↔ 服务之间的可断开 HTTP+WS 代理（DoD6 断线重连用）。 */
function proxyTo(targetPort: number) {
  const state: { server: Server | null; port: number; sockets: Set<net.Socket> } = {
    server: null,
    port: 0,
    sockets: new Set(),
  };
  const crlf = String.fromCharCode(13, 10);
  const start = async (port: number): Promise<number> =>
    new Promise((resolve) => {
      const server = createServer((req, res) => {
        const upstream = httpRequest(
          { host: '127.0.0.1', port: targetPort, path: req.url, method: req.method, headers: req.headers },
          (response) => {
            res.writeHead(response.statusCode ?? 502, response.headers);
            response.pipe(res);
          },
        );
        upstream.on('error', () => {
          if (!res.headersSent) res.writeHead(502);
          res.end();
        });
        req.pipe(upstream);
      });
      server.on('upgrade', (req, socket, head) => {
        const upstream = net.connect(targetPort, '127.0.0.1', () => {
          const lines: string[] = [];
          for (let index = 0; index < req.rawHeaders.length; index += 2) {
            lines.push(req.rawHeaders[index] + ': ' + req.rawHeaders[index + 1]);
          }
          upstream.write(req.method + ' ' + req.url + ' HTTP/1.1' + crlf + lines.join(crlf) + crlf + crlf);
          if (head.length > 0) upstream.write(head);
          socket.pipe(upstream);
          upstream.pipe(socket);
        });
        state.sockets.add(socket);
        socket.on('close', () => state.sockets.delete(socket));
        upstream.on('error', () => socket.destroy());
        socket.on('error', () => upstream.destroy());
      });
      server.listen(port, '127.0.0.1', () => {
        state.server = server;
        state.port = (server.address() as net.AddressInfo).port;
        resolve(state.port);
      });
    });
  const drop = async (): Promise<void> => {
    const server = state.server;
    state.server = null;
    if (server) server.close();
    for (const socket of state.sockets) socket.destroy();
    state.sockets.clear();
  };
  const restore = async (): Promise<void> => {
    if (!state.server) await start(state.port);
  };
  return { start, drop, restore };
}

export async function startRealKernelFixture(
  options: { proxy?: boolean } = {},
): Promise<KernelFixture> {
  const apiKey = process.env.DEEPSEEK_API_KEY;
  if (!apiKey) throw new Error('缺少 DEEPSEEK_API_KEY（D34：开发期从环境变量读 key）');
  const root = process.env.AIOS_DOD_ROOT ?? '/tmp/aios-t007-dod';
  const workDir = path.join(root, 'work');
  const dataDir = path.join(root, 'data');
  await mkdir(workDir, { recursive: true });
  await mkdir(dataDir, { recursive: true });
  await writeFile(path.join(workDir, 'hello.txt'), 'DOD-4-OK' + String.fromCharCode(10));
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');
  const identityPath = path.join(dataDir, 'identity.txt');
  const seed = spawnSync(
    bin,
    ['keyring', '--data-dir', dataDir, '--identity', identityPath, 'set', '--id', 'deepseek', '--name', 'DeepSeek', '--model', 'deepseek-flash', '--endpoint', 'https://api.deepseek.com/anthropic'],
    { input: apiKey + '\n', encoding: 'utf8' },
  );
  if (seed.status !== 0) throw new Error('seed keyring failed: ' + String(seed.stderr));
  const child = spawn(
    bin,
    ['serve', '--data-dir', dataDir, '--identity', identityPath, '--cwd', workDir, '--web-dist', path.join(repoRoot, 'web/dist')],
    { env: process.env, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let stderr = '';
  child.stderr?.on('data', (chunk: Buffer) => {
    stderr += chunk.toString();
  });
  let output = '';
  const parsed = await new Promise<{ baseUrl: string; token: string }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('真 key serve 启动超时: ' + output)), 60_000);
    child.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\/?\?token=([0-9a-f-]+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ baseUrl: 'http://127.0.0.1:' + match[1], token: match[2] });
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      reject(new Error('真 key serve 退出: ' + String(code) + ' ' + output));
    });
  });
  let baseUrl = parsed.baseUrl;
  const proxy = proxyTo(Number(new URL(parsed.baseUrl).port));
  if (options.proxy) {
    baseUrl = 'http://127.0.0.1:' + String(await proxy.start(0));
  }
  return {
    ...parsed,
    baseUrl,
    dataDir,
    fakeEndpoint: '',
    lastModel: async () => null,
    lastRequest: async () => null,
    stderr: () => stderr,
    dropProxy: proxy.drop,
    restoreProxy: proxy.restore,
    close: async () => {
      await proxy.drop();
      child.kill('SIGTERM');
      if (process.env.AIOS_DOD_KEEP !== '1') {
        await rm(root, { recursive: true, force: true });
      }
    },
  };
}

/** T008.5 真 key DoD：/tmp/aios-t008-dod，空 Keyring，页面自己填 key。 */
export async function startRealDodFixture(): Promise<KernelFixture> {
  const root = process.env.AIOS_DOD_ROOT ?? '/tmp/aios-t008-dod';
  const workDir = path.join(root, 'work');
  const dataDir = path.join(root, 'data');
  await rm(root, { recursive: true, force: true });
  await mkdir(workDir, { recursive: true });
  await mkdir(dataDir, { recursive: true });
  await writeFile(path.join(workDir, 'hello.txt'), 'DOD-T008-OK' + String.fromCharCode(10));
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');
  const identity = path.join(dataDir, 'identity.txt');
  const serveEnv = { ...process.env };
  delete serveEnv.DEEPSEEK_API_KEY;
  delete serveEnv.DEEPSEEK_BASE_URL;
  const child = spawn(
    bin,
    ['serve', '--data-dir', dataDir, '--identity', identity, '--cwd', workDir, '--web-dist', path.join(repoRoot, 'web/dist')],
    { env: serveEnv, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let output = '';
  const parsed = await new Promise<{ baseUrl: string; token: string }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('T008 serve 启动超时: ' + output)), 60_000);
    child.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\/?\?token=([0-9a-f-]+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ baseUrl: 'http://127.0.0.1:' + match[1], token: match[2] });
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      reject(new Error('T008 serve 退出: ' + String(code) + ' ' + output));
    });
  });
  return {
    ...parsed,
    dataDir,
    fakeEndpoint: '',
    lastModel: async () => null,
    dropProxy: async () => {},
    restoreProxy: async () => {},
    close: async () => {
      child.kill('SIGTERM');
      if (process.env.AIOS_DOD_KEEP !== '1') {
        await rm(root, { recursive: true, force: true });
      }
    },
  };
}

/** T013 DoD2：真 key（MiniMax）的夹具——/tmp/aios-t013-dod 下的数据目录 + 小项目工作目录。 */
export async function startT013KernelFixture(): Promise<KernelFixture> {
  const apiKey = process.env.MINIMAX_API_KEY;
  if (!apiKey) throw new Error('缺少 MINIMAX_API_KEY（D35：DoD 从环境变量读 key）');
  const root = process.env.AIOS_T013_DOD_ROOT ?? '/tmp/aios-t013-dod';
  const workDir = path.join(root, 'work');
  const dataDir = path.join(root, 'data');
  await mkdir(workDir, { recursive: true });
  await mkdir(dataDir, { recursive: true });
  // 小项目：只有几个文件（自己造的测试文件）。
  await writeFile(path.join(workDir, 'notes.md'), '# 项目笔记' + String.fromCharCode(10) + String.fromCharCode(10) + '第一行：DOD13-NOTES' + String.fromCharCode(10));
  await writeFile(path.join(workDir, 'todo.txt'), 'DOD13-TODO：买牛奶、写代码' + String.fromCharCode(10));
  await writeFile(path.join(workDir, 'config.json'), '{"name":"dod13"}' + String.fromCharCode(10));
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');
  const identityPath = path.join(dataDir, 'identity.txt');
  const seed = spawnSync(
    bin,
    [
      'keyring', '--data-dir', dataDir, '--identity', identityPath, 'set',
      '--id', 'minimax-cn',
      '--name', 'MiniMax（国内站）',
      '--model', 'MiniMax-M3.1-Flash',
      '--endpoint', 'https://api.minimaxi.com/anthropic',
    ],
    { input: apiKey + '\n', encoding: 'utf8' },
  );
  if (seed.status !== 0) throw new Error('seed keyring failed: ' + String(seed.stderr));
  const child = spawn(
    bin,
    ['serve', '--data-dir', dataDir, '--identity', identityPath, '--cwd', workDir, '--web-dist', path.join(repoRoot, 'web/dist')],
    { env: process.env, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let stderr = '';
  child.stderr?.on('data', (chunk: Buffer) => {
    stderr += chunk.toString();
  });
  let output = '';
  const parsed = await new Promise<{ baseUrl: string; token: string }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('T013 真 key serve 启动超时: ' + output)), 60_000);
    child.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\/?\?token=([0-9a-f-]+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ baseUrl: 'http://127.0.0.1:' + match[1], token: match[2] });
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      reject(new Error('T013 serve 提前退出（' + String(code) + '）：' + stderr));
    });
  });
  return {
    baseUrl: parsed.baseUrl,
    token: parsed.token,
    baseUrlRaw: parsed.baseUrl,
    close: async () => {
      child.kill('SIGTERM');
    },
    stderr: () => stderr,
    dataDir,
  } as unknown as KernelFixture;
}

/**
 * DoD 真 key 的通用夹具（T008 模式）：空库起服务，页面里真实加连接并验证（key 从 env 读、只进页面输入）。
 * 每条用例用自己的数据目录（root/<用例名>/），跑完不清，便于按条回查日志。
 */
export async function startDodConnectFixture(root: string): Promise<KernelFixture> {
  const workDir = path.join(root, 'work');
  const dataDir = path.join(root, 'data');
  await rm(root, { recursive: true, force: true });
  await mkdir(workDir, { recursive: true });
  await mkdir(dataDir, { recursive: true });
  await writeFile(path.join(workDir, 'notes.md'), '# 项目笔记' + String.fromCharCode(10) + String.fromCharCode(10) + '第一行：DOD13-NOTES' + String.fromCharCode(10));
  await writeFile(path.join(workDir, 'todo.txt'), 'DOD13-TODO：买牛奶、写代码' + String.fromCharCode(10));
  await writeFile(path.join(workDir, 'config.json'), '{"name":"dod13"}' + String.fromCharCode(10));
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');
  const identity = path.join(dataDir, 'identity.txt');
  const serveEnv = { ...process.env };
  delete serveEnv.MINIMAX_API_KEY;
  delete serveEnv.DEEPSEEK_API_KEY;
  const child = spawn(
    bin,
    ['serve', '--data-dir', dataDir, '--identity', identity, '--cwd', workDir, '--web-dist', path.join(repoRoot, 'web/dist')],
    { env: serveEnv, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let stderr = '';
  child.stderr?.on('data', (chunk: Buffer) => {
    stderr += chunk.toString();
  });
  let output = '';
  const parsed = await new Promise<{ baseUrl: string; token: string }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('T013 DoD serve 启动超时: ' + output)), 60_000);
    child.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\/?\?token=([0-9a-f-]+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ baseUrl: 'http://127.0.0.1:' + match[1], token: match[2] });
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      reject(new Error('T013 DoD serve 提前退出（' + String(code) + '）：' + stderr));
    });
  });
  return {
    baseUrl: parsed.baseUrl,
    token: parsed.token,
    close: async () => {
      child.kill('SIGTERM');
    },
    stderr: () => stderr,
    dataDir,
  } as unknown as KernelFixture;
}

/** T013 DoD2：真 key（MiniMax）的夹具——/tmp/aios-t013-dod 下的数据目录 + 小项目工作目录。 */
export async function startT013DodFixture(caseName: string): Promise<KernelFixture> {
  return startDodConnectFixture(
    path.join(process.env.AIOS_T013_DOD_ROOT ?? '/tmp/aios-t013-dod', caseName),
  );
}

/** T015 DoD2：真 key（MiniMax）画布四件事——/tmp/aios-t015-dod 下的数据目录 + 小项目工作目录。 */
export async function startT015DodFixture(caseName: string): Promise<KernelFixture> {
  return startDodConnectFixture(
    path.join(process.env.AIOS_T015_DOD_ROOT ?? '/tmp/aios-t015-dod', caseName),
  );
}

/** T016.7 DoD2：真 key（MiniMax 国内站）获取/打断/再发/解除——/tmp/aios-t016-dod 下的数据目录。 */
export async function startT016DodFixture(caseName: string): Promise<KernelFixture> {
  return startDodConnectFixture(
    path.join(process.env.AIOS_T016_DOD_ROOT ?? '/tmp/aios-t016-dod', caseName),
  );
}

/** T017.5 DoD2：真 key（MiniMax）卡片机制三步——/tmp/aios-t017-dod 下的数据目录 + 小项目工作目录。 */
export async function startT017DodFixture(): Promise<KernelFixture> {
  return startDodConnectFixture(process.env.AIOS_T017_DOD_ROOT ?? '/tmp/aios-t017-dod');
}
