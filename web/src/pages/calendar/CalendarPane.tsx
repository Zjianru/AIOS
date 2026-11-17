/**
 * 工作页「日历」——约定/周两种看法、详情四个按钮、运行点、服务端算的周计划。
 *
 * 事实只来自 calendar-read；页面不自己算规则、不补运行记录。约定视图画跳过（含没变/暂停），
 * 周视图只画计划点和有结果的运行（服务端 week 已按此给）。
 */

import { useMemo, useState } from 'react';
import { CalendarClock, ChevronLeft, ChevronRight, Eye, Moon, Repeat } from 'lucide-react';
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
import { useKindStore } from '@/pages/kind/kindStore';
import { useWorkStore } from '@/pages/work/workStore';
import {
  GROUP_ORDER,
  RESULT_TONE,
  RESULT_WORD,
  STATE_WORD,
  appointmentsOf,
  clockOf,
  daysOfWeek,
  recentRuns,
  stampInZone,
  stampOf,
  weekCells,
  weekStartOf,
  type ApptKind,
  type ApptRun,
  type Appointment,
} from './calendarModel';
import { useCalendarPolling, useCalendarStore } from './calendarStore';

const STAGE_WORD: Record<string, string> = {
  needs_conditions: '缺条件',
  pending: '排队',
  running: '在做',
  ready_for_review: '待你核',
  requirement_met: '已满足',
  closed: '已关闭',
};

const ICON: Record<ApptKind, typeof CalendarClock> = {
  time: CalendarClock,
  watch: Eye,
  loop: Repeat,
  builtin: Moon,
};

function openTurn(turn: number | null): void {
  if (!turn) return;
  go('kind');
  window.setTimeout(() => {
    void useKindStore.getState().revealTurnNumber(turn);
  }, 80);
}

function RunDot({ run, onClick }: { run: ApptRun; onClick?: () => void }) {
  const tone = RESULT_TONE[run.result];
  const label = stampOf(run.at) + ' · ' + RESULT_WORD[run.result] + (run.note ? '：' + run.note : '');
  return (
    <button
      type="button"
      data-kind="run-dot"
      data-result={run.result}
      title={label}
      onClick={(event) => {
        event.stopPropagation();
        if (onClick) onClick();
        else openTurn(run.turn);
      }}
      className="relative inline-grid size-2 shrink-0 place-items-center rounded-full border border-ink-3/40"
      style={{ backgroundColor: run.result === 'skipped' ? 'transparent' : tone }}
    >
      {run.result === 'skipped' ? <span className="block size-[3px] rounded-full bg-ink-3" /> : null}
    </button>
  );
}

function RunDots({ appt }: { appt: Appointment }) {
  const runs = recentRuns(appt);
  if (!runs.length) return <span className="text-1 text-ink-3">还没跑过</span>;
  return (
    <span className="flex items-center gap-1">
      {runs.map((run) => (
        <span key={run.id} className="flex items-center gap-0.5">
          <RunDot run={run} />
          {run.merged > 0 ? <span className="brand-type text-[10px] text-run">×{run.merged}</span> : null}
        </span>
      ))}
    </span>
  );
}

