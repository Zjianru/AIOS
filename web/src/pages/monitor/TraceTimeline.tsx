/**
 * 诊断链路：三泳道按事件时间横向排布；模型块拆「等首字 / 生成」，工具块有耗时，
 * step/start 画分界线；可拖选一段时间只看这段。点块/行只改 selected，详情由诊断页同处渲染。
 */
import { useEffect, useMemo, useRef, useState } from 'react';
import type { ServiceEvent } from '@/data/service';
import { cn } from '@/lib/utils';
import { fmtTime } from './monitorParts';

const MODEL_COLOR = 'var(--kf-c1)';
const TOOL_COLOR = 'var(--kf-c4)';
const OTHER_COLOR = 'var(--kf-c7)';

type Lane = 'model' | 'tool' | 'other';

interface TraceBlock {
  id: string;
  lane: Lane;
  label: string;
  event: ServiceEvent;
  start: number;
  end: number;
  waitMs?: number;
  generateMs?: number;
  step?: number;
  missing?: boolean;
}

interface StepMark {
  time: number;
  turn: number;
  step: number;
}

function streamTimes(data: Record<string, unknown>): { first: number; last: number } | null {
  const stream = Array.isArray(data.stream) ? data.stream : [];
  const times: number[] = [];
  for (const record of stream) {
    if (!record || typeof record !== 'object') continue;
    const item = record as Record<string, unknown>;
    if (typeof item.time === 'number') times.push(item.time);
    if (typeof item.time0 === 'number') {
      const dt = Array.isArray(item.dt) ? item.dt.filter((value): value is number => typeof value === 'number') : [];
      times.push(item.time0);
      times.push(dt.reduce((sum, value) => sum + value, item.time0));
    }
  }
  if (!times.length) return null;
  return { first: Math.min(...times), last: Math.max(...times) };
}

function callIdOf(event: ServiceEvent): string | null {
  const data = event.data as Record<string, unknown>;
  if (event.type === 'tool/call') return typeof data.callId === 'string' ? data.callId : null;
  if (event.type === 'tool/result') {
    const message = data.message as { toolCallId?: unknown } | undefined;
    return typeof message?.toolCallId === 'string' ? message.toolCallId : null;
  }
  return null;
}

function eventLabel(event: ServiceEvent): string {
  const data = event.data as Record<string, unknown>;
  if (event.type === 'assistant/message') return '模型回复';
  if (event.type === 'assistant/attempt') return '模型尝试';
  if (event.type === 'tool/call') return String(data.name ?? '工具');
  if (event.type === 'tool/result') return '工具结果';
  if (event.type === 'step/start') return '一步开始';
  if (event.type === 'compaction/summary') return '压缩摘要';
  return event.type;
}

function buildBlocks(events: ServiceEvent[]): TraceBlock[] {
  const blocks: TraceBlock[] = [];
  const calls = new Map<string, ServiceEvent>();
  for (const event of events) {
    if (event.type === 'tool/call') {
      const id = callIdOf(event);
      if (id) calls.set(id, event);
    }
    if (event.type === 'tool/result') {
      const id = callIdOf(event);
      const call = id ? calls.get(id) : undefined;
      if (!call) continue;
      blocks.push({
        id: event.type + ':' + event.seq,
        lane: 'tool',
        event: call,
        label: eventLabel(call),
        start: call.time,
        end: Math.max(call.time, event.time),
      });
      calls.delete(id!);
    }
  }
  for (const call of calls.values()) {
    blocks.push({
      id: 'tool/call:' + call.seq,
      lane: 'tool',
      event: call,
      label: eventLabel(call),
      start: call.time,
      end: call.time,
      missing: true,
    });
  }

  for (const event of events) {
    const data = event.data as Record<string, unknown>;
    if (event.type === 'assistant/message' || event.type === 'assistant/attempt') {
      const stream = streamTimes(data);
      const requestAt = typeof data.requestAt === 'number' ? data.requestAt : null;
      const start = requestAt ?? stream?.first ?? event.time;
      const end = Math.max(start, stream?.last ?? event.time);
      const first = stream?.first ?? null;
      const waitMs = requestAt != null && first != null ? Math.max(0, first - requestAt) : undefined;
      const generateMs = first != null ? Math.max(0, end - first) : undefined;
      blocks.push({
        id: event.type + ':' + event.seq,
        lane: 'model',
        event,
        label: eventLabel(event),
        start,
        end,
        waitMs,
        generateMs,
        step: typeof data.step === 'number' ? data.step : undefined,
        missing: requestAt == null && stream == null,
      });
    } else if (event.type !== 'tool/call' && event.type !== 'tool/result' && event.type !== 'step/start' && event.type !== 'step/end') {
      blocks.push({
        id: event.type + ':' + event.seq,
        lane: 'other',
        event,
        label: eventLabel(event),
        start: event.time,
        end: event.time,
      });
    }
  }
  return blocks;
}

