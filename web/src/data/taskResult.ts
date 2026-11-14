/**
 * 任务结果卡：relay 消息（Kind 会话里 request id 以 `task-report:` 或 `task-stop:` 开头的
 * user/message）→ 卡片信封。停止 relay 走同一种 JSON 形状，只是 request id 前缀和 state 不同。
 *
 * relay 正文是程序写的 JSON（任务、来源、状态、摘要、问题）。页面只认前缀和 JSON 的形状：
 * 解析不出来就退回普通文本气泡。状态文字由程序按 state + origin 给固定词（写任务当前
 * 要求版本的 ready 给「待你核」，Task 阶段仍是「待检查」），卡片不读摘要内容去改状态、
 * 文字或按钮（D73）；任何时候都不写「完成」。
 */

import { z } from 'zod';
import type {
  BodySlot,
  CardDefinition,
  CardEnvelope,
  LabelSlot,
} from '../lib/cardEnvelope.ts';
import { EMPTY_BODY, bodySlot, labelSlot, optionalBodySlot, textSlot } from '../lib/cardEnvelope.ts';

/** relay 消息的 request id 前缀（`task-report:<taskId>:<reportId>`）。 */
export const TASK_REPORT_PREFIX = 'task-report:';
/** 停止 relay 的 request id 前缀（`task-stop:<taskId>:<attemptId>`）。 */
export const TASK_STOP_PREFIX = 'task-stop:';
/** 启动扫描发给 Kind 的重启汇报的前缀（`restart:<bootId>`）。 */
export const TASK_RESTART_PREFIX = 'restart:';

export const TASK_REPORT_STATES = [
  'ready',
  'blocked',
  'failed',
  'waiting',
  'stopped',
  // 启动扫描打断、Kind 还没核实/用户还没确认：卡片和任务对话都用固定词「被打断了」。
  'interrupted',
] as const;
export type TaskReportState = (typeof TASK_REPORT_STATES)[number];

/**
 * 派发卡的状态键（不是 relay 的报告状态）：拿到 task id 前「派出中」；之后「进行中」；
 * T041 起「等待中」也走这里（task-read 说任务不在跑、没停、没被打断、还没交报告）。
 */
export const TASK_DISPATCH_STATES = ['dispatching', 'dispatched', 'pending'] as const;
export type TaskDispatchState = (typeof TASK_DISPATCH_STATES)[number];

/** T053：结果卡上的里程碑事实。 */
export interface GateCardFact {
  milestone: true;
  /** pending_verdict：等用户判；waiting：被把关任务等核查。 */
  state: 'pending_verdict' | 'waiting';
  id?: string;
  /** waiting 时是门的标题。 */
  title?: string;
  /** pending_verdict 时是被核查的任务。 */
  guarded?: { id: string; title: string };
  /** pending_verdict 时是这次核查的候选。 */
  candidate?: { report_id: string; requirement_version: number; ordinal: number };
  /** waiting 时：这不是门第一次核，自动行说「核查自动重跑」。 */
  recheck?: boolean;
}

/** 卡上的状态：报告状态 + 写任务当前版本 ready 的「待你核」（卡状态，不是 Task 阶段）+ 派发卡两态。 */
export const TASK_CARD_STATES = [...TASK_REPORT_STATES, 'review', ...TASK_DISPATCH_STATES] as const;
export type TaskCardState = (typeof TASK_CARD_STATES)[number];

export const TASK_REPORT_ORIGINS = ['package', 'agent', 'executor'] as const;
export type TaskReportOrigin = (typeof TASK_REPORT_ORIGINS)[number];

