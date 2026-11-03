/**
 * 假数据层的类型 = AIOS 真实协议的形状（第 1 期只取界面用得到的字段）。
 *
 * 依据：\`origin/main\` \`4a201c182\`
 *   - aios-rs/app-server-protocol/src/protocol/v2/thread.rs（ThreadResumeParams/Response、TurnsPage、ThreadTurnsListParams/Response）
 *   - aios-rs/app-server-protocol/src/protocol/v2/thread_data.rs（Turn）
 *   - aios-rs/app-server-protocol/src/protocol/v2/item.rs（ThreadItem，serde tag="type"，rename_all="camelCase"）
 *   - aios-rs/app-server-protocol/src/protocol/v2/turn.rs（UserInput、TurnStatus）
 *   - web/server/bridge.mjs:887、:1619-1626（真实前端怎么取首页、怎么 reverse）
 *
 * 字段名一律 camelCase（协议就是这样）。协议里查不到的，注释写「未核实」。
 * 界面只通过 src/data/api.ts 取数据；组件里不许直接 import 样例数据。
 */

export type JsonValue = null | boolean | number | string | JsonValue[] | { [k: string]: JsonValue };

/** 协议：SortDirection（v2/thread.rs）。 */
export type SortDirection = 'asc' | 'desc';

/** 协议：TurnItemsView。默认 summary；本期一直用 full。 */
export type TurnItemsView = 'summary' | 'full';

/** 协议：TurnStatus（v2/turn.rs：Completed / Interrupted / Failed / InProgress）。 */
export type TurnStatus = 'completed' | 'interrupted' | 'failed' | 'inProgress';

/** 协议：TurnError（只取界面用得到的 message）。 */
export interface TurnError {
  message: string;
}

/** 协议：UserInput（v2/turn.rs，serde tag="type"）。本期只用到 Text；其余变体不取。 */
export interface UserInputText {
  type: 'text';
  text: string;
  /** 协议字段就是 textElements；真实 TextElement 的字段未核实，这里按界面需要收窄。 */
  textElements: TextElement[];
}

export type UserInput = UserInputText;

/**
 * 文字里的「引用片」（v10-tables §1：@角色 / $能力 / ▤ 资料 / ◐ 任务 / ■ 项目 / 📍n 位置）。
 * 真实协议里可能只有 start/end（TextElement 的确切字段未核实），这里多存 kind/label/target 供渲染与跳转。
 */
export interface TextElement {
  /** 在 text 里的起止（字符下标）——协议 TextElement 有这个意思，字段名未核实。 */
  start: number;
  end: number;
  kind: QuoteKind;
  label: string;
  /** 点击跳到哪（v13 的 hash 路由或对象 id）。 */
  target: string;
}

export type QuoteKind = 'role' | 'capability' | 'material' | 'artifact' | 'task' | 'project' | 'pin';

/** 协议：ThreadItem（v2/item.rs，serde tag="type"）。只保留界面用得到的变体和字段。 */
export type ThreadItem =
  | UserMessageItem
  | AgentMessageItem
  | ReasoningItem
  | CommandExecutionItem
  | McpToolCallItem
  | FileChangeItem
  | AgentCardItem
  | ContextCompactionItem;

/** 协议：ThreadItem::UserMessage。clientId 用于乐观发送时的本地关联。 */
export interface UserMessageItem {
  type: 'userMessage';
  id: string;
  clientId: string | null;
  content: UserInput[];
}

/**
 * 协议：ThreadItem::AgentMessage。
 * 协议里还有 phase / delivery / questions 三个字段；界面这期不用，不取。
 */
export interface AgentMessageItem {
  type: 'agentMessage';
  id: string;
  text: string;
  /** 真协议的 phase（正文/前言）用来区分这段话是不是最终答复；本期没用，标未核实。 */
  phase?: 'commentary' | 'final_answer' | null;
  /**
   * 未核实：协议 AgentMessage 里没有 textElements 字段。Kind 的话里也会有引用片（v10-tables §1），
   * 真实实现里这些 span 走哪条通道没查；这里先挂在 item 上，接真接口时对齐。
   */
  textElements?: TextElement[];
}

