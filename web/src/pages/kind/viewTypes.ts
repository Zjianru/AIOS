//! 轮次视图的数据类型：TurnViewData 与过程中的卡片类型。

import type { SendAttachmentRef, AskAnswer, FailureCause } from '@/data/service';
import type { AskQuestion } from '@/data/questionView';
import type { TextElement, TurnStatus } from '@/data/types';
import type { ConfirmCardFact } from './cards/model';
import type { TaskResultFact } from '../../data/taskResult.ts';
import type { CanvasViewDoc } from './canvas.ts';

export interface ToolStep {
  id: string;
 /** ：工具名（read/write/bash/...）与解析后的参数。 */
  name: string;
  args: Record<string, unknown>;
  output: string;
  exitCode: number | null;
 /** 被信号杀掉时的信号名（bash 的 [killed by signal: X]）。 */
  signal?: string | null;
  durationMs: number | null;
 /** running/done/error/stopped（：停下的一轮把没结束的标 stopped）。 */
  state: 'running' | 'done' | 'error' | 'stopped';
 /** ：tool/result.meta（diffs/operation/stopped）。 */
  meta?: Record<string, unknown> | null;
 /** 调用事件的时间（算这一步用时）。 */
  startedAtMs?: number | null;
 /** 日志 seq：和工具前说明（D89 的 say 步）一起排先后。 */
  seq?: number;
  tool?: string;
}


/**
 * 模型提问：等你答 → 答了 / 不答 / 这一轮被停了。
 * ：带问题批次 id（ask_user_question 调用的 call id），卡片信封的 id 用它，不用数组下标。
 */
export type QuestionView =
  | { status: 'pending'; batchId: string; questions: AskQuestion[] }
  | { status: 'answered'; batchId: string; questions: AskQuestion[]; answers: AskAnswer[] }
  | { status: 'dismissed'; batchId: string; questions: AskQuestion[] }
  | { status: 'void'; batchId: string; questions: AskQuestion[] };


/** 压缩：在对话里是一条分隔线，不是一轮。 */
export interface CompactionView {
  auto: boolean;
 /** ：压缩前占窗口的百分比（窗口/前后任一拿不到就不显示）。 */
  before?: number;
 /** ：压缩后第一条响应占窗口的百分比。 */
  after?: number;
  summary: string;
}


/**
 * 你的消息（ F1）：正文只放打的字；每段粘贴、每个文件各是一张卡。
 * 块从日志的 user/message 来，实时和回放同一份。
 */
export interface TurnUserView {
  text: string;
  elements: TextElement[];
 /** paste: true 的文字块，一段一条（不拼进正文）。 */
  pastes: string[];
 /** file 块（服务发消息时写下的 id/名字/大小）。 */
  files: SendAttachmentRef[];
 /** ：这句话是画布里的按钮放进来的（日志块上的 canvas：id + 标题）；没有就是自己写的。 */
  canvas: { id: string; title: string } | null;
 /** ：这句话带着一张结果卡（「要求修改」放进输入框的卡）。 */
  resultCard: { taskId: string; title: string; version: number } | null;
 /** 服务端给这条消息的 rpcId（续改说明靠它认，不靠正文）。 */
  rpcId: string | null;
}


/** ：任务对话里说的一句话抄送到 Kind（task-cc:<inputId>）。 */
export interface TaskCcView {
  inputId: string;
  taskId: string;
  title: string;
  text: string;
 /** 原文字数（超过 600 时正文是截断的）。 */
  chars?: number;
}


/** 页面上显示名字用的表（纯派生不直接读 store，node 用例只给事件也能跑）。 */
export interface TurnLabels {
  connectionNames?: Record<string, string>;
  modelWindows?: Record<string, number>;
  modelNames?: Record<string, string>;
}


/** 工具前的说明（D89）：带工具调用的那条 assistant/message 的文字，按日志先后进工作过程。 */
export interface InterimSay {
  id: string;
  text: string;
 /** 日志 seq，和工具步一起排先后。 */
  seq: number;
}


export interface MemoCardInfo {
  id: string;
  kind: string;
  ref?: string;
}


export interface TurnViewData {
  id: string;
  startedAt: number | null;
  durationMs: number | null;
  status: TurnStatus;
  user: TurnUserView | null;
 /** 程序追加的记忆卡（许可/整理完/没跑成）。 */
  memoCards?: MemoCardInfo[];
 /** ：抄送行不是一轮回答；有它时只画虚线一行，不画用户气泡和 Kind 的回复。 */
  cc?: TaskCcView | null;
 /** 任务结果的 relay 消息：这一轮可能被 relay 叫起来；按到达顺序各出一张卡。 */
  taskResults: TaskResultFact[];
 /** D89：工具前说明，按日志先后插进工作过程，回答区不显示。 */
  says: InterimSay[];
 /** 思考全文（ 验收 F5）；摘要和秒数页面不用，日志里也没有（ C1）。 */
  reasoning: { full: string } | null;
  tools: ToolStep[];
  answer: { text: string; elements: TextElement[] } | null;
  thinkingSeconds: number;
  tokens: { input: number; output: number; cacheRead?: number; cacheWrite?: number };
 /** ：这一轮最后一条 assistant/message 的模型显示名（日志里没有就退回 id）。 */
  model: string;
  cost: number | null;
 /** 模型正在重试（第几次、为什么）。 */
  retry: { attempt: number; reason: string; inSeconds: number; code?: string } | null;
 /** 这一轮没做完：原因来自内核，按原因给下一步（去设置 / 再发一次）。 */
  failure: { cause: FailureCause; reason: string; code?: string } | null;
  question: QuestionView | null;
  compaction: CompactionView | null;
 /** 这一轮出的画布（日志里 tool/result.meta.canvas）。 */
  canvas: CanvasViewDoc | null;
 /** 这一轮回答下面的确认卡（日志里 propose_action 的提议 + card/decision）。 */
  card: ConfirmCardFact | null;
 /** 你的消息：发送中 / 没送到（重发用同一个请求编号，内核只收一次）。 */
  delivery: 'sending' | 'unsent' | null;
 /** 停下的时间（status 为 interrupted 时）。 */
  stoppedAt: string | null;
}
