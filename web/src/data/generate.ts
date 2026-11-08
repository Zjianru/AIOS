/**
 * 用固定种子生成两条长记录：
 *   - Kind 的主对话：2000 轮；
 *   - 一件任务的运行记录：520 轮（第 2 期的任务详情用）。
 *
 * 生成方式是「按 index 算」，不是「一次算完」：第 i 轮永远由 (SEED, i) 决定，
 * 所以 2000 轮的体积不进内存、不受加载顺序影响，翻到哪页算哪页。
 * 同一个 index 每次算出来一模一样（需求 §二）。
 */

import {
  CODE_BLOCK, LONG_MD, MARKDOWN_TABLE, SHORT_ANSWER, THINKING_FULL, THINKING_SUMMARY, TOOL_CALLS,
  TOOL_STEPS, USER_SAY, md,
} from './content';
import { artifacts, kindLog, rootTaskIds, tasks } from './fixtures/workspace';
import { hashSeed, intBetween, mulberry32, pick, pickN, seededAt } from './rng';
import type {
  QuoteKind, TextElement, Thread, ThreadItem, Turn, TurnStatus,
} from './types';

export const KIND_THREAD_ID = 'thread_kind_main';
export const KIND_TURN_COUNT = 2000;
export const TASK_RUN_THREAD_ID = 'thread_run_root';
export const TASK_RUN_TURN_COUNT = 520;

const KIND_SEED = hashSeed('aios-v14/kind/2000');
const RUN_SEED = hashSeed('aios-v14/task-run/520');
/** 固定基准时间：2026-10-27 14:05（+08:00）。时间戳固定，截图和测速才可重复。 */
const KIND_BASE_TS = Math.floor(Date.UTC(2026, 9, 27, 6, 5, 0) / 1000);
const RUN_BASE_TS = Math.floor(Date.UTC(2026, 9, 27, 4, 20, 0) / 1000);

export interface TurnSource {
  thread: Thread;
  totalTurns: number;
  /** 第 index 轮的开始时间（unix 秒）。 */
  startedAt(index: number): number;
  /** 第 index 轮的内容。同一个 index 每次一样。 */
  turnAt(index: number): Turn;
}

/* ───────────────────────── 时间轴 ───────────────────────── */

function buildStarts(count: number, seed: number, baseTs: number, longGapChance: number): number[] {
  const rnd = mulberry32(seed ^ 0x9e3779b9);
  const out = new Array<number>(count);
  let t = baseTs;
  out[count - 1] = t;
  for (let i = count - 2; i >= 0; i--) {
    const r = rnd();
    const gap =
      r < longGapChance ? intBetween(rnd, 3 * 3600, 9 * 3600)
      : r < longGapChance + 0.44 ? intBetween(rnd, 15, 180)
      : intBetween(rnd, 180, 1800);
    t -= gap;
    out[i] = t;
  }
  return out;
}

const kindStarts = buildStarts(KIND_TURN_COUNT, KIND_SEED, KIND_BASE_TS, 0.06);
const runStarts = buildStarts(TASK_RUN_TURN_COUNT, RUN_SEED, RUN_BASE_TS, 0.02);

/* ───────────────────────── 引用片（v10-tables §1） ───────────────────────── */

interface QuoteDef {
  kind: QuoteKind;
  word: string;
  label: string;
  target: string;
}

const QUOTES: QuoteDef[] = [
  { kind: 'role', word: '@研究员', label: '@研究员', target: '#team' },
  { kind: 'role', word: '@评测员', label: '@评测员', target: '#team' },
  { kind: 'capability', word: '$网页搜索', label: '$网页搜索', target: '#ability' },
  { kind: 'capability', word: '$命令执行', label: '$命令执行', target: '#ability' },
  { kind: 'material', word: '▤ 评测方法 › 免费额度', label: '▤ 搜索引擎评测方法 › §2 成本', target: '#read:搜索引擎评测方法.pdf' },
  { kind: 'material', word: '▤ 我的简介 › 读者是谁', label: '▤ 我的简介.md › 读者是谁', target: '#read:我的简介.md' },
  { kind: 'artifact', word: '▤ 评测记录 v2', label: '▤ 评测记录.md v2', target: '#read:评测记录.md' },
  { kind: 'task', word: '◐ 多引擎实测', label: '◐ 多引擎实测', target: '#task:B2' },
  { kind: 'task', word: '◐ 可行性验证', label: '◐ 可行性验证', target: '#task:C2' },
  { kind: 'project', word: '■ web-search-pro 研究', label: '■ web-search-pro 研究', target: '#project:wsp' },
  { kind: 'project', word: '■ 日常', label: '■ 日常', target: '#project:daily' },
  { kind: 'pin', word: '📍1 外包协议草案 › 第 3 条', label: '📍1 外包协议草案 › 第 3 条', target: '#read:外包协议草案.docx' },
];

