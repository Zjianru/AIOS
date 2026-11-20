/**
 * 监控 · 在跑：席位/在跑/排队/异常；每个 ThinAgent 一张卡，下面排队和最近完成。
 * 卡片数据来自 Snapshot 摘要 + MonitorTask（todo、五块、链路），WS TaskEvent 到达时重读该任务；
 * 页面只本地走 elapsed，不轮询。
 */

import { useEffect } from 'react';
import { AlertCircle, ChevronRight, Sparkles, Terminal, Wrench } from 'lucide-react';
import { go } from '@/app/navigate';
import { formatPercent, formatTokens } from '@/lib/numbers';
import { cn } from '@/lib/utils';
import { useMonitorStore } from './monitorStore';
import type { MonitorTaskSummary, ServiceEvent } from '@/data/service';
import { Card, Kpi, Missing, StackBar, fmtClock, useTick } from './monitorParts';

const MODEL_COLOR = 'var(--kf-c1)';
const TOOL_COLOR = 'var(--kf-c4)';
const CMD_COLOR = 'var(--kf-c3)';

function nowOf(events: ServiceEvent[]): { label: string; icon: 'model' | 'tool' | 'cmd' | 'wait'; at: number | null } | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    const data = event.data as Record<string, unknown>;
    if (event.type === 'tool/call') {
      const name = String(data.name ?? '工具');
      const args = data.arguments;
      const kind = name === 'bash' ? 'cmd' : 'tool';
      let detail = name;
      try {
        const parsed = typeof args === 'string' ? JSON.parse(args) : args;
        if (parsed && typeof parsed === 'object') {
          const record = parsed as Record<string, unknown>;
          const value = record.command ?? record.path ?? record.file_path ?? record.pattern;
          if (typeof value === 'string') detail = name + ' · ' + value;
        }
      } catch {
        // 参数不是 JSON 时只写工具名。
      }
      return { label: '正在调 ' + detail, icon: kind, at: event.time };
    }
    if (event.type === 'assistant/message' || event.type === 'assistant/attempt') {
      return { label: '正在写回复', icon: 'model', at: event.time };
    }
    if (event.type === 'step/start') return { label: '等模型首字', icon: 'model', at: event.time };
    if (event.type === 'compaction/start') return { label: '正在压缩上下文', icon: 'model', at: event.time };
  }
  return null;
}

function stepCount(events: ServiceEvent[]): number {
  return events.filter((event) => event.type === 'step/start').length;
}

function CtxBar({ used, window, parts }: { used: number; window: number; parts?: { fixedBlock: number; toolDefinitions: number; taskBookMessages: number; modelReplies: number; toolResults: number } }) {
  if (!window || !parts) {
    return <div className="text-1"><Missing what="上下文没采到" /></div>;
  }
  const rows = [
    { key: 'fixed', name: '固定块', value: parts.fixedBlock, color: 'var(--kf-c7)' },
    { key: 'tools', name: '工具定义', value: parts.toolDefinitions, color: 'var(--kf-c4)' },
    { key: 'user', name: '任务书和消息', value: parts.taskBookMessages, color: 'var(--kf-c5)' },
    { key: 'reply', name: '模型回复', value: parts.modelReplies, color: 'var(--kf-c1)' },
    { key: 'result', name: '工具结果', value: parts.toolResults, color: 'var(--kf-c3)' },
  ];
  return (
    <div>
      <div className="mb-1 flex text-1 text-ink-3">
        <span>上下文</span>
        <span className="flex-1" />
        <span className="brand-type">{formatTokens(used)} / {formatTokens(window)} · {formatPercent((used / window) * 100)}</span>
      </div>
      <StackBar parts={rows} used={used} max={window} reserve={window - used} height={6} />
    </div>
  );
}

