/**
 * T064.6：Kind 对话里日历唤醒那一轮开头的程序小条（不是用户气泡）。
 *
 * 认法：user/message 的 source.rpcId 前缀 appointment-wake:，正文是服务端写的 JSON。
 * 固定词由程序按 trigger 定：到点 / 变了 / 第 N 轮报回；变了能展开看 diff；看约定打开那一条。
 */

import { useState } from 'react';
import { ArrowUpRight, CalendarClock, ChevronRight, Eye, Repeat } from 'lucide-react';
import { openAppointment } from '@/pages/calendar/CalendarPane';
import { cn } from '@/lib/utils';

export interface WakeView {
  why: 'time' | 'changed' | 'report';
  label: string;
  apptId: string;
  words: string;
  merged: number;
  diff: string[] | null;
  attachmentId: string | null;
  turn: number | null;
}

function reasonLines(summary: string): string[] {
  return summary
    .split(String.fromCharCode(10))
    .filter((line) => line.length > 0)
    .slice(0, 200);
}

export function parseWake(rpcId: string | null, text: string): WakeView | null {
  if (!rpcId || !rpcId.startsWith('appointment-wake:')) return null;
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return null;
  }
  if (!body || typeof body !== 'object') return null;
  const root = body as {
    kind?: unknown;
    appointment?: { id?: unknown; condition?: unknown; run_id?: unknown };
    run?: unknown;
    change?: { summary?: unknown; attachment_id?: unknown } | null;
  };
  if (root.kind !== 'appointment_wake' || !root.appointment) return null;
  const appointment = root.appointment;
  if (typeof appointment.id !== 'string' || appointment.id === '') return null;
  const words = typeof appointment.condition === 'string' ? appointment.condition : '';
  const merged = typeof (body as { merged_count?: unknown }).merged_count === 'number' ? ((body as { merged_count: number }).merged_count) : 0;
  const manual = (body as { manual?: unknown }).manual === true;
  const trigger = typeof (body as { trigger?: unknown }).trigger === 'string' ? ((body as { trigger: string }).trigger) : '';
  const reason = typeof (body as { reason?: unknown }).reason === 'string' ? ((body as { reason: string }).reason) : '';
  const change = root.change;
  const summary = change && typeof change.summary === 'string' ? change.summary : '';
  const attachmentId = change && typeof change.attachment_id === 'string' ? change.attachment_id : null;
  const why: WakeView['why'] = trigger === 'report' ? 'report' : trigger === 'changed' ? 'changed' : 'time';
  const label = why === 'report' ? (reason || '第 N 轮报回') : why === 'changed' ? '变了' : manual ? '手动跑一次' : '到点';
  const diff = why === 'changed' && summary.includes('变了') ? reasonLines(summary) : null;
  return {
    why,
    label,
    apptId: appointment.id,
    words,
    merged,
    diff,
    attachmentId,
    turn: typeof (body as { sent_turn?: unknown }).sent_turn === 'number' ? ((body as { sent_turn: number }).sent_turn) : null,
  };
}

const ICON = { time: CalendarClock, changed: Eye, report: Repeat } as const;

export function WakeStrip({ wake }: { wake: WakeView }) {
  const [open, setOpen] = useState(false);
  const Icon = ICON[wake.why];
  return (
    <div className="mb-2 mt-3 rounded-card border border-dashed border-line-2 bg-pane/40 px-3 py-1.5 text-2" data-kind="wake-strip" data-why={wake.why}>
      <div className="flex min-w-0 items-center gap-1.5">
        <Icon className="size-3.5 shrink-0 text-brand" />
        <span className="shrink-0 text-ink-2">{wake.label}</span>
        {wake.merged > 0 ? <span className="shrink-0 text-1 text-ink-3">（合并了 {wake.merged} 次）</span> : null}
        <span className="shrink-0 text-ink-3">·</span>
        <span className="min-w-0 truncate text-ink-3" title={wake.words}>「{wake.words}」</span>
        {wake.diff ? (
          <button type="button" onClick={() => setOpen((value) => !value)} className="ml-auto flex shrink-0 items-center gap-0.5 text-1 text-ink-3 hover:text-ink-2" data-kind="wake-diff-toggle">
            改了哪里
            <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
          </button>
        ) : null}
        <button type="button" onClick={() => openAppointment(wake.apptId)} className={cn('flex shrink-0 items-center gap-0.5 text-1 text-ink-3 hover:text-ink-2', !wake.diff && 'ml-auto')} data-kind="wake-open">
          看约定 <ArrowUpRight className="size-3" />
        </button>
      </div>
      {open && wake.diff ? (
        <pre className="process-path mt-1.5 whitespace-pre-wrap break-words rounded-info bg-content/60 px-2 py-1 text-1 leading-4" data-kind="wake-diff">
          {wake.diff.map((line, index) => (
            <div key={index} className={line.startsWith('+') ? 'text-done' : line.startsWith('-') ? 'text-fail' : 'text-ink-3'}>{line}</div>
          ))}
        </pre>
      ) : null}
      {wake.attachmentId ? (
        <div className="mt-1 text-1 text-ink-3" data-kind="wake-attachment">完整 diff 在附件 {wake.attachmentId}</div>
      ) : null}
    </div>
  );
}
