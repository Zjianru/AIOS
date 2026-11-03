/**
 * 工作页（§5 看板；brief §四「工作」）。
 *
 * 默认布局「看板/日历 | 等你」（layout-motion-v1.md §一）。
 * - 看板四列：排队 / 在做 / 等你 / 完成（近 7 天）；等你列的内容和首页「等你决定」相同；
 * - 可按 全部项目 / 个人 / 某个项目 筛；
 * - 日历与唤醒的显示部分归第 3 期（§四 p3），这里只放一句说明，不放假日历；
 * - 「交给 Kind 一件事」打开三档露脸对话框（§四.4）。
 * 任务的地铁线、这句话的账、里程碑、暂停/改计划在任务页（#task:<id>）。
 */

import { useEffect, useState } from 'react';
import { ChevronDown, Plus } from 'lucide-react';
import { toast } from 'sonner';
import { getBoard, getNow, listProjects } from '@/data/api';
import { useRouteStore } from '@/app/routeStore';
import type { NowDecision, NowFixture } from '@/data/fixtures/now';
import type { ProjectFixture } from '@/data/fixtures/workspace';
import type { BoardCard, BoardColumns } from '@/data/selectors';
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

const WORD_TONE: Record<string, string> = {
  '待你接受': 'text-wait',
  '出问题 · Kind 处理中': 'text-fail',
  '在做': 'text-run',
  '检查中': 'text-run',
};

function CodeChip({ code, color }: { code: string; color: string }) {
  return (
    <span
      className="grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white"
      style={{ backgroundColor: color }}
    >
      {code}
    </span>
  );
}

function BoardTile({ card }: { card: BoardCard }) {
  return (
    <button
      type="button"
      onClick={() => go('task:' + card.id)}
      className="w-full rounded-card border border-line bg-pane/50 p-2.5 text-left transition-colors hover:border-ink-3/40 hover:bg-raised"
    >
      <span className="flex min-w-0 items-center gap-1.5 text-1 text-ink-3">
        <CodeChip code={card.projectCode} color={card.color} />
        <span className="truncate">{card.projectName}</span>
      </span>
      <span className="mt-1 block truncate text-2 text-ink" title={card.title}>
        {card.title}
      </span>
      {card.path ? (
        <span className="mt-0.5 block truncate text-1 text-ink-3" title={card.path}>
          {card.path}
        </span>
      ) : null}
      <span className="mt-1 flex items-center justify-between gap-2 text-1 text-ink-3">
        <span className="truncate">{card.role || '—'}</span>
        <span className={cn('shrink-0', WORD_TONE[card.word])}>{card.word}</span>
      </span>
      {card.artifact ? (
        <span className="mt-0.5 block truncate text-1 text-ink-3">
          {card.artifact.name} <span className="brand-type">{card.artifact.version}</span> · {card.artifact.size}
        </span>
      ) : null}
    </button>
  );
}

/** 看板「等你」列的紧凑卡：内容还是首页「等你决定」那几件，只是列窄，点开去任务页看完整选项。 */
function WaitTile({ decision, color }: { decision: NowDecision; color: string }) {
  return (
    <button
      type="button"
      onClick={() => go(decision.href)}
      className="w-full rounded-card border border-wait/30 bg-wait-soft/30 p-2.5 text-left transition-colors hover:bg-wait-soft/60"
    >
      <span className="flex items-center gap-1.5 text-1 text-ink-3">
        <span className="text-wait">{decision.typeLabel}</span>
        {decision.fromProblem ? <span className="rounded-badge bg-wait-soft px-1 py-[1px] text-wait">因问题而来</span> : null}
        <span className="ml-auto brand-type">{decision.time}</span>
      </span>
      <span className="mt-1 block text-2 leading-5 text-ink">{decision.title}</span>
      <span className="mt-0.5 flex items-center gap-1.5 text-1 text-ink-3">
        <CodeChip code={decision.projectCode ?? '—'} color={color} />
        <span className="truncate">{decision.projectName}</span>
        <span className="ml-auto shrink-0">去看 ›</span>
      </span>
    </button>
  );
}

