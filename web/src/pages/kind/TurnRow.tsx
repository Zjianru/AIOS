/**
 * 一回合：你的消息 → 工作过程 → 画布 / 问题卡 → Kind 的回答（工作过程在回答上面，T013 验收 F2；
 * 画布是 T015.1 加的，排在问题卡前面，照原型）。
 * 依据 experience-spec-v1.md §6.2 + prototype-to-product-v1.md §6（思考要展示，照 Codex / Claude 应用的样子）。
 *
 * 性能：整轮用 memo（比较 view 引用）；只有正在流式的那一条会重绘，别的不跟着重建。
 */

import { memo, useState } from 'react';
import { toast } from 'sonner';
import { ChevronRight, CircleAlert, RotateCw, Square } from 'lucide-react';
import { CopyButton } from './CopyButton';
import { Markdown } from './Markdown';
import { QuoteChip } from './QuoteChip';
import { FileReferenceChip } from './FileReferenceChip';
import { fileMentions } from './atGrammar';
import { QuestionCard } from './QuestionCard';
import { useKindStore, useStreamStore } from './kindStore';
import { processStepsOf } from './turnViews';
import { ProcessView } from './process/ProcessView';
import { CanvasView } from './CanvasView';
import { AttachmentPreview, FileCard, FromCanvasChip, PasteCard, pasteStats } from './composer/AttachmentCard';
import { attachmentGet } from '@/data/service';
import type { CompactionView, TurnUserView, TurnViewData } from './kindStore';
import type { CanvasViewDoc } from './turnViews';
import { go } from '@/app/navigate';
import type { TextElement } from '@/data/types';
import { cn } from '@/lib/utils';

function timeOf(seconds: number | null): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

/** 气泡里的引用块：开头的 `> ` 行（照 DSH QuotedUserText 的样子，左边框 + 弱一点的文字）。 */
function QuoteBlock({ lines }: { lines: string[] }) {
  return (
    <div
      data-kind="quote-block"
      className="my-1 border-l-2 border-line-2 pl-2 text-ink-2"
    >
      {lines.join(String.fromCharCode(10))}
    </div>
  );
}

/**
 * 你的消息带着的东西（T013 F1，照原型 app-v14 TurnRow UserBubble）：
 * 气泡上面一排卡片——粘贴卡和文件卡用输入框里的同一张卡；点开看全文。
 */
function UserCards({ user }: { user: TurnUserView }) {
  const [openPaste, setOpenPaste] = useState<number | null>(null);
  const [openFile, setOpenFile] = useState<{ name: string; text: string } | null>(null);
  return (
    <div className="flex max-w-[82%] flex-wrap items-center justify-end gap-2" data-kind="user-cards">
      {user.pastes.map((paste, index) => (
        <PasteCard
          key={'paste-' + String(index)}
          paste={paste}
          onOpen={() => setOpenPaste(openPaste === index ? null : index)}
        />
      ))}
      {user.files.map((file) => (
        <FileCard
          key={file.id + '-' + file.name}
          file={file}
          onOpen={() => {
            void attachmentGet(file.id)
              .then((result) =>
                setOpenFile({ name: file.name, text: result.text ?? '（这个文件没有可显示的文字）' }),
              )
              .catch(() => toast('读不回这个文件', { description: file.name }));
          }}
        />
      ))}
      {openPaste !== null && user.pastes[openPaste] !== undefined ? (
        <AttachmentPreview
          kind="paste"
          title={'粘贴内容 · ' + pasteStats(user.pastes[openPaste]).lines + ' 行 · ' + pasteStats(user.pastes[openPaste]).chars + ' 字'}
          text={user.pastes[openPaste]}
          onClose={() => setOpenPaste(null)}
          className="w-full"
        />
      ) : null}
      {openFile ? (
        <AttachmentPreview
          kind="file"
          title={'文件全文 · ' + openFile.name}
          text={openFile.text}
          onClose={() => setOpenFile(null)}
          className="w-full"
        />
      ) : null}
    </div>
  );
}

