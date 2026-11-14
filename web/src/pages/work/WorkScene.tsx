/**
 * 工作页：看板四列（排队 / 在做 / 等你 / 完成）+ 右栏「等你」。
 *
 * T055：数据只来自 Work 快照（workStore，两页共用，两秒轻轮询）；列由任务事实派生，
 * 「等你」优先；完成列默认近 7 天，筛选记在 localStorage，只管显示。
 * 项目筛选、样例看板已去掉；「交给 Kind 一件事」保留。
 */

import { useEffect, useMemo, useState } from 'react';
import { ChevronDown, Plus } from 'lucide-react';
import type { WaitItem } from '@/data/generated/WaitItem';
import { go } from '@/app/navigate';

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';
import { StartThingDialog } from './StartThingDialog';
import { WaitList, openWaitItem } from './WaitList';
import { DONE_RANGES, DONE_RANGE_KEY, type DoneRange, boardOf, filterDone } from './workModel';
import { useWorkPolling, useWorkStore } from './workStore';

const WAIT_WORD: Record<string, string> = {
  resume: '接续',
  confirm: '要你确认',
  answer: '有问题要你答',
  judge: '里程碑等你判',
  accept: '结果等你接受',
};

function readDoneRange(): DoneRange {
  try {
    const raw = window.localStorage.getItem(DONE_RANGE_KEY);
    if (raw && DONE_RANGES.some((range) => range.key === raw)) return raw as DoneRange;
  } catch {
    // 隐私模式读不到 localStorage：按默认显示。
  }
  return '7d';
}

function TaskTile({ card }: { card: ReturnType<typeof boardOf>['queue'][number] }) {
  const task = card.task;
  return (
    <button
      type="button"
      data-kind="work-card"
      data-column={card.column}
      onClick={() => go('taskchat:' + task.id)}
      className="w-full rounded-card border border-line bg-pane/50 p-2.5 text-left transition-colors hover:border-ink-3/40 hover:bg-raised"
    >
      <span className="block truncate text-2 text-ink" title={task.title}>
        {task.title}
      </span>
      <span className="mt-0.5 block truncate text-1 text-ink-3" title={task.writeScope ?? ''}>
        {task.writeScope ? '写入范围 ' + task.writeScope : '只读调查'}
      </span>
      <span className="mt-1 flex items-center justify-between gap-2 text-1 text-ink-3">
        <span className="truncate">下一步：{card.next}</span>
        <span
          data-kind="work-word"
          className={cn(
            'shrink-0',
            card.column === 'queue' && 'text-wait',
            card.column === 'doing' && 'text-run',
            card.column === 'done' && 'text-done',
            card.column === 'wait' && 'text-wait',
          )}
        >
          {card.word}
        </span>
      </span>
    </button>
  );
}

function WaitCompact({ item }: { item: WaitItem }) {
  return (
    <button
      type="button"
      data-kind="board-wait-item"
      data-wait-kind={item.kind}
      onClick={() => void openWaitItem(item)}
      className="w-full rounded-info border border-wait/30 bg-wait-soft/30 px-2 py-1.5 text-left transition-colors hover:bg-wait-soft/60"
    >
      <span className="flex items-center gap-1.5 text-1 text-ink-3">
        <span className="text-wait">{WAIT_WORD[item.kind] ?? item.kind}</span>
        <span className="ml-auto brand-type">
          {item.turn !== undefined && item.turn !== null ? '第 ' + item.turn + ' 轮' : ''}
        </span>
      </span>
      <span className="mt-0.5 block truncate text-2 text-ink">{item.title}</span>
      <span className="block truncate text-1 text-ink-3">{item.need}</span>
    </button>
  );
}

function EmptyColumn({ text }: { text: string }) {
  return <p className="px-1 py-2 text-1 text-ink-3">{text}</p>;
}

function CalendarPane() {
  return (
    <div className="grid min-h-0 flex-1 place-items-center p-6 text-center">
      <div className="max-w-[420px]">
        <h2 className="text-4 font-medium text-ink">日历</h2>
        <p className="mt-2 text-2 leading-5 text-ink-3">
          这一期先不做日历。切回「看板」能看到全部任务；到点提醒、假日历归后面的期。
        </p>
      </div>
    </div>
  );
}