function ApptRow({ appt, on, onSelect }: { appt: Appointment; on: boolean; onSelect: () => void }) {
  const Icon = ICON[appt.kind];
  const work = useWorkStore((state) => state.work);
  const done = appt.state === 'ended' || appt.state === 'cancelled';
  const reportRounds = appt.runs.filter((run) => run.trigger === 'report').length;
  const boundTask = appt.cycleTaskId
    ? work?.tasks.find((task) => task.id === appt.cycleTaskId)
    : undefined;
  return (
    <button
      type="button"
      data-kind="appt-row"
      data-appt={appt.id}
      data-state={appt.state}
      onClick={onSelect}
      className={cn(
        'w-full rounded-info border px-3 py-2 text-left transition-colors',
        on ? 'border-ink-3/60 bg-raised' : 'border-line bg-pane/30 hover:bg-hover/50',
        done && 'opacity-60',
      )}
    >
      <span className="flex min-w-0 items-center gap-2">
        <Icon className="size-3.5 shrink-0 text-ink-3" />
        <span className="min-w-0 flex-1 truncate text-2 text-ink" title={appt.words}>
          「{appt.words}」
        </span>
        <span className={cn('shrink-0 text-1', appt.state === 'waiting' ? 'text-wait' : appt.state === 'active' ? 'text-done' : 'text-ink-3')}>
          {STATE_WORD[appt.state]}
        </span>
      </span>
      <span className="mt-0.5 flex min-w-0 items-center gap-2 text-1 text-ink-3">
        <span className="min-w-0 truncate">
          {appt.trigger}
          {appt.next ? ' · 下次 ' + stampInZone(appt.next, appt.timezone) : ''}
        </span>
        <span className="ml-auto shrink-0">
          <RunDots appt={appt} />
        </span>
      </span>
      {appt.cycleTaskId ? (
        <span className="mt-1 block truncate text-1 text-ink-3" data-kind="loop-band">
          第 {reportRounds} 轮
          {boundTask ? ' · ' + boundTask.title + ' · ' + (STAGE_WORD[boundTask.stage] ?? boundTask.stage) : ''}
        </span>
      ) : null}
      {appt.stateWhy ? (
        <span className="mt-1 block truncate text-1 text-wait" data-kind="appt-why">
          暂停了：{appt.stateWhy}
        </span>
      ) : null}
    </button>
  );
}