/** 你说的话：普通行照旧（带内联引用片），`> ` 开头的行画成引用块。 */
function UserBubbleText({ text, elements }: { text: string; elements: TextElement[] }) {
  const lines = text.split(String.fromCharCode(10));
  const nodes: React.ReactNode[] = [];
  let plain: string[] = [];
  let quote: string[] = [];
  const flushPlain = () => {
    if (plain.length === 0) return;
    const value = plain.join(String.fromCharCode(10));
    nodes.push(<RichText key={'plain-' + nodes.length} text={value} elements={elements} />);
    plain = [];
  };
  const flushQuote = () => {
    if (quote.length === 0) return;
    nodes.push(<QuoteBlock key={'quote-' + nodes.length} lines={quote} />);
    quote = [];
  };
  for (const line of lines) {
    if (line.startsWith('> ')) {
      flushPlain();
      quote.push(line.slice(2));
    } else {
      flushQuote();
      plain.push(line);
    }
  }
  flushPlain();
  flushQuote();
  return <>{nodes}</>;
}

/** 一段普通文字：把 @文件引用换成引用片，其余原样（F6：不显示 @"..." 生文本）。 */
function MentionText({ text }: { text: string }) {
  const mentions = fileMentions(text);
  if (mentions.length === 0) return <>{text}</>;
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  mentions.forEach((mention, index) => {
    if (mention.start > cursor) nodes.push(text.slice(cursor, mention.start));
    nodes.push(<FileReferenceChip key={'m' + index} path={mention.path} />);
    cursor = mention.end;
  });
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return <>{nodes}</>;
}

/** 这一轮出的画布：最新版展开，旧版缩成一行（T015.2）；版本从日志折叠的 canvasTracks 来。 */
function CanvasBlock({
  doc,
  onCanvasMessage,
}: {
  doc: CanvasViewDoc;
  onCanvasMessage: (text: string, canvas: { id: string; title: string }) => void;
}) {
  const track = useKindStore((state) => state.canvasTracks[doc.id]);
  const revealTurn = useKindStore((state) => state.revealTurn);
  const latest = track?.latest ?? doc.n;
  const versions =
    track?.versions ?? [{ n: doc.n, title: doc.title, note: doc.note, html: doc.html }];
  return (
    <CanvasView
      doc={doc}
      latest={latest}
      versions={versions}
      onJumpLatest={() => {
        if (track) revealTurn(track.latestTurnId);
      }}
      onCanvasMessage={onCanvasMessage}
    />
  );
}