/**
 * 协议：ThreadItem::Reasoning（summary 是「思考摘要」，content 是思考全文）。
 * 2026-10-24 用户裁定：思考要展示，样式照 Codex / Claude 应用重做（见 prototype-to-product-v1.md §6）。
 */
export interface ReasoningItem {
  type: 'reasoning';
  id: string;
  summary: string[];
  content: string[];
}

/** 协议：ThreadItem::CommandExecution。 */
export interface CommandExecutionItem {
  type: 'commandExecution';
  id: string;
  command: string;
  cwd: string;
  status: 'inProgress' | 'completed' | 'failed';
  aggregatedOutput: string | null;
  /** 协议里是 exitCode，可能为 null。 */
  exitCode: number | null;
  durationMs: number | null;
}

/** 协议：ThreadItem::McpToolCall（命令之外的工具调用）。 */
export interface McpToolCallItem {
  type: 'mcpToolCall';
  id: string;
  server: string;
  tool: string;
  status: 'inProgress' | 'completed' | 'failed';
  arguments: JsonValue;
  durationMs: number | null;
}

/** 协议：ThreadItem::FileChange。 */
export interface FileChangeItem {
  type: 'fileChange';
  id: string;
  changes: { path: string; kind: 'add' | 'delete' | 'update' }[];
  status: 'completed' | 'failed';
}

/**
 * 未核实：卡片（v10-tables §2.2 的封闭类型表）在真实协议里怎么下发。
 * 产品现状是「Kind 做了/改了一个东西」由后台推对象状态，前端按类型渲染；
 * 这里先做成一类 item，接真接口时对齐。
 */
export interface AgentCardItem {
  type: 'agentCard';
  id: string;
  card: KindCard;
}

/** 协议：ThreadItem::ContextCompaction。字段未核实（只知道有这个变体，语义是「上下文压缩」）。 */
export interface ContextCompactionItem {
  type: 'contextCompaction';
  id: string;
  /** 未核实：真实字段可能不是 summary。 */
  summary: string;
  /** 未核实：原文是否可取。 */
  sourceAvailable: true;
}

/** 协议：Turn（v2/thread_data.rs）。 */
export interface Turn {
  id: string;
  items: ThreadItem[];
  itemsView: TurnItemsView;
  status: TurnStatus;
  error: TurnError | null;
  /** 协议：startedAt，unix 秒。 */
  startedAt: number | null;
  completedAt: number | null;
  durationMs: number | null;
}

/** 协议：TurnsPage（v2/thread.rs）。nextCursor 为空表示到头了。 */
export interface TurnsPage {
  data: Turn[];
  nextCursor: string | null;
  backwardsCursor: string | null;
}

/** 协议：Thread（v2/thread_data.rs 的 Thread；这里只取界面用得到的）。 */
export interface Thread {
  id: string;
  /** 协议有 preview 字段（会话摘要）。 */
  preview: string;
  model: string;
  modelProvider: string;
  cwd: string;
  createdAt: number | null;
  updatedAt: number | null;
  status: TurnStatus;
}

/** 协议：ThreadResumeParams（只取本期用的）。 */
export interface ThreadResumeParams {
  threadId: string;
  excludeTurns: boolean;
  initialTurnsPage?: {
    limit?: number;
    sortDirection?: SortDirection;
    itemsView?: TurnItemsView;
  };
}

/** 协议：ThreadResumeInitialTurnsPageParams。 */
export interface ThreadResumeInitialTurnsPageParams {
  limit?: number;
  sortDirection?: SortDirection;
  itemsView?: TurnItemsView;
}

/** 协议：ThreadResumeResponse（只取界面用得到的）。 */
export interface ThreadResumeResponse {
  thread: Thread;
  model: string;
  modelProvider: string;
  reasoningEffort: string | null;
  initialTurnsPage: TurnsPage;
}

