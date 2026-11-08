/**
 * T013.5：工作过程的纯计算（照原型 app-v14 a5d6f58 的 process/toolModel.ts，适配 AIOS 数据形状）。
 *
 * - 分类、组头文案、行的标题/摘要/NKB/+N-M 照 DSH ui-tool 与 conversation-nodes；
 * - AIOS 的 tool 事件带 arguments（tool/call）和 tool/result.meta（T013.1 的 diffs/operation/stopped）；
 * - 工具名换成 AIOS 实际注册的十三个：read/glob/grep/write/edit/apply_patch/bash/
 *   job_list/job_output/job_kill/skill/todo_write/ask_user_question。
 */

export type ToolState = 'running' | 'done' | 'error' | 'stopped';

export interface ToolStepView {
  kind: 'tool';
  id: string;
  /** AIOS 工具名（read/write/bash/...）。 */
  name: string;
  args: Record<string, unknown>;
  /** 工具结果的正文；还在跑时为空。 */
  output: string;
  exitCode: number | null;
  /** 被信号杀掉时的信号名（没有退出码时）。 */
  signal?: string | null;
  durationMs: number | null;
  state: ToolState;
  /** T013.1：tool/result.meta（diffs/operation/stopped）原样带过来。 */
  meta?: Record<string, unknown> | null;
}

export interface ThinkStepView {
  kind: 'think';
  id: string;
  text: string;
  streaming?: boolean;
}

export interface SayStepView {
  kind: 'say';
  id: string;
  text: string;
}

export type ProcessStep = ToolStepView | ThinkStepView | SayStepView;

/* ───────── 类别 ───────── */

export type Category = 'read' | 'search' | 'write' | 'edit' | 'commands' | 'questions' | 'plan' | 'tools';

export function categoryOf(name: string): Category {
  switch (name) {
    case 'read':
      return 'read';
    case 'grep':
    case 'glob':
      return 'search';
    case 'write':
      return 'write';
    case 'edit':
    case 'apply_patch':
      return 'edit';
    case 'bash':
      return 'commands';
    case 'ask_user_question':
      return 'questions';
    case 'todo_write':
      return 'plan';
    default:
      return 'tools';
  }
}

const RUNNING_TEXT: Record<Category | 'thinking', string> = {
  thinking: '正在分析请求',
  read: '正在读取文件',
  search: '正在搜索代码',
  write: '正在写入文件',
  edit: '正在编辑文件',
  commands: '正在运行命令',
  questions: '等待你的操作',
  plan: '正在更新计划',
  tools: '正在调用工具',
};

const DONE_TEXT: Record<Category | 'thinking', string> = {
  thinking: '已完成分析',
  read: '已读取文件',
  search: '已搜索代码',
  write: '已写入文件',
  edit: '修改了文件',
  commands: '执行了命令',
  questions: '向用户提出了问题',
  plan: '更新了计划',
  tools: '已调用工具',
};

/** 类别的排序：次数多的在前，一样多按第一次出现的先后。 */
function rankedCategories(steps: ProcessStep[]): Category[] {
  const counts = new Map<Category, { count: number; first: number }>();
  steps.forEach((step, index) => {
    if (step.kind !== 'tool') return;
    const category = categoryOf(step.name);
    const entry = counts.get(category);
    if (entry) entry.count += 1;
    else counts.set(category, { count: 1, first: index });
  });
  return [...counts.entries()]
    .sort((a, b) => b[1].count - a[1].count || a[1].first - b[1].first)
    .map(([category]) => category);
}

/**
 * 已结束的组：排序后的前三类，不写次数。两类用「并」连，第二类去掉打头的「已」；
 * 三类用「，」连；超过三类末尾加「等」。没有类别就是「已完成分析」。
 */
export function doneSummary(steps: ProcessStep[]): { category: Category | 'thinking'; text: string } {
  const ranked = rankedCategories(steps);
  if (ranked.length === 0) return { category: 'thinking', text: DONE_TEXT.thinking };
  const top = ranked.slice(0, 3).map((category) => DONE_TEXT[category]);
  let text: string;
  if (top.length === 1) text = top[0];
  else if (top.length === 2) text = top[0] + '并' + (top[1].startsWith('已') ? top[1].slice(1) : top[1]);
  else text = top.join('，');
  if (ranked.length > 3) text += '等';
  return { category: ranked[0], text };
}

