/**
 * 监控页（#monitor 消耗/在跑， #monitor:task:<id> 诊断， #usage 同消耗）。
 * 只读；数据全走服务端 Snapshot/MonitorTask 与 /ws?monitor=1 帧，页面只做本地计时。
 */

import { useEffect, useMemo } from 'react';
import { ChevronDown } from 'lucide-react';
import { useRouteStore } from '@/app/routeStore';
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
import { useMonitorLive, useMonitorStore } from './monitorStore';
import { CostTab } from './CostTab';
import { LiveTab } from './LiveTab';
import { Diagnose } from './Diagnose';
import { useProjectStore } from '@/pages/project/projectStore';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

function targetOf(route: { kind: string; id: string }): { tab: 'cost' | 'live'; taskId: string | null } {
  if (route.kind === 'usage') {
    return { tab: 'cost', taskId: route.id ? (route.id.startsWith('task:') ? route.id.slice(5) : route.id) : null };
  }
  if (route.kind === 'monitor' && route.id.startsWith('task:')) {
    return { tab: 'cost', taskId: route.id.slice(5) };
  }
  return { tab: 'cost', taskId: null };
}

export function MonitorScene() {
  const route = useRouteStore((state) => state.route);
  const { taskId } = targetOf(route);
  useMonitorLive();
  const loadSnapshot = useMonitorStore((state) => state.loadSnapshot);
  const snapshot = useMonitorStore((state) => state.snapshot);
  const projects = useProjectStore((state) => state.projects);
  const refreshProjects = useProjectStore((state) => state.refresh);
  const tab: 'cost' | 'live' = route.query.get('tab') === 'live' ? 'live' : 'cost';
  const projectFilter = route.query.get('p');
  const runningCount = snapshot?.running.running ?? 0;
  const projectFilterLabel =
    projectFilter === 'personal'
      ? '个人'
      : projectFilter
        ? (projects?.find((project) => project.id === projectFilter)?.name ?? projectFilter)
        : '全部项目';

  useEffect(() => {
    if (!snapshot) void loadSnapshot();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (projects === null) void refreshProjects();
  }, [projects, refreshProjects]);

  function switchProjectFilter(value: string) {
    const params = new URLSearchParams();
    params.set('tab', tab);
    if (value !== 'all') params.set('p', value);
    go('monitor?' + params.toString());
  }

  const tabs = useMemo(
    () =>
      [
        ['cost', '消耗'],
        ['live', '在跑'],
      ] as const,
    [],
  );

  if (taskId) return <Diagnose key={taskId + (route.query.get('view') ?? '')} taskId={taskId} />;

  return (
    <div className="h-full overflow-y-auto" data-page="monitor">
      <div className="sticky top-0 z-10 border-b border-line bg-content/95 px-4 pt-3 backdrop-blur">
        <div className="mx-auto flex max-w-[1080px] flex-wrap items-end gap-x-6 gap-y-2">
          <div>
            <h1 className="text-5 font-semibold text-ink">监控</h1>
            <nav className="mt-1 flex gap-4">
              {tabs.map(([key, label]) => (
                <button
                  key={key}
                  type="button"
                  data-tab-key={key}
                  className={cn(
                    '-mb-px flex items-center gap-1.5 border-b-2 pb-1.5 text-2 transition-colors',
                    tab === key
                      ? 'border-ink text-ink'
                      : 'border-transparent text-ink-3 hover:text-ink-2',
                  )}
                  onClick={() => go('monitor?tab=' + key)}
                >
                  {label}
                  {key === 'live' && runningCount ? (
                    <span className="brand-type rounded-full bg-run-soft px-1.5 text-1 leading-4 text-run">
                      {runningCount}
                    </span>
                  ) : null}
                </button>
              ))}
            </nav>
          </div>
          <span className="flex-1" />
          <div className="mb-2 flex flex-wrap items-center gap-1" data-kind="project-filter">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  data-kind="monitor-project-trigger"
                  className="flex items-center gap-1.5 rounded-full border border-line-2 bg-active px-2.5 py-0.5 text-1 text-ink"
                >
                  {projectFilterLabel}
                  <ChevronDown className="size-3 text-ink-3" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-44">
                <DropdownMenuRadioGroup
                  value={projectFilter ?? 'all'}
                  onValueChange={switchProjectFilter}
                >
                  <DropdownMenuRadioItem value="all">全部项目</DropdownMenuRadioItem>
                  <DropdownMenuRadioItem value="personal">个人</DropdownMenuRadioItem>
                  {(projects ?? []).map((project) => (
                    <DropdownMenuRadioItem key={project.id} value={project.id}>
                      {project.name}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
            {projectFilter ? (
              <button
                type="button"
                onClick={() => switchProjectFilter('all')}
                className="text-1 text-ink-3 hover:text-ink-2"
              >
                清除
              </button>
            ) : null}
          </div>
        </div>
      </div>
      <div className="px-4 pb-16">
        <div className="mx-auto max-w-[1080px]">
          {tab === 'cost' ? <CostTab /> : <LiveTab />}
        </div>
      </div>
    </div>
  );
}
