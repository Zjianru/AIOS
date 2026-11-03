import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer, request as httpRequest, type Server } from 'node:http';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const FINAL_TEXT_SSE = [
  'event: message_start',
  'data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}',
  '',
  'event: content_block_start',
  'data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}',
  '',
  'event: content_block_delta',
  'data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"（T007 e2e 假回放）"}}',
  '',
  'event: content_block_stop',
  'data: {"type":"content_block_stop","index":0}',
  '',
  'event: message_delta',
  'data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}',
  '',
  'event: message_stop',
  'data: {"type":"message_stop"}',
  '',
  '',
].join('\n');

const HIGH_USAGE_FINAL_TEXT_SSE = FINAL_TEXT_SSE.replace(
  '"usage":{"input_tokens":10,"output_tokens":1}',
  '"usage":{"input_tokens":4500,"output_tokens":1}',
);

const TOOL_CALL_SSE = [
  'event: message_start',
  'data: {"type":"message_start","message":{"id":"msg_tool","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}',
  '',
  'event: content_block_start',
  'data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_read_1","name":"read","input":{}}}',
  '',
  'event: content_block_delta',
  'data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\\"file_path\\":\\"hello.txt\\"}"}}',
  '',
  'event: content_block_stop',
  'data: {"type":"content_block_stop","index":0}',
  '',
  'event: message_delta',
  'data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}',
  '',
  'event: message_stop',
  'data: {"type":"message_stop"}',
  '',
  '',
].join('\n');

const ASK_QUESTIONS = [
  { id: 'q1', question: '先选一个？', options: [{ label: 'Yes' }, { label: 'No' }] },
  { id: 'q2', question: '再自己写一句？' },
];

const ASK_CALL_SSE = (() => {
  const input = JSON.stringify({ questions: ASK_QUESTIONS });
  const data = (value: unknown) => 'data: ' + JSON.stringify(value);
  return [
    'event: message_start',
    data({
      type: 'message_start',
      message: {
        id: 'msg_ask',
        type: 'message',
        role: 'assistant',
        model: 'deepseek-flash',
        content: [],
        usage: { input_tokens: 10, output_tokens: 1 },
      },
    }),
    '',
    'event: content_block_start',
    data({
      type: 'content_block_start',
      index: 0,
      content_block: { type: 'tool_use', id: 'tool_ask_1', name: 'ask_user_question', input: {} },
    }),
    '',
    'event: content_block_delta',
    data({
      type: 'content_block_delta',
      index: 0,
      delta: { type: 'input_json_delta', partial_json: input },
    }),
    '',
    'event: content_block_stop',
    data({ type: 'content_block_stop', index: 0 }),
    '',
    'event: message_delta',
    data({ type: 'message_delta', delta: { stop_reason: 'tool_use' }, usage: { output_tokens: 3 } }),
    '',
    'event: message_stop',
    data({ type: 'message_stop' }),
    '',
    '',
  ].join('\n');
})();

export interface KernelFixture {
  baseUrl: string;
  token: string;
  dataDir: string;
  dropProxy?: () => Promise<void>;
  restoreProxy?: () => Promise<void>;
  close(): Promise<void>;
}

export interface FixtureOptions {
  /// first request 回一次工具调用（read hello.txt），拿到工具结果后再回最终文本。
  toolTurn?: boolean;
  /// 每次模型回放延迟毫秒（T007.5 停止用例要在运行中按停止/Esc）。
  slowMs?: number;
  /// first request 回一次 ask_user_question（两题：一题选项、一题自己写）。
  askTurn?: boolean;
  /// 模型失败脚本：前两次 500 后成功 / 一直 500 / 401 凭证错误。
  failure?: 'retry-then-succeed' | 'model' | 'credential';
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
}