/** relay 解出来的事实：程序给的状态和文字都从这里来。 */
export interface TaskResultFact {
  /** relay 的 request id（也是卡片的 id，日志里稳定）。 */
  id: string;
  taskId: string;
  /** 全文；卡上按标签槽截到 20 字，全文留给任务对话。 */
  title: string;
  /** relay 报告状态；派发卡在拿到 task id 前是 dispatching、之后是 dispatched。 */
  state: TaskReportState | TaskDispatchState;
  origin: TaskReportOrigin;
  summary: string;
  question?: string;
  /** relay 摘要被截到 600 字时，原文的总字数。 */
  chars?: number;
  /** 事件时间（秒）；卡上显示到达时间。 */
  at: number;
  /** 写任务的写入范围（相对任务 cwd 显示）；没有 = 只读调查任务。 */
  writeScope?: string;
  /** 写任务改过的文件（相对执行 cwd，服务去重后给）。 */
  changed?: string[];
  /** 服务给的固定一行「改了 N 个文件（写入范围 X）」。 */
  changedLine?: string;
  /** 开了 bash 的写任务，卡上的固定说明（服务给原文）。 */
  bashNote?: string;
  /** 工作树根路径（开了工作树的任务）。 */
  worktreePath?: string;
  /** T052：这份 ready 报告自动放出的下游标题（卡上固定一行「已自动开跑」）。 */
  autoStarted?: string[];
  /** 卡状态：写任务当前要求版本的 ready 报告给 review（卡上「待你核」）。 */
  cardState?: 'review';
  /** T053：里程碑门卡的里程碑事实（门卡 = pending_verdict，被把关卡 = waiting）。 */
  gate?: GateCardFact;
  /** T053：这张结果卡对应的报告 id（服务 relay 里的程序事实；旧日志可缺）。 */
  reportId?: string;
  /** T053：被把关任务卡的门判定结论（只匹配这张卡那份报告；刷新可重建）。 */
  gateVerdict?: 'pass' | 'needs_fix';
  /** T053：上游有新可放行结果，这张结果卡待按新结果核对（D95）。 */
  upstreamChanged?: boolean;
  /** 接受/判定决定（card/decision 事件派生）；还没点过是 undefined。 */
  decision?: {
    phase: 'confirmed' | 'dismissed' | 'refused' | 'failed';
    data?: Record<string, unknown>;
    at?: number;
  };
  /** 这张卡最初是 task_accept 派生的派发卡（跨轮/跨页折卡时认它）。 */
  fromDispatch?: boolean;
}

export interface TaskResultSlots {
  /** 打开任务对话要用的任务 id（来自 relay 的 request id 和 JSON，不是模型文字槽）。 */
  taskId: string;
  title: LabelSlot;
  /** 固定状态文字（有结果 / 待你核 / 没开跑 / 卡住了 / 失败 / 等报告）。 */
  status: LabelSlot;
  /** 程序用的状态键，视图按它选颜色和图标。 */
  state: TaskCardState;
  summary: BodySlot;
  question?: BodySlot;
  at: number;
  /** 写任务：小标题「写任务」、清单和固定行都看它；调查任务是 false。 */
  write: boolean;
  /** 写入范围（服务显示用，相对任务 cwd）；只读任务为空串。 */
  writeScope: string;
  /** 改过的文件（服务去重后给）；只读任务为空。 */
  changed: string[];
  /** 固定一行；只读任务为空串。 */
  changedLine: string;
  /** 开了 bash 的固定说明；没开为空串。 */
  bashNote: string;
  /** 工作树路径；没开为空串。 */
  worktreePath: string;
  /** T052：自动开跑的下游标题；没有为空数组。 */
  autoStarted: string[];
  /** T053：里程碑事实；不是门相关卡为空。 */
  gate?: GateCardFact;
  /** T053：门判定结论（从 card/decision 回填）。 */
  verdict?: 'pass' | 'needs_fix';
  /** 用户原话（需修正时）。 */
  verdictNote?: string;
  /** 判定回执时间（秒）。 */
  verdictAt?: number;
  /** T053：判定时发现已经变了（refused，没记判定）。 */
  verdictChanged: boolean;
  /** T053：上游有新结果，待按新结果核对。 */
  upstreamChanged: boolean;
  /** 这轮结果已经被用户接受（card/decision 的 confirmed）。 */
  accepted: boolean;
}

const gateCardSchema = z.object({
  milestone: z.literal(true),
  state: z.enum(['pending_verdict', 'waiting']),
  id: z.string().optional(),
  title: z.string().optional(),
  guarded: z.object({ id: z.string(), title: z.string() }).optional(),
  candidate: z
    .object({ report_id: z.string(), requirement_version: z.number(), ordinal: z.number() })
    .optional(),
  recheck: z.boolean().optional(),
});

