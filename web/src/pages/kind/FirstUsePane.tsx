/**
 * 首次使用的 Kind：对话面板（brief §四，用户 2026-10-27 晚定稿；10 篇第 2 节）。
 *
 * 两步都在对话里完成，不跳设置页：
 * 1) 接模型：选提供方、填密钥。密钥交给 Keyring（和「设置 › 模型与连接」同一套：
 *    同一份提供方目录、同一个保存和验证）；「已保存」和「连接可用」分两步显示；
 * 2) 接好后 Kind 说一句，下面 3 张起手卡片（每张对应一个真实能力），点一张它说明这次按哪档做；
 *    再可选问一句「你主要用我做什么」，按回答推荐内置流程和角色——只推荐、不自动采用。
 * 打开开发开关「首次使用（空库）」时显示，数据从 api 取，不 import 样例。
 * 原型约定：以 sk-bad 开头的密钥模拟被服务商拒绝。
 */

import { useEffect, useState } from 'react';
import { Check, Eye, EyeOff, Loader2, X } from 'lucide-react';
import { toast } from 'sonner';
import { addConnection, checkConnection, getModelSettings, KERNEL_PROVIDERS } from '@/data/keyring';
import type { CatalogProvider, ConnectionView } from '@/data/keyring';
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

function ConnectForm({ onConnected }: { onConnected: () => void }) {
  const [catalog, setCatalog] = useState<CatalogProvider[]>([]);
  const [provider, setProvider] = useState('');
  const [key, setKey] = useState('');
  const [shown, setShown] = useState(false);
  const [busy, setBusy] = useState(false);
  /** 这次提交以后的连接状态；没提交过是 null。 */
  const [conn, setConn] = useState<ConnectionView | null>(null);
  const active = catalog.find((item) => item.id === provider);

  useEffect(() => {
    let alive = true;
    void getModelSettings().then((view) => {
      if (!alive) return;
      // 只显示接上内核的部分时，目录里只有内核会说的（D36：DeepSeek）
      const items = view.catalog.filter((item) => KERNEL_PROVIDERS.has(item.id));
      setCatalog(items);
      setProvider((current) => current || items[0]?.id || '');
    });
    return () => {
      alive = false;
    };
  }, []);

  const submit = async () => {
    if (!active || !key.trim()) return;
    setBusy(true);
    try {
      const saved = await addConnection({
        source: 'catalog',
        id: active.id,
        name: active.name,
        protocol: active.protocol,
        endpoint: active.defaultEndpoint,
        models: active.defaultModels,
        key,
      });
      setKey('');
      setShown(false);
      setConn(saved.connections.find((item) => item.id === active.id) ?? null);
      const checked = await checkConnection(active.id);
      setConn(checked.connections.find((item) => item.id === active.id) ?? null);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const ok = conn?.check.kind === 'ok';
  const failed = conn?.check.kind === 'fail' ? conn.check.reason : null;

  return (
    <div className="flex h-full items-center justify-center overflow-y-auto px-6">
      <div className="w-full max-w-[560px] py-10" data-first-use="connect">
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
            {catalog.map((item) => (
              <button
                key={item.id}
                type="button"
                disabled={busy || ok}
                onClick={() => {
                  setProvider(item.id);
                  setConn(null);
                }}
                className={cn(
                  'rounded-btn border px-2.5 py-1 text-2 transition-colors disabled:cursor-not-allowed',
                  item.id === provider
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

          {conn ? (
            <div className="mt-4 space-y-1" data-first-use="status">
              <StepLine state={conn.key.saved ? 'ok' : 'wait'}>已保存到 Keyring</StepLine>
              <StepLine state={ok ? 'ok' : failed ? 'fail' : 'run'}>
                {ok ? '连接可用' : failed ? `连接不可用：${failed}` : `正在向 ${conn.name} 验证…`}
              </StepLine>
            </div>
          ) : null}

          <div className="mt-4 flex flex-wrap items-center gap-2">
            {ok ? (
              <button
                type="button"
                onClick={onConnected}
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

export function FirstUsePane({ onConnected }: { onConnected: () => void }) {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto bg-content" data-pane="conversation">
      <ConnectForm onConnected={onConnected} />
    </div>
  );
}