function LiveCard({ task }: { task: MonitorTaskSummary }) {
  const tick = useTick();
  const detail = useMonitorStore((state) => state.details[task.taskId]);
  const error = useMonitorStore((state) => state.detailErrors[task.taskId]);
  const loadTask = useMonitorStore((state) => state.loadTask);

  useEffect(() => {
    if (!detail) void loadTask(task.taskId);
  }, [detail, loadTask, task.taskId]);

  const events = detail?.events ?? [];
  const now = nowOf(events);
  const since = now?.at ? (Date.now() - now.at) / 1000 + tick : 0;
  const todoDone = detail ? detail.todo.filter((todo) => todo.status === 'completed').length : 0;
  const runningFor = task.startedAt ? Math.max(0, (Date.now() - task.startedAt) / 1000) + tick : null;

  return (
    <button
      type="button"
      onClick={() => go('monitor:task:' + task.taskId)}
      data-kind="live-card"
      className="group flex min-w-0 flex-col gap-3 rounded-card border border-line bg-raised/40 p-4 text-left transition-colors hover:border-line-2 hover:bg-hover"
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <span className="grid size-7 shrink-0 place-items-center rounded-[5px] bg-line text-1 font-semibold text-ink-2">未</span>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-run" />
            <span className="min-w-0 truncate text-3 font-semibold text-ink">{task.title}</span>
          </div>
          <div className="truncate text-1 text-ink-3">
            未归项目 · {task.model}
            {task.attemptId ? ' · 第 ' + task.attemptId + ' 次尝试' : ''}
          </div>
        </div>
        <ChevronRight className="size-4 shrink-0 text-ink-3 opacity-0 group-hover:opacity-100" />
      </div>

      {error ? <div className="text-1 text-fail">{error}</div> : null}
      {now ? (
        <div className="flex items-center gap-2 rounded-btn border border-line bg-content/60 px-2.5 py-1.5">
          {now.icon === 'model' ? <Sparkles className="size-3.5 shrink-0" style={{ color: MODEL_COLOR }} /> : now.icon === 'cmd' ? <Terminal className="size-3.5 shrink-0" style={{ color: CMD_COLOR }} /> : <Wrench className="size-3.5 shrink-0" style={{ color: TOOL_COLOR }} />}
          <span className="min-w-0 flex-1 truncate text-2 text-ink">{now.label}</span>
          <span className="brand-type shrink-0 text-3 text-run">{now.at ? fmtClock(since / 1000) : <Missing what="没采到" />}</span>
        </div>
      ) : (
        <div className="rounded-btn border border-line bg-content/60 px-2.5 py-1.5 text-1">
          <Missing what="现在停在哪没采到" />
        </div>
      )}

      <div className="grid grid-cols-4 gap-2">
        <div>
          <div className="brand-type truncate text-3 text-ink">{task.costUsd == null ? <Missing /> : '$' + task.costUsd.toFixed(3)}</div>
          <div className="text-1 text-ink-3">花费</div>
        </div>
        <div>
          <div className="brand-type truncate text-3 text-ink">{formatTokens(task.inputTokens + task.outputTokens)}</div>
          <div className="text-1 text-ink-3">token</div>
        </div>
        <div>
          <div className="brand-type truncate text-3 text-ink">{runningFor == null ? <Missing /> : fmtClock(runningFor)}</div>
          <div className="text-1 text-ink-3">跑了</div>
        </div>
        <div>
          <div className="brand-type truncate text-3 text-ink">{events.length ? String(stepCount(events)) : <Missing />}</div>
          <div className="text-1 text-ink-3">步数</div>
        </div>
      </div>

      <CtxBar used={task.contextUsed} window={task.contextWindow} parts={detail?.contextParts ?? undefined} />

      {detail?.todo.length ? (
        <div className="flex items-center gap-2 text-1 text-ink-3">
          <span>待办</span>
          <div className="flex flex-1 gap-[3px]">
            {detail.todo.map((todo) => (
              <i key={todo.content} className={cn('h-1 flex-1 rounded-full', todo.status === 'completed' ? 'bg-done' : todo.status === 'in_progress' ? 'animate-pulse bg-run' : 'bg-line-2')} title={todo.content} />
            ))}
          </div>
          <span className="brand-type">{todoDone}/{detail.todo.length}</span>
        </div>
      ) : (
        <div className="text-1"><Missing what="待办没列" /></div>
      )}

      {detail?.events.length ? null : (
        <div className="flex items-start gap-1.5 text-1 text-ink-3">
          <AlertCircle className="mt-px size-3 shrink-0" />
          诊断链路没采到；卡片数字仍来自快照。
        </div>
      )}
    </button>
  );
}

