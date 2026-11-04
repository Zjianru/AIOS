/**
 * 一回合：你的消息 → (思考) → 卡片 → Kind 的回答 → 回答下方一行「工作过程」。
 * 依据 experience-spec-v1.md §6.2 + prototype-to-product-v1.md §6（思考要展示，照 Codex / Claude 应用的样子）。
 *
 * 性能：整轮用 memo（比较 view 引用）；只有正在流式的那一条会重绘，别的不跟着重建。
 */

import { memo, useState } from 'react';
import { ChevronRight, CircleAlert, LoaderCircle, RotateCw, Square } from 'lucide-react';
import { Markdown } from './Markdown';
import { QuoteChip } from './QuoteChip';
import { QuestionCard } from './QuestionCard';
import { useKindStore, useStreamStore } from './kindStore';
import type { CompactionView, TurnViewData } from './kindStore';
import type { TextElement } from '@/data/types';
import { cn } from '@/lib/utils';

function timeOf(seconds: number | null): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

/** 纯文本 + 内联引用片（你说的话用这个，不走 markdown）。 */
function RichText({ text, elements }: { text: string; elements: TextElement[] }) {
  if (elements.length === 0) return <>{text}</>;
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  elements.forEach((element, index) => {
    if (element.start < cursor) return;
    if (element.start > cursor) nodes.push(text.slice(cursor, element.start));
    nodes.push(<QuoteChip key={'q' + index} element={element} />);
    cursor = element.end;
  });
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return <>{nodes}</>;
}

/** 流式回答：订阅流式文本，只有这一条重绘。 */
function Answer({ view }: { view: TurnViewData }) {
  const streamed = useStreamStore((state) => (state.turnId === view.id ? state.text : null));
  const text = streamed ?? view.answer?.text ?? '';
  const busyElsewhere =
    view.status !== 'inProgress' ||
    view.delivery !== null ||
    view.retry !== null ||
    view.question?.status === 'pending' ||
    view.tools.some((tool) => tool.running);
  if (!text && busyElsewhere) return null;
  if (!text) {
    return (
      <div className="flex items-center gap-2 py-1 text-3 text-ink-3">
        <span className="inline-flex gap-1">
          <i className="size-1 animate-pulse rounded-full bg-ink-3" />
          <i className="size-1 animate-pulse rounded-full bg-ink-3 [animation-delay:150ms]" />
          <i className="size-1 animate-pulse rounded-full bg-ink-3 [animation-delay:300ms]" />
        </span>
        正在想…
      </div>
    );
  }
  return <Markdown text={text} elements={view.answer?.elements} />;
}

