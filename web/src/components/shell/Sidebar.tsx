/**
 * 侧栏：220px，可收到 56px 图标列（layout-motion-v1.md §二）。
 * 用 shadcn 的 Sidebar（collapsible="icon"），只改样式 token 与宽度变量，行为不动。
 */

import { useEffect, useState } from 'react';
import { Activity, Brain, Clock3, FolderKanban, Moon, Plus, Settings, Sparkles, Sun } from 'lucide-react';
import {
  Sidebar as ShadcnSidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from '@/components/ui/sidebar';
import { useAppStore } from '@/app/store';
import { go } from '@/app/navigate';
import {
  mountProjectLive,
  useProjectStore,
  projectCode,
  projectColor,
} from '@/pages/project/projectStore';
import { NewProjectDialog } from '@/pages/project/NewProjectDialog';
import type { ParsedRoute } from '@/app/route';
import { navOf } from '@/app/route';
import { cn } from '@/lib/utils';

interface NavItem {
  id: string;
  label: string;
  route: string;
  icon: React.ComponentType<{ className?: string }>;
}

const MAIN_NAV: NavItem[] = [
  { id: 'now', label: '现在', route: 'now', icon: Clock3 },
  { id: 'kind', label: 'Kind', route: 'kind', icon: Sparkles },
  { id: 'work', label: '工作', route: 'work', icon: FolderKanban },
  { id: 'memory', label: '记忆', route: 'memory', icon: Brain },
  { id: 'monitor', label: '监控', route: 'monitor', icon: Activity },
  { id: 'settings', label: '设置', route: 'settings:model', icon: Settings },
];

function TrafficLights() {
  return (
    <div className="flex items-center gap-[8px] px-1 pt-1">
      <i className="size-[11px] rounded-full bg-[#ff5f57]" />
      <i className="size-[11px] rounded-full bg-[#febc2e]" />
      <i className="size-[11px] rounded-full bg-[#28c840]" />
    </div>
  );
}

function Item({ item, active }: { item: NavItem; active: boolean }) {
  const Icon = item.icon;
  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        isActive={active}
        tooltip={item.label}
        onClick={() => go(item.route)}
        className={cn(
          'h-8 gap-2.5 rounded-btn px-2 text-3 text-ink-2 transition-colors',
          active && 'bg-active text-ink',
          'hover:bg-hover hover:text-ink',
        )}
      >
        <Icon className="size-4 shrink-0" />
        <span className="truncate group-data-[collapsible=icon]:hidden">{item.label}</span>
        {item.id === 'now' ? (
          <span className="ml-auto flex items-center gap-[3px] group-data-[collapsible=icon]:hidden">
            <i className="size-1 rounded-full bg-fail" />
            <i className="size-1 rounded-full bg-wait" />
          </span>
        ) : null}
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}

function SidebarInner({ route }: { route: ParsedRoute }) {
  const active = navOf(route);
  const theme = useAppStore((s) => s.theme);
  const toggleTheme = useAppStore((s) => s.toggleTheme);
  const projects = useProjectStore((state) => state.projects);
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  // T059 F3：现在 / Kind / 工作都有入口；设置仍在左下角。
  const mainNav = MAIN_NAV.filter((item) => item.id !== 'settings');
  const settingsItem = MAIN_NAV.find((item) => item.id === 'settings')!;

  useEffect(() => mountProjectLive(), []);

  return (
    <ShadcnSidebar
      collapsible="icon"
      className="border-r border-line bg-side backdrop-blur-xl"
      style={{ ['--sidebar-width' as string]: '220px', ['--sidebar-width-icon' as string]: '56px' }}
    >
      <SidebarHeader className="gap-2 px-2.5 pt-3">
        <TrafficLights />
      </SidebarHeader>

      <SidebarContent className="gap-0 px-2" data-kind="sidebar-content">
        <SidebarGroup className="py-1">
          <SidebarMenu>
            {mainNav.map((item) => (
              <Item key={item.id} item={item} active={active === item.id} />
            ))}
          </SidebarMenu>
        </SidebarGroup>

        <SidebarGroup className="mt-1 border-t border-line/70 px-1 pt-2" data-kind="sidebar-projects">
          <div className="flex h-6 items-center px-1.5">
            <span className="text-1 text-ink-3 group-data-[collapsible=icon]:hidden">项目</span>
            <button
              type="button"
              data-kind="new-project"
              title="新建项目"
              onClick={() => setNewProjectOpen(true)}
              className="ml-auto grid size-5 place-items-center rounded-btn text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
            >
              <Plus className="size-3.5" />
            </button>
          </div>
          <SidebarMenu>
            {(projects ?? []).map((project, index) => {
              const isActive = route.kind === 'project' && route.id === project.id;
              return (
                <SidebarMenuItem key={project.id}>
                  <SidebarMenuButton
                    isActive={isActive}
                    tooltip={project.name}
                    onClick={() => go('project:' + project.id)}
                    data-kind="sidebar-project"
                    data-project-id={project.id}
                    className={cn(
                      'h-7 gap-2 rounded-btn px-1.5 text-2 text-ink-2 transition-colors',
                      isActive && 'bg-active text-ink',
                      'hover:bg-hover hover:text-ink',
                    )}
                  >
                    <span
                      className="grid size-4 shrink-0 place-items-center rounded-[4px] text-[9px] font-semibold text-white"
                      style={{ backgroundColor: projectColor(index) }}
                    >
                      {projectCode(project.name)}
                    </span>
                    <span className="truncate group-data-[collapsible=icon]:hidden">{project.name}</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              );
            })}
            {projects && projects.length === 0 ? (
              <p className="px-2 py-1 text-1 text-ink-3 group-data-[collapsible=icon]:hidden">
                还没有项目
              </p>
            ) : null}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter className="gap-0.5 px-2 pb-3" data-kind="sidebar-footer">
        <SidebarMenu>
          <Item item={settingsItem} active={active === 'settings'} />
          <SidebarMenuItem>
            <SidebarMenuButton
              tooltip={theme === 'dark' ? '换浅色（外观）' : '换深色（外观）'}
              onClick={toggleTheme}
              className="h-8 gap-2.5 rounded-btn px-2 text-3 text-ink-2 hover:bg-hover hover:text-ink"
            >
              {theme === 'dark' ? <Moon className="size-4 shrink-0" /> : <Sun className="size-4 shrink-0" />}
              <span className="truncate group-data-[collapsible=icon]:hidden">外观</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SidebarRail />
      <NewProjectDialog open={newProjectOpen} onOpenChange={setNewProjectOpen} />
    </ShadcnSidebar>
  );
}

export function Sidebar({ route }: { route: ParsedRoute }) {
  return <SidebarInner route={route} />;
}