function CalendarPane() {
  return (
    <div className="grid min-h-0 flex-1 place-items-center px-6">
      <div className="max-w-[360px] text-center">
        <div className="text-3 text-ink-2">日历与唤醒 · 第 3 期</div>
        <p className="mt-1 text-1 leading-4 text-ink-3">
          这里是任务的时间安排与唤醒（显示部分照 v13-o O3）。第 2 期先不做；切回「看板」能看到全部任务。
        </p>
      </div>
    </div>
  );
}

function BoardPane() {
  const [columns, setColumns] = useState<BoardColumns | null>(null);
  const [now, setNow] = useState<NowFixture | null>(null);
  const [projects, setProjects] = useState<Record<string, ProjectFixture>>({});
  // #work?wv=cal 要能直接打开日历；点击看板/日历是本地状态。
  const route = useRouteStore((state) => state.route);
  const wantedView: 'board' | 'cal' = route.query.get('wv') === 'cal' ? 'cal' : 'board';
  const [view, setView] = useState<'board' | 'cal'>(wantedView);
  useEffect(() => {
    setView(wantedView);
  }, [wantedView]);
  const [filter, setFilter] = useState<string>('all');
  const [startOpen, setStartOpen] = useState(false);

  useEffect(() => {
    let alive = true;
    void (async () => {
      const [boardView, nowView, projectList] = await Promise.all([getBoard(), getNow(), listProjects()]);
      if (!alive) return;
      setColumns(boardView);
      setNow(nowView);
      setProjects(projectList);
    })();
    return () => {
      alive = false;
    };
  }, []);

  if (!columns || !now) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">正在取工作页…</div>;
  }

  const projectNameToId = new Map(Object.values(projects).map((project) => [project.name, project.id]));
  const keepProject = (projectId: string | null): boolean => {
    if (filter === 'all') return true;
    if (filter === 'me') return false;
    return projectId === filter;
  };
  const keepDecision = (decision: NowDecision): boolean => {
    if (filter === 'all') return true;
    if (filter === 'me') return true;
    return projectNameToId.get(decision.projectName ?? '') === filter;
  };

  const queue = columns.queue.filter((card) => keepProject(card.projectId));
  const doing = columns.doing.filter((card) => keepProject(card.projectId));
  const done = columns.done.filter((card) => keepProject(card.projectId));
  const decisions = now.decisions.filter(keepDecision);

  const filterLabel =
    filter === 'all'
      ? '全部项目'
      : filter === 'me'
        ? '个人'
        : Object.values(projects).find((project) => project.id === filter)?.name ?? '全部项目';

  const groups = [
    { key: 'queue', label: '排队', tone: 'var(--ink-3)', hint: '等前置 / 缺条件', count: queue.length },
    { key: 'doing', label: '在做', tone: 'var(--run)', hint: '执行中、待检查', count: doing.length },
    { key: 'wait', label: '等你', tone: 'var(--wait)', hint: '只放需要你的', count: decisions.length },
    { key: 'done', label: '完成', tone: 'var(--done)', hint: '近 7 天', count: done.length },
  ] as const;

  return (
    <div className="flex h-full min-h-0 flex-col bg-content" data-scene-page="work">
      <header className="flex h-11 shrink-0 items-center gap-1.5 border-b border-line px-3 text-3">
        <span className="text-ink">工作</span>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="flex h-7 items-center gap-1 rounded-btn px-2 text-2 text-ink-2 transition-colors hover:bg-hover"
            >
              {filterLabel} <ChevronDown className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-56">
            <DropdownMenuRadioGroup value={filter} onValueChange={setFilter}>
              <DropdownMenuRadioItem value="all">全部项目</DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="me">个人</DropdownMenuRadioItem>
              {Object.values(projects).map((project) => (
                <DropdownMenuRadioItem key={project.id} value={project.id}>
                  {project.name}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>

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
        <div className="grid min-h-0 flex-1 grid-cols-4 gap-2.5 overflow-hidden p-3">
          {groups.map((group) => (
            <section key={group.key} className="flex min-h-0 min-w-0 flex-col rounded-card border border-line/70 bg-pane/30 p-2">
              <header className="mb-1.5 flex shrink-0 flex-wrap items-baseline gap-x-1.5 gap-y-0 text-1">
                <span className="size-[7px] rounded-full" style={{ backgroundColor: group.tone }} />
                <span className="text-2 text-ink">{group.label}</span>
                <span className="brand-type text-ink-3">{group.count}</span>
                <span className="ml-auto text-ink-3">{group.hint}</span>
              </header>
              <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-0.5">
                {group.key === 'queue' ? queue.map((card) => <BoardTile key={card.id} card={card} />) : null}
                {group.key === 'doing' ? doing.map((card) => <BoardTile key={card.id} card={card} />) : null}
                {group.key === 'done' ? done.map((card) => <BoardTile key={card.id} card={card} />) : null}
                {group.key === 'wait'
                  ? decisions.map((decision) => (
                      <WaitTile
                        key={decision.id}
                        decision={decision}
                        color={
                          Object.values(projects).find((project) => project.name === decision.projectName)?.color ??
                          'var(--ink-3)'
                        }
                      />
                    ))
                  : null}
                {group.key === 'wait' && !decisions.length ? (
                  <p className="px-1 py-2 text-1 text-ink-3">没有要你的事。</p>
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
  const [now, setNow] = useState<NowFixture | null>(null);
  const [allowed, setAllowed] = useState(false);

  useEffect(() => {
    let alive = true;
    void (async () => {
      const next = await getNow();
      if (alive) setNow(next);
    })();
    return () => {
      alive = false;
    };
  }, []);

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-content" data-pane="waiting">
      <header className="shrink-0 border-b border-line px-4 pb-2 pt-3">
        <div className="flex items-baseline gap-2">
          <h3 className="text-3 font-medium text-ink">等你</h3>
          <span className="brand-type text-1 text-ink-3">{now ? now.decisions.length : 0}</span>
          <span className="ml-auto text-1 text-ink-3">只放需要你的</span>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {!now ? (
          <div className="text-2 text-ink-3">正在取…</div>
        ) : (
          <>
            {now.decisions.map((decision) => (
              <button
                key={decision.id}
                type="button"
                onClick={() => go(decision.href)}
                className="mb-1.5 block w-full rounded-info border border-line bg-pane/40 px-2.5 py-2 text-left transition-colors hover:bg-hover"
              >
                <span className="flex items-center gap-1.5 text-1 text-ink-3">
                  <span className="text-wait">{decision.typeLabel}</span>
                  <span className="truncate">{decision.projectName}</span>
                  <span className="ml-auto brand-type">{decision.time}</span>
                </span>
                <span className="mt-0.5 block truncate text-2 text-ink">{decision.title}</span>
              </button>
            ))}

            {/* §四「中途要你允许」：三类请求的样例（允许一次 / 始终允许 / 不允许） */}
            <div className="mt-2 rounded-card border border-wait/30 bg-wait-soft/40 p-2.5">
              <div className="text-1 text-ink-3">要你允许 · Kind</div>
              <div className="mt-0.5 text-2 text-ink">清理构建缓存，会删除 2.1 GB 文件</div>
              <div className="mt-1.5 flex flex-wrap gap-1.5">
                {['允许一次', '始终允许', '不允许'].map((label) => (
                  <button
                    key={label}
                    type="button"
                    disabled={allowed && label === '允许一次'}
                    onClick={() => {
                      setAllowed(true);
                      toast('（原型）你选了：' + label);
                    }}
                    className={cn(
                      'rounded-btn border px-2 py-0.5 text-1 transition-colors',
                      label === '允许一次'
                        ? 'border-ink-3 bg-ink text-content'
                        : 'border-line-2 text-ink-2 hover:bg-hover',
                    )}
                    style={label === '允许一次' ? { color: 'var(--content)' } : undefined}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
          </>
        )}
      </div>

      <footer className="shrink-0 border-t border-line px-4 py-2 text-1 leading-4 text-ink-3">
        工作：看板四列是事实派生的；「等你」只放需要你的，问题默认 Kind 先处理。
      </footer>
    </div>
  );
}

export function WorkScene({ pane }: { pane: string }) {
  if (pane === 'waiting') return <WaitingPane />;
  return <BoardPane />;
}
