/**
 * 工作页「日历」——约定/周两种看法、详情四个按钮、运行点、服务端算的周计划。
 *
 * 事实只来自 calendar-read；页面不自己算规则、不补运行记录。约定视图画跳过（含没变/暂停），
 * 周视图只画计划点和有结果的运行（服务端 week 已按此给）。
 */

import { useEffect, useMemo, useState } from 'react';
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
  colorsOf,
  dayStartOf,
  daysOfWeek,
  denseOf,
  monthCells,
  monthStartOf,
  plansOf,
  rangeBlocksOf,
  recentRuns,
  ruleColor,
  stampInZone,
  stampOf,
  weekStartOf,
  type ApptKind,
  type ApptRun,
  type Appointment,
  type DenseDay,
  type PlanBlock,
  type RangeBlock,
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
      {appt.cycleTaskId || appt.cycleMaxRounds != null ? (
        <span className="mt-1 block truncate text-1 text-ink-3" data-kind="loop-band">
          第 {reportRounds} 轮
          {boundTask
            ? ' · ' + boundTask.title + ' · ' + (STAGE_WORD[boundTask.stage] ?? boundTask.stage)
            : ' · 任务不在工作快照'}
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
  const response = useCalendarStore((state) => state.response);
  const blocks = rangeBlocksOf(response).filter((block) => block.appointmentId === appt.id);
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
        {appt.durationMinutes != null ? <div className="flex gap-2"><dt className="w-20 shrink-0 text-ink-3">时长</dt><dd className="text-ink-2">{appt.durationMinutes} 分钟</dd></div> : null}
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

      {blocks.length ? (
        <div className="mt-3" data-kind="appt-block-list">
          <div className="mb-1 text-1 text-ink-3">这次到点</div>
          {blocks.slice().reverse().map((block) => (
            <div key={block.id} className="flex items-center gap-2 text-1" data-kind="appt-block" data-status={block.status}>
              <span className="text-ink-3">{stampOf(block.start)}</span>
              <span className="shrink-0 text-ink-2">{block.status}</span>
              {block.taskId ? (
                <button type="button" data-kind="block-task-link" data-task={block.taskId} onClick={() => go('taskchat:' + block.taskId)} className="min-w-0 truncate text-ink-3 underline-offset-2 hover:underline">
                  看任务
                </button>
              ) : null}
            </div>
          ))}
        </div>
      ) : null}

      <div className="mt-4 text-1 text-ink-3">运行记录</div>
      <div className="mt-1 space-y-1" data-kind="appt-runs">
        {appt.runs.length === 0 ? <p className="text-1 text-ink-3">还没跑过</p> : appt.runs.slice(-10).reverse().map((run) => (
          <div key={run.id} className="flex min-w-0 items-start gap-1.5 text-1">
            <span className="mt-[3px]"><RunDot run={run} /></span>
            <span className="shrink-0 text-ink-3">{stampOf(run.at)}</span>
            {run.result === 'sent' && run.turn ? (
              <button type="button" data-kind="run-kind-done" onClick={() => openTurn(run.turn)} className="shrink-0 text-ink-2 underline-offset-2 hover:underline">Kind 办完了</button>
            ) : (
              <span className="shrink-0 text-ink-2">{RESULT_WORD[run.result]}{run.merged ? ' ×' + run.merged : ''}{run.manual ? ' · 手动' : ''}</span>
            )}
            <span className="min-w-0 flex-1 truncate text-ink-3" title={run.note}>{run.note}</span>
          </div>
        ))}
      </div>
    </aside>
  );
}

const HOUR_PX = 44;
const DAY_MS = 86_400_000;

function minutesOfDay(ms: number): number {
  const date = new Date(ms);
  return date.getHours() * 60 + date.getMinutes();
}

function isToday(ms: number): boolean {
  return dayStartOf(ms) === dayStartOf(Date.now());
}