/** 运行中的组：正在跑的工具的类别 + 详情；没有在跑的工具就是「正在分析请求」。 */
export function runningSummary(
  steps: ProcessStep[],
  cwd: string,
): { category: Category | 'thinking'; text: string; detail: string } {
  const live = [...steps].reverse().find((step): step is ToolStepView => step.kind === 'tool' && step.state === 'running');
  if (!live) return { category: 'thinking', text: RUNNING_TEXT.thinking, detail: '' };
  const category = categoryOf(live.name);
  return { category, text: RUNNING_TEXT[category], detail: rowSummary(live, cwd) };
}

/* ───────── 行 ───────── */

const TITLES: Record<string, string> = {
  bash: '运行命令',
  read: '读取',
  write: '写入',
  edit: '编辑',
  apply_patch: '编辑',
  grep: '搜索文件内容',
  glob: '搜索文件',
  ask_user_question: '提问',
  todo_write: '计划',
  skill: '能力',
  job_list: '任务列表',
  job_output: '任务输出',
  job_kill: '结束任务',
};

export function toolTitle(name: string): string {
  return TITLES[name] ?? '工具调用';
}

const DETAIL_FIELDS = [
  'title', 'description', 'objective', 'task', 'task_name', 'name', 'question', 'questions', 'prompt', 'message',
  'command', 'cmd', 'queries', 'query', 'pattern', 'url', 'uri', 'file_path', 'path', 'target', 'action', 'status',
];

function collapse(text: string): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  const chars = [...flat];
  return chars.length > 160 ? chars.slice(0, 160).join('') + '…' : flat;
}

/** 按字段优先顺序取第一个能用的值当详情；数组取第一项的 question / 本身。 */
export function detailOf(args: Record<string, unknown>): string {
  for (const field of DETAIL_FIELDS) {
    const value = args[field];
    if (typeof value === 'string' && value.trim()) return collapse(value);
    if (Array.isArray(value) && value.length > 0) {
      const first = value[0] as unknown;
      if (typeof first === 'string') return collapse(first);
      if (first && typeof first === 'object' && typeof (first as Record<string, unknown>).question === 'string') {
        return collapse((first as Record<string, string>).question);
      }
    }
  }
  return '';
}

/** 路径先相对会话工作目录（家目录展开留给后续票）。 */
export function displayPath(path: string, cwd: string): string {
  if (path === cwd) return '.';
  if (path.startsWith(cwd + '/')) return path.slice(cwd.length + 1);
  return path;
}

function stringArg(args: Record<string, unknown>, key: string): string | null {
  const value = args[key];
  return typeof value === 'string' && value.trim() ? value : null;
}

/** 行上点后面那段：文件工具写路径；其他先用 description，再用详情；工具名兜底。 */
export function rowSummary(step: ToolStepView, cwd: string): string {
  // 提问的问句由问题卡显示（页面已有 QuestionCard），行上不再重复一遍。
  if (step.name === 'ask_user_question') return '';
  const file = stringArg(step.args, 'file_path');
  if (file && (step.name === 'read' || step.name === 'write' || step.name === 'edit')) return displayPath(file, cwd);
  if (step.name === 'apply_patch') return '';
  const description = stringArg(step.args, 'description');
  if (description) return collapse(description);
  if (step.name === 'grep' || step.name === 'glob') {
    const pattern = stringArg(step.args, 'pattern') ?? '';
    const path = stringArg(step.args, 'path');
    return collapse(pattern + (path && path !== cwd ? ' · ' + displayPath(path, cwd) : ''));
  }
  const detail = detailOf(step.args);
  if (detail) return detail;
  return TITLES[step.name] ? '' : step.name;
}

/** 失败行：结果的第一行。 */
export function firstLine(text: string): string {
  return text.split('\n').find((row) => row.trim()) ?? '';
}

/** write / edit 的 NKB：输入文本的长度 ÷ 1024 向上取整（edit 是新旧两段加起来）。不是文件大小。 */
export function sizeSuffix(step: ToolStepView): string | null {
  let length = 0;
  if (step.name === 'write') length = (stringArg(step.args, 'content') ?? '').length;
  else if (step.name === 'edit')
    length = (stringArg(step.args, 'old_string') ?? '').length + (stringArg(step.args, 'new_string') ?? '').length;
  else return null;
  return Math.max(1, Math.ceil(length / 1024)) + 'KB';
}

/** 这一行有没有可展开的东西（没有就不给点）：还在跑的工具只有命令能看半截输出。 */
export function hasCard(step: ToolStepView): boolean {
  return step.state !== 'running' || step.name === 'bash';
}

