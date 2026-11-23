/**
 * 任务对话标题下的项目选择器：只改归属，不动目录和要求；任务图整张一起改。
 * 这是页面改归属的入口；Kind 没有改归属的工具。
 */

import { useEffect, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { taskSetProject } from '@/data/service';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { projectLabel, useProjectStore } from '@/pages/project/projectStore';

export function TaskProjectPicker({
  task,
  onChanged,
}: {
  task: TaskReadHeader;
  onChanged: (projectId: string | null) => void;
}) {
  const projects = useProjectStore((state) => state.projects);
  const refresh = useProjectStore((state) => state.refresh);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (projects === null) void refresh();
  }, [projects, refresh]);

  async function select(value: string) {
    const projectId = value === 'personal' ? null : value;
    if (projectId === (task.projectId ?? null) || saving) return;
    setSaving(true);
    setError(null);
    try {
      await taskSetProject(task.id, projectId);
      onChanged(projectId);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="flex flex-wrap items-center gap-2 text-1 text-ink-3" data-kind="task-project-row">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            data-kind="task-project-picker"
            className="flex items-center gap-1 rounded-full border border-line bg-hover px-2 py-0.5 text-1 text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
          >
            {saving ? '保存中…' : projectLabel(projects, task.projectId)}
            <ChevronDown className="size-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-48">
          <DropdownMenuLabel className="text-1 text-ink-3">
            {task.graphMember ? '整张任务图一起改' : '只改归属。目录和要求不变。'}
          </DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={task.projectId ?? 'personal'}
            onValueChange={(value) => void select(value)}
          >
            <DropdownMenuRadioItem value="personal">个人</DropdownMenuRadioItem>
            {(projects ?? []).map((project) => (
              <DropdownMenuRadioItem key={project.id} value={project.id}>
                {project.name}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <span>{task.graphMember ? '改归属时整张任务图一起改' : '只改归属；目录和要求不变'}</span>
      {error ? <span className="text-fail" data-kind="task-project-error">{error}</span> : null}
    </div>
  );
}