/** 一天里的重叠分组：按开始排序，给每块分一条竖道并算总道数。 */
function lanesOf<T extends { start: number; end: number }>(blocks: T[]): (T & { lane: number; lanes: number })[] {
  const sorted = [...blocks].sort((left, right) => left.start - right.start);
  const result: (T & { lane: number; lanes: number })[] = [];
  let group: T[] = [];
  let groupEnd = 0;
  const flush = () => {
    if (!group.length) return;
    const lanes = group.length;
    group.forEach((block, lane) => result.push({ ...block, lane, lanes }));
    group = [];
  };
  for (const block of sorted) {
    if (group.length && block.start >= groupEnd) flush();
    group.push(block);
    groupEnd = Math.max(groupEnd, block.end);
  }
  flush();
  return result;
}

function WeekBlock({ block, color, onSelect, highlighted, onHover }: {
  block: RangeBlock; color: string; onSelect: () => void; highlighted: boolean; onHover: (id: string | null) => void;
}) {
  const start = Math.max(block.start, dayStartOf(block.start));
  const top = (minutesOfDay(start) / 1440) * 24 * HOUR_PX;
  const height = Math.max(18, ((Math.min(block.end, dayStartOf(block.start) + DAY_MS) - start) / 60_000) * (HOUR_PX / 60));
  return (
    <button
      type="button"
      data-kind="week-run"
      data-result="sent"
      data-appt={block.appointmentId}
      data-status={block.status}
      data-task={block.taskId ?? ''}
      data-turn={block.turn ?? ''}
      title={block.title + ' · ' + block.status}
      onClick={(event) => {
        event.stopPropagation();
        if (block.taskId) go('taskchat:' + block.taskId);
        else onSelect();
      }}
      onMouseEnter={() => onHover(block.appointmentId)}
      onMouseLeave={() => onHover(null)}
      className={cn('absolute z-10 overflow-hidden rounded-[4px] border px-1 py-0.5 text-left text-[10px] text-ink', highlighted && 'ring-2 ring-ink/50')}
      style={{ top, height, borderColor: color, backgroundColor: color + '33', left: '2px', right: '2px' }}
    >
      <span className="block truncate leading-3">{block.title}</span>
      <span className="block truncate text-ink-3">{block.status}</span>
    </button>
  );
}