export const taskResultSlotsSchema = z.object({
  taskId: textSlot,
  title: labelSlot(''),
  status: textSlot,
  state: z.enum(TASK_CARD_STATES).catch('waiting'),
  summary: bodySlot,
  question: optionalBodySlot,
  at: z.number().int().catch(0),
  write: z.boolean().catch(false),
  writeScope: textSlot,
  changed: z.array(z.string()).catch([]),
  changedLine: textSlot,
  bashNote: textSlot,
  worktreePath: textSlot,
  autoStarted: z.array(z.string()).catch([]),
  gate: gateCardSchema.optional(),
  verdict: z.enum(['pass', 'needs_fix']).optional(),
  verdictNote: textSlot.optional(),
  verdictAt: z.number().optional(),
  verdictChanged: z.boolean().catch(false),
  upstreamChanged: z.boolean().catch(false),
  accepted: z.boolean().catch(false),
});

const TASK_RESULT_FALLBACK_SLOTS: TaskResultSlots = {
  taskId: '',
  title: '',
  status: '等报告',
  state: 'waiting',
  summary: EMPTY_BODY,
  at: 0,
  write: false,
  writeScope: '',
  changed: [],
  changedLine: '',
  bashNote: '',
  worktreePath: '',
  autoStarted: [],
  verdictChanged: false,
  upstreamChanged: false,
  accepted: false,
};

/** relay 的 state + origin → 卡上的固定状态文字；不读摘要内容（D73）。 */
export function taskStatusOf(
  state: TaskReportState | TaskDispatchState,
  origin: TaskReportOrigin,
  cardState?: 'review',
  accepted?: boolean,
): string {
  // 写任务当前要求版本的 ready：卡状态是「待你核」，Task 阶段仍是「待检查」。
  // 用户接受以后状态词换成「已接受」（回执从 card/decision 回填）。
  if (cardState === 'review') return accepted ? '已接受' : '待你核';
  if (state === 'dispatching') return '派出中';
  if (state === 'dispatched') return '进行中';
  if (state === 'pending') return '等待中';
  switch (state) {
    case 'ready':
      return '有结果';
    case 'failed':
      return '失败';
    case 'waiting':
      return '等报告';
    case 'stopped':
      // 停止是程序给的固定结果，和 stopping 的那一轮无关（D62、D68）。
      return '已停止';
    case 'interrupted':
      // 重启打断也是程序给的固定结果；不自动重跑（D84）。
      return '被打断了';
    case 'blocked':
      // 包缺项是没开跑；其余 blocked（agent 卡住、执行者派发失败）都算卡住了。
      return origin === 'package' ? '没开跑' : '卡住了';
  }
}

/** 卡片信封的状态：只影响外框颜色，按程序事实给。 */
function envelopeStateOf(state: TaskCardState): 'pending' | 'running' | 'done' | 'failed' {
  switch (state) {
    case 'dispatching':
    case 'dispatched':
      // 派发卡一出现就是活的（任务是程序接下的，不是等报告才有）。
      return 'running';
    case 'pending':
      // 派出去了但任务库说不在跑（停下这一轮等）：仍是活的，只是没在跑。
      return 'pending';
    case 'ready':
    case 'review':
      return 'done';
    case 'failed':
      return 'failed';
    case 'waiting':
      return 'running';
    case 'stopped':
      return 'pending';
    case 'interrupted':
      return 'pending';
    case 'blocked':
      return 'pending';
  }
}