function SimpleTaskRow({ task, state }: { task: MonitorTaskSummary; state: string }) {
  return (
    <button type="button" data-kind="live-row" onClick={() => go('monitor:task:' + task.taskId)} className="flex w-full items-center gap-2 border-b border-line py-2 text-left last:border-0">
      <span className="min-w-0 flex-1 truncate text-2 text-ink">{task.title}</span>
      <span className="shrink-0 text-1 text-ink-3">{state}</span>
      <span className="shrink-0 text-1 text-ink-3">{task.attemptId ? '第 ' + task.attemptId + ' 次' : '没开跑'}</span>
    </button>
  );
}

export function LiveTab() {
  const snapshot = useMonitorStore((state) => state.snapshot);
  if (!snapshot) {
    return <div className="grid h-40 place-items-center text-2 text-ink-3">正在取在跑快照…</div>;
  }
  const running = snapshot.running.tasks ?? [];
  const queue = snapshot.queue;
  const finished = snapshot.tasks
    .filter((task) => !task.running && (task.state === 'closed' || task.state === 'requirement_met' || task.endedAt))
    .sort((left, right) => (right.endedAt ?? 0) - (left.endedAt ?? 0))
    .slice(0, 8);
  const anomalous = snapshot.anomalies.length;

  return (
    <div data-tab="live" className="space-y-4 pt-4">
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4" data-kind="kpis">
        <Kpi label="席位" value={snapshot.running.seats + ' 个'} sub="满了新任务排队" />
        <Kpi label="在跑" value={snapshot.running.running} tone="run" sub="ThinAgent" />
        <Kpi label="排队" value={snapshot.running.queue} sub="席位空出来按顺序开" />
        <Kpi label="值得看一眼" value={anomalous} tone={anomalous ? 'wait' : undefined} sub={anomalous ? '下面写了是什么' : '没有异常事实'} />
      </div>

      {running.length ? (
        <div className="grid grid-cols-1 gap-3 lg:grid-cols-2" data-kind="live-cards">
          {running.map((task) => (
            <LiveCard key={task.taskId} task={task} />
          ))}
        </div>
      ) : (
        <Card>
          <p className="text-2 text-ink-3">现在没有在跑的 ThinAgent。</p>
        </Card>
      )}

      {snapshot.anomalies.length ? (
        <Card title="异常事实" note="只写事实，不判断好坏">
          <div className="space-y-1">
            {snapshot.anomalies.map((anomaly, index) => (
              <div key={anomaly.kind + index} className="flex items-start gap-1.5 rounded-btn bg-wait-soft px-2 py-1 text-1 leading-4 text-wait" data-kind="monitor-fact">
                <AlertCircle className="mt-px size-3 shrink-0" />
                <span className="min-w-0 flex-1">{anomaly.detail}</span>
                {anomaly.taskId ? (
                  <button type="button" className="shrink-0 underline" onClick={() => go('monitor:task:' + anomaly.taskId)}>
                    看任务
                  </button>
                ) : null}
              </div>
            ))}
          </div>
        </Card>
      ) : null}

      <div className="grid gap-3 lg:grid-cols-2">
        <Card title="排队" note={queue.length + ' 件'}>
          {queue.length ? queue.map((task) => <SimpleTaskRow key={task.taskId} task={task} state="等待中" />) : <p className="text-2 text-ink-3">没有排队的任务。</p>}
        </Card>
        <Card title="最近完成" note={finished.length + ' 件'}>
          {finished.length ? finished.map((task) => <SimpleTaskRow key={task.taskId} task={task} state={task.state === 'closed' ? '已关闭' : '完成'} />) : <p className="text-2 text-ink-3">这段时间没有完成的任务。</p>}
        </Card>
      </div>
    </div>
  );
}
