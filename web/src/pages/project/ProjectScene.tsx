/**
 * 项目页。名字/一句目标/默认目录逐项可改；任务按等你/在做/排队/完成四组，
 * 完成只列最近五件；入口到工作页/监控/记忆。这里不搬旧原型的里程碑/成果/资料/项目对话。
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import { ChevronRight, Pencil } from 'lucide-react';
import { useRouteStore } from '@/app/routeStore';
import { go } from '@/app/navigate';
import type { TaskFact } from '@/data/generated/TaskFact';
import { displayPath, memoryRead, ServiceHttpError } from '@/data/service';
import { useWorkPolling, useWorkStore } from '@/pages/work/workStore';
import { STAGE_LABEL, columnOf, filterWorkForProject, nextOf } from '@/pages/work/workModel';
import { useProjectStore, projectCode, projectColor } from './projectStore';

const GROUP_ORDER = ['wait', 'doing', 'queue', 'done'] as const;
type Group = (typeof GROUP_ORDER)[number];

const GROUP_LABEL: Record<Group, string> = {
  wait: '等你',
  doing: '在做',
  queue: '排队',
  done: '完成',
};

const GROUP_TONE: Record<Group, string> = {
  wait: 'text-wait',
  doing: 'text-run',
  queue: 'text-ink-3',
  done: 'text-done',
};

type EditField = 'name' | 'goal' | 'dir' | null;

function EditLine({
  label,
  value,
  field,
  editing,
  draft,
  placeholder,
  onStart,
  onChange,
  onCommit,
  onCancel,
}: {
  label: string;
  value: string;
  field: Exclude<EditField, null>;
  editing: EditField;
  draft: string;
  placeholder: string;
  onStart: () => void;
  onChange: (value: string) => void;
  onCommit: () => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef(false);
  const active = editing === field;
  return (
    <div className="group/line flex min-h-7 items-baseline gap-2" data-kind={'project-' + field}>
      <span className="w-12 shrink-0 text-1 text-ink-3">{label}</span>
      {active ? (
        <input
          autoFocus
          data-kind={'project-edit-' + field}
          value={draft}
          placeholder={placeholder}
          onChange={(event) => onChange(event.target.value)}
          onBlur={() => {
            if (cancelRef.current) {
              cancelRef.current = false;
              return;
            }
            onCommit();
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.currentTarget.blur();
            } else if (event.key === 'Escape') {
              cancelRef.current = true;
              onCancel();
            }
          }}
          className="min-w-0 flex-1 rounded-btn border border-line-2 bg-pane px-2 py-0.5 text-2 text-ink outline-none focus:border-brand"
        />
      ) : (
        <span className="flex min-w-0 flex-1 items-baseline gap-1.5">
          <span className="min-w-0 break-words text-2 text-ink-2">{value || placeholder}</span>
          <button
            type="button"
            title={'改' + label}
            data-kind={'project-edit-' + field}
            onClick={onStart}
            className="rounded-btn p-0.5 text-ink-3 opacity-0 transition-opacity hover:bg-hover hover:text-ink-2 group-hover/line:opacity-100"
          >
            <Pencil className="size-3" />
          </button>
        </span>
      )}
    </div>
  );
}

function TaskGroup({
  group,
  tasks,
  waitTaskIds,
}: {
  group: Group;
  tasks: TaskFact[];
  waitTaskIds: Set<string>;
}) {
  if (!tasks.length) return null;
  return (
    <section data-kind="project-task-group" data-group={group} className="space-y-1.5">
      <div className="flex items-baseline gap-2">
        <h2 className={'text-2 font-medium ' + GROUP_TONE[group]}>{GROUP_LABEL[group]}</h2>
        <span className="brand-type text-1 text-ink-3">{tasks.length}</span>
      </div>
      <div className="overflow-hidden rounded-card border border-line bg-pane/40">
        {tasks.map((task) => (
          <button
            key={task.id}
            type="button"
            data-kind="project-task"
            data-task-id={task.id}
            onClick={() => go('taskchat:' + task.id)}
            className="flex w-full items-center gap-2 border-b border-line px-3 py-2 text-left transition-colors last:border-0 hover:bg-hover"
          >
            <span className="min-w-0 flex-1">
              <span className="block truncate text-2 text-ink">{task.title}</span>
              <span className="mt-0.5 block truncate text-1 text-ink-3">
                {STAGE_LABEL[task.stage] ?? task.stage}
                {group === 'done' ? '' : ' · 下一步：' + nextOf(task, group as Exclude<Group, 'wait'>)}
                {waitTaskIds.has(task.id) ? ' · 等你' : ''}
              </span>
            </span>
            <ChevronRight className="size-3.5 shrink-0 text-ink-3" />
          </button>
        ))}
      </div>
    </section>
  );
}

function ProjectPage() {
  const projectId = useRouteStore((state) => state.route.id);
  const projects = useProjectStore((state) => state.projects);
  const projectsLoading = useProjectStore((state) => state.loading);
  const projectsError = useProjectStore((state) => state.error);
  const refreshProjects = useProjectStore((state) => state.refresh);
  const updateProject = useProjectStore((state) => state.update);
  const work = useWorkStore((state) => state.work);
  const workError = useWorkStore((state) => state.error);
  useWorkPolling();

  const [editing, setEditing] = useState<EditField>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [memoryCount, setMemoryCount] = useState<number | null>(null);

  useEffect(() => {
    if (projects === null) void refreshProjects();
  }, [projects, refreshProjects]);

  useEffect(() => {
    let alive = true;
    setMemoryCount(null);
    void memoryRead({ scope: 'project:' + projectId })
      .then((memory) => {
        if (!alive) return;
        setMemoryCount(
          memory.aboutYou.length + memory.entries.length + memory.notes.length,
        );
      })
      .catch(() => {
        if (alive) setMemoryCount(null);
      });
    return () => {
      alive = false;
    };
  }, [projectId]);

  const projectIndex = projects?.findIndex((project) => project.id === projectId) ?? -1;
  const project = projectIndex >= 0 ? projects?.[projectIndex] : null;

  const filtered = useMemo(
    () => filterWorkForProject(work?.tasks ?? [], work?.wait ?? [], projectId),
    [projectId, work],
  );
  const waitTaskIds = useMemo(
    () => new Set(filtered.wait.map((item) => item.taskId).filter((id): id is string => Boolean(id))),
    [filtered.wait],
  );
  const groups = useMemo(() => {
    const result: Record<Group, TaskFact[]> = { wait: [], doing: [], queue: [], done: [] };
    for (const task of filtered.tasks) {
      result[columnOf(task, waitTaskIds)].push(task);
    }
    result.done.sort((left, right) => (right.closedAt ?? 0) - (left.closedAt ?? 0));
    return result;
  }, [filtered.tasks, waitTaskIds]);
  const doneAll = groups.done.length;
  const doneVisible = groups.done.slice(0, 5);

  if (projects === null && projectsLoading) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">正在取项目…</div>;
  }
  if (projectsError && projects === null) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">{projectsError}</div>;
  }
  if (!project) {
    return (
      <div className="grid h-full place-items-center text-2 text-ink-3" data-page="project-missing">
        没有这个项目：{projectId}
      </div>
    );
  }

  const startEdit = (field: Exclude<EditField, null>) => {
    setSaveError(null);
    setEditing(field);
    setDraft(field === 'name' ? project.name : field === 'goal' ? project.goal : project.dir ?? '');
  };

  const saveEdit = async () => {
    if (!editing || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      await updateProject(project.id, {
        name: editing === 'name' ? draft.trim() : project.name,
        goal: editing === 'goal' ? draft.trim() : project.goal,
        dir: editing === 'dir' ? draft.trim() || null : (project.dir ?? null),
      });
      setEditing(null);
    } catch (reason) {
      setSaveError(
        reason instanceof ServiceHttpError && reason.detail
          ? reason.detail
          : reason instanceof Error
            ? reason.message
            : String(reason),
      );
    } finally {
      setSaving(false);
    }
  };

  const allTasks = groups.wait.length + groups.doing.length + groups.queue.length + doneAll;

  return (
    <div className="h-full overflow-y-auto" data-page="project" data-project-id={project.id}>
      <div className="mx-auto w-full max-w-[860px] px-6 py-5">
        <div className="flex items-start gap-3">
          <span
            className="grid size-7 shrink-0 place-items-center rounded-btn text-2 font-semibold text-white"
            style={{ backgroundColor: projectColor(projectIndex) }}
          >
            {projectCode(project.name)}
          </span>
          <div className="min-w-0 flex-1">
            <EditLine
              label="名字"
              value={project.name}
              field="name"
              editing={editing}
              draft={draft}
              placeholder="给这个项目起个名字"
              onStart={() => startEdit('name')}
              onChange={setDraft}
              onCommit={() => void saveEdit()}
              onCancel={() => setEditing(null)}
            />
            <EditLine
              label="目标"
              value={project.goal}
              field="goal"
              editing={editing}
              draft={draft}
              placeholder="这个项目要变成什么"
              onStart={() => startEdit('goal')}
              onChange={setDraft}
              onCommit={() => void saveEdit()}
              onCancel={() => setEditing(null)}
            />
            <EditLine
              label="默认目录"
              value={displayPath(project.dir ?? '')}
              field="dir"
              editing={editing}
              draft={draft}
              placeholder="还没有默认目录"
              onStart={() => startEdit('dir')}
              onChange={setDraft}
              onCommit={() => void saveEdit()}
              onCancel={() => setEditing(null)}
            />
          </div>
          <span className="shrink-0 pt-0.5 text-1 text-ink-3">{saving ? '保存中…' : ''}</span>
        </div>
        {saveError ? (
          <p data-kind="project-save-error" className="mt-2 text-1 text-fail">
            {saveError}
          </p>
        ) : null}

        <div className="mt-4 flex flex-wrap gap-1.5" data-kind="project-links">
          <button
            type="button"
            data-kind="project-work-link"
            onClick={() => go('work?p=' + encodeURIComponent(project.id))}
            className="rounded-full border border-line px-2.5 py-1 text-1 text-ink-2 transition-colors hover:bg-hover"
          >
            在工作页看这个项目
          </button>
          <button
            type="button"
            data-kind="project-monitor-link"
            onClick={() => go('monitor?p=' + encodeURIComponent(project.id))}
            className="rounded-full border border-line px-2.5 py-1 text-1 text-ink-2 transition-colors hover:bg-hover"
          >
            监控
          </button>
          <button
            type="button"
            data-kind="project-memory-link"
            onClick={() => go('memory?scope=project:' + encodeURIComponent(project.id))}
            className="rounded-full border border-line px-2.5 py-1 text-1 text-ink-2 transition-colors hover:bg-hover"
          >
            记忆
            {memoryCount != null ? (
              <span data-kind="project-memory-count" className="ml-1 text-ink-3">
                项目记忆 {memoryCount} 条
              </span>
            ) : null}
          </button>
        </div>

        <div className="mt-7 space-y-5">
          {work === null ? (
            <p className="text-2 text-ink-3">{workError ?? '正在取任务…'}</p>
          ) : allTasks === 0 ? (
            <div data-kind="project-empty" className="rounded-card border border-dashed border-line-2 px-4 py-8 text-center">
              <p className="text-2 text-ink-2">这个项目还没有任务。</p>
              <p className="mt-1 text-1 text-ink-3">交给 Kind 一件事，它会按你的判断归到这个项目。</p>
            </div>
          ) : (
            <>
              {GROUP_ORDER.map((group) => (
                <TaskGroup
                  key={group}
                  group={group}
                  tasks={group === 'done' ? doneVisible : groups[group]}
                  waitTaskIds={waitTaskIds}
                />
              ))}
              {doneAll > doneVisible.length ? (
                <button
                  type="button"
                  data-kind="project-done-more"
                  onClick={() => go('work?p=' + encodeURIComponent(project.id))}
                  className="text-1 text-ink-3 hover:text-ink-2"
                >
                  更早的完成还有 {doneAll - doneVisible.length} 件 · 去工作页看全部
                </button>
              ) : null}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export function ProjectScene({ pane }: { pane: string }) {
  if (pane !== 'main') return null;
  return <ProjectPage />;
}
