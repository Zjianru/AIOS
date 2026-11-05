import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
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
  if (options.usageReadOnly) return { file: 'usage-read-only', paceMs: 400 };
  if (options.askTurn) return { file: 'ask', paceMs: options.askTurnPaceMs };
  if (options.toolTurn) return { file: 'tool' };
  if (options.highUsage) return { file: 'high-usage' };
  return {
    file: 'text',
    paceMs: options.slowMs ? Math.max(1, Math.round(options.slowMs / 6)) : undefined,
  };
}

/** 起 aios-replay（打印一行 aios-replay: http://…）；每夹具一个进程。 */
function startReplay(options: FixtureOptions): { child: ReturnType<typeof spawn>; url: Promise<string> } {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_REPLAY_BIN ?? path.join(repoRoot, 'target/debug/aios-replay');
  const selection = replaySelection(options);
  const args = ['--file', path.join(FIXTURE_DIR, selection.file + '.jsonl')];
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
  /** 真 key fixture：serve 进程的 stderr（DoD4 断言写失败那行用）。 */
  stderr?: () => string;
  dropProxy?: () => Promise<void>;
  restoreProxy?: () => Promise<void>;
  /// T007 #1：页面开着时重启同一个端口的服务（新进程=新 cookie 钥匙，旧页面会话失效）。
  restart?: () => Promise<void>;
  close(): Promise<void>;
}

export interface FixtureOptions {
  /// first request 回一次工具调用（read hello.txt），拿到工具结果后再回最终文本。
  toolTurn?: boolean;
  /// 每次模型回放延迟毫秒（T007.5 停止用例要在运行中按停止/Esc）。
  slowMs?: number;
  /// first request 回一次 ask_user_question（两题：一题选项、一题自己写）。
  askTurn?: boolean;
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
  /// T009.5：serve 进程的 HOME；传 'dataDir' 用数据目录（页面路径缩写用例）。
  home?: 'dataDir' | string;
  /// T010.3：用带 reasoning/缓存读写的用量夹具（turn-usage 用例）。
  usageTurn?: boolean;
  /// T012 F1：用量夹具去掉缓存写（只填缓存读价的端到端用）。
  usageReadOnly?: boolean;
  /// T010.5：用真 key 录的夹具（多步工具/思考/缓存读/压缩）。
  realTurn?: boolean;
}

/** 接缝 5：假 DeepSeek 脚本回放 + 真 aios-dev serve（页面也由它同源托管）。 */
export async function startKernelFixture(
  options: FixtureOptions = {},
): Promise<KernelFixture> {
  const replay = startReplay(options);
  const replayUrl = await replay.url;
  const dataDir = await mkdtemp(path.join(tmpdir(), 'aios-t007-'));
  if (options.rootFault === 'corrupt') {
    await writeFile(path.join(dataDir, 'kind-root.json'), '{"version":1, ');
  } else if (options.rootFault === 'lost') {
    await writeFile(
      path.join(dataDir, 'kind-root.json'),
      JSON.stringify({ version: 1, sessionId: randomUUID() }),
    );
  }
  if (options.toolTurn) {
    await writeFile(path.join(dataDir, 'hello.txt'), 'hello from fixture\n');
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
  const serveArgs = ['serve', '--data-dir', dataDir, '--identity', identityPath, '--cwd', dataDir, '--web-dist', path.join(repoRoot, 'web/dist')];
  if (options.contextWindow) {
    serveArgs.push('--context-window', String(options.contextWindow));
  }
  const serviceEnv = { ...process.env };
  if (options.home) {
    serviceEnv.HOME = options.home === 'dataDir' ? dataDir : options.home;
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
      holder?.kill('SIGTERM');
      child.kill('SIGTERM');
      replay.child.kill('SIGTERM');
      await rm(dataDir, { recursive: true, force: true });
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
