/**
 * 监控 · 任务诊断（#monitor:task:<id>）：大数字、现在、todo、上下文五块、三泳道、事件表和详情。
 * 只读；数据来自 Snapshot 摘要 + MonitorTask。Kind 没有诊断。
 */

import { useEffect, useState } from 'react';
import { ArrowLeft, X } from 'lucide-react';
import { go } from '@/app/navigate';
import type { MonitorAttempt, MonitorContextParts, ServiceEvent } from '@/data/service';
import { formatTokens } from '@/lib/numbers';
import { elapsedSeconds } from './diagnoseTime';
import { TraceTimeline } from './TraceTimeline';
import { cn } from '@/lib/utils';
import { useMonitorStore } from './monitorStore';
import { useProjectName } from '@/pages/project/projectStore';
import { Card, Kpi, Missing, StackBar, fmtClock, fmtTime, useTick } from './monitorParts';

function redact(value: unknown, key = ''): unknown {
  if (typeof value === 'string') {
    if (/^(reasoning|thinking|signature)$/i.test(key)) return '（思考不显示）';
    return value.replace(/sk-[A-Za-z0-9_-]{8,}/g, 'sk-***').slice(0, 4000);
  }
  if (Array.isArray(value)) return value.map((item) => redact(item));
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [childKey, child] of Object.entries(value)) {
      if (/^(reasoning|thinking|signature)$/i.test(childKey)) {
        out[childKey] = '（思考不显示）';
      } else if (/rawOutput/i.test(childKey)) {
        out[childKey] = '（原始输出不显示）';
      } else {
        out[childKey] = redact(child, childKey);
      }
    }
    return out;
  }
  return value;
}

function laneOf(event: ServiceEvent): 'model' | 'tool' | 'other' {
  if (event.type.startsWith('assistant') || event.type.startsWith('step') || event.type.startsWith('turn') || event.type.startsWith('compaction')) return 'model';
  if (event.type.startsWith('tool') || event.type.startsWith('job') || event.type.startsWith('bash')) return 'tool';
  return 'other';
}

function eventLabel(event: ServiceEvent): string {
  const data = event.data as Record<string, unknown>;
  if (event.type === 'assistant/message') return '模型回复';
  if (event.type === 'assistant/attempt') return '模型尝试';
  if (event.type === 'step/start') return '一步开始';
  if (event.type === 'tool/call') return String(data.name ?? '工具调用');
  if (event.type === 'tool/result') return '工具结果';
  if (event.type === 'compaction/summary') return '压缩摘要';
  if (event.type === 'todo/write') return 'todo';
  return event.type;
}

function lastDone(events: ServiceEvent[]): { label: string; at: number; summary: string } | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    const data = event.data as Record<string, unknown>;
    if (event.type === 'tool/call' && data.name === 'task_report') {
      try {
        const args = typeof data.arguments === 'string' ? JSON.parse(data.arguments) : data.arguments;
        const record = (args ?? {}) as Record<string, unknown>;
        const summary = typeof record.summary === 'string' ? record.summary : '';
        return { label: '任务上报：' + String(record.state ?? 'report'), at: event.time, summary };
      } catch {
        return { label: '任务上报', at: event.time, summary: '' };
      }
    }
    if (event.type === 'assistant/message') {
      const message = data.message as { content?: { type?: string; text?: string }[] } | undefined;
      const text = (message?.content ?? [])
        .filter((block) => block.type === 'text' && typeof block.text === 'string')
        .map((block) => block.text ?? '')
        .join('')
        .slice(0, 240);
      return { label: '模型刚说完', at: event.time, summary: text };
    }
  }
  return null;
}

