/**
 * 连接卡（T017.4）：首次使用的连接卡从信封渲染。
 * 槽位来自 Keyring 状态（提供方列表、已保存、验证结果），密钥输入只留在组件里，不进信封。
 * 保存、验证、开始都走 CardHost 给的程序操作，组件不直接碰 /api。
 */

import { useState } from 'react';
import { Check, Eye, EyeOff, Loader2, X } from 'lucide-react';
import type { ConnectionProviderSlot, ConnectionSlots } from '@/data/connectionCard';
import { modelErrorReason } from '@/data/errors';
import type { CardViewProps } from './model';
import { cn } from '@/lib/utils';

function KindOrb({ size = 34 }: { size?: number }) {
  return (
    <span
      className="block shrink-0 rounded-full"
      style={{
        width: size,
        height: size,
        background: 'radial-gradient(circle at 35% 30%, var(--brand), var(--brand-soft))',
      }}
    />
  );
}

/** 两步各一行：已保存 / 连接可用。 */
function StepLine({ state, children }: { state: 'wait' | 'run' | 'ok' | 'fail'; children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-2 text-2 leading-5">
      <span className="mt-0.5 grid size-4 shrink-0 place-items-center">
        {state === 'ok' ? <Check className="size-3.5" style={{ color: 'var(--done)' }} /> : null}
        {state === 'fail' ? <X className="size-3.5" style={{ color: 'var(--fail)' }} /> : null}
        {state === 'run' ? <Loader2 className="size-3.5 animate-spin text-ink-3" /> : null}
        {state === 'wait' ? <span className="block size-1.5 rounded-full bg-line-2" /> : null}
      </span>
      <span className={state === 'wait' ? 'text-ink-3' : state === 'fail' ? '' : 'text-ink'} style={state === 'fail' ? { color: 'var(--fail)' } : undefined}>
        {children}
      </span>
    </div>
  );
}

/** 信封里选中的提供方；没有就退回目录里的第一个。 */
function selectedProviderOf(slots: ConnectionSlots): string {
  if (slots.provider && slots.providers.some((item) => item.id === slots.provider)) return slots.provider;
  return slots.providers[0]?.id ?? '';
}

function activeProviderOf(slots: ConnectionSlots, selected: string): ConnectionProviderSlot | undefined {
  return slots.providers.find((item) => item.id === selected) ?? slots.providers[0];
}

export function ConnectionCardView({ envelope, slots, host }: CardViewProps<ConnectionSlots>) {
  const [selected, setSelected] = useState(() => selectedProviderOf(slots));
  /** 用户换了提供方：上一个连接的状态不再显示（和以前点提供方就清空 conn 一样）。 */
  const [dismissed, setDismissed] = useState(false);
  const [key, setKey] = useState('');
  const [shown, setShown] = useState(false);
  const [busy, setBusy] = useState(false);
  const active = activeProviderOf(slots, selected);
  const saved = slots.saved && !dismissed;
  const ok = saved && slots.check === 'ok';
  const failed = saved && slots.check === 'fail' ? modelErrorReason(slots.reason, slots.name) : null;

  const submit = async () => {
    if (!active || !key.trim() || busy || !host.saveConnection) return;
    setBusy(true);
    try {
      const savedId = await host.saveConnection(active.id, key);
      if (!savedId) return;
      setDismissed(false);
      setKey('');
      setShown(false);
      await host.verifyConnection?.(savedId);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-full items-center justify-center overflow-y-auto px-6">
      <div
        className="w-full max-w-[560px] py-10"
        data-first-use="connect"
        data-card="connection"
        data-card-state={envelope.state}
      >
        <div className="flex items-center gap-2.5">
          <KindOrb />
          <span className="text-3 text-ink-2">Kind</span>
        </div>
        <h1 className="mt-5 text-5 font-medium leading-7 text-ink">先接一个模型，我们就能开始了</h1>
        <p className="mt-1 text-2 leading-5 text-ink-2">
          用你自己的模型服务，不用注册 AIOS 账号。密钥交给 Keyring 加密保存在这台 Mac 上，保存以后页面、日志和对话里都不再出现。
        </p>

        <div className="mt-5 rounded-card border border-line-2 bg-raised/60 p-4">
          <div className="text-1 text-ink-3">提供方</div>
          <div className="mt-1.5 flex flex-wrap gap-1.5">
            {slots.providers.map((item) => (
              <button
                key={item.id}
                type="button"
                disabled={busy || ok}
                onClick={() => {
                  setSelected(item.id);
                  setDismissed(true);
                }}
                className={cn(
                  'rounded-btn border px-2.5 py-1 text-2 transition-colors disabled:cursor-not-allowed',
                  item.id === active?.id
                    ? 'border-ink-3 bg-hover text-ink'
                    : 'border-line-2 text-ink-2 hover:bg-hover hover:text-ink disabled:hover:bg-transparent',
                )}
              >
                {item.name}
              </button>
            ))}
          </div>

          {ok ? null : (
            <>
              <label className="mt-4 block text-1 text-ink-3" htmlFor="first-use-key">
                {active?.name ?? '提供方'} 的 API 密钥
              </label>
              <div className="relative mt-1">
                <input
                  id="first-use-key"
                  type={shown ? 'text' : 'password'}
                  autoComplete="new-password"
                  spellCheck={false}
                  value={key}
                  disabled={busy}
                  onChange={(event) => {
                    setKey(event.target.value);
                    setShown(false);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') void submit();
                  }}
                  placeholder={failed ? '换一把再试' : '输入 API 密钥'}
                  className={cn(
                    'w-full rounded-btn border border-line-2 bg-pane px-2.5 py-1.5 text-3 text-ink outline-none placeholder:text-ink-3 focus:border-ink-3',
                    key && 'pr-9',
                  )}
                />
                {key ? (
                  <button
                    type="button"
                    onClick={() => setShown((on) => !on)}
                    aria-label={shown ? '隐藏密钥' : '显示密钥'}
                    className="absolute right-1.5 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink"
                  >
                    {shown ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
                  </button>
                ) : null}
              </div>
            </>
          )}

          {saved ? (
            <div className="mt-4 space-y-1" data-first-use="status">
              <StepLine state={slots.saved ? 'ok' : 'wait'}>已保存到 Keyring</StepLine>
              <StepLine state={ok ? 'ok' : failed ? 'fail' : 'run'}>
                {ok ? '连接可用' : failed ? `连接不可用：${failed}` : `正在向 ${slots.name || active?.name || '提供方'} 验证…`}
              </StepLine>
            </div>
          ) : null}

          <div className="mt-4 flex flex-wrap items-center gap-2">
            {ok ? (
              <button
                type="button"
                onClick={() => host.start?.()}
                className="rounded-btn bg-ink px-3 py-1.5 text-2 text-content transition-opacity hover:opacity-90"
              >
                开始
              </button>
            ) : (
              <button
                type="button"
                disabled={!key.trim() || busy}
                onClick={() => void submit()}
                className={cn(
                  'rounded-btn px-3 py-1.5 text-2 transition-opacity',
                  key.trim() && !busy ? 'bg-ink text-content hover:opacity-90' : 'cursor-not-allowed bg-hover text-ink-3',
                )}
              >
                {busy ? '正在接…' : failed ? '再试一次' : '接上'}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