/* ───────── 卡片 ───────── */

export interface DiffLine {
  kind: 'add' | 'del' | 'ctx' | 'hunk';
  text: string;
  number: number | null;
}

export interface DiffFile {
  path: string;
  isNew: boolean;
  lines: DiffLine[];
  added: number;
  removed: number;
}

function splitLines(text: string): string[] {
  if (text === '') return [];
  const rows = text.split('\n');
  if (rows[rows.length - 1] === '') rows.pop();
  return rows;
}

/** 新旧两段的行级差异（最长公共子序列；页面预览用，真产品照 DSH 的 DiffBlock）。 */
function lineDiff(oldText: string, newText: string): DiffLine[] {
  const a = splitLines(oldText);
  const b = splitLines(newText);
  const table: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      table[i][j] = a[i] === b[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      out.push({ kind: 'ctx', text: a[i], number: j + 1 });
      i += 1;
      j += 1;
    } else if (j < b.length && (i >= a.length || table[i][j + 1] >= table[i + 1][j])) {
      out.push({ kind: 'add', text: b[j], number: j + 1 });
      j += 1;
    } else {
      out.push({ kind: 'del', text: a[i], number: i + 1 });
      i += 1;
    }
  }
  return out;
}

function fileOf(path: string, isNew: boolean, lines: DiffLine[], cwd: string): DiffFile {
  return {
    path: displayPath(path, cwd),
    isNew,
    lines,
    added: lines.filter((line) => line.kind === 'add').length,
    removed: lines.filter((line) => line.kind === 'del').length,
  };
}

/** Codex 补丁文本 → 每个文件的增删行（行号未知，不写）。 */
function patchFiles(input: string, cwd: string): DiffFile[] {
  const files: DiffFile[] = [];
  let current: { path: string; isNew: boolean; lines: DiffLine[] } | null = null;
  const flush = () => {
    if (current) files.push(fileOf(current.path, current.isNew, current.lines, cwd));
  };
  for (const row of input.split('\n')) {
    const header = /^\*\*\* (Add|Update|Delete) File: (.+)$/.exec(row);
    if (header) {
      flush();
      current = { path: header[2].trim(), isNew: header[1] === 'Add', lines: [] };
      continue;
    }
    if (!current || row.startsWith('***')) continue;
    if (row.startsWith('@@')) current.lines.push({ kind: 'hunk', text: row, number: null });
    else if (row.startsWith('+')) current.lines.push({ kind: 'add', text: row.slice(1), number: null });
    else if (row.startsWith('-')) current.lines.push({ kind: 'del', text: row.slice(1), number: null });
    else current.lines.push({ kind: 'ctx', text: row.startsWith(' ') ? row.slice(1) : row, number: null });
  }
  flush();
  return files;
}

interface MetaDiff {
  path?: unknown;
  oldText?: unknown;
  newText?: unknown;
}

/** T013.1 的 meta.diffs → 页面 DiffFile（跑完/中途停用实际改动的文本）。 */
function diffFilesFromMeta(step: ToolStepView, cwd: string): DiffFile[] | null {
  const meta = step.meta;
  if (!meta || !Array.isArray(meta.diffs)) return null;
  const diffs = meta.diffs as MetaDiff[];
  const files: DiffFile[] = [];
  for (const diff of diffs) {
    if (typeof diff.newText !== 'string') continue;
    const path = typeof diff.path === 'string' ? diff.path : '';
    if (diff.oldText === null) {
      files.push(fileOf(path, true, lineDiff('', diff.newText), cwd));
    } else if (typeof diff.oldText === 'string') {
      files.push(fileOf(path, false, lineDiff(diff.oldText, diff.newText), cwd));
    }
  }
  return files.length > 0 ? files : null;
}

/**
 * 一次调用的差异：跑完/中途停用结果 meta（实际改了什么）；还在跑或没有 meta 时按 arguments 推（打算改成什么）。
 * 失败、停下的调用没有实际改动时不出 diff（照 DSH：失败行省略）。
 */