/** 接缝 5：假 DeepSeek 脚本回放 + 真 aios-dev serve（页面也由它同源托管）。 */
export async function startKernelFixture(
  options: FixtureOptions = {},
): Promise<KernelFixture> {
  let askCount = 0;
  let modelCalls = 0;
  const fake: Server = createServer((req, res) => {
    if (req.method === 'POST' && req.url === '/v1/messages') {
      let body = '';
      req.on('data', (chunk: Buffer) => {
        body += chunk.toString();
      });
      req.on('end', () => {
        if (options.failure === 'credential') {
          res.writeHead(401, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ type: 'error', error: { type: 'authentication_error', message: 'invalid api key' } }));
          return;
        }
        modelCalls += 1;
        if (options.failure === 'model' || (options.failure === 'retry-then-succeed' && modelCalls <= 3)) {
          res.writeHead(500, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ type: 'error', error: { type: 'server_error', message: 'overloaded' } }));
          return;
        }
        res.writeHead(200, { 'content-type': 'text/event-stream' });
        const finalReply = options.highUsage ? HIGH_USAGE_FINAL_TEXT_SSE : FINAL_TEXT_SSE;
        const toolResult = body.includes('tool_result');
        let reply = finalReply;
        if (options.askTurn) {
          // 奇数次请求 = 每轮首个请求：回 ask；偶数次 = 工具结果之后：回最终文本。
          reply = askCount % 2 === 0 ? ASK_CALL_SSE : finalReply;
          askCount += 1;
        } else if (options.toolTurn && !toolResult) {
          reply = TOOL_CALL_SSE;
        }
        if (options.slowMs) {
          setTimeout(() => res.end(reply), options.slowMs);
        } else {
          res.end(reply);
        }
      });
      return;
    }
    res.writeHead(404);
    res.end();
  });
  await new Promise<void>((resolve) => fake.listen(0, '127.0.0.1', resolve));
  const address = fake.address();
  const fakePort = typeof address === 'object' && address !== null ? address.port : 0;
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
  const serveArgs = ['serve', '--data-dir', dataDir, '--cwd', dataDir, '--web-dist', path.join(repoRoot, 'web/dist')];
  if (options.contextWindow) {
    serveArgs.push('--context-window', String(options.contextWindow));
  }
  const serviceEnv = {
    ...process.env,
    DEEPSEEK_API_KEY: 'test-key',
    DEEPSEEK_BASE_URL: 'http://127.0.0.1:' + String(fakePort),
  };
  let holder: ReturnType<typeof spawn> | null = null;
  if (options.busyRoot) {
    holder = spawn(bin, ['serve', '--data-dir', dataDir, '--cwd', dataDir], {
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
  const child = spawn(bin, serveArgs, {
    env: serviceEnv,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  const parsed = await new Promise<{ baseUrl: string; token: string }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`serve 启动超时: ${output}`)), 30_000);
    child.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\?token=([0-9a-f-]+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ baseUrl: `http://127.0.0.1:${match[1]}`, token: match[2] });
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      reject(new Error(`serve 退出（${code}）: ${output}`));
    });
  });
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
    dataDir,
    dropProxy,
    restoreProxy,
    close: async () => {
      await dropProxy();
      holder?.kill('SIGTERM');
      child.kill('SIGTERM');
      fake.close();
      await rm(dataDir, { recursive: true, force: true });
    },
  };
}


/** T007.11 真 key DoD：真 DeepSeek + /tmp/aios-t007-dod/work；无 key 时调用方 skip。 */
export async function startRealKernelFixture(): Promise<KernelFixture> {
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
  const child = spawn(
    bin,
    ['serve', '--data-dir', dataDir, '--cwd', workDir, '--web-dist', path.join(repoRoot, 'web/dist')],
    { env: process.env, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let output = '';
  const parsed = await new Promise<{ baseUrl: string; token: string }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('真 key serve 启动超时: ' + output)), 60_000);
    child.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\?token=([0-9a-f-]+)/);
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
  return {
    ...parsed,
    dataDir,
    dropProxy: async () => {},
    restoreProxy: async () => {},
    close: async () => {
      child.kill('SIGTERM');
      await rm(root, { recursive: true, force: true });
    },
  };
}

