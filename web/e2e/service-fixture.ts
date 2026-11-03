import { spawn } from 'node:child_process';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer, type Server } from 'node:http';
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
  close(): Promise<void>;
}

export interface FixtureOptions {
  /// first request 回一次工具调用（read hello.txt），拿到工具结果后再回最终文本。
  toolTurn?: boolean;
  /// 每次模型回放延迟毫秒（T007.5 停止用例要在运行中按停止/Esc）。
  slowMs?: number;
  /// first request 回一次 ask_user_question（两题：一题选项、一题自己写）。
  askTurn?: boolean;
}

/** 接缝 5：假 DeepSeek 脚本回放 + 真 aios-dev serve（页面也由它同源托管）。 */
export async function startKernelFixture(
  options: FixtureOptions = {},
): Promise<KernelFixture> {
  const fake: Server = createServer((req, res) => {
    if (req.method === 'POST' && req.url === '/v1/messages') {
      let body = '';
      req.on('data', (chunk: Buffer) => {
        body += chunk.toString();
      });
      req.on('end', () => {
        res.writeHead(200, { 'content-type': 'text/event-stream' });
        const toolResult = body.includes('tool_result');
        const firstReply = options.askTurn ? ASK_CALL_SSE : TOOL_CALL_SSE;
        const reply = (options.toolTurn || options.askTurn) && !toolResult ? firstReply : FINAL_TEXT_SSE;
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
  if (options.toolTurn) {
    await writeFile(path.join(dataDir, 'hello.txt'), 'hello from fixture\n');
  }
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const bin = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');
  const child = spawn(
    bin,
    ['serve', '--data-dir', dataDir, '--cwd', dataDir, '--web-dist', path.join(repoRoot, 'web/dist')],
    {
      env: {
        ...process.env,
        DEEPSEEK_API_KEY: 'test-key',
        DEEPSEEK_BASE_URL: `http://127.0.0.1:${fakePort}`,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  );
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
  return {
    ...parsed,
    close: async () => {
      child.kill('SIGTERM');
      fake.close();
      await rm(dataDir, { recursive: true, force: true });
    },
  };
}