export function diffOf(step: ToolStepView, cwd: string): DiffFile[] | null {
  if (step.state === 'error') return null;
  if (step.state === 'stopped') {
    return diffFilesFromMeta(step, cwd);
  }
  const fromMeta = diffFilesFromMeta(step, cwd);
  if (fromMeta) return fromMeta;
  const path = stringArg(step.args, 'file_path');
  if (step.name === 'write' && path) {
    return [fileOf(path, true, lineDiff('', stringArg(step.args, 'content') ?? ''), cwd)];
  }
  if (step.name === 'edit' && path) {
    return [fileOf(path, false, lineDiff(String(step.args.old_string ?? ''), String(step.args.new_string ?? '')), cwd)];
  }
  if (step.name === 'apply_patch') {
    const input = stringArg(step.args, 'input');
    const files = input ? patchFiles(input, cwd) : [];
    return files.length > 0 ? files : null;
  }
  return null;
}

export function diffTotals(files: DiffFile[]): { added: number; removed: number } {
  return files.reduce(
    (sum, file) => ({ added: sum.added + file.added, removed: sum.removed + file.removed }),
    { added: 0, removed: 0 },
  );
}

export interface TerminalCard {
  command: string;
  output: string;
  exitCode: number | null;
  signal?: string | null;
}

/** bash：命令、输出（去掉末尾的退出标记）、退出码/信号。 */
export function terminalOf(step: ToolStepView): TerminalCard | null {
  if (step.name !== 'bash') return null;
  const command = stringArg(step.args, 'command') ?? '';
  const output = step.output
    .split('\n')
    .filter((row) => !/^\[(exit code: -?\d+|killed by signal: .+)\]$/.test(row))
    .join('\n')
    .replace(/\n+$/, '');
  return { command, output, exitCode: step.exitCode, signal: step.signal ?? null };
}

export interface ReadCard {
  path: string;
  lines: { number: number; text: string }[];
  footer: string;
}

/** read：从 content 段里取出带行号的正文。 */
export function readOf(step: ToolStepView, cwd: string): ReadCard | null {
  if (step.name !== 'read' || step.state !== 'done') return null;
  const match = /<content>\n([\s\S]*)\n<\/content>/.exec(step.output);
  if (!match) return null;
  const rows: { number: number; text: string }[] = [];
  let footer = '';
  for (const row of match[1].split('\n')) {
    const line = /^(\d+): ?(.*)$/.exec(row);
    if (line) rows.push({ number: Number(line[1]), text: line[2] });
    else if (row.startsWith('(')) footer = row;
  }
  return { path: displayPath(stringArg(step.args, 'file_path') ?? '', cwd), lines: rows, footer };
}

export interface SearchCard {
  groups: { path: string; rows: { number: number; text: string }[] }[];
  header: string;
}

/** grep/glob 的命中列表。 */
export function searchOf(step: ToolStepView): SearchCard | null {
  if (step.state !== 'done') return null;
  if (step.name === 'glob') {
    const paths = step.output.split('\n').filter((row) => row.trim() && !row.startsWith('('));
    return { header: paths.length + ' 个文件', groups: paths.map((path) => ({ path: path.replace(/^\.\//, ''), rows: [] })) };
  }
  if (step.name !== 'grep') return null;
  const blocks = step.output.split('\n\n');
  const head = blocks.shift() ?? '';
  const count = /Found (\d+)/.exec(head);
  const groups = blocks
    .map((block) => {
      const [path, ...rest] = block.split('\n');
      return {
        path: path.replace(/^\.\//, ''),
        rows: rest
          .map((row) => /^Line (\d+): (.*)$/.exec(row))
          .filter((match): match is RegExpExecArray => match !== null)
          .map((match) => ({ number: Number(match[1]), text: match[2] })),
      };
    })
    .filter((group) => group.rows.length > 0);
  return { header: count ? count[1] + ' 处命中' : head, groups };
}

/* ───────── 退出码 ───────── */

/** DSH parseExitStatus：读末尾的 [exit code: N] / [killed by signal: X] 并从正文去掉；没有标记就是 0。 */
export function parseExitStatus(output: string): { text: string; exitCode: number | null; signal: string | null } {
  const trimmed = output.replace(/\n+$/, '');
  const signal = /(?:^|\n)\[killed by signal: ([^\]]+)\]$/.exec(trimmed);
  if (signal) {
    return { text: trimmed.slice(0, signal.index).replace(/\n+$/, ''), exitCode: null, signal: signal[1] };
  }
  const code = /(?:^|\n)\[exit code: (-?\d+)\]$/.exec(trimmed);
  if (code) {
    return { text: trimmed.slice(0, code.index).replace(/\n+$/, ''), exitCode: Number(code[1]), signal: null };
  }
  return { text: trimmed, exitCode: 0, signal: null };
}