function WeekPane({ weekStart, setWeekStart, onSelect }: { weekStart: number; setWeekStart: (next: number) => void; onSelect: (id: string) => void }) {
  const response = useCalendarStore((state) => state.response);
  const [hover, setHover] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const days = daysOfWeek(weekStart);
  const ranges = useMemo(() => rangeBlocksOf(response), [response]);
  const plans = useMemo(() => plansOf(response), [response]);
  const dense = useMemo(() => denseOf(response), [response]);
  const colors = useMemo(() => colorsOf(response), [response]);
  const crossDay = (block: RangeBlock) => dayStartOf(block.start) !== dayStartOf(block.end);
  const allDayRuns = ranges.filter(crossDay);
  const allDayPlans = plans.filter((plan) => dayStartOf(plan.start) !== dayStartOf(plan.end));
  const denseByDay = (day: number) => dense.filter((item) => item.day === day);
  const todayIndex = days.findIndex(isToday);
  const nowTop = todayIndex >= 0 ? (minutesOfDay(now) / 1440) * 24 * HOUR_PX : 0;
  return (
    <div className="flex min-h-0 flex-1 flex-col p-3" data-kind="calendar-week">
      <div className="mb-2 flex items-center gap-2 text-2 text-ink-3">
        <button type="button" data-kind="week-shift" data-dir="prev" onClick={() => setWeekStart(weekStart - 7 * DAY_MS)} className="rounded-btn p-1 hover:bg-hover"><ChevronLeft className="size-3.5" /></button>
        <span data-kind="week-title">{stampOf(weekStart).split(' ')[0]} 这周</span>
        <button type="button" data-kind="week-shift" data-dir="next" onClick={() => setWeekStart(weekStart + 7 * DAY_MS)} className="rounded-btn p-1 hover:bg-hover"><ChevronRight className="size-3.5" /></button>
        <button type="button" data-kind="week-today" onClick={() => setWeekStart(weekStartOf(0))} className="rounded-btn border border-line px-2 py-0.5 text-1 hover:bg-hover">今天</button>
      </div>
      <div className="grid shrink-0 grid-cols-[42px_repeat(7,minmax(0,1fr))] gap-0 border-b border-line pb-1">
        <span />
        {days.map((day, index) => (
          <div key={day} data-kind="week-day" data-day={index} data-today={isToday(day) ? 'true' : 'false'} className={cn('px-1 text-center text-1 text-ink-3', isToday(day) && 'rounded bg-fail-soft/30 text-fail')}>
            {['一', '二', '三', '四', '五', '六', '日'][index]} {String(new Date(day).getMonth() + 1)}/{String(new Date(day).getDate())}
          </div>
        ))}
      </div>
      <div className="grid shrink-0 grid-cols-[42px_repeat(7,minmax(0,1fr))] border-b border-line/60" data-kind="week-allday">
        <span className="py-1 text-right text-[9px] text-ink-3">全天</span>
        {days.map((day) => (
          <div key={day} className="relative min-h-[26px] border-l border-line/40 px-0.5 py-0.5">
            {denseByDay(day).map((item: DenseDay) => (
              <span key={item.appointmentId} data-kind="dense-strip" data-appt={item.appointmentId} className="mb-0.5 block truncate rounded bg-hover px-1 text-[9px] text-ink-3" title={item.title}>
                查了 {item.runs} 次 · 没变 {item.unchanged} 次
              </span>
            ))}
            {allDayRuns.filter((block) => block.start < day + DAY_MS && block.end >= day).map((block) => {
              const first = dayStartOf(block.start) === day;
              return (
                <button
                  key={block.id + ':' + day}
                  type="button"
                  data-kind="week-run"
                  data-allday="true"
                  data-result="sent"
                  data-appt={block.appointmentId}
                  data-status={block.status}
                  data-task={block.taskId ?? ''}
                  data-turn={block.turn ?? ''}
                  title={block.title + ' · ' + block.status}
                  onClick={(event) => {
                    event.stopPropagation();
                    if (block.taskId) go('taskchat:' + block.taskId);
                    else onSelect(block.appointmentId);
                  }}
                  onMouseEnter={() => setHover(block.appointmentId)}
                  onMouseLeave={() => setHover(null)}
                  className={cn('mb-0.5 block w-full truncate rounded px-1 text-left text-[9px] text-ink', hover === block.appointmentId && 'ring-1 ring-ink/50')}
                  style={{ backgroundColor: ruleColor(colors.get(block.appointmentId)) + '44' }}
                >
                  {first ? block.title : '\u00a0'}
                </button>
              );
            })}
            {allDayPlans.filter((plan) => plan.start < day + DAY_MS && plan.end >= day).map((plan, index) => (
              <span key={plan.appointmentId + ':' + String(plan.start) + ':' + index} data-kind="week-plan" className="mb-0.5 block h-2 w-full rounded border border-dashed" style={{ borderColor: ruleColor(plan.color) }} />
            ))}
          </div>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto" data-kind="week-hours">
        <div className="grid grid-cols-[42px_repeat(7,minmax(0,1fr))]" style={{ height: 24 * HOUR_PX }}>
          <div className="relative">
            {Array.from({ length: 24 }, (_, hour) => (
              <span key={hour} className="absolute right-1 text-[9px] text-ink-3" style={{ top: hour * HOUR_PX }}>{String(hour).padStart(2, '0')}:00</span>
            ))}
          </div>
          {days.map((day) => {
            const dayRuns = ranges.filter((block) => !crossDay(block) && dayStartOf(block.start) === day);
            const dayPlans = plans.filter((plan) => dayStartOf(plan.start) === day);
            const lanes = lanesOf(dayRuns);
            return (
              <div key={day} className={cn('relative border-l border-line/40', isToday(day) && 'bg-fail-soft/10')}>
                {Array.from({ length: 24 }, (_, hour) => (
                  <span key={hour} className="absolute inset-x-0 border-t border-line/20" style={{ top: hour * HOUR_PX }} />
                ))}
                {dayPlans.map((plan: PlanBlock, index) => {
                  const top = (minutesOfDay(plan.start) / 1440) * 24 * HOUR_PX;
                  const height = Math.max(14, ((plan.end - plan.start) / 60_000) * (HOUR_PX / 60));
                  return (
                    <span
                      key={'plan:' + plan.appointmentId + ':' + String(plan.start) + ':' + index}
                      data-kind="week-plan"
                      data-appt={plan.appointmentId}
                      title={plan.title + ' · 计划'}
                      className="absolute z-[5] rounded-[3px] border border-dashed opacity-80"
                      style={{
                        top, height, left: '2px', right: '2px',
                        borderColor: ruleColor(plan.color),
                        backgroundImage: 'repeating-linear-gradient(45deg, transparent 0 4px, ' + ruleColor(plan.color) + '55 4px 6px)',
                      }}
                    />
                  );
                })}
                {lanes.map((block) => (
                  <div key={block.id} className="absolute inset-0" style={{ left: (block.lane / block.lanes) * 100 + '%', width: 100 / block.lanes + '%' }}>
                    <WeekBlock block={block} color={ruleColor(colors.get(block.appointmentId))} onSelect={() => onSelect(block.appointmentId)} highlighted={hover === block.appointmentId} onHover={setHover} />
                  </div>
                ))}
                {isToday(day) ? <span data-kind="now-line" className="absolute inset-x-0 z-20 border-t border-fail" style={{ top: nowTop }} /> : null}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function MonthPane({ monthStart, setMonthStart, setWeekStart, onSelect }: {
  monthStart: number; setMonthStart: (next: number) => void; setWeekStart: (next: number) => void; onSelect: (id: string) => void;
}) {
  const response = useCalendarStore((state) => state.response);
  const [hover, setHover] = useState<string | null>(null);
  const ranges = useMemo(() => rangeBlocksOf(response), [response]);
  const plans = useMemo(() => plansOf(response), [response]);
  const dense = useMemo(() => denseOf(response), [response]);
  const colors = useMemo(() => colorsOf(response), [response]);
  const cells = monthCells(monthStart);
  const month = new Date(monthStart).getMonth();
  return (
    <div className="flex min-h-0 flex-1 flex-col p-3" data-kind="calendar-month">
      <div className="mb-2 flex items-center gap-2 text-2 text-ink-3">
        <button type="button" data-kind="month-shift" data-dir="prev" onClick={() => setMonthStart(monthStartOf(-1))} className="rounded-btn p-1 hover:bg-hover"><ChevronLeft className="size-3.5" /></button>
        <span data-kind="month-title">{new Date(monthStart).getFullYear()} 年 {month + 1} 月</span>
        <button type="button" data-kind="month-shift" data-dir="next" onClick={() => setMonthStart(monthStartOf(1))} className="rounded-btn p-1 hover:bg-hover"><ChevronRight className="size-3.5" /></button>
        <button type="button" data-kind="month-today" onClick={() => setMonthStart(monthStartOf(0))} className="rounded-btn border border-line px-2 py-0.5 text-1 hover:bg-hover">今天</button>
      </div>
      <div className="grid grid-cols-7 border-l border-t border-line/60 text-1 text-ink-3">
        {['一', '二', '三', '四', '五', '六', '日'].map((label) => <span key={label} className="border-b border-r border-line/60 px-1 py-0.5 text-center">{label}</span>)}
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-7 grid-rows-6 border-l border-line/60" data-kind="month-grid">
        {cells.map((day) => {
          const blocks = [
            ...ranges.filter((block) => block.start < day + DAY_MS && block.end >= day).map((block) => ({ kind: 'run' as const, start: block.start, block })),
            ...plans.filter((plan) => dayStartOf(plan.start) === day).map((plan) => ({ kind: 'plan' as const, start: plan.start, plan })),
          ].sort((left, right) => left.start - right.start);
          const visible = blocks.slice(0, 3);
          const more = blocks.length - visible.length;
          return (
            <div key={day} data-kind="month-day" data-day={String(day)} data-today={isToday(day) ? 'true' : 'false'} className={cn('min-h-0 overflow-hidden border-b border-r border-line/60 p-0.5', isToday(day) && 'bg-fail-soft/15', new Date(day).getMonth() !== month && 'opacity-40')}>
              <button type="button" className="block w-full text-left text-[9px] text-ink-3" onClick={() => { setWeekStart(day); }}>{new Date(day).getDate()}</button>
              {visible.map((item) => item.kind === 'run' ? (
                <button
                  key={'run:' + item.block.id + ':' + day}
                  type="button"
                  data-kind="month-block"
                  data-appt={item.block.appointmentId}
                  data-status={item.block.status}
                  data-task={item.block.taskId ?? ''}
                  title={item.block.title + ' · ' + item.block.status}
                  onClick={(event) => {
                    event.stopPropagation();
                    if (item.block.taskId) go('taskchat:' + item.block.taskId);
                    else onSelect(item.block.appointmentId);
                  }}
                  onMouseEnter={() => setHover(item.block.appointmentId)}
                  onMouseLeave={() => setHover(null)}
                  className={cn('mb-0.5 block w-full truncate rounded bg-hover/60 px-1 text-left text-[9px] text-ink', hover === item.block.appointmentId && 'ring-1 ring-ink/50')}
                  style={{ borderLeft: '3px solid ' + ruleColor(colors.get(item.block.appointmentId)) }}
                >
                  {item.block.title} · {item.block.status}
                </button>
              ) : (
                <span key={'plan:' + item.plan.appointmentId + ':' + String(item.plan.start)} data-kind="month-plan" className="mb-0.5 block h-2 w-full rounded border border-dashed" style={{ borderColor: ruleColor(item.plan.color) }} />
              ))}
              {more > 0 ? (
                <button type="button" data-kind="month-more" data-day={String(day)} onClick={() => { setWeekStart(day); }} className="block text-[9px] text-ink-3 hover:text-ink">还有 {more} 条</button>
              ) : null}
              {dense.filter((item) => item.day === day).map((item) => (
                <span key={item.appointmentId} data-kind="dense-strip" className="block truncate text-[8px] text-ink-3">{item.title} 查了 {item.runs} 次</span>
              ))}
            </div>
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
  const [view, setView] = useState<'appointments' | 'week' | 'month'>('appointments');
  const [weekStart, setWeekStart] = useState(() => weekStartOf(0));
  const [monthStart, setMonthStart] = useState(() => monthStartOf(0));
  const days = daysOfWeek(weekStart);
  const cells = monthCells(monthStart);
  const range = useMemo(() => {
    if (view === 'week') return { from: days[0], to: days[6] + DAY_MS - 1 };
    if (view === 'month') return { from: cells[0], to: cells[41] + DAY_MS - 1 };
    return { from: 0, to: 0 };
  }, [view, weekStart, monthStart]);
  useCalendarPolling(range.from, range.to);
  const appointments = useMemo(() => appointmentsOf(response), [response]);
  const chosen = appointments.find((appointment) => appointment.id === selected) ?? null;

  if (!response) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">{error ?? '正在取日历…'}</div>;
  }
  const openWeek = (day: number) => {
    setWeekStart(dayStartOf(day));
    setView('week');
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-content" data-kind="calendar-page">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-3">
        <span className="text-2 text-ink-2">日历</span>
        <span className="text-1 text-ink-3">{appointments.length} 条约定</span>
        <div className="ml-auto flex items-center rounded-btn border border-line p-[2px]">
          {(['appointments', 'week', 'month'] as const).map((key) => (
            <button key={key} type="button" data-kind="calendar-view" data-view={key} onClick={() => setView(key)} className={cn('rounded-badge px-2 py-0.5 text-2 transition-colors', view === key ? 'bg-hover text-ink' : 'text-ink-3 hover:text-ink-2')}>
              {key === 'appointments' ? '约定' : key === 'week' ? '周' : '月'}
            </button>
          ))}
        </div>
      </header>
      {error ? <div className="px-3 py-1 text-1 text-fail">{error}</div> : null}
      <div className="flex min-h-0 flex-1" data-kind="calendar-body" data-view={view}>
        {view === 'week' ? (
          <WeekPane weekStart={weekStart} setWeekStart={setWeekStart} onSelect={(id) => select(id === selected ? null : id)} />
        ) : view === 'month' ? (
          <MonthPane monthStart={monthStart} setMonthStart={setMonthStart} setWeekStart={(day) => openWeek(day)} onSelect={(id) => select(id === selected ? null : id)} />
        ) : (
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
        )}
        {chosen ? <ApptDetail key={chosen.id + ':' + String(chosen.version) + ':' + chosen.state} appt={chosen} /> : null}
      </div>
    </div>
  );
}

export function openAppointment(id: string): void {
  useCalendarStore.getState().select(id);
  go('work?wv=cal');
}
