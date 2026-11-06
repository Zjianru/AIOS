/**
 * T013.5：一轮的工作过程（照原型 app-v14 a5d6f58 的 process/ProcessView.tsx）。
 *
 * - 整轮的开关：做完了是「已完成，用时 X」，默认收起；停下、失败、还在跑时一直展开，不能收；
 * - 组头：做完了写排前三的类别；还在跑写当前类别 + 详情；
 * - 行：图标、标题、圆点、摘要，后面跟退出码、NKB、+N -M；点开按内容出卡片。
 */

import { useEffect, useState } from 'react';
import {
  ChevronDown,
  ChevronUp,
  FilePen,
  FilePlus,
  FileText,
  FolderSearch,
  Lightbulb,
  ListTodo,
  MessageCircleQuestionMark,
  Search,
  Sparkles,
  SquareTerminal,
  Wrench,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { TurnStatus } from '@/data/types';
import { Markdown } from '../Markdown';
import { ToolCard, hasCard } from './ToolCards';
import {
  categoryOf,
  diffOf,
  diffTotals,
  doneSummary,
  firstLine,
  formatDuration,
  rowSummary,
  runningSummary,
  sizeSuffix,
  toolTitle,
} from './toolModel';
import type { Category, ProcessStep, ThinkStepView, ToolStepView } from './toolModel';

const CATEGORY_ICONS: Record<Category | 'thinking', LucideIcon> = {
  thinking: Lightbulb,
  read: FileText,
  search: Search,
  write: FilePlus,
  edit: FilePen,
  commands: SquareTerminal,
  questions: MessageCircleQuestionMark,
  plan: ListTodo,
  tools: Wrench,
};

function toolIcon(name: string): LucideIcon {
  if (name === 'glob') return FolderSearch;
  if (name === 'skill') return Sparkles;
  return CATEGORY_ICONS[categoryOf(name)];
}

/** 圆点分隔（2×2，左右各 8px）。 */
function Dot() {
  return <i className="mx-2 inline-block size-[2px] shrink-0 rounded-full bg-ink-3" />;
}

/* ───────── 行 ───────── */

function ThinkRow({ step }: { step: ThinkStepView }) {
  const [open, setOpen] = useState(false);
  const preview = firstLine(step.text);
  return (
    <div data-kind="think-row">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="group/row flex h-6 w-full min-w-0 items-center text-left text-3 text-ink-3"
      >
        <Lightbulb className="size-4 shrink-0" strokeWidth={1.6} />
        <span className={cn('ml-1.5 shrink-0 transition-colors group-hover/row:text-ink-2', step.streaming && 'process-shimmer')}>思考</span>
        {preview ? (
          <>
            <Dot />
            <span className="min-w-0 truncate">{preview}</span>
          </>
        ) : null}
      </button>
      {open ? (
        <div className="mb-1 mt-0.5 pl-[22px]">
          <Markdown text={step.text} />
        </div>
      ) : null}
    </div>
  );
}

function ToolRow({ step, cwd }: { step: ToolStepView; cwd: string }) {
  const [open, setOpen] = useState(false);
  const Icon = toolIcon(step.name);
  const summary = rowSummary(step, cwd);
  const running = step.state === 'running';
  const failed = step.state === 'error';
  const stopped = step.state === 'stopped';
  const diff = diffOf(step, cwd);
  const totals = diff ? diffTotals(diff) : null;
  // 失败、停下的行不写大小和增删
  const size = failed || stopped ? null : sizeSuffix(step);
  const exitCode = step.name === 'bash' && step.exitCode !== null && step.exitCode !== 0 ? step.exitCode : null;
  const isPath = step.name === 'read' || step.name === 'write' || step.name === 'edit';
  const expandable = hasCard(step);
  return (
    <div data-kind="tool-row" data-tool={step.name} data-state={step.state}>
      <button
        type="button"
        disabled={!expandable}
        onClick={() => setOpen((value) => !value)}
        className="group/row flex h-6 w-full min-w-0 items-center text-left text-3 text-ink-3 disabled:cursor-default"
      >
        <Icon className="size-4 shrink-0" strokeWidth={1.6} />
        <span
          className={cn(
            'ml-1.5 shrink-0 transition-colors',
            running ? 'process-shimmer' : 'group-hover/row:text-ink-2',
            open && 'text-ink-2',
          )}
        >
          {toolTitle(step.name)}
        </span>
        {summary ? (
          <>
            <Dot />
            <span className={cn('min-w-0 truncate', isPath && 'process-path', stopped && 'text-wait')}>{summary}</span>
          </>
        ) : null}
        {failed && step.output ? (
          <>
            <Dot />
            <span className="min-w-0 truncate text-fail">{firstLine(step.output)}</span>
          </>
        ) : null}
        {stopped ? <span className="ml-2 shrink-0 text-wait">已停止</span> : null}
        {step.signal ? (
          <span className="ml-2 inline-flex shrink-0 items-center gap-1 rounded-full bg-hover px-1.5 text-1 leading-[18px] text-ink-2">
            <i className="size-1.5 rounded-full bg-fail" />
            {step.signal}
          </span>
        ) : exitCode !== null ? (
          <span className="ml-2 inline-flex shrink-0 items-center gap-1 rounded-full bg-hover px-1.5 text-1 leading-[18px] text-ink-2">
            <i className="size-1.5 rounded-full bg-fail" />
            退出码 {exitCode}
          </span>
        ) : null}
        {size ? <span className="ml-2 shrink-0 font-mono text-1 text-ink-3">{size}</span> : null}
        {totals && (totals.added > 0 || totals.removed > 0) ? (
          <span className="ml-[10px] shrink-0 font-mono text-1">
            <span className={cn('transition-colors group-hover/row:text-done', open ? 'text-done' : 'text-ink-3')}>
              +{totals.added}
            </span>{' '}
            <span className={cn('transition-colors group-hover/row:text-fail', open ? 'text-fail' : 'text-ink-3')}>
              -{totals.removed}
            </span>
          </span>
        ) : null}
      </button>
      {open ? (
        <div className="mb-2 mt-1 pl-[22px]">
          <ToolCard step={step} cwd={cwd} />
        </div>
      ) : null}
    </div>
  );
}

/* ───────── 组 ───────── */

function Group({ steps, live, cwd }: { steps: ProcessStep[]; live: boolean; cwd: string }) {
  const [open, setOpen] = useState(false);
  const running = live ? runningSummary(steps, cwd) : null;
  const done = running ? null : doneSummary(steps);
  const category = running ? running.category : done!.category;
  const Icon = CATEGORY_ICONS[category];
  const Toggle = open ? ChevronUp : ChevronDown;
  return (
    <div data-kind="process-group">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        className="group/head flex h-6 w-full min-w-0 items-center text-left text-3 text-ink-3 transition-colors hover:text-ink-2"
      >
        <span className="relative size-4 shrink-0">
          <Icon
            className={cn('absolute inset-0 size-4 transition-opacity group-hover/head:opacity-0', open && 'opacity-0')}
            strokeWidth={1.6}
          />
          <Toggle
            className={cn('absolute inset-0 size-4 opacity-0 transition-opacity group-hover/head:opacity-100', open && 'opacity-100')}
            strokeWidth={1.6}
          />
        </span>
        {running ? (
          <>
            <span className="ml-1.5 shrink-0 process-shimmer">{running.text}</span>
            {running.detail ? (
              <>
                <Dot />
                <span className="min-w-0 truncate">{running.detail}</span>
              </>
            ) : null}
          </>
        ) : (
          <span className="ml-1.5 min-w-0 truncate">{done!.text}</span>
        )}
      </button>
      {open ? (
        <div className="process-scroll mt-2 flex flex-col gap-1">
          {steps.map((step) =>
            step.kind === 'think' ? (
              <ThinkRow key={step.id} step={step} />
            ) : step.kind === 'tool' ? (
              <ToolRow key={step.id} step={step} cwd={cwd} />
            ) : null,
          )}
        </div>
      ) : null}
    </div>
  );
}

/* ───────── 整轮 ───────── */

type Segment = { kind: 'group'; key: string; steps: ProcessStep[] } | { kind: 'say'; key: string; text: string };

/** 中途的话把过程切成几组。 */
function segmentsOf(steps: ProcessStep[]): Segment[] {
  const out: Segment[] = [];
  let current: ProcessStep[] = [];
  const flush = () => {
    if (current.length > 0) out.push({ kind: 'group', key: current[0].id, steps: current });
    current = [];
  };
  for (const step of steps) {
    if (step.kind === 'say') {
      flush();
      out.push({ kind: 'say', key: step.id, text: step.text });
    } else current.push(step);
  }
  flush();
  return out;
}

/** 运行中的用时：每秒走一下。 */
function useElapsed(startedAtMs: number | null, running: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);
  return startedAtMs === null ? 0 : Math.max(0, now - startedAtMs);
}