/** 卡上固定一行：改了 N 个文件（写入范围 X）。服务没给时按同一句话拼。 */
function changedLineOf(count: number, scope: string): string {
  return `改了 ${count} 个文件（写入范围 ${scope}）`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isState(value: unknown): value is TaskReportState {
  return TASK_REPORT_STATES.includes(value as TaskReportState);
}

/** 卡状态的表：relay 只认报告状态，卡片事实还认派发卡两态。 */
function isCardState(value: unknown): value is TaskCardState {
  return TASK_CARD_STATES.includes(value as TaskCardState);
}

function isOrigin(value: unknown): value is TaskReportOrigin {
  return TASK_REPORT_ORIGINS.includes(value as TaskReportOrigin);
}

/** 事实 → 卡片信封；形状不对返回 null（调用方退回文本），不抛异常。 */
export function taskResultCardOf(
  fact: TaskResultFact | null | undefined,
): CardEnvelope<TaskResultSlots> | null {
  if (!fact || typeof fact !== 'object') return null;
  if (!fact.id || !isCardState(fact.state) || !isOrigin(fact.origin)) return null;
  const writeScope = fact.writeScope ?? '';
  const changed = fact.changed ?? [];
  const cardState = fact.cardState === 'review' ? 'review' : undefined;
  const state = cardState ?? fact.state;
  const decisionData = fact.decision?.data;
  const verdict =
    decisionData?.conclusion === 'pass' || decisionData?.conclusion === 'needs_fix'
      ? decisionData.conclusion
      : undefined;
  const verdictNote =
    typeof decisionData?.note === 'string' && decisionData.note !== ''
      ? decisionData.note
      : undefined;
  const gate = fact.gate;
  let status = taskStatusOf(
    fact.state,
    fact.origin,
    cardState,
    fact.decision?.phase === 'confirmed',
  );
  if (fact.decision?.phase === 'refused') {
    status = '已经变了';
  } else if (gate?.state === 'pending_verdict' && verdict === undefined) {
    status = '等你判';
  } else if (verdict === 'pass') {
    status = '你判了通过';
  } else if (verdict === 'needs_fix') {
    status = '你判了需修正';
  } else if (gate?.state === 'waiting' && fact.gateVerdict) {
    // T053 状态词表：判后「任务要求已满足」或「需修正」。
    status = fact.gateVerdict === 'pass' ? '任务要求已满足' : '需修正';
  } else if (gate?.state === 'waiting') {
    status = '等核查';
  }
  const parsed = taskResultSlotsSchema.safeParse({
    taskId: fact.taskId,
    title: fact.title,
    status,
    state,
    summary: fact.summary,
    ...(fact.question === undefined ? {} : { question: fact.question }),
    at: fact.at,
    write: writeScope !== '',
    writeScope,
    changed,
    // 服务给的固定行优先；旧数据没有就按同一句话拼。
    changedLine:
      writeScope === '' ? '' : (fact.changedLine ?? changedLineOf(changed.length, writeScope)),
    bashNote: fact.bashNote ?? '',
    worktreePath: fact.worktreePath ?? '',
    autoStarted: fact.autoStarted ?? [],
    ...(gate === undefined ? {} : { gate }),
    ...(verdict === undefined ? {} : { verdict }),
    ...(verdictNote === undefined ? {} : { verdictNote }),
    ...(fact.decision?.at === undefined ? {} : { verdictAt: fact.decision.at }),
    verdictChanged: fact.decision?.phase === 'refused',
    upstreamChanged: fact.upstreamChanged === true,
    accepted:
      fact.decision?.phase === 'confirmed' &&
      (verdict === undefined || verdict === 'pass') &&
      gate?.state !== 'pending_verdict',
  });
  if (!parsed.success) return null;
  return {
    id: fact.id,
    type: 'task-result',
    role: 'state',
    state: envelopeStateOf(state),
    slots: parsed.data,
  };
}

/** 从一条 user/message 认 relay：request id 前缀 + JSON 形状；任一步不对都返回 null。 */
export function parseTaskRelay(message: unknown, at: number): TaskResultFact | null {
  if (!isRecord(message)) return null;
  const source = message.source;
  if (!isRecord(source) || source.kind !== 'user') return null;
  const rpcId = source.rpcId;
  if (typeof rpcId !== 'string') return null;
  const prefix = rpcId.startsWith(TASK_REPORT_PREFIX)
    ? TASK_REPORT_PREFIX
    : rpcId.startsWith(TASK_STOP_PREFIX)
      ? TASK_STOP_PREFIX
      : null;
  if (prefix === null) return null;
  // 两种前缀后面都是 <taskId>:<reportId|attemptId>，两段都不能空。
  const parts = rpcId.slice(prefix.length).split(':');
  if (parts.length < 2 || parts[0] === '' || parts[1] === '') return null;

  const content = message.content;
  if (!Array.isArray(content)) return null;
  const text = content
    .map((block) =>
      isRecord(block) && block.type === 'text' && typeof block.text === 'string' ? block.text : '',
    )
    .join('');
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isRecord(body) || body.kind !== 'task_report') return null;
  const task = body.task;
  if (!isRecord(task)) return null;
  const taskId = task.id;
  const title = task.title;
  if (typeof taskId !== 'string' || taskId === '') return null;
  if (typeof title !== 'string' || title === '') return null;
  const state = body.state;
  const origin = body.origin;
  if (!isState(state) || !isOrigin(origin)) return null;
  const summary = body.summary;
  if (typeof summary !== 'string') return null;
  const question = body.question;
  if (question !== undefined && typeof question !== 'string') return null;
  const chars = body.chars;
  if (chars !== undefined && typeof chars !== 'number') return null;
  // 写任务的卡数据（T030）：服务给什么画什么；坏字段逐项当没有，不整卡退回文本。
  const writeScope =
    typeof body.write_scope === 'string' && body.write_scope !== '' ? body.write_scope : undefined;
  const changed = Array.isArray(body.changed)
    ? body.changed.filter((path): path is string => typeof path === 'string')
    : undefined;
  const changedLine = typeof body.changed_line === 'string' ? body.changed_line : undefined;
  const bashNote = typeof body.bash_note === 'string' ? body.bash_note : undefined;
  const worktreePath =
    typeof body.worktree_path === 'string' ? body.worktree_path : undefined;
  // T052：程序事实「已自动开跑：<下游标题>」；坏字段当没有。
  const autoStarted = Array.isArray(body.auto_started)
    ? body.auto_started.filter(
        (item): item is string => typeof item === 'string' && item !== '',
      )
    : undefined;
  const cardState = body.card_state === 'review' ? ('review' as const) : undefined;
  const gate = parseGateCard(body.gate);
  const reportId = typeof body.report_id === 'string' ? body.report_id : undefined;

  return {
    id: rpcId,
    taskId,
    title,
    state,
    origin,
    summary,
    ...(question === undefined ? {} : { question }),
    ...(chars === undefined ? {} : { chars }),
    at: Number.isFinite(at) ? Math.floor(at) : 0,
    ...(writeScope === undefined ? {} : { writeScope }),
    ...(changed === undefined ? {} : { changed }),
    ...(changedLine === undefined ? {} : { changedLine }),
    ...(bashNote === undefined ? {} : { bashNote }),
    ...(worktreePath === undefined ? {} : { worktreePath }),
    ...(autoStarted === undefined ? {} : { autoStarted }),
    ...(gate === undefined ? {} : { gate }),
    ...(reportId === undefined ? {} : { reportId }),
    ...(cardState === undefined ? {} : { cardState }),
  };
}

