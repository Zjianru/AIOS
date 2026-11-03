/**
 * 接不上内核时 Kind 页整页显示的状态（T007 的参照）。
 *
 * - 连不上服务：服务没起来或已经退出；页面自己隔几秒再试，也可以马上试。
 * - 令牌不对：地址里的启动令牌和这次服务的不一样（服务重启过）；用最新打印的地址重新打开。
 * - Kind 的对话不见了：状态文件指向的日志文件找不到。AIOS 不会悄悄建一条空的新对话。
 * - 日志读不懂：某一行坏了。不跳过、不重建，原样留着等你处理。
 * - 被占用：别的进程正在写这条对话（单写者）。关掉它再检查一次。
 *
 * 文案只说事实和能做的一步；路径、行号是假的样例，真的由服务报上来。
 */

import { useEffect, useState } from 'react';
import { CircleAlert, FileWarning, KeyRound, Lock, PlugZap, RotateCw } from 'lucide-react';
import { toast } from 'sonner';
import type { KernelDemo } from '@/app/store';
import { useAppStore } from '@/app/store';

type GateState = Exclude<KernelDemo, 'ok'>;

const SESSION_PATH = '~/.aios/sessions/kind-01JC8M4Q.jsonl';

function Code({ children }: { children: React.ReactNode }) {
  return <code className="brand-type rounded-[4px] bg-hover px-1 py-[1px] text-2 text-ink">{children}</code>;
}

function Block({ children }: { children: React.ReactNode }) {
  return (
    <pre className="brand-type mt-3 overflow-x-auto rounded-btn border border-line bg-pane px-3 py-2 text-2 leading-5 text-ink-2">
      {children}
    </pre>
  );
}

function useCountdown(active: boolean, every: number): number {
  const [left, setLeft] = useState(every);
  useEffect(() => {
    if (!active) return;
    setLeft(every);
    const timer = window.setInterval(() => setLeft((n) => (n <= 1 ? every : n - 1)), 1000);
    return () => window.clearInterval(timer);
  }, [active, every]);
  return left;
}

export function KernelGate({ state }: { state: GateState }) {
  const setKernel = useAppStore((s) => s.setKernel);
  const left = useCountdown(state === 'unreachable', 5);
  const [checking, setChecking] = useState(false);

  // 原型里「再检查一次」总是还失败；真的由服务决定。
  const recheck = () => {
    setChecking(true);
    window.setTimeout(() => {
      setChecking(false);
      toast('还是一样', { description: '处理好之后再检查一次' });
    }, 700);
  };

  const content: Record<GateState, { icon: React.ReactNode; title: string; body: React.ReactNode; action?: React.ReactNode }> = {
    unreachable: {
      icon: <PlugZap className="size-4" />,
      title: '连不上内核',
      body: (
        <>
          <p>内核的服务没有在运行，或者刚刚退出了。在终端里启动它：</p>
          <Block>aios-dev serve</Block>
          <p className="mt-3">
            启动后用它打印出来的地址打开。这一页每 5 秒自己再试一次（<span className="brand-type">{left}</span> 秒后）。
          </p>
        </>
      ),
      action: (
        <button type="button" onClick={recheck} className={primary}>
          <RotateCw className={'size-3.5 ' + (checking ? 'animate-spin' : '')} />
          现在重试
        </button>
      ),
    },
    token: {
      icon: <KeyRound className="size-4" />,
      title: '这个地址已经失效',
      body: (
        <>
          <p>内核每次启动都会换一个令牌。这个页面用的还是上一次的，服务拒绝了它。</p>
          <p className="mt-2">用终端里最新打印出来的地址重新打开这一页：</p>
          <Block>{'AIOS 已启动：http://127.0.0.1:4417/#token=…'}</Block>
        </>
      ),
    },
    lost: {
      icon: <CircleAlert className="size-4" />,
      title: 'Kind 的对话找不到了',
      body: (
        <>
          <p>
            这条对话应该在 <Code>{SESSION_PATH}</Code>，但文件不在那里。
          </p>
          <p className="mt-2">
            AIOS 没有替你建一条空的新对话：那样之前说过的话就悄悄没了。如果是你移走了它，把它放回原处再检查一次。
          </p>
        </>
      ),
      action: (
        <button type="button" onClick={recheck} className={primary}>
          <RotateCw className={'size-3.5 ' + (checking ? 'animate-spin' : '')} />
          再检查一次
        </button>
      ),
    },
    broken: {
      icon: <FileWarning className="size-4" />,
      title: 'Kind 的对话日志读不懂',
      body: (
        <>
          <p>
            <Code>{SESSION_PATH}</Code> 的第 <span className="brand-type">1284</span> 行不是一条完整的记录：
          </p>
          <Block>{'{"seq":1284,"type":"message","role":"assi'}</Block>
          <p className="mt-3">AIOS 没有跳过这一行，也没有重建日志，文件原样留着。处理好之后再检查一次。</p>
        </>
      ),
      action: (
        <button type="button" onClick={recheck} className={primary}>
          <RotateCw className={'size-3.5 ' + (checking ? 'animate-spin' : '')} />
          再检查一次
        </button>
      ),
    },
    busy: {
      icon: <Lock className="size-4" />,
      title: 'Kind 的对话正被别的程序占用',
      body: (
        <>
          <p>另一个 AIOS 进程正在写这条对话。同一条对话同时只能有一个写的人，免得两边互相覆盖。</p>
          <p className="mt-2">
            常见的是终端里还开着 <Code>aios-dev chat</Code> 或另一个 <Code>aios-dev serve</Code>。关掉它，再检查一次。
          </p>
        </>
      ),
      action: (
        <button type="button" onClick={recheck} className={primary}>
          <RotateCw className={'size-3.5 ' + (checking ? 'animate-spin' : '')} />
          再检查一次
        </button>
      ),
    },
  };

  const view = content[state];
  return (
    <div className="flex h-full items-center justify-center overflow-y-auto px-6 py-10" data-kind="kernel-gate" data-state={state}>
      <div className="w-full max-w-[520px]">
        <div className="flex items-center gap-2 text-4 text-ink">
          <span className={state === 'unreachable' || state === 'busy' ? 'text-wait' : 'text-fail'}>{view.icon}</span>
          {view.title}
        </div>
        <div className="mt-3 text-3 leading-6 text-ink-2">{view.body}</div>
        <div className="mt-5 flex items-center gap-3">
          {view.action}
          <button type="button" onClick={() => setKernel('ok')} className="text-1 text-ink-3 hover:text-ink-2">
            原型：回到正常状态
          </button>
        </div>
      </div>
    </div>
  );
}

const primary =
  'flex items-center gap-1.5 rounded-btn bg-ink px-3 py-1.5 text-2 text-content transition-opacity hover:opacity-90';