function BoardPane() {
  useWorkPolling();
  const work = useWorkStore((state) => state.work);
  const error = useWorkStore((state) => state.error);
  const [view, setView] = useState<'board' | 'cal'>('board');
  const [startOpen, setStartOpen] = useState(false);
  const [doneRange, setDoneRange] = useState<DoneRange>(readDoneRange);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    setNow(Date.now());
  }, [work]);

  useEffect(() => {
    try {
      window.localStorage.setItem(DONE_RANGE_KEY, doneRange);
    } catch {
      // 存不下（隐私模式）就算了，只影响下次默认。
    }
  }, [doneRange]);

  const board = useMemo(() => boardOf(work?.tasks ?? [], work?.wait ?? []), [work]);
  const done = useMemo(() => filterDone(board.done, doneRange, now), [board.done, doneRange, now]);
  const rangeLabel = DONE_RANGES.find((range) => range.key === doneRange)?.label ?? '近 7 天';

  if (!work) {
    return (
      <div className="grid h-full place-items-center text-2 text-ink-3">
        {error ?? '正在取工作页…'}
      </div>
    );
  }

  const groups = [
    { key: 'queue', label: '排队', tone: 'var(--ink-3)', hint: '等前置 / 缺条件', count: board.queue.length },
    { key: 'doing', label: '在做', tone: 'var(--run)', hint: '执行中、待检查', count: board.doing.length },
    { key: 'wait', label: '等你', tone: 'var(--wait)', hint: '只放需要你的', count: board.wait.length },
    { key: 'done', label: '完成', tone: 'var(--done)', hint: rangeLabel, count: done.visible.length },
  ] as const;

  return (
    <div className="flex h-full min-h-0 flex-col bg-content" data-scene-page="work">
      <header className="flex h-11 shrink-0 items-center gap-1.5 border-b border-line px-3 text-3">
        <span className="text-ink">工作</span>
        <span className="ml-auto" />
        <div className="flex items-center rounded-btn border border-line p-[2px]">
          {(['board', 'cal'] as const).map((key) => (
            <button
              key={key}
              type="button"
              onClick={() => setView(key)}
              className={cn(
                'rounded-badge px-2 py-0.5 text-2 transition-colors',
                view === key ? 'bg-hover text-ink' : 'text-ink-3 hover:text-ink-2',
              )}
            >
              {key === 'board' ? '看板' : '日历'}
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={() => setStartOpen(true)}
          className="ml-1 flex h-7 items-center gap-1 rounded-btn bg-ink px-2.5 text-2 text-content transition-opacity hover:opacity-90"
        >
          <Plus className="size-3.5" />
          交给 Kind 一件事
        </button>
      </header>

      {view === 'cal' ? (
        <CalendarPane />
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-4 gap-2.5 overflow-hidden p-3" data-kind="work-board">
          {groups.map((group) => (
            <section
              key={group.key}
              className="flex min-h-0 min-w-0 flex-col rounded-card border border-line/70 bg-pane/30 p-2"
              data-column={group.key}
            >
              <header className="mb-1.5 flex shrink-0 flex-wrap items-baseline gap-x-1.5 gap-y-0 text-1">
                <span className="size-[7px] rounded-full" style={{ backgroundColor: group.tone }} />
                <span className="text-2 text-ink">{group.label}</span>
                <span className="brand-type text-ink-3">{group.count}</span>
                <span className="ml-auto text-ink-3">{group.hint}</span>
              </header>
              {group.key === 'done' ? (
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <button
                      type="button"
                      data-kind="done-range"
                      className="mb-1.5 flex h-6 w-full items-center gap-1 rounded-btn border border-line px-1.5 text-1 text-ink-2 transition-colors hover:bg-hover"
                    >
                      {rangeLabel} <ChevronDown className="ml-auto size-3" />
                    </button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="start" className="w-36">
                    <DropdownMenuRadioGroup
                      value={doneRange}
                      onValueChange={(value) => setDoneRange(value as DoneRange)}
                    >
                      {DONE_RANGES.map((range) => (
                        <DropdownMenuRadioItem key={range.key} value={range.key}>
                          {range.label}
                        </DropdownMenuRadioItem>
                      ))}
                    </DropdownMenuRadioGroup>
                  </DropdownMenuContent>
                </DropdownMenu>
              ) : null}
              <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-0.5">
                {group.key === 'queue' ? board.queue.map((card) => <TaskTile key={card.task.id} card={card} />) : null}
                {group.key === 'doing' ? board.doing.map((card) => <TaskTile key={card.task.id} card={card} />) : null}
                {group.key === 'wait'
                  ? board.wait.map((item, index) => (
                      <WaitCompact key={item.kind + ':' + (item.taskId ?? item.title) + ':' + String(index)} item={item} />
                    ))
                  : null}
                {group.key === 'done' ? done.visible.map((card) => <TaskTile key={card.task.id} card={card} />) : null}
                {group.key === 'queue' && !board.queue.length ? <EmptyColumn text="没有排队的任务。" /> : null}
                {group.key === 'doing' && !board.doing.length ? <EmptyColumn text="没有正在做的任务。" /> : null}
                {group.key === 'wait' && !board.wait.length ? <EmptyColumn text="没有要你的事。" /> : null}
                {group.key === 'done' && !done.visible.length ? <EmptyColumn text="这段时间没有完成的任务。" /> : null}
                {group.key === 'done' && done.older > 0 ? (
                  <button
                    type="button"
                    data-kind="done-older"
                    onClick={() => setDoneRange('all')}
                    className="w-full rounded-btn border border-dashed border-line-2 px-2 py-1 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
                  >
                    更早的还有 {done.older} 项 · 看全部
                  </button>
                ) : null}
              </div>
            </section>
          ))}
        </div>
      )}

      <StartThingDialog open={startOpen} onOpenChange={setStartOpen} />
    </div>
  );
}

function WaitingPane() {
  useWorkPolling();
  const work = useWorkStore((state) => state.work);
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-content" data-pane="waiting">
      <header className="shrink-0 border-b border-line px-4 pb-2 pt-3">
        <div className="flex items-baseline gap-2">
          <h3 className="text-3 font-medium text-ink">等你</h3>
          <span className="brand-type text-1 text-ink-3">{work ? work.wait.length : 0}</span>
          <span className="ml-auto text-1 text-ink-3">只放需要你的</span>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {!work ? <div className="text-2 text-ink-3">正在取…</div> : <WaitList items={work.wait} />}
      </div>
    </div>
  );
}

export function WorkScene({ pane }: { pane: string }) {
  if (pane === 'waiting') return <WaitingPane />;
  return <BoardPane />;
}