/** T053：结果卡上的里程碑事实；形状不对当没有。 */
function parseGateCard(value: unknown): GateCardFact | undefined {
  if (!isRecord(value) || value.milestone !== true) return undefined;
  const state =
    value.state === 'pending_verdict' || value.state === 'waiting' ? value.state : undefined;
  if (!state) return undefined;
  const guarded = isRecord(value.guarded)
    ? { id: String(value.guarded.id ?? ''), title: String(value.guarded.title ?? '') }
    : undefined;
  const candidate = isRecord(value.candidate)
    ? {
        report_id: String(value.candidate.report_id ?? ''),
        requirement_version: Number(value.candidate.requirement_version ?? 0),
        ordinal: Number(value.candidate.ordinal ?? 0),
      }
    : undefined;
  return {
    milestone: true,
    state,
    ...(typeof value.id === 'string' ? { id: value.id } : {}),
    ...(typeof value.title === 'string' ? { title: value.title } : {}),
    ...(guarded ? { guarded } : {}),
    ...(candidate ? { candidate } : {}),
    ...(value.recheck === true ? { recheck: true } : {}),
  };
}

/** 重启汇报里一条被打断的任务（服务只给事实：标题、最后报到哪、第几次 Attempt）。 */
export interface RestartTaskFact {
  id: string;
  title: string;
  attempts: number;
  lastReport?: string;
}

/** 没有旧报告时卡上的固定摘要（程序事实，不是模型写的）。 */
export const INTERRUPTED_SUMMARY = 'AIOS 重启时这次 Attempt 还在跑，没交出结果。';

