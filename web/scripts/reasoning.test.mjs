// T013 验收 F5：解析会话日志里的 reasoning（真夹具 real.jsonl 第一轮 + 空输入边界）。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { reasoningTextOf, reasoningTextOfStream } from '../src/pages/kind/reasoning.ts';
import { reasoningSpanMs, thinkingSeconds } from '../src/data/turnUsage.ts';

const events = readFileSync(new URL('../e2e/fixtures/real.jsonl', import.meta.url), 'utf8')
  .trim()
  .split('\n')
  .map((line) => JSON.parse(line));
const firstTurn = events.filter((event) => event.type === 'assistant/message' && event.data?.turn === 1);
assert.equal(firstTurn.length, 2, 'real.jsonl 第一轮有两条 assistant/message');

// 第一条是 reasoning 块 + tool-call；第二条是空 reasoning 块 + text。
const full = firstTurn.map((event) => reasoningTextOf(event.data.message)).filter(Boolean).join('\n\n');
assert.match(full, /Simple task\. Let me read the file\./);
assert.match(full, /No skill needed for this trivial task\./);
assert.equal(reasoningTextOf(firstTurn[1].data.message), '', '空 reasoning 块不算');

// 同一条事件的 stream 里也有 reasoning-chunks（assistant/attempt 只有 stream 时用）。
const fromStream = reasoningTextOfStream(firstTurn[0].data.stream);
assert.match(fromStream, /No skill needed for this trivial task\./);
assert.equal(reasoningTextOfStream([]), '');
assert.match(reasoningTextOfStream([{ type: 'text-chunks', texts: ['不算'] }]), /^$/);

// 时长/秒数：跨度为正，秒数至少 1。
const span = reasoningSpanMs(firstTurn[0].data.stream);
assert.ok(span > 0, 'reasoning-chunks 的 time0+dt 能算跨度');
assert.equal(thinkingSeconds(span), Math.max(1, Math.ceil(span / 1000)));

console.log('reasoning 单测通过（real.jsonl 第一轮：全文/stream/时长）');
