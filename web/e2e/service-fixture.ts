import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { createServer, request as httpRequest, type Server } from 'node:http';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import type { FixtureOptions } from './support/options';
import { startReplay } from './support/replay';

const FIXTURE_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures');

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
    options.t030 !== undefined ||
    options.t054 === 'oldCard' ||
    options.t054 === 'update' ||
    options.t054 === 'updateWindow' ||
    options.t054 === 'updateWindow2' ||
    options.t055 === 'history'
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
  // 程序记忆卡（许可/整理完/没跑成）的渲染夹具。触发它们的接线在下一票，
  // 这里只预写会话日志，验卡片按 kind 渲染、和回复分开、刷新后从日志重建。
  if (options.memoryCards) {
    const sessionId = '00000000-0000-4000-8000-000000000603';
    const sessions = path.join(dataDir, 'sessions');
    await mkdir(sessions, { recursive: true });
    const startedAt = Date.now();
    const cwd = await realpath(dataDir);
    const rows: Record<string, unknown>[] = [];
    let seq = 0;
    let at = 0;
    const next = (type: string, data: Record<string, unknown>) => {
      seq += 1;
      at += 1;
      rows.push({ type, seq, time: startedAt + at, data });
    };
    next('turn/start', { turn: 1 });
    next('user/message', {
      turn: 1,
      step: 1,
      message: {
        id: 'msg-memory-cards',
        role: 'user',
        source: { kind: 'user' },
        content: [{ type: 'text', text: '记一下我的偏好，攒够了再整理你的笔记' }],
      },
    });
    next('assistant/message', {
      turn: 1,
      step: 1,
      message: {
        id: 'msg-memory-cards-answer',
        role: 'assistant',
        source: { kind: 'model', provider: 'deepseek', model: 'deepseek-flash' },
        content: [{ type: 'text', text: '好，我先记着；整理的事到点再说。' }],
      },
      stream: [],
      usage: { inputTokens: 10, outputTokens: 3 },
    });
    next('turn/end', { turn: 1, reason: { kind: 'completed' } });
    next('memory/card', { id: 'card-permission-1', kind: 'permission', ref: 'dream-run-1' });
    next('memory/card', { id: 'card-done-1', kind: 'dream_done', ref: 'dream-run-2' });
    next('memory/card', { id: 'card-failed-1', kind: 'dream_failed', ref: 'dream-run-3' });
    const header = { type: 'session', version: 18, id: sessionId, createdAt: startedAt, cwd };
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
export function proxyTo(targetPort: number) {
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

/** 真 key DoD：/tmp/aios-t008-dod，空 Keyring，页面自己填 key。 */
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

/** DoD2：真 key（MiniMax）的夹具——/tmp/aios-t013-dod 下的数据目录 + 小项目工作目录。 */
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
 * DoD 真 key 的通用夹具（ 模式）：空库起服务，页面里真实加连接并验证（key 从 env 读、只进页面输入）。
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

/** DoD2：真 key（MiniMax）的夹具——/tmp/aios-t013-dod 下的数据目录 + 小项目工作目录。 */
export async function startT013DodFixture(caseName: string): Promise<KernelFixture> {
  return startDodConnectFixture(
    path.join(process.env.AIOS_T013_DOD_ROOT ?? '/tmp/aios-t013-dod', caseName),
  );
}

/** DoD2：真 key（MiniMax）画布四件事——/tmp/aios-t015-dod 下的数据目录 + 小项目工作目录。 */
export async function startT015DodFixture(caseName: string): Promise<KernelFixture> {
  return startDodConnectFixture(
    path.join(process.env.AIOS_T015_DOD_ROOT ?? '/tmp/aios-t015-dod', caseName),
  );
}

/** DoD2：真 key（MiniMax 国内站）获取/打断/再发/解除——/tmp/aios-t016-dod 下的数据目录。 */
export async function startT016DodFixture(caseName: string): Promise<KernelFixture> {
  return startDodConnectFixture(
    path.join(process.env.AIOS_T016_DOD_ROOT ?? '/tmp/aios-t016-dod', caseName),
  );
}

/** DoD2：真 key（MiniMax）卡片机制三步——/tmp/aios-t017-dod 下的数据目录 + 小项目工作目录。 */
export async function startT017DodFixture(): Promise<KernelFixture> {
  return startDodConnectFixture(process.env.AIOS_T017_DOD_ROOT ?? '/tmp/aios-t017-dod');
}
