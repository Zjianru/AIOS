/**
 * 接不上内核时 Kind 页整页显示的状态（T007.10）：文案只说事实和能做的一步；
 * 路径/行号来自服务 ready 的 root 状态，不在页面上编。
 */

import { useEffect, useState } from 'react';
import { CircleAlert, FileWarning, KeyRound, Lock, PlugZap, RotateCw } from 'lucide-react';

export type GateState = 'unreachable' | 'token' | 'lost' | 'broken' | 'busy';

export interface GateDetail {
  path?: string;
  line?: number;
}

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

function RetryButton({ onRecheck, label }: { onRecheck: () => Promise<void> | void; label: string }) {
  const [checking, setChecking] = useState(false);
  return (
    <button
      type="button"
      onClick={() => {
        setChecking(true);
        void Promise.resolve(onRecheck()).finally(() => setChecking(false));
      }}
      className={primary}
    >
      <RotateCw className={'size-3.5 ' + (checking ? 'animate-spin' : '')} />
      {label}
    </button>
  );
}

export function KernelGate({
  state,
  detail,
  onRecheck,
}: {
  state: GateState;
  detail?: GateDetail;
  onRecheck: () => Promise<void> | void;
}) {
  const left = useCountdown(state === 'unreachable', 5);
  const path = detail?.path;

  // 「连不上服务」时页面每 5 秒自己再试一次。
  useEffect(() => {
    if (state !== 'unreachable') return;
    const timer = window.setInterval(() => {
      void onRecheck();
    }, 5000);
    return () => window.clearInterval(timer);
  }, [state, onRecheck]);

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
      action: <RetryButton onRecheck={onRecheck} label="现在重试" />,
    },
    token: {
      icon: <KeyRound className="size-4" />,
      title: '这个页面需要重新打开',
      body: (
        <>
          <p>这个页面的登录已经失效：登录满 30 天会过期，Keyring 重新开始以后也会失效。服务不再认它。</p>
          <p className="mt-2">用终端里打印出来的地址重新打开这一页：</p>
          <Block>{'AIOS 已启动：http://127.0.0.1:4417/?token=…'}</Block>
        </>
      ),
    },
    lost: {
      icon: <CircleAlert className="size-4" />,
      title: 'Kind 的对话找不到了',
      body: (
        <>
          <p>
            这条对话应该在 <Code>{path ?? '服务没有给出日志路径'}</Code>，但文件不在那里。
          </p>
          <p className="mt-2">
            AIOS 没有替你建一条空的新对话：那样之前说过的话就悄悄没了。如果是你移走了它，把它放回原处再检查一次。
          </p>
        </>
      ),
      action: <RetryButton onRecheck={onRecheck} label="再检查一次" />,
    },
    broken: {
      icon: <FileWarning className="size-4" />,
      title: 'Kind 的对话日志读不懂',
      body: (
        <>
          <p>
            <Code>{path ?? '服务没有给出日志路径'}</Code> 的第{' '}
            <span className="brand-type">{detail?.line ?? '?'}</span> 行不是一条完整的记录。
          </p>
          <p className="mt-3">AIOS 没有跳过这一行，也没有重建日志，文件原样留着。处理好之后再检查一次。</p>
        </>
      ),
      action: <RetryButton onRecheck={onRecheck} label="再检查一次" />,
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
      action: <RetryButton onRecheck={onRecheck} label="再检查一次" />,
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
        <div className="mt-5 flex items-center gap-3">{view.action}</div>
      </div>
    </div>
  );
}

const primary =
  'flex items-center gap-1.5 rounded-btn bg-ink px-3 py-1.5 text-2 text-content transition-opacity hover:opacity-90';
