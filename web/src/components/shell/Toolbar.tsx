/**
 * 工具栏：固定 44px，放面包屑、搜索、布局切换器、状态（layout-motion-v1.md §二）。
 * 第 1 期只做骨架：搜索是第 3 期，这里放一个不可用的入口，不假装箱功能。
 */

import { ChevronRight, LayoutDashboard, PlugZap, RotateCcw, Search, Settings2 } from 'lucide-react';
import type { KernelDemo } from '@/app/store';
import { KEYRING_PROBLEM, setKeyringProblem } from '@/data/api';
import { useKindStore } from '@/pages/kind/kindStore';
import { go } from '@/app/navigate';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { SidebarTrigger } from '@/components/ui/sidebar';
import { useAppStore } from '@/app/store';
import type { ParsedRoute } from '@/app/route';
import { crumbIdOf, crumbOf } from '@/app/route';

interface ToolbarProps {
  route: ParsedRoute;
  onResetLayout: () => void;
}

export function Toolbar({ route, onResetLayout }: ToolbarProps) {
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const latency = useAppStore((s) => s.latency);
  const setLatency = useAppStore((s) => s.setLatency);
  const reduced = useAppStore((s) => s.reducedMotion);
  const setReduced = useAppStore((s) => s.setReducedMotion);
  const devStressFlow = useAppStore((s) => s.devStressFlow);
  const setDevStressFlow = useAppStore((s) => s.setDevStressFlow);
  const firstUse = useAppStore((s) => s.firstUse);
  const setFirstUse = useAppStore((s) => s.setFirstUse);
  const connectedOnly = useAppStore((s) => s.connectedOnly);
  const setConnectedOnly = useAppStore((s) => s.setConnectedOnly);
  const kernel = useAppStore((s) => s.kernel);
  const setKernel = useAppStore((s) => s.setKernel);
  const keyringBroken = useAppStore((s) => s.keyringBroken);
  const setKeyringBroken = useAppStore((s) => s.setKeyringBroken);
  const demo = (name: Parameters<ReturnType<typeof useKindStore.getState>['demo']>[0]) => {
    setKernel('ok');
    go('kind');
    useKindStore.getState().demo(name);
  };

  return (
    <header className="flex h-11 shrink-0 items-center gap-1 border-b border-line px-2 text-3">
      <SidebarTrigger className="text-ink-3 hover:bg-hover hover:text-ink" />
      <div className="ml-1 flex min-w-0 items-center gap-1 text-ink-3">
        <span className="truncate text-ink">{crumbOf(route)}</span>
        {route.id ? (
          <>
            <ChevronRight className="size-3.5 shrink-0 opacity-60" />
            <span className={route.kind === 'settings' ? 'truncate' : 'truncate brand-type'}>{crumbIdOf(route)}</span>
          </>
        ) : null}
      </div>
      <span className="flex-1" />

      {connectedOnly ? null : (
      <>
      <Button
        variant="ghost"
        size="sm"
        disabled
        title="全局搜索在第 3 期"
        className="h-7 gap-1.5 px-2 text-2 text-ink-3"
      >
        <Search className="size-3.5" />
        <span>搜索</span>
        <kbd className="brand-type text-1 opacity-70">⌘K</kbd>
      </Button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="sm" className="h-7 gap-1.5 px-2 text-2 text-ink-2 hover:bg-hover">
            <LayoutDashboard className="size-3.5" />
            布局
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-52">
          <DropdownMenuLabel className="text-1 text-ink-3">这个场景的布局</DropdownMenuLabel>
          <DropdownMenuItem onSelect={onResetLayout}>
            <RotateCcw className="size-3.5" />
            重置我的布局
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-1 text-ink-3">第 3 期：四套预设</DropdownMenuLabel>
          <DropdownMenuItem disabled>经典</DropdownMenuItem>
          <DropdownMenuItem disabled>专注</DropdownMenuItem>
          <DropdownMenuItem disabled>工作台</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      </>
      )}

      {/* 接内核之后的各种状态（T007、T008 的参照），只在原型里有 */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="sm" className="h-7 gap-1.5 px-2 text-2 text-ink-2 hover:bg-hover" data-dev="kernel-menu">
            <PlugZap className="size-3.5" />
            内核状态
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-60">
          <DropdownMenuCheckboxItem checked={connectedOnly} onCheckedChange={(value) => setConnectedOnly(Boolean(value))}>
            只显示接上内核的部分
          </DropdownMenuCheckboxItem>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-1 text-ink-3">内核（整页）</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={kernel}
            onValueChange={(v) => {
              setKernel(v as KernelDemo);
              go('kind');
            }}
          >
            <DropdownMenuRadioItem value="ok">正常</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="unreachable">连不上服务</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="token">地址里的令牌失效</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="lost">Kind 的对话找不到</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="broken">对话日志读不懂</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="busy">对话被别的进程占用</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-1 text-ink-3">Kind 对话里演一轮</DropdownMenuLabel>
          <DropdownMenuItem onSelect={() => demo('question')}>模型问你两件事</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('retry')}>模型重试后答出来</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('failModel')}>模型一直没回应</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('failCredential')}>密钥被拒</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('failKeyring')}>读不到 Keyring</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('compactAuto')}>上下文快满，自动压缩</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('offline')}>和内核断开再连上</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('unsent')}>下一条发不出去</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('empty')}>空对话</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => demo('restore')}>回到样例对话</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuCheckboxItem
            checked={keyringBroken}
            onCheckedChange={(value) => {
              setKeyringProblem(value ? KEYRING_PROBLEM : null);
              setKeyringBroken(Boolean(value));
            }}
          >
            Keyring 的钥匙文件读不出来
          </DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="sm" className="h-7 gap-1.5 px-2 text-2 text-ink-2 hover:bg-hover">
            <Settings2 className="size-3.5" />
            开发
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuLabel className="text-1 text-ink-3">主题</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={theme} onValueChange={(v) => setTheme(v as 'dark' | 'light')}>
            <DropdownMenuRadioItem value="dark">深色</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="light">浅色</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-1 text-ink-3">减少动效</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={reduced === null ? 'system' : reduced ? 'on' : 'off'}
            onValueChange={(v) => setReduced(v === 'system' ? null : v === 'on')}
          >
            <DropdownMenuRadioItem value="system">跟随系统</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="on">开</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="off">关</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-1 text-ink-3">假数据延迟（§二）</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={String(latency)}
            onValueChange={(v) => setLatency(Number(v) as 0 | 40 | 300)}
          >
            <DropdownMenuRadioItem value="0">0ms</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="40">40ms（默认）</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="300">300ms</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-1 text-ink-3">压测（只影响流程列表）</DropdownMenuLabel>
          <DropdownMenuCheckboxItem checked={devStressFlow} onCheckedChange={(value) => setDevStressFlow(Boolean(value))}>
            压测流程（60 节点）
          </DropdownMenuCheckboxItem>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-1 text-ink-3">首次使用（空库）</DropdownMenuLabel>
          <DropdownMenuCheckboxItem checked={firstUse} onCheckedChange={(value) => setFirstUse(Boolean(value))}>
            模拟没接过模型时的 Kind
          </DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <span className="ml-1 flex items-center gap-1.5 rounded-full border border-line px-2 py-[3px] text-2 text-ink-3">
        <i className="size-1.5 rounded-full bg-done" />
        {connectedOnly ? '原型 · 接上内核的部分' : '原型 · 第 2 期'}
      </span>
    </header>
  );
}