/** 在一句话末尾挂一个引用片，并把 span 记进 textElements。 */
function decorate(rnd: () => number, base: string): { text: string; elements: TextElement[] } {
  if (rnd() > 0.28) return { text: base, elements: [] };
  const q = pick(rnd, QUOTES);
  const text = rnd() > 0.5 ? base + ' ' + q.word : q.word + ' ' + base;
  const start = text.indexOf(q.word);
  return { text, elements: [{ start, end: start + q.word.length, kind: q.kind, label: q.label, target: q.target }] };
}

/* ───────────────────────── Kind 主对话 ───────────────────────── */

type Shape = 'quiet' | 'short' | 'long' | 'table' | 'code' | 'tool' | 'mixed';

const SHAPES: readonly [Shape, number][] = [
  ['quiet', 0.2],
  ['short', 0.16],
  ['long', 0.1],
  ['table', 0.08],
  ['code', 0.07],
  ['tool', 0.13],
  ['mixed', 0.1],
];

function pickShape(rnd: () => number): Shape {
  let r = rnd();
  for (const [shape, w] of SHAPES) {
    if (r < w) return shape;
    r -= w;
  }
  return 'quiet';
}

function itemsForShape(rnd: () => number, index: number, shape: Shape): ThreadItem[] {
  const items: ThreadItem[] = [];
  const say = decorate(rnd, pick(rnd, USER_SAY));
  items.push({
    type: 'userMessage',
    id: 'u_' + index,
    clientId: null,
    content: [{ type: 'text', text: say.text, textElements: say.elements }],
  });

  const withThinking = shape !== 'quiet' && rnd() < 0.55;
  if (withThinking) {
    items.push({
      type: 'reasoning',
      id: 'r_' + index,
      summary: pickN(rnd, THINKING_SUMMARY, intBetween(rnd, 1, 3)),
      content: [pick(rnd, THINKING_FULL)],
    });
  }

  let answer = pick(rnd, SHORT_ANSWER);
  if (shape === 'long') answer = md(pick(rnd, LONG_MD));
  if (shape === 'table') answer = '整理好了，直接看表：' + String.fromCharCode(10) + String.fromCharCode(10) + md(pick(rnd, MARKDOWN_TABLE));
  if (shape === 'code') answer = md(pick(rnd, CODE_BLOCK));
  if (shape === 'mixed') answer = md(pick(rnd, LONG_MD)) + String.fromCharCode(10) + String.fromCharCode(10) + md(pick(rnd, MARKDOWN_TABLE));

  const toolCount = shape === 'tool' ? intBetween(rnd, 1, 2) : shape === 'mixed' ? 1 : 0;
  for (let k = 0; k < toolCount; k++) {
    const step = pick(rnd, TOOL_STEPS);
    items.push({
      type: 'commandExecution',
      id: 'x_' + index + '_' + k,
      command: step.command,
      cwd: step.cwd,
      status: 'completed',
      aggregatedOutput: step.out,
      exitCode: step.exit,
      durationMs: step.ms,
    });
    const [name, arg] = pick(rnd, TOOL_CALLS);
    items.push({ type: 'mcpToolCall', id: 'm_' + index + '_' + k, server: 'aios', tool: name, status: 'completed', arguments: { arg }, durationMs: intBetween(rnd, 60, 1800) });
  }

  if (shape !== 'quiet' || rnd() < 0.5) {
    items.push({ type: 'agentMessage', id: 'a_' + index, text: answer, phase: 'final_answer' });
  }

  return items;
}

function turnStatus(index: number, total: number): TurnStatus {
  if (index === total - 1) return 'inProgress';
  if (index % 617 === 0) return 'interrupted';
  if (index % 431 === 0) return 'failed';
  return 'completed';
}