function ApptDetail({ appt }: { appt: Appointment }) {
  const act = useCalendarStore((state) => state.act);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [busy, setBusy] = useState(false);
  const run = async (action: 'pause' | 'resume' | 'run-now' | 'cancel') => {
    setBusy(true);
    try {
      await act(appt.id, action);
      setConfirmCancel(false);
    } finally {
      setBusy(false);
    }
  };
  const builtin = appt.kind === 'builtin';
  const canPause = appt.state === 'active';
  const canResume = appt.state === 'paused' || appt.state === 'waiting';
  const canRun = appt.state === 'active' || appt.state === 'paused' || appt.state === 'waiting';
  const canCancel = !builtin && !(appt.state === 'ended' || appt.state === 'cancelled');
  return (
    <aside className="flex min-h-0 w-[360px] shrink-0 flex-col overflow-y-auto border-l border-line bg-content px-4 py-3" data-kind="appt-detail" data-appt={appt.id}>
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1 text-3 text-ink">约定详情</span>
        <span className={cn('text-1', appt.state === 'waiting' ? 'text-wait' : appt.state === 'active' ? 'text-done' : 'text-ink-3')}>
          {STATE_WORD[appt.state]}
        </span>
      </div>
      <h2 className="mt-2 text-3 leading-6 text-ink" data-kind="detail-words">「{appt.words}」</h2>
      <dl className="mt-3 space-y-1.5 text-2">
        <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">触发</dt><dd className="text-ink-2">{appt.trigger}</dd></div>
        <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">绑定</dt><dd className="text-ink-2">{appt.binding === 'task' ? '任务 ' + (appt.boundTaskId ?? '') : 'Kind'}</dd></div>
        {appt.sourceTurn ? (
          <div className="flex gap-2">
            <dt className="w-20 shrink-0 text-ink-3">来源</dt>
            <dd>
              <button type="button" data-kind="detail-source-turn" onClick={() => openTurn(appt.sourceTurn)} className="text-ink-2 underline-offset-2 hover:underline">
                第 {appt.sourceTurn} 轮
              </button>
            </dd>
          </div>
        ) : (
          <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">来源</dt><dd className="text-ink-3">旧记录（没有来源轮）</dd></div>
        )}
        <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">版本</dt><dd className="text-ink-2">第 {appt.version} 版</dd></div>
        <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">下次触发</dt><dd className="text-ink-2">{appt.next ? stampInZone(appt.next, appt.timezone) : '不自动触发'}</dd></div>
        {appt.watchTarget ? <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">盯变化</dt><dd className="min-w-0 break-all text-ink-2">{appt.watchKind} · {appt.watchTarget}</dd></div> : null}
        {appt.cycleTaskId ? <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">循环</dt><dd className="text-ink-2">任务 {appt.cycleTaskId}{appt.cycleMaxRounds ? ' · 最多 ' + appt.cycleMaxRounds + ' 轮' : ''}</dd></div> : null}
        {appt.stateWhy ? <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">暂停原因</dt><dd className="text-wait">{appt.stateWhy}</dd></div> : null}
      </dl>

      <div className="mt-3 flex flex-wrap gap-1.5" data-kind="appt-actions">
        {canPause ? <button type="button" disabled={busy} data-kind="appt-action" data-action="pause" onClick={() => void run('pause')} className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover disabled:opacity-50">暂停</button> : null}
        {canResume ? <button type="button" disabled={busy} data-kind="appt-action" data-action="resume" onClick={() => void run('resume')} className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover disabled:opacity-50">恢复</button> : null}
        {canRun ? <button type="button" disabled={busy} data-kind="appt-action" data-action="run-now" onClick={() => void run('run-now')} className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover disabled:opacity-50">立即跑一次</button> : null}
        {canCancel ? (
          confirmCancel ? (
            <button type="button" disabled={busy} data-kind="appt-cancel-confirm" onClick={() => void run('cancel')} className="rounded-btn border border-fail/50 bg-fail-soft px-2.5 py-1 text-2 text-fail hover:bg-fail/20 disabled:opacity-50">确认取消</button>
          ) : (
            <button type="button" data-kind="appt-action" data-action="cancel" onClick={() => setConfirmCancel(true)} className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-3 hover:bg-hover">取消</button>
          )
        ) : null}
      </div>

      <div className="mt-4 text-1 text-ink-3">运行记录</div>
      <div className="mt-1 space-y-1" data-kind="appt-runs">
        {appt.runs.length === 0 ? <p className="text-1 text-ink-3">还没跑过</p> : appt.runs.slice(-10).reverse().map((run) => (
          <div key={run.id} className="flex min-w-0 items-start gap-1.5 text-1">
            <span className="mt-[3px]"><RunDot run={run} /></span>
            <span className="shrink-0 text-ink-3">{stampOf(run.at)}</span>
            <span className="shrink-0 text-ink-2">{RESULT_WORD[run.result]}{run.merged ? ' ×' + run.merged : ''}{run.manual ? ' · 手动' : ''}</span>
            <span className="min-w-0 flex-1 truncate text-ink-3" title={run.note}>{run.note}</span>
          </div>
        ))}
      </div>
    </aside>
  );
}

function WeekPane({ weekStart, setWeekStart }: { weekStart: number; setWeekStart: (next: number) => void }) {
  const response = useCalendarStore((state) => state.response);
  const days = daysOfWeek(weekStart);
  const cells = useMemo(() => weekCells(response, weekStart), [response, weekStart]);
  const today = new Date();
  const todayKey = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  return (
    <div className="flex min-h-0 flex-1 flex-col p-3" data-kind="calendar-week">
      <div className="mb-2 flex items-center gap-2 text-2 text-ink-3">
        <button type="button" data-kind="week-shift" data-dir="prev" onClick={() => setWeekStart(weekStartOf(-1))} className="rounded-btn p-1 hover:bg-hover"><ChevronLeft className="size-3.5" /></button>
        <span>{stampOf(weekStart).split(' ')[0]} 这周</span>
        <button type="button" data-kind="week-shift" data-dir="next" onClick={() => setWeekStart(weekStartOf(1))} className="rounded-btn p-1 hover:bg-hover"><ChevronRight className="size-3.5" /></button>
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-7 gap-1.5">
        {days.map((day, index) => {
          const date = new Date(day);
          const isToday = day === todayKey;
          return (
            <section key={day} data-kind="week-day" data-day={index} data-today={isToday ? 'true' : 'false'} className={cn('min-h-0 rounded-card border border-line/70 bg-pane/20 p-1.5', isToday && 'border-wait/50 bg-wait-soft/20')}>
              <div className="mb-1 flex items-baseline justify-between text-1 text-ink-3">
                <span>{['一', '二', '三', '四', '五', '六', '日'][index]}</span>
                <span>{String(date.getMonth() + 1) + '/' + String(date.getDate())}</span>
              </div>
              <div className="space-y-1">
                {cells[index].run.map((point) => (
                  <span key={'run-' + point.appointmentId + '-' + String(point.at)} data-kind="week-run" data-result={point.result ?? ''} title={point.appointmentId + ' · ' + RESULT_WORD[(point.result as keyof typeof RESULT_WORD) ?? 'sent']} className="block size-2 rounded-full" style={{ backgroundColor: RESULT_TONE[(point.result as keyof typeof RESULT_TONE) ?? 'sent'] }} />
                ))}
                {cells[index].plan.map((point) => (
                  <span key={'plan-' + point.appointmentId + '-' + String(point.at)} data-kind="week-plan" title={clockOf(point.at) + ' 计划'} className="block size-2 rounded-full border-[1.5px] border-ink-3/60" />
                ))}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}

export function CalendarPage() {
  const response = useCalendarStore((state) => state.response);
  const error = useCalendarStore((state) => state.error);
  const selected = useCalendarStore((state) => state.selected);
  const select = useCalendarStore((state) => state.select);
  const [view, setView] = useState<'appointments' | 'week'>('appointments');
  const [weekStart, setWeekStart] = useState(() => weekStartOf(0));
  useCalendarPolling(view === 'week' ? weekStart : 0);
  const appointments = useMemo(() => appointmentsOf(response), [response]);
  const chosen = appointments.find((appointment) => appointment.id === selected) ?? null;

  if (!response) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">{error ?? '正在取日历…'}</div>;
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-content" data-kind="calendar-page">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-3">
        <span className="text-2 text-ink-2">日历</span>
        <span className="text-1 text-ink-3">{appointments.length} 条约定</span>
        <div className="ml-auto flex items-center rounded-btn border border-line p-[2px]">
          {(['appointments', 'week'] as const).map((key) => (
            <button key={key} type="button" data-kind="calendar-view" data-view={key} onClick={() => setView(key)} className={cn('rounded-badge px-2 py-0.5 text-2 transition-colors', view === key ? 'bg-hover text-ink' : 'text-ink-3 hover:text-ink-2')}>
              {key === 'appointments' ? '约定' : '周'}
            </button>
          ))}
        </div>
      </header>
      {error ? <div className="px-3 py-1 text-1 text-fail">{error}</div> : null}
      {view === 'week' ? (
        <WeekPane weekStart={weekStart} setWeekStart={setWeekStart} />
      ) : (
        <div className="flex min-h-0 flex-1">
          <div className="min-h-0 flex-1 space-y-3 overflow-y-auto p-3" data-kind="calendar-appointments">
            {GROUP_ORDER.map((group) => {
              const rows = appointments.filter((appointment) => appointment.kind === group.key);
              if (!rows.length) return null;
              const tail = rows.filter((appointment) => appointment.state === 'ended' || appointment.state === 'cancelled');
              const head = rows.filter((appointment) => !(appointment.state === 'ended' || appointment.state === 'cancelled'));
              return (
                <section key={group.key} data-kind="calendar-group" data-group={group.key}>
                  <div className="mb-1 text-1 text-ink-3">{group.label} · {rows.length}</div>
                  <div className="space-y-1.5">
                    {head.map((appointment) => <ApptRow key={appointment.id} appt={appointment} on={appointment.id === selected} onSelect={() => select(appointment.id === selected ? null : appointment.id)} />)}
                    {tail.map((appointment) => <ApptRow key={appointment.id} appt={appointment} on={appointment.id === selected} onSelect={() => select(appointment.id === selected ? null : appointment.id)} />)}
                  </div>
                </section>
              );
            })}
            {appointments.length === 0 ? (
              <p className="rounded-card border border-dashed border-line-2 px-4 py-6 text-center text-2 text-ink-3">还没有约定。和 Kind 说一句要提醒的事，它会出一张确认卡。</p>
            ) : null}
          </div>
          {chosen ? <ApptDetail key={chosen.id + ':' + String(chosen.version) + ':' + chosen.state} appt={chosen} /> : null}
        </div>
      )}
    </div>
  );
}

export function openAppointment(id: string): void {
  useCalendarStore.getState().select(id);
  go('work?wv=cal');
}