function Thinking({ view }: { view: TurnViewData }) {
  const [open, setOpen] = useState(false);
  const reasoning = view.reasoning;
  if (!reasoning) return null;
  return (
    <div className="mt-2">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="flex items-center gap-1.5 text-2 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3.5 transition-transform', open && 'rotate-90')} />
        已思考 {reasoning.seconds} 秒
        {!open && reasoning.summary[0] ? <span className="truncate opacity-70">· {reasoning.summary[0]}</span> : null}
      </button>
      {open ? (
        <div className="mt-1.5 border-l border-line pl-3 text-2 leading-5 text-ink-3">
          {reasoning.full ? reasoning.full.split(String.fromCharCode(10)).map((line, index) => <p key={index}>{line}</p>) : null}
          <ul className="mt-1 list-disc pl-4">
            {reasoning.summary.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

function WorkProcess({ view }: { view: TurnViewData }) {
  const [open, setOpen] = useState(false);
  const toolCount = view.tools.length;
  const seconds = view.durationMs ? Math.max(1, Math.round(view.durationMs / 1000)) : 0;
  const tokens = view.tokens.input + view.tokens.output;
  return (
    <div className="group/work relative mt-2 text-2">
      {/* hover 出来的模型 / token / 花费：绝对定位在行右侧，不参与布局，所以行不会跳 */}
      <span className="pointer-events-none absolute right-0 top-0 hidden items-center gap-1.5 whitespace-nowrap text-ink-3 group-hover/work:flex">
        {view.model} · 输入 <span className="brand-type">{view.tokens.input}</span> · 输出{' '}
        <span className="brand-type">{view.tokens.output}</span>
        
      </span>
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="flex items-center gap-1.5 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3.5 transition-transform', open && 'rotate-90')} />
        工作过程 · <span className="brand-type">{seconds} 秒</span>
      </button>
      {open ? (
        <div className="mt-1.5 space-y-1 border-l border-line pl-3">
          {/* 信息分层（brief §四.3）：默认只显示用时；想了多久、调了几次、多少 token 点开才看 */}
          <div className="text-2 text-ink-3">
            <span className="text-ink-2">经过</span> · 想了 {view.thinkingSeconds} 秒 · 调了 {toolCount} 次工具 ·{' '}
            <span className="brand-type">{(tokens / 1000).toFixed(1)}k</span>
          </div>
          {view.reasoning ? (
            <div className="text-2 text-ink-3">
              <span className="text-ink-2">思考</span> · {view.reasoning.summary.join(' / ') || '（略）'}
            </div>
          ) : null}
          {view.tools.map((tool) => (
            <div key={tool.id} className="flex items-start gap-2 text-2">
              <span className="mt-[1px] shrink-0 text-ink-3">›</span>
              <code className="min-w-0 flex-1 break-all font-mono text-ink-2">{tool.command}</code>
              {tool.output ? <span className="min-w-0 max-w-[38%] truncate text-ink-3">{tool.output.slice(0, 40)}</span> : null}
              {tool.durationMs ? (
                <span className="ml-auto shrink-0 brand-type text-ink-3">{(tool.durationMs / 1000).toFixed(1)}s</span>
              ) : null}
            </div>
          ))}
          
        </div>
      ) : null}
    </div>
  );
}

/** 运行中的一轮：工作过程展开着，一步一步实时出现；正在跑的那步转圈。 */
function LiveWork({ view }: { view: TurnViewData }) {
  if (view.tools.length === 0) return null;
  const running = view.tools.find((tool) => tool.running);
  return (
    <div className="mt-2 text-2" data-kind="live-work">
      <div className="flex items-center gap-1.5 text-ink-3">
        <ChevronRight className="size-3.5 rotate-90" />
        工作中{running ? <span className="truncate">· 正在 <code className="font-mono text-ink-2">{running.command}</code></span> : null}
      </div>
      <div className="mt-1.5 space-y-1 border-l border-line pl-3">
        {view.tools.map((tool) => (
          <div key={tool.id} className="flex items-start gap-2">
            {tool.running ? (
              <LoaderCircle className="mt-[2px] size-3 shrink-0 animate-spin text-run" />
            ) : (
              <span className="mt-[1px] shrink-0 text-ink-3">›</span>
            )}
            <code className={cn('min-w-0 flex-1 break-all font-mono', tool.running ? 'text-ink' : 'text-ink-2')}>{tool.command}</code>
            {tool.output ? <span className="min-w-0 max-w-[38%] truncate text-ink-3">{tool.output}</span> : null}
            {tool.durationMs ? (
              <span className="ml-auto shrink-0 brand-type text-ink-3">{(tool.durationMs / 1000).toFixed(1)}s</span>
            ) : null}
          </div>
        ))}
      </div>
    </div>
  );
}

/** 模型请求失败、内核在重试：说清楚第几次、为什么。 */
function RetryLine({ retry }: { retry: NonNullable<TurnViewData['retry']> }) {
  return (
    <div className="mt-2 flex items-center gap-1.5 text-2 text-wait" data-kind="retry">
      <RotateCw className="size-3.5 animate-spin [animation-duration:2s]" />
      {retry.reason}，第 {retry.attempt} 次重试
      <span className="text-ink-3">· {retry.inSeconds} 秒后</span>
    </div>
  );
}

/** 这一轮没做完：原因原样给出；密钥类给「去设置」，其余给「再发一次」。 */
function FailureBlock({ view }: { view: TurnViewData }) {
  const send = useKindStore((state) => state.send);
  const streaming = useKindStore((state) => state.streamingTurnId);
  const failure = view.failure;
  if (!failure) return null;
  return (
    <div className="mt-2 flex items-start gap-2 rounded-card border border-fail/40 bg-fail-soft/50 px-3 py-2 text-2" data-kind="failure">
      <CircleAlert className="mt-[2px] size-3.5 shrink-0 text-fail" />
      <div className="min-w-0 flex-1">
        <div className="leading-5 text-fail">这一轮没做完：{failure.reason}</div>
        <div className="mt-0.5 leading-5 text-ink-3">
          {failure.cause === 'keyring'
            ? 'AIOS 不会改用别处的密钥。把钥匙文件找回来，或者在设置里重新填。'
            : failure.cause === 'credential'
              ? '换一把密钥，或者检查这个提供商的账户。'
              : '内核已经按规矩重试过了。过一会儿再发一次，或者换个模型。'}
        </div>
      </div>
      {view.user ? (
        <button
          type="button"
          disabled={Boolean(streaming)}
          onClick={() => view.user && send(view.user.text)}
          className="shrink-0 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:opacity-50"
        >
          再发一次
        </button>
      ) : null}
    </div>
  );
}

/** 压缩：对话里一条分隔线，点开看摘要。手动和自动都一样，只是标不同。 */
function CompactionDivider({ compaction }: { compaction: CompactionView }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="px-6 py-3" data-kind="compaction">
      <div className="flex items-center gap-3 text-1 text-ink-3">
        <span className="h-px flex-1 bg-line" />
        <button type="button" onClick={() => setOpen((value) => !value)} className="flex items-center gap-1.5 transition-colors hover:text-ink-2">
          <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
          上下文已压缩 · {compaction.auto ? '快满了，自动' : '你手动'}
          {compaction.before > 0 ? (
            <>
              {' '}· 占用 <span className="brand-type">{compaction.before}%</span> →{' '}
              <span className="brand-type">{compaction.after}%</span>
            </>
          ) : null}
        </button>
        <span className="h-px flex-1 bg-line" />
      </div>
      {open ? (
        <div className="mx-auto mt-2 max-w-[560px] rounded-card border border-line bg-pane/60 px-3 py-2 text-2 leading-5 text-ink-2">
          <div className="mb-1 text-1 text-ink-3">之前的对话由这份摘要代替；原文都还在日志里。</div>
          <Markdown text={compaction.summary} />
        </div>
      ) : null}
    </div>
  );
}

function DeliveryLine({ view }: { view: TurnViewData }) {
  const resend = useKindStore((state) => state.resend);
  if (view.delivery === 'sending') {
    return <div className="-mt-1 mb-2 text-1 text-ink-3">发送中…</div>;
  }
  if (view.delivery === 'unsent') {
    return (
      <div className="-mt-1 mb-2 flex items-center gap-1.5 text-1 text-fail" data-kind="unsent">
        <CircleAlert className="size-3" />
        没送到内核
        <button type="button" onClick={() => resend(view.id)} className="rounded-btn px-1.5 py-[1px] text-ink-2 transition-colors hover:bg-hover hover:text-ink">
          重发
        </button>
        <span className="text-ink-3">· 重发不会让 Kind 收到两次</span>
      </div>
    );
  }
  return null;
}

interface TurnProps {
  view: TurnViewData;
  /** 换了一天就在这一轮上面画一条日期分隔。 */
  dayLabel: string | null;
  /** 「上次查看之后」的分隔线画在这一轮上面。 */
  showLastSeen: boolean;
  lastSeenCounts: { moved: number; waiting: number };
}

export const TurnRow = memo(function TurnRow({ view, dayLabel, showLastSeen, lastSeenCounts }: TurnProps) {
  const streaming = view.status === 'inProgress';
  if (view.compaction) return <CompactionDivider compaction={view.compaction} />;
  const waitingForKernel = view.delivery !== null;
  return (
    <div className="px-6 pb-2">
      {dayLabel ? (
        <div className="mb-3 mt-4 flex items-center gap-3 text-1 text-ink-3">
          <span className="h-px flex-1 bg-line" />
          <span className="brand-type">{dayLabel}</span>
          <span className="h-px flex-1 bg-line" />
        </div>
      ) : null}
      {showLastSeen ? (
        <div className="mb-3 mt-4 flex items-center gap-2 rounded-lg border border-dashed border-line-2 px-3 py-1.5 text-2 text-ink-3">
          <span className="text-ink-3">◯</span>
          上次查看之后：{lastSeenCounts.moved} 件推进、{lastSeenCounts.waiting} 件等你
          <ChevronRight className="ml-auto size-3.5" />
        </div>
      ) : null}

      {view.user ? (
        <div className="mb-2 mt-3 text-4 leading-6 text-ink">
          <RichText text={view.user.text} elements={view.user.elements} />
        </div>
      ) : (
        <div className="mb-2 mt-3 flex items-center gap-1.5 text-2 text-ink-3">
          <span className="inline-block size-2 rounded-full bg-brand" />
          Kind · {timeOf(view.startedAt)}
        </div>
      )}

      <DeliveryLine view={view} />

      {view.user && !waitingForKernel ? (
        <div className="mb-1 flex items-center gap-1.5 text-2 text-ink-3">
          <span className="inline-block size-2 rounded-full bg-brand" />
          Kind
          <span className="brand-type">{timeOf(view.startedAt)}</span>
        </div>
      ) : null}

      {waitingForKernel ? null : (
        <>
          <Thinking view={view} />

          

          {streaming ? <LiveWork view={view} /> : null}
          {view.question ? <QuestionCard turnId={view.id} question={view.question} /> : null}
          {view.retry ? <RetryLine retry={view.retry} /> : null}

          <div className={cn('mt-1', streaming && 'min-h-[24px]')}>
            <Answer view={view} />
          </div>

          {view.status === 'interrupted' && view.stoppedAt ? (
            <div className="mt-1 flex items-center gap-1.5 text-2 text-ink-3" data-kind="stopped">
              <Square className="size-3 fill-current" />
              你在 <span className="brand-type">{view.stoppedAt}</span> 停下了这一轮，已经做的留着
            </div>
          ) : null}
          <FailureBlock view={view} />

          {!streaming && (view.tools.length > 0 || view.answer) ? <WorkProcess view={view} /> : null}
        </>
      )}
    </div>
  );
});