/** 协议：ThreadTurnsListParams。 */
export interface ThreadTurnsListParams {
  threadId: string;
  cursor?: string | null;
  limit?: number;
  sortDirection?: SortDirection;
  itemsView?: TurnItemsView;
}

/** 协议：ThreadTurnsListResponse。 */
export interface TurnsListResponse {
  data: Turn[];
  nextCursor: string | null;
  backwardsCursor: string | null;
}

/* ───────────────────────────── 卡片（v10-tables §2.2 的封闭类型表） ───────────────────────────── */

export type CardType =
  | 'routeDraft'
  | 'task'
  | 'askDecision'
  | 'askApproval'
  | 'askWeb'
  | 'artifact'
  | 'memory'
  | 'wake'
  | 'roleJoin'
  | 'planChanged'
  | 'milestone'
  | 'assignRole'
  | 'quotaAlert'
  | 'problem';

/** 卡片头：「来自哪（项目 › 任务）」。项目用线路标：彩色圆角方块 + 代码字母（visual-plan-v12.md）。 */
export interface CardOrigin {
  projectCode: string | null;
  projectName: string | null;
  taskTitle: string | null;
  /** 跳转用的 hash 路由。 */
  go: string | null;
}

export interface CardOption {
  label: string;
  description: string;
}

/** 固定五格模板里的「主体」——按类型不同，可省。 */
export type CardBody =
  | { kind: 'routeDraft'; stations: { role: string; title: string; what: string }[]; criterion: string; estimate: string }
  | { kind: 'task'; progress: string; running: string }
  | { kind: 'askDecision'; options: CardOption[] }
  | { kind: 'askApproval'; command: string; why: string }
  | { kind: 'askWeb'; url: string; why: string }
  | { kind: 'artifact'; artifact: string; version: string; changes: string[]; satisfied: { ok: boolean; text: string }[] }
  | { kind: 'memory'; note: string; noteKind: '用户声明' | '事实' | '判断' | '假设' }
  | { kind: 'wake'; when: string; todo: string }
  | { kind: 'roleJoin'; role: string; why: string }
  | { kind: 'planChanged'; before: string[]; after: string[]; why: string }
  | { kind: 'milestone'; criteria: { ok: boolean; text: string }[] }
  | { kind: 'assignRole'; role: string; why: string }
  | { kind: 'quotaAlert'; line: string; used: string }
  | { kind: 'problem'; what: string; handled: '已处理' | '处理中' | '已转为决定' };

/** 卡片动作。词只能从 v10-tables §3.2 的固定动词表里取。 */
export interface CardAction {
  label: string;
  /** 主操作实心（一张卡最多一个）。 */
  primary?: boolean;
  /** 点了会改变东西 → 给「撤销」提示条。 */
  mutates?: boolean;
}

export interface KindCard {
  id: string;
  type: CardType;
  origin: CardOrigin;
  /** 协议/UI 时间：显示用，格式已按 v13 写好。 */
  time: string;
  title: string;
  body: CardBody | null;
  /** 办完收成一行：✓ 类型 · 结果 · 时间。 */
  done: { line: string; at: string } | null;
  actions: CardAction[];
}

/* ───────────────────────────── 流式（本地事件模拟） ───────────────────────────── */

/**
 * 未核实：真实接口的流式事件名。协议 v2 里 agent message 的增量走哪条通知，未核实。
 * 接真接口时按这张表对齐。
 */
export interface ItemDelta {
  /** 未核实：真实事件名。 */
  type: 'item/started' | 'item/delta' | 'item/completed' | 'turn/completed';
  threadId: string;
  turnId: string;
  itemId: string;
  /** item/delta 时的增量文本。 */
  delta?: string;
}

/* ───────────────────────────── 假数据层里其余只读对象（第 2、3 期用） ───────────────────────────── */

export interface ProjectFixtures {
  id: string;
  name: string;
  code: string;
  color: string;
  brief: string;
  milestones: {
    id: string;
    name: string;
    state: 'passed' | 'open' | 'judging';
    when?: string;
    due?: string;
    criteria?: [string, 'ok' | 'no' | 'todo', string?][];
  }[];
}
