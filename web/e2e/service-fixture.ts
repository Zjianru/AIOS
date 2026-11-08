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
  if (options.codeBlock) return { file: 'tool-code' };
  if (options.t013Process) return { file: 'tool-process' };
  if (options.toolCards) return { file: 'tool-cards' };
  if (options.t013Interrupted) return { file: 'tool-interrupted', paceMs: 800 };
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

/** 起 aios-replay（打印一行 aios-replay: http://…）；每夹具一个进程。 */
function startReplay(
  options: FixtureOptions,
  replayFile?: string,
): { child: ReturnType<typeof spawn>; url: Promise<string> } {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_REPLAY_BIN ?? path.join(repoRoot, 'target/debug/aios-replay');
  const selection = replaySelection(options);
  const args = ['--file', replayFile ?? path.join(FIXTURE_DIR, selection.file + '.jsonl')];
  if (selection.override) args.push('--override', path.join(FIXTURE_DIR, selection.override + '.json'));
  if (selection.paceMs) args.push('--pace-ms', String(selection.paceMs));
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
  dropProxy?: () => Promise<void>;
  restoreProxy?: () => Promise<void>;
  /// T007 #1：页面开着时重启同一个端口的服务（新进程=新 cookie 钥匙，旧页面会话失效）。
  restart?: () => Promise<void>;
  close(): Promise<void>;
}

export interface FixtureOptions {
  /// first request 回一次工具调用（read hello.txt），拿到工具结果后再回最终文本。
  toolTurn?: boolean;
  /// 路径显示：三步回放（cwd 内写、cwd 外写、最终文本）；夹具里的 __CWD__ / __OUTSIDE__ 起服务前换成真实目录。
  pathsTurn?: boolean;
  /// T013.6：最终回答里带一个代码块（复制按钮用）。
  codeBlock?: boolean;
  /// T013.5：一轮里 write/apply_patch/bash 三个工具 + 最终文本（工作过程页的卡片用）。
  t013Process?: boolean;
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
  const replay = startReplay(options, replayFile);
  const replayUrl = await replay.url;
  if (options.rootFault === 'corrupt') {
    await writeFile(path.join(dataDir, 'kind-root.json'), '{"version":1, ');
  } else if (options.rootFault === 'lost') {
    await writeFile(
      path.join(dataDir, 'kind-root.json'),
      JSON.stringify({ version: 1, sessionId: randomUUID() }),
    );
  }
  if (options.toolTurn || options.t013Process) {
    await writeFile(path.join(dataDir, 'hello.txt'), 'hello from fixture\n');
    // T013.7：带空格的路径（@ 菜单插入要给引号用）。
    await writeFile(path.join(dataDir, 'with space.txt'), 'spaced file\n');
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
      ['keyring', '--data-dir', dataDir, '--identity', identityPath, 'set', '--id', 'deepseek', '--name', 'DeepSeek', '--preset', 'deepseek', '--model', 'deepseek-flash', '--endpoint', replayUrl],
      { input: 'test-key' + String.fromCharCode(10), encoding: 'utf8' },
    );
    if (seed.status !== 0) throw new Error('seed keyring failed: ' + String(seed.stderr));
  }
  // T016.5：坏文件要在 serve 起来之前就在；不预置 Keyring 时也照写（坏文件与连接无关）。
  if (options.settingsFault === 'corrupt') {
    await writeFile(path.join(dataDir, 'settings.json'), '{"lastSelection": ');
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
  if (options.contextWindow) {
    serveArgs.push('--context-window', String(options.contextWindow));
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
    token,
    dataDir,
    paths,
    exa: exaBaseUrl ? { baseUrl: exaBaseUrl, requests: () => exaRequests } : undefined,
    fetch: fetchServer ? { requests: () => fetchRequests } : undefined,
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
    close: async () => {
      await dropProxy();
      exaServer?.close();
      fetchServer?.close();
      holder?.kill('SIGTERM');
      child.kill('SIGTERM');
      replay.child.kill('SIGTERM');
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