function stepMarks(events: ServiceEvent[]): StepMark[] {
  return events
    .filter((event) => event.type === 'step/start')
    .map((event) => {
      const data = event.data as Record<string, unknown>;
      return {
        time: event.time,
        turn: typeof data.turn === 'number' ? data.turn : 0,
        step: typeof data.step === 'number' ? data.step : 0,
      };
    });
}

function duration(ms: number | undefined): string {
  if (ms == null) return '没采到';
  if (ms < 1_000) return ms + '毫秒';
  return (ms / 1_000).toFixed(1) + '秒';
}

export function TraceTimeline({
  events,
  selected,
  onSelect,
  onRangeChange,
}: {
  events: ServiceEvent[];
  selected: ServiceEvent | null;
  onSelect: (event: ServiceEvent) => void;
  onRangeChange?: (range: { from: number; to: number } | null) => void;
}) {
  const blocks = useMemo(() => buildBlocks(events), [events]);
  const marks = useMemo(() => stepMarks(events), [events]);
  const start = useMemo(() => Math.min(...events.map((event) => event.time), ...blocks.map((block) => block.start)), [events, blocks]);
  const end = useMemo(() => Math.max(...events.map((event) => event.time), ...blocks.map((block) => block.end)), [events, blocks]);
  const span = Math.max(1, end - start);
  const track = useRef<HTMLDivElement>(null);
  const [range, setRange] = useState<{ from: number; to: number; x1: number; x2: number } | null>(null);
  const [dragStart, setDragStart] = useState<number | null>(null);

  const percent = (time: number) => Math.max(0, Math.min(100, ((time - start) / span) * 100));
  useEffect(() => {
    onRangeChange?.(range ? { from: range.from, to: range.to } : null);
  }, [range, onRangeChange]);
  const inRange = (block: TraceBlock) => !range || (block.end >= range.from && block.start <= range.to);
  const visibleBlocks = blocks.filter(inRange);
  const timeAt = (clientX: number) => {
    const rect = track.current?.getBoundingClientRect();
    if (!rect || rect.width === 0) return start;
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    return Math.round(start + ratio * span);
  };

  if (!events.length) {
    return <p className="text-2 text-ink-3">这个 Attempt 没有事件。</p>;
  }

  return (
    <div data-kind="trace-timeline">
      <div className="mb-2 flex items-center gap-2 text-1 text-ink-3">
        <span>{fmtTime(start)}</span>
        <span className="flex-1" />
        {range ? (
          <span className="flex items-center gap-1" data-kind="trace-range">
            只看 {fmtTime(range.from)}–{fmtTime(range.to)}
            <button type="button" className="underline" onClick={() => setRange(null)}>清空</button>
          </span>
        ) : (
          <span>拖选一段只看这段</span>
        )}
        <span>{fmtTime(end)}</span>
      </div>
      <div
        ref={track}
        data-kind="trace-track"
        className="relative select-none rounded-btn border border-line bg-content p-2 pt-5"
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          const x = timeAt(event.clientX);
          setDragStart(x);
          setRange({ from: x, to: x, x1: x, x2: x });
        }}
        onPointerMove={(event) => {
          if (dragStart == null) return;
          const x = timeAt(event.clientX);
          setRange({ from: Math.min(dragStart, x), to: Math.max(dragStart, x), x1: dragStart, x2: x });
        }}
        onPointerUp={(event) => {
          if (dragStart == null) return;
          const x = timeAt(event.clientX);
          const from = Math.min(dragStart, x);
          const to = Math.max(dragStart, x);
          setDragStart(null);
          setRange(to - from < 2 ? null : { from, to, x1: dragStart, x2: x });
        }}
        onPointerLeave={() => {
          if (dragStart != null) {
            setDragStart(null);
            setRange(null);
          }
        }}
      >
        {marks.map((mark) => (
          <div
            key={mark.time + ':' + mark.turn + ':' + mark.step}
            data-kind="trace-step"
            className="pointer-events-none absolute bottom-1 top-1 w-px border-l border-dashed border-ink-3/40"
            style={{ left: percent(mark.time) + '%' }}
            title={'第 ' + mark.turn + ' 轮 · 第 ' + mark.step + ' 步'}
          />
        ))}
        {(['model', 'tool', 'other'] as const).map((lane) => (
          <div key={lane} className="relative mb-2 h-9 border-t border-line/50 first:border-t-0" data-lane={lane}>
            <span className="absolute left-1 top-1 text-1 text-ink-3">
              {lane === 'model' ? '模型' : lane === 'tool' ? '工具' : '其他'}
            </span>
            {visibleBlocks
              .filter((block) => block.lane === lane)
              .map((block) => {
                const left = percent(block.start);
                const width = Math.max(0.6, percent(block.end) - left);
                const active = selected?.seq === block.event.seq;
                const waitShare =
                  block.waitMs != null && block.waitMs + (block.generateMs ?? 0) > 0
                    ? block.waitMs / (block.waitMs + (block.generateMs ?? 0))
                    : 0;
                return (
                  <button
                    key={block.id}
                    type="button"
                    data-kind="trace-block"
                    data-seq={block.event.seq}
                    data-selected={active ? 'true' : 'false'}
                    title={
                      eventLabel(block.event) +
                      ' · ' +
                      duration(block.end - block.start) +
                      (block.waitMs != null ? '（等首字 ' + duration(block.waitMs) + ' / 生成 ' + duration(block.generateMs) + '）' : '')
                    }
                    onClick={(event) => {
                      event.stopPropagation();
                      onSelect(block.event);
                    }}
                    className={cn(
                      'absolute top-5 h-3 overflow-hidden rounded-[3px] border px-1 text-left text-[10px] leading-3 text-ink',
                      active ? 'border-ink bg-active shadow-[0_0_0_1px_var(--ink)]' : 'border-transparent',
                    )}
                    style={{
                      left: left + '%',
                      width: width + '%',
                      minWidth: 6,
                      background: lane === 'model' ? MODEL_COLOR : lane === 'tool' ? TOOL_COLOR : OTHER_COLOR,
                      opacity: block.missing ? 0.45 : 0.85,
                    }}
                  >
                    {lane === 'model' && block.waitMs != null ? (
                      <>
                        <span
                          data-kind="trace-wait"
                          className="absolute inset-y-0 left-0 bg-ink/25"
                          style={{ width: waitShare * 100 + '%' }}
                          title={'等首字 ' + duration(block.waitMs)}
                        />
                        <span data-kind="trace-generate" className="relative z-10 truncate">
                          {eventLabel(block.event)}
                        </span>
                      </>
                    ) : (
                      <span className="relative z-10 truncate">{eventLabel(block.event)}</span>
                    )}
                  </button>
                );
              })}
          </div>
        ))}
        {range ? (
          <div
            data-kind="trace-selection"
            className="pointer-events-none absolute inset-y-0 border-x border-ink/40 bg-ink/5"
            style={{ left: Math.min(percent(range.x1), percent(range.x2)) + '%', width: Math.abs(percent(range.x2) - percent(range.x1)) + '%' }}
          />
        ) : null}
      </div>
      <div className="mt-1 flex gap-3 text-1 text-ink-3">
        <span>模型：等首字/生成</span>
        <span>工具：调用到结果</span>
        <span className="text-ink-3">虚线是每一步开始</span>
        <span>{visibleBlocks.length} 块</span>
      </div>
    </div>
  );
}
