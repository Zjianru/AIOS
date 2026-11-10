/**
 * 任务结果卡：relay 消息（Kind 会话里 request id 以 `task-report:` 开头的 user/message）→ 卡片信封。
 *
 * relay 正文是程序写的 JSON（任务、来源、状态、摘要、问题）。页面只认前缀和 JSON 的形状：
 * 解析不出来就退回普通文本气泡。状态文字由程序按 state + origin 给固定词，卡片不读摘要内容
 * 去改状态、文字或按钮（D73）；任何时候都不写「完成」（ready 只说「有结果」）。
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

export const TASK_REPORT_STATES = ['ready', 'blocked', 'failed', 'waiting'] as const;
export type TaskReportState = (typeof TASK_REPORT_STATES)[number];

export const TASK_REPORT_ORIGINS = ['package', 'agent', 'executor'] as const;
export type TaskReportOrigin = (typeof TASK_REPORT_ORIGINS)[number];

/** relay 解出来的事实：程序给的状态和文字都从这里来。 */
export interface TaskResultFact {
  /** relay 的 request id（也是卡片的 id，日志里稳定）。 */
  id: string;
  taskId: string;
  /** 全文；卡上按标签槽截到 20 字，全文留给任务对话。 */
  title: string;
  state: TaskReportState;
  origin: TaskReportOrigin;
  summary: string;
  question?: string;
  /** relay 摘要被截到 600 字时，原文的总字数。 */
  chars?: number;
  /** 事件时间（秒）；卡上显示到达时间。 */
  at: number;
}

export interface TaskResultSlots {
  /** 打开任务对话要用的任务 id（来自 relay 的 request id 和 JSON，不是模型文字槽）。 */
  taskId: string;
  title: LabelSlot;
  /** 固定状态文字（有结果 / 没开跑 / 卡住了 / 失败 / 等报告）。 */
  status: LabelSlot;
  /** 程序用的状态键，视图按它选颜色和图标。 */
  state: TaskReportState;
  summary: BodySlot;
  question?: BodySlot;
  at: number;
}

export const taskResultSlotsSchema = z.object({
  taskId: textSlot,
  title: labelSlot(''),
  status: textSlot,
  state: z.enum(TASK_REPORT_STATES).catch('waiting'),
  summary: bodySlot,
  question: optionalBodySlot,
  at: z.number().int().catch(0),
});

const TASK_RESULT_FALLBACK_SLOTS: TaskResultSlots = {
  taskId: '',
  title: '',
  status: '等报告',
  state: 'waiting',
  summary: EMPTY_BODY,
  at: 0,
};

/** relay 的 state + origin → 卡上的固定状态文字；不读摘要内容（D73）。 */
export function taskStatusOf(state: TaskReportState, origin: TaskReportOrigin): string {
  switch (state) {
    case 'ready':
      return '有结果';
    case 'failed':
      return '失败';
    case 'waiting':
      return '等报告';
    case 'blocked':
      // 包缺项是没开跑；其余 blocked（agent 卡住、执行者派发失败）都算卡住了。
      return origin === 'package' ? '没开跑' : '卡住了';
  }
}

/** 卡片信封的状态：只影响外框颜色，按程序事实给。 */
function envelopeStateOf(state: TaskReportState): 'pending' | 'running' | 'done' | 'failed' {
  switch (state) {
    case 'ready':
      return 'done';
    case 'failed':
      return 'failed';
    case 'waiting':
      return 'running';
    case 'blocked':
      return 'pending';
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isState(value: unknown): value is TaskReportState {
  return TASK_REPORT_STATES.includes(value as TaskReportState);
}

function isOrigin(value: unknown): value is TaskReportOrigin {
  return TASK_REPORT_ORIGINS.includes(value as TaskReportOrigin);
}

/** 事实 → 卡片信封；形状不对返回 null（调用方退回文本），不抛异常。 */
export function taskResultCardOf(
  fact: TaskResultFact | null | undefined,
): CardEnvelope<TaskResultSlots> | null {
  if (!fact || typeof fact !== 'object') return null;
  if (!fact.id || !isState(fact.state) || !isOrigin(fact.origin)) return null;
  const parsed = taskResultSlotsSchema.safeParse({
    taskId: fact.taskId,
    title: fact.title,
    status: taskStatusOf(fact.state, fact.origin),
    state: fact.state,
    summary: fact.summary,
    ...(fact.question === undefined ? {} : { question: fact.question }),
    at: fact.at,
  });
  if (!parsed.success) return null;
  return {
    id: fact.id,
    type: 'task-result',
    role: 'state',
    state: envelopeStateOf(fact.state),
    slots: parsed.data,
  };
}

/** 从一条 user/message 认 relay：request id 前缀 + JSON 形状；任一步不对都返回 null。 */
export function parseTaskRelay(message: unknown, at: number): TaskResultFact | null {
  if (!isRecord(message)) return null;
  const source = message.source;
  if (!isRecord(source) || source.kind !== 'user') return null;
  const rpcId = source.rpcId;
  if (typeof rpcId !== 'string' || !rpcId.startsWith(TASK_REPORT_PREFIX)) return null;
  // request id 的形状是 task-report:<taskId>:<reportId>，两段都不能空。
  const parts = rpcId.slice(TASK_REPORT_PREFIX.length).split(':');
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
  };
}

export const taskResultCard: CardDefinition<TaskResultFact, TaskResultSlots> = {
  schema: taskResultSlotsSchema,
  from: (fact) => taskResultCardOf(fact),
  fallback: TASK_RESULT_FALLBACK_SLOTS,
};