export interface ProcessViewProps {
  steps: ProcessStep[];
  status: TurnStatus;
  /** 这一轮正在跑（页面上有东西在推它）。 */
  live: boolean;
  startedAt: number | null;
  durationMs: number | null;
  /** 会话工作目录（行上路径相对化用）。 */
  cwd: string;
  /** 回答已经在写了：过程这边不再有在跑的组。 */
  answering?: boolean;
  /** T010.3：这一轮的用量（hover 整轮行时显示；没有就不显示用量行）。 */
  usage?: {
    model: string;
    input: number;
    output: number;
    cacheRead?: number;
    cacheWrite?: number;
  };
  /** 这一轮想了多少秒（展开后「经过」行显示）。 */
  thinkingSeconds?: number;
  /** 没有工具也没有思考的完成轮也显示（T010 承诺：用量/用时的「工作过程」行）。 */
  alwaysShow?: boolean;
}

export function ProcessView({
  steps,
  status,
  live,
  startedAt,
  durationMs,
  cwd,
  answering = false,
  usage,
  thinkingSeconds = 0,
  alwaysShow = false,
}: ProcessViewProps) {
  const [open, setOpen] = useState(false);
  const elapsed = useElapsed(startedAt === null ? null : startedAt * 1000, live);
  if (steps.length === 0 && !live && !alwaysShow) return null;
  const segments = segmentsOf(steps);
  // 停下、失败、还在跑：一直展开，不能收
  const pinned = live || status === 'failed' || status === 'interrupted';
  const expanded = pinned || open;
  const duration = formatDuration(live ? elapsed : (durationMs ?? 0));
  const label = live ? 'Kind 工作中' : status === 'interrupted' ? '已停止' : status === 'failed' ? '没做完' : '已完成';
  const visible = segments.filter((segment) => expanded || segment.kind === 'say');
  const lastGroup = segments.length > 0 && segments[segments.length - 1].kind === 'group' ? segments.length - 1 : -1;
  // T010.3：悬停整轮行显示模型名与用量（输入含缓存读写；厂商没报缓存就不显示命中）。
  const shownInput = usage ? usage.input + (usage.cacheRead ?? 0) + (usage.cacheWrite ?? 0) : 0;
  const cacheHit =
    usage && usage.cacheRead !== undefined && usage.cacheRead !== null && shownInput > 0
      ? Math.round((usage.cacheRead / shownInput) * 100)
      : null;
  const totalTokens = shownInput + (usage?.output ?? 0);
  const toolCount = steps.filter((step) => step.kind === 'tool').length;
  return (
    <div data-kind="process" data-open={expanded ? 'true' : 'false'} className="group/turn relative">
      {usage ? (
        <span data-kind="turn-usage" className="pointer-events-none absolute right-0 top-0 hidden items-center gap-1.5 whitespace-nowrap text-ink-3 group-hover/turn:flex">
          {usage.model} · 输入 <span data-kind="usage-number">{shownInput}</span>
          {cacheHit === null ? null : (
            <>
              {' '}· 缓存命中 <span data-kind="usage-number">{cacheHit}%</span>
            </>
          )}{' '}
          · 输出 <span data-kind="usage-number">{usage.output}</span>
        </span>
      ) : null}
      <button
        type="button"
        disabled={pinned}
        onClick={() => setOpen((value) => !value)}
        className="group/turn flex h-[33px] w-full items-center gap-1 border-b-[0.5px] border-line-2 text-left text-3 text-ink-3 transition-colors enabled:hover:text-ink-2 disabled:cursor-default"
      >
        <span className={cn(live && 'process-shimmer', status === 'failed' && !live && 'text-fail')}>
          {label}，用时 <span className="brand-type">{duration}</span>
        </span>
        {live ? <span className="tracking-widest">···</span> : null}
        {pinned ? null : <ChevronDown className={cn('size-3.5 transition-transform', expanded && 'rotate-180')} />}
      </button>
      {visible.length > 0 || live || expanded ? (
        <div className="mt-4 flex flex-col gap-1">
          {/* 信息分层：默认只显示用时；想了多久、调了几次、多少 token 展开才看（T010 承诺）。 */}
          {expanded && !live ? (
            <div className="text-2 text-ink-3" data-kind="process-meta">
              <span className="text-ink-2">经过</span> ·{' '}
              {thinkingSeconds > 0 ? <>想了 {thinkingSeconds} 秒 · </> : null}调了 {toolCount} 次工具 ·{' '}
              <span className="brand-type">{(totalTokens / 1000).toFixed(1)}k</span>
            </div>
          ) : null}
          {segments.map((segment, index) => {
            if (segment.kind === 'say') {
              return (
                <div key={segment.key} className="my-3" data-kind="interim-say">
                  <Markdown text={segment.text} />
                </div>
              );
            }
            if (!expanded) return null;
            return <Group key={segment.key} steps={segment.steps} live={live && !answering && index === lastGroup} cwd={cwd} />;
          })}
          {/* 正在跑、最后一段是中途的话：下一步还没出来，先占一个「正在分析请求」 */}
          {live && !answering && lastGroup === -1 ? <Group steps={[]} live cwd={cwd} /> : null}
        </div>
      ) : null}
    </div>
  );
}