function restartTaskOf(value: unknown): RestartTaskFact | null {
  if (!isRecord(value)) return null;
  const id = value.id;
  const title = value.title;
  const attempts = value.attempts;
  const lastReport = value.last_report;
  if (typeof id !== 'string' || id === '') return null;
  if (typeof title !== 'string' || title === '') return null;
  if (typeof attempts !== 'number' || !Number.isFinite(attempts)) return null;
  if (lastReport !== undefined && typeof lastReport !== 'string') return null;
  return {
    id,
    title,
    attempts,
    ...(lastReport === undefined ? {} : { lastReport }),
  };
}

/**
 * 启动扫描发给 Kind 的 restart 消息 → 每个被打断的任务一张结果卡。
 *
 * 消息形状是 `{kind:"tasks_interrupted", tasks:[{id,title,attempts,last_report?}]}`；
 * 解析不出来返回空数组，调用方照普通用户消息显示（和坏 relay 一样，不吞原文）。
 */
export function parseRestartNotice(message: unknown, at: number): TaskResultFact[] {
  if (!isRecord(message)) return [];
  const source = message.source;
  if (!isRecord(source) || source.kind !== 'user') return [];
  const rpcId = source.rpcId;
  if (typeof rpcId !== 'string' || !rpcId.startsWith(TASK_RESTART_PREFIX)) return [];
  const content = message.content;
  if (!Array.isArray(content)) return [];
  const text = content
    .map((block) =>
      isRecord(block) && block.type === 'text' && typeof block.text === 'string' ? block.text : '',
    )
    .join('');
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return [];
  }
  if (!isRecord(body) || body.kind !== 'tasks_interrupted') return [];
  const tasks = body.tasks;
  if (!Array.isArray(tasks)) return [];
  const atSeconds = Number.isFinite(at) ? Math.floor(at) : 0;
  const facts: TaskResultFact[] = [];
  for (const value of tasks) {
    const task = restartTaskOf(value);
    if (!task) continue;
    facts.push({
      id: rpcId + ':' + task.id,
      taskId: task.id,
      title: task.title,
      state: 'interrupted',
      origin: 'executor',
      summary: task.lastReport ?? INTERRUPTED_SUMMARY,
      at: atSeconds,
    });
  }
  return facts;
}

/** 派发卡的 id 前缀（`task-dispatch:<callId>`）；relay 到达后同一张卡改用 relay 的 id。 */
export const TASK_DISPATCH_PREFIX = 'task-dispatch:';

/**
 * task_accept 的 tool/call 参数 → 派发卡事实：标题、写范围、要求都取自工具参数。
 * 任务 id 要等 tool/result 才拿得到，所以此时 state='dispatching'（派出中）。
 * 参数形状不对返回 null（调用方按普通工具调用显示，不吞错误）。
 */
export function taskDispatchFactOf(
  callId: string,
  args: Record<string, unknown>,
  at: number,
): TaskResultFact | null {
  const title = typeof args.title === 'string' ? args.title.trim() : '';
  if (!callId || !title) return null;
  const writeScope =
    typeof args.write_scope === 'string' && args.write_scope !== ''
      ? args.write_scope
      : undefined;
  const requirement = typeof args.requirement === 'string' ? args.requirement : '';
  return {
    id: TASK_DISPATCH_PREFIX + callId,
    taskId: '',
    title,
    state: 'dispatching',
    origin: 'executor',
    summary: requirement,
    at: Number.isFinite(at) ? Math.floor(at) : 0,
    ...(writeScope === undefined ? {} : { writeScope }),
    fromDispatch: true,
  };
}

/** task_accept 的 tool/result 文本里取任务 id：服务回执是「已派出任务 <id>。…」。 */
export function dispatchedTaskIdOf(output: string): string | null {
  const match = /已派出任务\s+([0-9a-zA-Z_-]+)/.exec(output);
  return match ? match[1] : null;
}

export const taskResultCard: CardDefinition<TaskResultFact, TaskResultSlots> = {
  schema: taskResultSlotsSchema,
  from: (fact) => taskResultCardOf(fact),
  fallback: TASK_RESULT_FALLBACK_SLOTS,
};