function ctxRows(parts: MonitorContextParts | undefined) {
  return [
    { key: 'fixed', name: '固定块', value: parts?.fixedBlock ?? 0, color: 'var(--kf-c7)' },
    { key: 'tools', name: '工具定义', value: parts?.toolDefinitions ?? 0, color: 'var(--kf-c4)' },
    { key: 'user', name: '任务书和消息', value: parts?.taskBookMessages ?? 0, color: 'var(--kf-c5)' },
    { key: 'reply', name: '模型回复', value: parts?.modelReplies ?? 0, color: 'var(--kf-c1)' },
    { key: 'result', name: '工具结果', value: parts?.toolResults ?? 0, color: 'var(--kf-c3)' },
  ];
}

function attemptLabel(attempt: MonitorAttempt): string {
  return '第 ' + attempt.attemptId + ' 次' + (attempt.endedAt ? '' : ' · 在跑');
}

export function Diagnose({ taskId }: { taskId: string }) {
  const tick = useTick();
  const snapshot = useMonitorStore((state) => state.snapshot);
  const summary = snapshot?.tasks.find((task) => task.taskId === taskId);
  const [attemptId, setAttemptId] = useState<number | null>(null);
  const detail = useMonitorStore((state) => state.details[taskId]);
  const error = useMonitorStore((state) => state.detailErrors[taskId] ?? null);
  const loadTask = useMonitorStore((state) => state.loadTask);
  const [selected, setSelected] = useState<ServiceEvent | null>(null);
  const [traceRange, setTraceRange] = useState<{ from: number; to: number } | null>(null);
  const projectName = useProjectName(detail?.task.projectId);

  // 诊断页不再自己取一次就停：读 store 的 details，跟同一路 WS 由事件刷新；
  // Attempt 切换仍按需重读（带 attemptId）。
  useEffect(() => {
    void loadTask(taskId, attemptId ?? undefined);
  }, [attemptId, loadTask, taskId]);

  const events = detail?.events ?? [];
  const traceEvents = traceRange
    ? events.filter((event) => event.time >= traceRange.from && event.time <= traceRange.to)
    : events;
  const compactions = events.filter((event) => event.type === 'compaction/summary').length;
  const attempts = detail?.attempts ?? [];
  const used = summary?.contextUsed ?? ctxRows(detail?.contextParts ?? undefined).reduce((sum, row) => sum + row.value, 0);
  const window = summary?.contextWindow ?? 0;
  const running = Boolean(detail?.task.running || summary?.running);
  const endedAt = summary?.endedAt ?? attempts.at(-1)?.endedAt ?? null;
  const runningFor = elapsedSeconds(running, summary?.startedAt, endedAt, Date.now(), tick);
  const done = lastDone(events);

  if (error) {
    return (
      <div className="h-full overflow-y-auto" data-page="monitor-task">
        <div className="mx-auto max-w-[1080px] px-6 py-6">
          <button type="button" onClick={() => go('monitor?tab=live')} className="flex items-center gap-1 text-1 text-ink-3">
            <ArrowLeft className="size-3.5" /> 监控
          </button>
          <Card title="这件任务诊断不了" className="mt-4">
            <p className="text-2 text-fail">{error}</p>
            <p className="mt-1 text-1 text-ink-3">Kind 不诊断；只能看任务。</p>
          </Card>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full overflow-y-auto" data-page="monitor-task">
      <div className="sticky top-0 z-10 border-b border-line bg-content/95 px-4 pt-3 backdrop-blur">
        <div className="mx-auto max-w-[1080px]">
          <button type="button" onClick={() => go('monitor?tab=live')} className="-ml-1 flex items-center gap-1 rounded-btn px-1 text-1 text-ink-3 hover:text-ink-2">
            <ArrowLeft className="size-3.5" /> 监控
          </button>
          <div className="mt-1 flex items-center gap-2">
            {detail?.task.running || summary?.running ? <span className="size-2 shrink-0 animate-pulse rounded-full bg-run" /> : null}
            <h1 className="min-w-0 truncate text-5 font-semibold text-ink">{detail?.task.title ?? summary?.title ?? taskId}</h1>
            <span data-kind="diagnose-project" className="rounded-badge border border-line px-1.5 text-1 leading-4 text-ink-2">
              {projectName}
            </span>
            <span className="hidden truncate text-1 text-ink-3 md:inline">ThinAgent · {detail?.task.model ?? summary?.model ?? '模型没采到'}</span>
          </div>
          <nav className="mt-2 flex gap-4">
            <span className="-mb-px border-b-2 border-ink pb-1.5 text-2 text-ink">诊断</span>
            {attempts.length > 1 ? (
              <div className="ml-auto flex items-center gap-1 pb-1" data-kind="attempts">
                {attempts.map((attempt) => (
                  <button
                    key={attempt.attemptId}
                    type="button"
                    className={cn('rounded-btn px-2 py-0.5 text-1', (attemptId ?? attempts[attempts.length - 1]?.attemptId) === attempt.attemptId ? 'bg-active text-ink' : 'text-ink-3 hover:text-ink-2')}
                    onClick={() => setAttemptId(attempt.attemptId)}
                  >
                    {attemptLabel(attempt)}
                  </button>
                ))}
              </div>
            ) : null}
          </nav>
        </div>
      </div>
      <div className="px-4 pb-16 pt-4">
        <div className="mx-auto max-w-[1080px] space-y-4">
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6" data-kind="kpis">
            <Kpi label="花费" value={summary?.costUsd == null ? <Missing /> : '$' + summary.costUsd.toFixed(3)} />
            <Kpi label="Token" value={summary ? formatTokens(summary.inputTokens + summary.outputTokens) : <Missing />} />
            <Kpi label="请求" value={summary?.requests ?? <Missing />} />
            <Kpi label="工具" value={summary?.toolCalls ?? <Missing />} />
            <Kpi label="用时" value={runningFor == null ? <Missing /> : fmtClock(runningFor)} tone="run" />
            <Kpi label="压缩" value={events.length ? compactions + ' 次' : <Missing />} />
          </div>
          <p className="text-1 text-ink-3" data-kind="tick-note">
            运行中「用时」每 1 秒一跳；结束后静态。后台标签页浏览器会节流到约 2 秒，不拿本地钟补时间。
          </p>

          <div className="grid gap-3 lg:grid-cols-2">
            <Card title="现在" note="服务端链路里的最后一步">
              <div className="space-y-2">
                <div className="flex items-center justify-between text-2">
                  <span className="text-ink-3">状态</span>
                  <span className="text-ink">{detail?.task.running ? '在跑' : detail?.task.state ?? '没采到'}</span>
                </div>
                <div className="flex items-center justify-between text-2">
                  <span className="text-ink-3">开始</span>
                  <span className="text-ink">{fmtTime(summary?.startedAt)}</span>
                </div>
                <div className="flex items-center justify-between text-2">
                  <span className="text-ink-3">写入范围</span>
                  <span className="min-w-0 truncate text-ink">{detail?.task.writeScope ?? '只读调查'}</span>
                </div>
                <div className="flex items-center justify-between text-2">
                  <span className="text-ink-3">Attempt</span>
                  <span className="text-ink">{attempts.length ? attempts.length + ' 次' : '没采到'}</span>
                </div>
              </div>
            </Card>
            <Card title="待办" note="最近一次 todo/write">
              {detail?.todo.length ? (
                <ul className="space-y-1">
                  {detail.todo.map((todo) => (
                    <li key={todo.content} className="flex items-start gap-1.5 text-2 text-ink-2">
                      <span className={cn('mt-1 size-2 shrink-0 rounded-full', todo.status === 'completed' ? 'bg-done' : todo.status === 'in_progress' ? 'bg-run' : 'bg-line-2')} />
                      <span className="min-w-0 flex-1">{todo.content}</span>
                      <span className="shrink-0 text-1 text-ink-3">{todo.status === 'completed' ? '做完' : todo.status === 'in_progress' ? '正在做' : '待办'}</span>
                    </li>
                  ))}
                </ul>
              ) : (
                <Missing what="没列待办" />
              )}
            </Card>
          </div>

          <Card title="刚做完" note="链路里最后一条成品事实">
            {done ? (
              <div>
                <div className="flex items-center gap-2 text-1 text-ink-3">
                  <span className="text-ink-2">{done.label}</span>
                  <span className="flex-1" />
                  <span className="brand-type">{fmtTime(done.at)}</span>
                </div>
                <p className="mt-1 line-clamp-4 whitespace-pre-wrap text-2 text-ink-2">{done.summary || <Missing what="摘要没采到" />}</p>
              </div>
            ) : (
              <Missing what="还没有做完的事实" />
            )}
          </Card>

          <Card title="上下文" note="最近一次请求 · 五块约值，总数实报" data-kind="ctx-card">
            {detail?.contextParts ? (
              <StackBar parts={ctxRows(detail.contextParts)} used={used} max={window} reserve={window - used} height={8} />
            ) : (
              <Missing what="这次请求的五块上下文没采到" />
            )}
            <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-1 text-ink-2">
              <span>用了 {used ? formatTokens(used) : '没采到'}{window ? ' / ' + formatTokens(window) : ''}</span>
              <span>压缩 {events.length ? compactions + ' 次' : '没采到'}</span>
              <span className="text-ink-3">斜线是压缩前留的余量</span>
            </div>
          </Card>

          <Card title="链路" note="三泳道按事件时间横向排；模型块拆等首字/生成，虚线是每步开始" data-kind="trace">
            <TraceTimeline
              events={events}
              selected={selected}
              onSelect={setSelected}
              onRangeChange={setTraceRange}
            />
          </Card>

          {selected ? (
            <div className="rounded-card border border-line-2 bg-raised p-3" data-kind="event-detail">
              <div className="mb-1 flex items-center gap-2 text-1 text-ink-3">
                <span className="brand-type">#{selected.seq}</span>
                <span>{selected.type}</span>
                <span className="flex-1" />
                <button type="button" className="rounded-btn p-0.5 text-ink-3 hover:bg-hover" onClick={() => setSelected(null)} aria-label="关闭详情">
                  <X className="size-3.5" />
                </button>
              </div>
              <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all text-1 text-ink-2">{JSON.stringify(redact(selected.data), null, 2)}</pre>
            </div>
          ) : (
            <p className="text-1 text-ink-3">点链路里的块或下面的事件行，详情就在这里出现。</p>
          )}

          <Card
            title="事件表"
            note={
              traceRange
                ? '只看 ' + fmtTime(traceRange.from) + '–' + fmtTime(traceRange.to) + ' · ' + traceEvents.length + ' 条；点行看下方详情'
                : '点一行看下方详情；密钥遮、思考不显示'
            }
          >
            <div className="max-h-[420px] overflow-auto">
              <table className="w-full table-fixed border-collapse text-1" data-kind="event-table">
                <thead>
                  <tr className="text-left text-ink-3">
                    <th className="w-14 py-1 font-normal">seq</th>
                    <th className="w-36 py-1 font-normal">时间</th>
                    <th className="py-1 font-normal">类型</th>
                    <th className="w-24 py-1 font-normal">泳道</th>
                  </tr>
                </thead>
                <tbody>
                  {traceEvents.slice(-200).map((event) => (
                    <tr key={event.seq} className={cn('cursor-pointer border-t border-line/60 hover:bg-hover', selected?.seq === event.seq && 'bg-active')} onClick={() => setSelected(event)}>
                      <td className="brand-type py-1 text-ink-3">{event.seq}</td>
                      <td className="brand-type py-1 text-ink-3">{fmtTime(event.time)}</td>
                      <td className="truncate py-1 text-ink-2">{eventLabel(event)}</td>
                      <td className="py-1 text-ink-3">{laneOf(event)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Card>
        </div>
      </div>
    </div>
  );
}
