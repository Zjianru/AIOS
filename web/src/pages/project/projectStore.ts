/**
 * 项目清单/创建的页面 store。侧栏、项目页、工作页/监控的项目名都读这一份；
 * 服务端没有项目变更事件，第一次用时取一次，页面自己的创建/更新直接写回。
 */

import { create } from 'zustand';
import type { ProjectCreateRequest } from '@/data/generated/ProjectCreateRequest';
import type { ProjectUpdateRequest } from '@/data/generated/ProjectUpdateRequest';
import type { ProjectView } from '@/data/generated/ProjectView';
import { projectCreate, projectList, projectUpdate } from '@/data/service';
import { subscribeRootFrames } from '@/data/liveRoot';

/** 项目色只影响页面；按列表顺序取，不进库/协议。 */
export const PROJECT_COLORS = [
  'var(--kf-c1)',
  'var(--kf-c2)',
  'var(--kf-c3)',
  'var(--kf-c4)',
  'var(--kf-c5)',
  'var(--kf-c6)',
  'var(--kf-c7)',
  'var(--kf-c8)',
];

/** 首字：中文取第一个字，英文取大写首字母。 */
export function projectCode(name: string): string {
  return name.trim().slice(0, 1).toUpperCase() || '?';
}

export function projectColor(index: number): string {
  return PROJECT_COLORS[index % PROJECT_COLORS.length];
}

interface ProjectState {
  projects: ProjectView[] | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  create: (input: ProjectCreateRequest) => Promise<ProjectView>;
  update: (id: string, input: Omit<ProjectUpdateRequest, 'id'>) => Promise<ProjectView>;
}

let refreshSequence = 0;

export const useProjectStore = create<ProjectState>()((set) => ({
  projects: null,
  loading: false,
  error: null,
  refresh: async () => {
    const sequence = ++refreshSequence;
    set({ loading: true, error: null });
    try {
      const response = await projectList();
      if (sequence !== refreshSequence) return;
      set({ projects: response.projects, loading: false });
    } catch (error) {
      if (sequence !== refreshSequence) return;
      set({
        loading: false,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  },
  create: async (input) => {
    const response = await projectCreate(input);
    set((state) => ({
      projects: [...(state.projects ?? []), response.project].sort(
        (left, right) => left.createdAt - right.createdAt,
      ),
      error: null,
    }));
    return response.project;
  },
  update: async (id, input) => {
    const response = await projectUpdate({ id, ...input });
    set((state) => ({
      projects: (state.projects ?? []).map((project) =>
        project.id === id ? response.project : project,
      ),
      error: null,
    }));
    return response.project;
  },
}));

/** p=personal / 项目 id / 空（全部）：页面筛选用同一套判断。 */
export function matchesProject(
  projectId: string | null | undefined,
  filter: string | null | undefined,
): boolean {
  if (!filter) return true;
  return filter === 'personal' ? !projectId : projectId === filter;
}

/** 任务卡上的归属名：没项目写「个人」。 */
export function projectLabel(
  projects: { id: string; name: string }[] | null,
  projectId: string | null | undefined,
): string {
  if (!projectId) return '个人';
  return projects?.find((project) => project.id === projectId)?.name ?? projectId;
}

/** 挂上根会话的 task-changed：Kind 建/改项目后不刷新页面也能看到。 */
export function mountProjectLive(): () => void {
  const unsubscribe = subscribeRootFrames((frame) => {
    if (frame.type === 'task-changed' || frame.type === 'ready') {
      void useProjectStore.getState().refresh();
    }
  });
  void useProjectStore.getState().refresh();
  return unsubscribe;
}

/** 组件里用的归属名；个人返回「个人」。 */
export function useProjectName(projectId: string | null | undefined): string {
  return projectLabel(useProjectStore((state) => state.projects), projectId);
}
