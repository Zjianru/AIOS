/**
 * 侧栏：220px，可收到 56px 图标列（layout-motion-v1.md §二）。
 * 用 shadcn 的 Sidebar（collapsible="icon"），只改样式 token 与宽度变量，行为不动。
 */

import { Clock3, FolderKanban, Moon, Sparkles, Sun } from 'lucide-react';
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
  // 只显示接上内核的部分（D46）：只有 Kind 和设置（外观是本地开关，留着）。
  const mainNav = MAIN_NAV.filter((item) => item.id === 'kind');

  return (
    <ShadcnSidebar
      collapsible="icon"
      className="border-r border-line bg-side backdrop-blur-xl"
      style={{ ['--sidebar-width' as string]: '220px', ['--sidebar-width-icon' as string]: '56px' }}
    >
      <SidebarHeader className="gap-2 px-2.5 pt-3">
        <TrafficLights />
      </SidebarHeader>

      <SidebarContent className="gap-0 px-2">
        <SidebarGroup className="py-1">
          <SidebarMenu>
            {mainNav.map((item) => (
              <Item key={item.id} item={item} active={active === item.id} />
            ))}
          </SidebarMenu>
        </SidebarGroup>

        
      </SidebarContent>

      <SidebarFooter className="gap-0.5 px-2 pb-3">
        <SidebarMenu>
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
    </ShadcnSidebar>
  );
}

export function Sidebar({ route }: { route: ParsedRoute }) {
  return <SidebarInner route={route} />;
}