/** 纯文本 + 内联引用片（你说的话用这个，不走 markdown）。 */
function RichText({ text, elements }: { text: string; elements: TextElement[] }) {
  if (elements.length === 0) return <MentionText text={text} />;
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  elements.forEach((element, index) => {
    if (element.start < cursor) return;
    if (element.start > cursor) {
      nodes.push(<MentionText key={'t' + index} text={text.slice(cursor, element.start)} />);
    }
    nodes.push(<QuoteChip key={'q' + index} element={element} />);
    cursor = element.end;
  });
  if (cursor < text.length) nodes.push(<MentionText key="tail" text={text.slice(cursor)} />);
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
    view.tools.some((tool) => tool.state === 'running');
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
  return (
    <div data-kind="assistant-answer">
      <Markdown text={text} elements={view.answer?.elements} />
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
      {failure.cause === 'keyring' || failure.cause === 'credential' ? (
        <button
          type="button"
          onClick={() => go('settings:model')}
          className="shrink-0 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
        >
          去设置 ›
        </button>
      ) : null}
      {view.user ? (
        <button
          type="button"
          disabled={Boolean(streaming)}
          // F1：粘贴和文件跟着一起再发一次（不是只剩打的字）。
          onClick={() =>
            view.user &&
            send(view.user.text, {
              pastes: view.user.pastes,
              attachments: view.user.files,
              canvas: view.user.canvas ?? undefined,
            })
          }
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
          {compaction.before !== undefined && compaction.after !== undefined ? (
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
  /** T015.4：画布里的按钮交上来的一句话（带画布 id 和标题）。 */
  onCanvasMessage: (text: string, canvas: { id: string; title: string }) => void;
}

export const TurnRow = memo(function TurnRow({
  view,
  dayLabel,
  showLastSeen,
  lastSeenCounts,
  onCanvasMessage,
}: TurnProps) {
  const streaming = view.status === 'inProgress';
  // T015.5：这一轮正在画的画布标题（工作过程里「正在画 · 标题」）。
  const drawing = useKindStore((state) =>
    state.drawingCanvas?.turnId === view.id ? state.drawingCanvas.title : null,
  );
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
        <div className="mb-2 mt-3 flex flex-col items-end gap-1.5" data-kind="user-message-row">
          {view.user.canvas ? <FromCanvasChip title={view.user.canvas.title} /> : null}
          {view.user.pastes.length > 0 || view.user.files.length > 0 ? <UserCards user={view.user} /> : null}
          {view.user.text ? (
            <div
              className="group/user relative max-w-[82%] rounded-2xl rounded-br-md border border-line-2 bg-raised px-3.5 py-2 text-4 leading-6 text-ink"
              data-kind="user-bubble"
            >
              <UserBubbleText text={view.user.text} elements={view.user.elements} />
              <CopyButton
                text={view.user.text}
                className="absolute -right-8 top-1 opacity-0 transition-opacity group-hover/user:opacity-100"
              />
            </div>
          ) : null}
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
          {streaming ? (
            <ProcessView
              steps={processStepsOf(view, '')}
              status={view.status}
              live
              startedAt={view.startedAt}
              durationMs={view.durationMs}
              cwd=""
              answering={false}
              usage={view.tokens.input || view.tokens.output ? { model: view.model, ...view.tokens } : undefined}
              thinkingSeconds={view.thinkingSeconds}
              drawing={drawing}
              alwaysShow
            />
          ) : null}
          {/* T013 验收 F2：完成轮的工作过程在回答上面（原型 a5d6f58 的顺序：过程 → 问题卡 → 回答）。 */}
          {!streaming &&
          (view.tools.length > 0 ||
            view.answer ||
            view.status === 'interrupted' ||
            view.status === 'failed') ? (
            <div className="mt-2">
              <ProcessView
                steps={processStepsOf(view, '')}
                status={view.status}
                live={false}
                startedAt={view.startedAt}
                durationMs={view.durationMs}
                cwd=""
                answering
                usage={view.tokens.input || view.tokens.output ? { model: view.model, ...view.tokens } : undefined}
                thinkingSeconds={view.thinkingSeconds}
                alwaysShow
              />
            </div>
          ) : null}
          {view.canvas ? <CanvasBlock doc={view.canvas} onCanvasMessage={onCanvasMessage} /> : null}

          {view.question ? <QuestionCard turnId={view.id} question={view.question} /> : null}
          {view.retry ? <RetryLine retry={view.retry} /> : null}

          <div className={cn('group/answer relative mt-1', streaming && 'min-h-[24px]')} data-kind="answer">
            <Answer view={view} />
            {view.answer ? (
              <CopyButton
                text={view.answer.text}
                className="absolute -right-8 top-0 opacity-0 transition-opacity group-hover/answer:opacity-100"
              />
            ) : null}
          </div>

          {view.status === 'interrupted' && view.stoppedAt ? (
            <div className="mt-1 flex items-center gap-1.5 text-2 text-ink-3" data-kind="stopped">
              <Square className="size-3 fill-current" />
              你在 <span className="brand-type">{view.stoppedAt}</span> 停下了这一轮，已经做的留着
            </div>
          ) : null}
          <FailureBlock view={view} />
        </>
      )}
    </div>
  );
});