export function kindTurnAt(index: number): Turn {
  const rnd = seededAt(KIND_SEED, index);
  const ts = kindStarts[index];
  const shape = pickShape(rnd);
  const items = itemsForShape(rnd, index, shape);
  const status = turnStatus(index, KIND_TURN_COUNT);
  const dur = intBetween(rnd, 1200, 48000);
  return {
    id: 'turn_' + String(index).padStart(4, '0'),
    items,
    itemsView: 'full',
    status,
    error: status === 'failed' ? { message: '模型调用超时（原型里是写好的样例）' } : null,
    startedAt: ts,
    completedAt: status === 'inProgress' ? null : ts + Math.round(dur / 1000),
    durationMs: status === 'inProgress' ? null : dur,
  };
}

const kindThread: Thread = {
  id: KIND_THREAD_ID,
  preview: 'web-search-pro 还有没有价值；news-intelligence 结构说明；每日简报',
  model: 'DeepSeek V4 Pro',
  modelProvider: 'deepseek',
  cwd: '~/projects',
  createdAt: kindStarts[0],
  updatedAt: kindStarts[KIND_TURN_COUNT - 1],
  status: 'inProgress',
};

export const kindSource: TurnSource = {
  thread: kindThread,
  totalTurns: KIND_TURN_COUNT,
  startedAt: (i) => kindStarts[i],
  turnAt: kindTurnAt,
};

/* ───────────────────────── 一件任务的运行记录（第 2 期用） ───────────────────────── */

const RUN_EVENTS = [
  ['评测员', '模型请求', 'MiniMax M3.1 Flash · 输入 6.2k / 输出 1.1k'],
  ['engine-runner-1', '工具调用', 'web_search × 20 · 全部返回'],
  ['engine-runner-2', '工具调用', 'web_search × 20 · 2 次超时后重试成功'],
  ['评测员', '工具调用', 'web_search × 20（Brave）'],
  ['评测员', '文件修改', 'scripts/dedupe.py（+18）'],
  ['评测员', '提交候选', '评测记录.md · 候选 c2'],
  ['审查员', '审查结论', '需修正：结果未去重'],
  ['审查员', '审查结论', '通过'],
  ['Kind', '交回成果', '评测记录.md · c2'],
  ['Kind', '检查点', '上下文压缩成一张检查点卡'],
];

export function runTurnAt(index: number): Turn {
  const rnd = seededAt(RUN_SEED, index);
  const ts = runStarts[index];
  const [role, kind, what] = pick(rnd, RUN_EVENTS);
  const items: ThreadItem[] = [
    { type: 'agentMessage', id: 'ra_' + index, text: role + ' · ' + kind + ' · ' + what },
  ];
  if (rnd() < 0.35) {
    const step = pick(rnd, TOOL_STEPS);
    items.push({ type: 'commandExecution', id: 'rx_' + index, command: step.command, cwd: step.cwd, status: 'completed', aggregatedOutput: step.out, exitCode: step.exit, durationMs: step.ms });
  }
  const status = index === TASK_RUN_TURN_COUNT - 1 ? 'inProgress' : 'completed';
  return {
    id: 'run_' + String(index).padStart(4, '0'),
    items,
    itemsView: 'full',
    status,
    error: null,
    startedAt: ts,
    completedAt: status === 'inProgress' ? null : ts + 8,
    durationMs: status === 'inProgress' ? null : 8000,
  };
}

export const taskRunSource: TurnSource = {
  thread: {
    id: TASK_RUN_THREAD_ID,
    preview: '多引擎实测 · 运行记录',
    model: 'MiniMax M3.1 Flash',
    modelProvider: 'minimax',
    cwd: '~/projects',
    createdAt: runStarts[0],
    updatedAt: runStarts[TASK_RUN_TURN_COUNT - 1],
    status: 'completed',
  },
  totalTurns: TASK_RUN_TURN_COUNT,
  startedAt: (i) => runStarts[i],
  turnAt: runTurnAt,
};

/** Kind 的工作日志（§6.3）：右栏顶部的「Kind 最近做了什么」。 */
export function getKindLog() {
  return kindLog;
}

/** 「在做的事」右栏要的三个项目的线路与站。 */
export function inProgressProjects() {
  return rootTaskIds.map((id) => tasks[id]);
}

/** 出现的成果，第 2 期用。 */
export function artifactOf(name: string) {
  return artifacts[name] ?? null;
}
