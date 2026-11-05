/**
 * 设置 › 模型与连接。照 DSH 的模型设置页（用户 2026-11-01 给的两张截图）：
 *
 * - 提供方一行一行列，一次只展开一张编辑卡片；
 * - 「添加模型提供商」分两种方式：第三方模型提供商（内置目录）、自定义模型 API；
 *   两边的草稿各自保留，切换方式不丢；
 * - 密钥框只写不读：初始为空，保存后从不回显；留空就是不动已保存的那一把；
 * - 和 DSH 不同的地方：没有环境变量后备（D34），在这里存的密钥全部进 Keyring；
 *   「已保存」和「连接可用」分开显示（docs 10 篇第 2 节）。
 *
 * 下面的「Keyring」一节：加密钥匙放在哪（用户定，D24、D41）、签发记录（docs 09 篇 7.1）。
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import { ChevronDown, ChevronRight, CircleAlert, Eye, EyeOff, Plus, X } from 'lucide-react';
import { toast } from 'sonner';
import {
  addConnection, checkConnection, discoverModels, getModelSettings, moveKeyFile, relinkKeyFile, removeConnection,
  resetKeyring, revokeKey, updateConnection,
} from '@/data/keyring';
import { displayPath, fetchConnectionQuota, type ServiceQuota } from '@/data/service';
import { CURRENCIES } from '@/lib/numbers';
import { useKindStore } from '@/pages/kind/kindStore';
import type { ConnectionView, ModelRow, ModelSettingsView } from '@/data/fixtures/settings';
import { PROTOCOL_LABEL } from '@/data/fixtures/settings';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { modelErrorReason } from '@/data/errors';

/* ───────────────────────── 小部件 ───────────────────────── */

/** 用户单价（美元/百万 token）；空串表示继承目录价。 */
function parsePrice(text: string): number | undefined {
  const n = Number(text.trim());
  return text.trim() !== '' && Number.isFinite(n) && n >= 0 ? n : undefined;
}

const inputClass =
  'w-full rounded-btn border border-line-2 bg-content px-2.5 py-1.5 text-3 text-ink outline-none placeholder:text-ink-3 focus:border-ink-3 aria-invalid:border-[var(--fail)]';

function Field({ label, help, hint, tone, children }: {
  label: string;
  help?: string;
  hint?: React.ReactNode;
  tone?: 'warn' | 'fail' | 'ok';
  children: React.ReactNode;
}) {
  return (
    <div className="mt-4 first:mt-0">
      <div className="mb-1.5 flex items-center gap-1.5 text-2 text-ink-2">
        {label}
        {help ? (
          <span
            title={help}
            aria-label={help}
            className="grid size-4 cursor-help place-items-center rounded-full border border-line-2 text-[10px] text-ink-3"
          >
            ?
          </span>
        ) : null}
      </div>
      {children}
      {hint ? (
        <div
          className={cn('mt-1.5 text-1 leading-4', tone ? '' : 'text-ink-3')}
          style={tone ? { color: `var(--${tone === 'warn' ? 'wait' : tone === 'ok' ? 'done' : 'fail'})` } : undefined}
        >
          {hint}
        </div>
      ) : null}
    </div>
  );
}

function Btn({ kind = 'plain', className, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { kind?: 'primary' | 'plain' | 'quiet' | 'danger' }) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        'shrink-0 whitespace-nowrap rounded-btn px-3 py-1.5 text-2 transition-colors disabled:cursor-not-allowed disabled:opacity-45',
        kind === 'primary' && 'bg-ink text-content hover:opacity-90',
        kind === 'plain' && 'border border-line-2 text-ink-2 hover:bg-hover hover:text-ink',
        kind === 'quiet' && 'text-ink-3 hover:bg-hover hover:text-ink',
        kind === 'danger' && 'text-[var(--fail)] hover:bg-[var(--fail-soft)]',
        className,
      )}
    />
  );
}

/** 一行的状态点：绿 = 可用；红 = 不可用；黄 = 没有密钥；蓝 = 正在验证；灰 = 有密钥但还没验证。 */
function StatusDot({ conn, broken }: { conn: ConnectionView; broken?: boolean }) {
  const color = broken && conn.key.saved
    ? 'var(--fail)'
    : !conn.key.saved
    ? 'var(--wait)'
    : conn.check.kind === 'ok'
      ? 'var(--done)'
      : conn.check.kind === 'fail'
        ? 'var(--fail)'
        : conn.check.kind === 'checking'
          ? 'var(--run)'
          : 'var(--ink-3)';
  return (
    <span
      className={cn('block size-2 shrink-0 rounded-full', conn.check.kind === 'checking' && 'animate-pulse')}
      style={{ backgroundColor: color }}
    />
  );
}

/** 密钥和连接状态的一句话（「已保存」「连接可用」分开说）。 */
function keyStatus(conn: ConnectionView | null, broken?: boolean): { text: string; tone?: 'warn' | 'fail' | 'ok' } {
  if (!conn || !conn.key.saved) return { text: '还没有密钥。填上才能用这个提供商。', tone: 'warn' };
  const saved = `已保存在 Keyring（${conn.key.savedAt}）`;
  if (broken) return { text: `${saved} · 现在读不出来，暂时不能用`, tone: 'fail' };
  switch (conn.check.kind) {
    case 'ok':
      return { text: `${saved} · 连接可用（${conn.check.at} 验证）`, tone: 'ok' };
    case 'fail':
      return { text: `${saved} · 不可用：${modelErrorReason(conn.check.reason)}`, tone: 'fail' };
    case 'checking':
      return { text: `${saved} · 正在验证…` };
    default:
      return { text: `${saved} · 还没验证过` };
  }
}

function KeyInput({ id, value, onChange, conn }: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  conn: ConnectionView | null;
}) {
  const [shown, setShown] = useState(false);
  const status = keyStatus(conn);
  return (
    <Field
      label="API 密钥"
      help="密钥交给 Keyring 加密保存在这台 Mac 上；保存以后页面、日志和对话里都不再出现。"
      hint={value ? undefined : status.text}
      tone={value ? undefined : status.tone}
    >
      <div className="relative">
        <input
          id={id}
          type={shown ? 'text' : 'password'}
          autoComplete="new-password"
          spellCheck={false}
          value={value}
          onChange={(event) => {
            onChange(event.target.value);
            setShown(false);
          }}
          placeholder={conn?.key.saved ? '已保存一把；留空就继续用它' : '输入 API 密钥'}
          className={cn(inputClass, value && 'pr-9')}
        />
        {value ? (
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
    </Field>
  );
}

function formatTokens(n?: number): string {
  if (!n) return '';
  return n >= 1000000 ? n / 1000000 + 'M' : n >= 1000 ? Math.round(n / 1000) + 'K' : String(n);
}

function parseTokens(text: string): number | undefined {
  const t = text.trim().toUpperCase();
  if (!t) return undefined;
  const m = /^(\d+(?:\.\d+)?)([KM]?)$/.exec(t);
  if (!m) return undefined;
  const base = parseFloat(m[1]);
  return Math.round(m[2] === 'M' ? base * 1000000 : m[2] === 'K' ? base * 1000 : base);
}

/* ───────────────────────── 模型目录 ───────────────────────── */

function ModelCatalog({ models, onChange, defaults, discover, required }: {
  models: ModelRow[];
  onChange: (models: ModelRow[]) => void;
  /** 目录为空时用的默认模型（内置目录才有）。 */
  defaults?: ModelRow[];
  discover: () => Promise<ModelRow[]>;
  required?: boolean;
}) {
  const [candidates, setCandidates] = useState<ModelRow[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState('');
  const [asking, setAsking] = useState(false);

  const ask = async () => {
    setAsking(true);
    try {
      const list = await discover();
      setCandidates(list);
      const configured = new Set(models.map((row) => row.id));
      setPicked(new Set(models.length ? list.filter((row) => configured.has(row.id)).map((row) => row.id) : list.map((row) => row.id)));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setAsking(false);
    }
  };

  const visible = (candidates ?? []).filter(
    (row) => !query || row.id.includes(query) || (row.name ?? '').toLowerCase().includes(query.toLowerCase()),
  );

  const applyPicked = () => {
    if (!candidates) return;
    const listed = new Set(candidates.map((row) => row.id));
    const kept = models.filter((row) => !listed.has(row.id) || picked.has(row.id));
    const have = new Set(kept.map((row) => row.id));
    const added = candidates.filter((row) => picked.has(row.id) && !have.has(row.id));
    onChange(kept.concat(added));
    setCandidates(null);
    setQuery('');
  };

  const setRow = (index: number, patch: Partial<ModelRow>) =>
    onChange(models.map((row, i) => (i === index ? { ...row, ...patch } : row)));

  return (
    <div className="mt-4 border-t border-line pt-4">
      <div className="flex items-start">
        <div>
          <div className="text-2 text-ink-2">模型目录</div>
          <div className="mt-0.5 text-1 text-ink-3">
            {models.length ? `${models.length} 个模型` : defaults?.length ? '正在使用适配器默认模型' : '还没有模型'}
          </div>
        </div>
        <Btn kind="quiet" className="ml-auto" disabled={asking} onClick={() => void ask()}>
          {asking ? '正在询问…' : '获取可用模型'}
        </Btn>
      </div>

      {candidates ? (
        <div className="mt-2 rounded-card border border-line-2 bg-content p-2.5" data-settings="discover">
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索模型"
            className={inputClass}
          />
          <div className="mt-2 flex items-center gap-2 text-1 text-ink-3">
            服务商回了 {candidates.length} 个
            <button type="button" className="ml-auto hover:text-ink" onClick={() => setPicked((s) => new Set([...s, ...visible.map((r) => r.id)]))}>全选</button>
            <button type="button" className="hover:text-ink" onClick={() => setPicked((s) => new Set([...s].filter((id) => !visible.some((r) => r.id === id))))}>取消全选</button>
          </div>
          <div className="mt-1.5 max-h-[180px] overflow-y-auto">
            {visible.map((row) => (
              <label key={row.id} className="flex cursor-pointer items-center gap-2 rounded-btn px-1.5 py-1 hover:bg-hover" title={row.name ?? row.id}>
                <input
                  type="checkbox"
                  checked={picked.has(row.id)}
                  onChange={(event) =>
                    setPicked((s) => {
                      const next = new Set(s);
                      if (event.target.checked) next.add(row.id);
                      else next.delete(row.id);
                      return next;
                    })
                  }
                />
                <span className="brand-type truncate text-2 text-ink">{row.id}</span>
                {row.contextWindow ? <span className="ml-auto text-1 text-ink-3">{formatTokens(row.contextWindow)}</span> : null}
              </label>
            ))}
          </div>
          <div className="mt-2 flex justify-end gap-2">
            <Btn kind="quiet" onClick={() => setCandidates(null)}>取消</Btn>
            <Btn kind="primary" disabled={models.length > 0 && picked.size === 0} onClick={applyPicked}>更新列表</Btn>
          </div>
        </div>
      ) : null}

      {models.length ? (
        <div className="mt-2 space-y-1.5">
          <div className="grid grid-cols-[1fr_1fr_64px_64px_80px_24px] gap-1.5 px-0.5 text-1 text-ink-3">
            <span>模型 ID</span>
            <span>显示名称</span>
            <span>上下文</span>
            <span>最大输出</span>
            <span>单价 $/M</span>
            <span />
          </div>
          {models.map((row, index) => (
            <div key={index} className="grid grid-cols-[1fr_1fr_64px_64px_80px_24px] items-center gap-1.5">
              <input className={cn(inputClass, 'brand-type')} value={row.id} placeholder="model-id" onChange={(e) => setRow(index, { id: e.target.value })} />
              <input className={inputClass} value={row.name ?? ''} placeholder="可不填" onChange={(e) => setRow(index, { name: e.target.value || undefined })} />
              <input key={'cw' + index + ':' + row.contextWindow} className={inputClass} defaultValue={formatTokens(row.contextWindow)} placeholder="继承" onBlur={(e) => setRow(index, { contextWindow: parseTokens(e.target.value) })} />
              <input key={'mt' + index + ':' + row.maxTokens} className={inputClass} defaultValue={formatTokens(row.maxTokens)} placeholder="继承" onBlur={(e) => setRow(index, { maxTokens: parseTokens(e.target.value) })} />
              <input
                key={'pr' + index + ':' + row.price}
                className={cn(inputClass, 'brand-type')}
                data-settings="model-price"
                defaultValue={row.price === undefined ? '' : String(row.price)}
                placeholder="继承"
                inputMode="decimal"
                title="每百万 token 的美元价格（输入/输出同价）；填了就用它算花费，不乘倍率"
                onBlur={(e) => setRow(index, { price: parsePrice(e.target.value) })}
              />
              <button
                type="button"
                aria-label="删掉这个模型"
                onClick={() => onChange(models.filter((_, i) => i !== index))}
                className="grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink"
              >
                <X className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
      ) : (
        <div
          className="mt-2 rounded-card border border-dashed border-line-2 px-3 py-3 text-center text-2 leading-5 text-ink-3"
          style={required ? { color: 'var(--wait)' } : undefined}
        >
          {defaults?.length
            ? `模型选择器里列出默认的 ${defaults.map((row) => row.id).join('、')}；要改就在这里加。`
            : required
              ? '自定义接口至少要写一个模型。'
              : '模型选择器里不会列出模型；目录外的 ID 仍可直接发送。'}
        </div>
      )}

      <Btn kind="plain" className="mt-2.5 inline-flex items-center gap-1" onClick={() => onChange(models.concat({ id: '' }))}>
        <Plus className="size-3.5" />
        添加模型
      </Btn>
    </div>
  );
}

function Collapsible({ title, children, defaultOpen = false }: { title: string; children: React.ReactNode; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="mt-4 border-t border-line pt-3">
      <button type="button" onClick={() => setOpen((on) => !on)} className="flex items-center gap-1 text-2 text-ink-2 hover:text-ink" aria-expanded={open}>
        {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        {title}
      </button>
      {open ? <div className="mt-3">{children}</div> : null}
    </div>
  );
}

/* ───────────────────────── 已有提供方的编辑卡片 ───────────────────────── */

function ConnectionEditor({ conn, view, onDone, onView }: {
  conn: ConnectionView;
  view: ModelSettingsView;
  onDone: () => void;
  onView: (view: ModelSettingsView) => void;
}) {
  const [key, setKey] = useState('');
  const [endpoint, setEndpoint] = useState(conn.endpoint);
  const [models, setModels] = useState<ModelRow[]>(conn.models);
  const [priceRate, setPriceRate] = useState(String(conn.priceRate ?? 1));
  const [quota, setQuota] = useState<ServiceQuota | null>(null);
  const [askingQuota, setAskingQuota] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirm, setConfirm] = useState<'revoke' | 'remove' | null>(null);
  const catalog = view.catalog.find((item) => item.id === conn.id);

  const askQuota = async () => {
    setAskingQuota(true);
    try {
      setQuota(await fetchConnectionQuota(conn.id));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setAskingQuota(false);
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      const withKey = Boolean(key.trim());
      // 空串 = 不传（保留原值）；非法/负数也不传，避免把「免费」误存成 0。
      const trimmedRate = priceRate.trim();
      const rate = trimmedRate === '' ? undefined : Number(trimmedRate);
      onView(
        await updateConnection(conn.id, {
          endpoint,
          models,
          key,
          priceRate: rate !== undefined && Number.isFinite(rate) && rate >= 0 ? rate : undefined,
        }),
      );
      onDone();
      toast.success(withKey ? `已保存；${conn.name} 的新密钥进了 Keyring，正在验证` : '已保存');
      if (withKey) onView(await checkConnection(conn.id));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setSaving(false);
    }
  };

  const act = async () => {
    const what = confirm;
    setConfirm(null);
    if (what === 'revoke') {
      onView(await revokeKey(conn.id));
      toast(`${conn.name} 的密钥已撤销，之后的请求不再签发`);
    } else if (what === 'remove') {
      onView(await removeConnection(conn.id));
      onDone();
      toast(`已删除 ${conn.name}，它的密钥一起撤销了`);
    }
  };

  return (
    <div className="mx-1 mb-2 rounded-card border border-line-2 bg-raised/60 p-4" data-settings="editor">
      <KeyInput id={'key-' + conn.id} value={key} onChange={setKey} conn={conn} />
      <Collapsible title="自定义设置" defaultOpen={conn.source === 'custom'}>
        <Field label="API 地址" hint={`协议：${PROTOCOL_LABEL[conn.protocol]}`}>
          <input className={inputClass} value={endpoint} onChange={(e) => setEndpoint(e.target.value)} placeholder={conn.defaultEndpoint} />
        </Field>
        <ModelCatalog
          models={models}
          onChange={setModels}
          defaults={catalog?.defaultModels}
          required={conn.source === 'custom'}
          discover={() => discoverModels(conn.id)}
        />
        <Field
          label="价格倍率"
          help="目录标价乘这个数算花费；1 = 原价，0 = 这个连接免费（例如 Coding Plan）。"
        >
          <input
            className={cn(inputClass, 'brand-type')}
            value={priceRate}
            inputMode="decimal"
            onChange={(event) => setPriceRate(event.target.value)}
          />
        </Field>
      </Collapsible>
      <Collapsible title="余额与套餐余量">
        <div data-settings="quota">
          <Btn kind="quiet" disabled={askingQuota} onClick={() => void askQuota()}>
            {askingQuota ? '正在查询…' : '查询余额/余量'}
          </Btn>
          {quota ? (
            <div className="mt-2 space-y-0.5 text-2 text-ink-2">
              {quota.balance ? (
                <div>
                  余额：
                  <span className="brand-type">
                    {quota.balance.currency} {quota.balance.amount.toFixed(2)}
                  </span>
                </div>
              ) : null}
              {quota.plan ? (
                <div>
                  套餐余量：5 小时窗口{' '}
                  <span className="brand-type">{quota.plan.intervalRemainingPercent ?? '—'}%</span> · 每周{' '}
                  <span className="brand-type">{quota.plan.weeklyRemainingPercent ?? '—'}%</span>
                </div>
              ) : null}
              {!quota.balance && !quota.plan ? (
                <div className="text-ink-3">这个连接查不到余额/余量（只有 DeepSeek 与 MiniMax 官方地址支持）。</div>
              ) : null}
            </div>
          ) : null}
        </div>
      </Collapsible>
      <div className="mt-5 flex items-center gap-2">
        {conn.key.saved ? <Btn kind="danger" onClick={() => setConfirm('revoke')}>撤销密钥</Btn> : null}
        <Btn kind="quiet" onClick={() => setConfirm('remove')}>删除这个提供商</Btn>
        <Btn kind="plain" className="ml-auto" onClick={onDone}>取消</Btn>
        <Btn kind="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存'}</Btn>
      </div>

      <Dialog open={confirm !== null} onOpenChange={(open) => (open ? null : setConfirm(null))}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{confirm === 'revoke' ? `撤销 ${conn.name} 的密钥？` : `删除 ${conn.name}？`}</DialogTitle>
            <DialogDescription>
              {confirm === 'revoke'
                ? '立刻生效：之后用它的模型请求都会停下，Kind 正在做的事会停在「等连接」，直到你再填一把。撤销会留下记录，密钥本身不留。'
                : '这个提供商的地址、模型目录一起删掉；它在 Keyring 里的密钥同时撤销。用它的模型请求会停下。'}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Btn kind="plain" onClick={() => setConfirm(null)}>取消</Btn>
            <Btn kind="primary" className="!bg-[var(--fail)] !text-white" onClick={() => void act()}>
              {confirm === 'revoke' ? '撤销' : '删除'}
            </Btn>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/* ───────────────────────── 添加模型提供商 ───────────────────────── */


function CatalogPanel({ view, onCreated, onCancel }: { view: ModelSettingsView; onCreated: (id: string, withKey: boolean, next: ModelSettingsView) => void; onCancel: () => void }) {
  const choices = view.catalog.filter((item) => !view.connections.some((conn) => conn.id === item.id));
  const [providerId, setProviderId] = useState(choices[0]?.id ?? '');
  const [key, setKey] = useState('');
  const [endpoint, setEndpoint] = useState('');
  const [models, setModels] = useState<ModelRow[]>([]);
  const [saving, setSaving] = useState(false);
  const provider = choices.find((item) => item.id === providerId) ?? choices[0];

  if (!provider) {
    return <p className="text-2 text-ink-3">内置目录里的提供商都已经加过了。</p>;
  }

  const save = async () => {
    setSaving(true);
    try {
      const next = await addConnection({
        source: 'catalog', id: provider.id, name: provider.name, protocol: provider.protocol, endpoint, models, key,
      });
      onCreated(provider.id, Boolean(key.trim()), next);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <Field label="提供商">
        <select
          value={provider.id}
          onChange={(e) => {
            setProviderId(e.target.value);
            setModels([]);
            setEndpoint('');
          }}
          className={cn(inputClass, 'w-auto min-w-[240px]')}
        >
          {choices.map((item) => (
            <option key={item.id} value={item.id}>{item.name}</option>
          ))}
        </select>
      </Field>
      <KeyInput id="add-catalog-key" value={key} onChange={setKey} conn={null} />
      <Collapsible title="自定义设置">
        <Field label="API 地址" hint={`协议：${PROTOCOL_LABEL[provider.protocol]}`}>
          <input className={inputClass} value={endpoint} onChange={(e) => setEndpoint(e.target.value)} placeholder={provider.defaultEndpoint} />
        </Field>
        <ModelCatalog
          models={models}
          onChange={setModels}
          defaults={provider.defaultModels}
          // 新连接还没有 id：先列内置目录的模型；保存后在「编辑」里能对真实厂商再拉一次。
          discover={async () => provider.defaultModels ?? []}
        />
      </Collapsible>
      <div className="mt-5 flex justify-end gap-2">
        <Btn kind="plain" onClick={onCancel}>取消</Btn>
        <Btn kind="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存'}</Btn>
      </div>
    </div>
  );
}



function AddCard({ view, onCreated, onCancel }: { view: ModelSettingsView; onCreated: (id: string, withKey: boolean, next: ModelSettingsView) => void; onCancel: () => void }) {
  return (
    <div className="mt-2 rounded-card border border-line-2 bg-raised/60 p-4" data-settings="add">
      <p className="mb-4 text-1 leading-4 text-ink-3">从内置目录选一家（DeepSeek / MiniMax 国内站 / MiniMax 国际站），填上 API 密钥就能用。</p>
      <CatalogPanel view={view} onCreated={onCreated} onCancel={onCancel} />
    </div>
  );
}

/* ───────────────────────── Keyring 读不出来 ───────────────────────── */

/**
 * 钥匙文件不在原处、打不开或解不开（09 篇第 7 节）：
 * - 所有提供商一起停用，不悄悄改用环境变量或别处的密钥；
 * - 能做的两步：告诉 AIOS 钥匙文件现在在哪（只改指向）；找不回就重新开始（换新钥匙，密钥重填）。
 */
function KeyringBroken({ view, onView }: { view: ModelSettingsView; onView: (view: ModelSettingsView) => void }) {
  const [relinking, setRelinking] = useState(false);
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);

  const relink = async () => {
    setBusy(true);
    try {
      const next = await relinkKeyFile(path);
      onView(next);
      toast.success('找到了，Keyring 又能读出来了');
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const reset = async () => {
    const next = await resetKeyring();
    setConfirmReset(false);
    onView(next);
    toast.success('换了一把新钥匙；给每个提供商重新填一次密钥');
  };

  return (
    <div className="mt-5 rounded-card border border-fail/40 bg-fail-soft/40 px-4 py-3" data-settings="keyring-broken">
      <div className="flex items-start gap-2">
        <CircleAlert className="mt-[3px] size-3.5 shrink-0 text-fail" />
        <div className="min-w-0 text-2 leading-5">
          <div className="text-3 text-ink">Keyring 现在读不出来</div>
          <p className="mt-0.5 text-ink-2">
            加密钥匙 <span className="brand-type">{displayPath(view.keyFile.path)}</span>：{view.keyFile.problem}。
            所有提供商都暂时不能用；AIOS 不会改用环境变量或别处的密钥。
          </p>
        </div>
      </div>
      {relinking ? (
        <div className="mt-3 flex items-center gap-2 pl-5.5">
          <input
            autoFocus
            className={cn(inputClass, 'brand-type')}
            value={path}
            onChange={(event) => setPath(event.target.value)}
            placeholder="钥匙文件现在的完整路径，比如 ~/Backups/keyring.key"
            onKeyDown={(event) => event.key === 'Enter' && void relink()}
          />
          <Btn kind="plain" onClick={() => setRelinking(false)}>取消</Btn>
          <Btn kind="primary" disabled={busy || !path.trim()} onClick={() => void relink()}>{busy ? '正在读…' : '用这个'}</Btn>
        </div>
      ) : (
        <div className="mt-3 flex items-center gap-2 pl-5.5">
          <Btn kind="primary" onClick={() => setRelinking(true)}>指到它现在的位置</Btn>
          <Btn kind="danger" onClick={() => setConfirmReset(true)}>找不回了，重新开始</Btn>
        </div>
      )}

      <Dialog open={confirmReset} onOpenChange={(open) => (open ? null : setConfirmReset(false))}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>换一把新钥匙，重新开始？</DialogTitle>
            <DialogDescription>
              旧钥匙加密的密钥都读不回来了：每个提供商要重新填一次密钥。提供商的地址和模型目录都留着。旧钥匙以后找回来也用不上了。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Btn kind="plain" onClick={() => setConfirmReset(false)}>取消</Btn>
            <Btn kind="primary" className="!bg-[var(--fail)] !text-white" onClick={() => void reset()}>重新开始</Btn>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/* ───────────────────────── Keyring 一节 ───────────────────────── */

/** T012.9：显示币种（settings.json 的 currency；价格和花费一律按美元存，显示时换算）。 */
function CurrencySection() {
  const money = useKindStore((state) => state.money);
  const setCurrency = useKindStore((state) => state.setCurrency);
  return (
    <section className="mt-5" data-settings="currency">
      <div className="flex items-center gap-2">
        <span className="text-2 text-ink-2">显示币种</span>
        {CURRENCIES.map((item) => (
          <button
            key={item.id}
            type="button"
            onClick={() => void setCurrency(item.id)}
            className={cn(
              'h-6 rounded-btn px-2 text-2 transition-colors',
              money.currency === item.id ? 'bg-ink text-content' : 'text-ink-2 hover:bg-hover hover:text-ink',
            )}
          >
            {item.label}
          </button>
        ))}
        <span className="text-1 text-ink-3">
          花费按 1 美元 = {money.cnyPerUsd} 元换算{money.fxAt ? '' : '（还没取到汇率，先按 7.2）'}
        </span>
      </div>
    </section>
  );
}

function KeyringSection({ view, onView }: { view: ModelSettingsView; onView: (view: ModelSettingsView) => void }) {
  const [moving, setMoving] = useState(false);
  const [path, setPath] = useState(view.keyFile.path);
  const [showIssues, setShowIssues] = useState(false);

  const move = async () => {
    try {
      onView(await moveKeyFile(path));
      setMoving(false);
      toast.success('钥匙已经挪过去，旧位置的文件删掉了');
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <section className="mt-10" data-settings="keyring">
      <h2 className="text-3 font-medium text-ink">Keyring</h2>
      <p className="mt-1 text-2 leading-5 text-ink-2">
        上面存的密钥都在这里，加密保存。Kind 和模型从不拿到密钥本身：每次请求由内核向 Keyring 申请一次，只在发出的那一刻展开。
      </p>

      <div className="mt-3 divide-y divide-line rounded-card border border-line">
        <div className="px-4 py-3">
          <div className="flex items-center gap-2">
            <div className="min-w-0">
              <div className="text-3 text-ink">加密钥匙</div>
              <div className="mt-0.5 text-1 text-ink-3">放在哪由你定。现在是一个文件，{view.keyFile.since}起放在这里。</div>
            </div>
            {moving || view.keyFile.problem ? null : <Btn kind="plain" className="ml-auto" onClick={() => setMoving(true)}>换位置</Btn>}
          </div>
          {moving ? (
            <div className="mt-2.5 flex items-center gap-2">
              <input className={cn(inputClass, 'brand-type')} value={path} onChange={(e) => setPath(e.target.value)} />
              <Btn kind="plain" onClick={() => { setMoving(false); setPath(view.keyFile.path); }}>取消</Btn>
              <Btn kind="primary" onClick={() => void move()}>挪过去</Btn>
            </div>
          ) : (
            <div className="mt-2 flex items-center gap-2 text-2">
              <span className={cn('brand-type truncate', view.keyFile.problem ? 'text-ink-3 line-through' : 'text-ink-2')}>{displayPath(view.keyFile.path)}</span>
              {view.keyFile.problem ? <span className="shrink-0 text-fail">{view.keyFile.problem}</span> : null}
            </div>
          )}
          <p className="mt-2.5 text-1 leading-4 text-ink-3">
            这台 Mac 上以你的身份运行的程序，包括 Kind 替你执行的命令，都能读到这个文件。加密防的是数据被复制、同步或备份出去，挡不住本机上的程序。
          </p>
        </div>

        <div className="px-4 py-3">
          <div className="flex items-center gap-2">
            <div>
              <div className="text-3 text-ink">签发记录</div>
              <div className="mt-0.5 text-1 text-ink-3">
                {view.issuedToday ? `今天签发 ${view.issuedToday} 次，最近一次 ${view.issues[0]?.at ?? '—'}。只记谁用、做什么，不记密钥。` : '今天还没有签发过。'}
              </div>
            </div>
            {view.issues.length ? (
              <Btn kind="quiet" className="ml-auto" onClick={() => setShowIssues((on) => !on)}>{showIssues ? '收起' : '看最近的'}</Btn>
            ) : null}
          </div>
          {showIssues ? (
            <table className="mt-2.5 w-full text-2">
              <thead>
                <tr className="text-left text-1 text-ink-3">
                  <th className="py-1 font-normal">时间</th>
                  <th className="py-1 font-normal">提供商</th>
                  <th className="py-1 font-normal">用途</th>
                  <th className="py-1 font-normal">有效到</th>
                </tr>
              </thead>
              <tbody>
                {view.issues.map((row) => (
                  <tr key={row.id} className="border-t border-line text-ink-2">
                    <td className="brand-type py-1">{row.at}</td>
                    <td className="py-1">{row.connection}</td>
                    <td className="py-1">{row.purpose}</td>
                    <td className="brand-type py-1">{row.until}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : null}
        </div>
      </div>
    </section>
  );
}

/* ───────────────────────── 整组 ───────────────────────── */

export function ModelSettings() {
  useEffect(() => {
    // T012.9：直接打开设置页（不经 Kind 页）时也要拿币种/汇率。
    void useKindStore.getState().refreshStats();
  }, []);
  const [view, setView] = useState<ModelSettingsView | null>(null);
  /** 展开的是哪张卡片：连接 id、'add'，或者都不展开。一次只展开一张。 */
  const [open, setOpen] = useState<string | null>(null);

  const load = useCallback(async () => {
    const next = await getModelSettings();
    setView(next);
    // 首次运行：一个提供商都没有时，直接展开添加卡片
    if (!next.connections.length) setOpen('add');
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const verify = async (id: string) => {
    // 先把这一行标成「正在验证」，结果回来再换
    setView((current) =>
      current && {
        ...current,
        connections: current.connections.map((conn) => (conn.id === id ? { ...conn, check: { kind: 'checking' } } : conn)),
      },
    );
    try {
      setView(await checkConnection(id));
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  const onCreated = async (id: string, withKey: boolean, next: ModelSettingsView) => {
    setView(next);
    setOpen(null);
    toast.success(withKey ? '已添加；密钥进了 Keyring，正在验证' : '已添加；填上密钥才能用');
    if (withKey) setView(await checkConnection(id));
  };

  const rows = useMemo(() => view?.connections ?? [], [view]);

  if (!view) return <div className="mt-6 text-2 text-ink-3">正在读取…</div>;
  const broken = Boolean(view.keyFile.problem);

  return (
    <div data-settings="model">
      <p className="mt-1.5 text-2 leading-5 text-ink-2">
        用你自己的模型服务，不用注册 AIOS 账号。在这里填的密钥都存进 Keyring。
      </p>

      {broken ? <KeyringBroken view={view} onView={setView} /> : null}

      <CurrencySection />

      <section className="mt-6">
        <h2 className="text-3 font-medium text-ink">模型提供商</h2>
        <div className="mt-2">
          {rows.length === 0 ? (
            <p className="text-2 text-ink-3">还没有接任何模型。接上一个，Kind 才能开始做事。</p>
          ) : (
            <div className="rounded-card border border-line">
              {rows.map((conn, index) => (
                <div key={conn.id} className={cn(index > 0 && 'border-t border-line')}>
                  <div className="flex items-center gap-3 px-4 py-3">
                    <StatusDot conn={conn} broken={broken} />
                    <div className="min-w-0">
                      <div className="text-3 text-ink">{conn.name}</div>
                      <div className="mt-0.5 truncate text-1 text-ink-3">
                        {keyStatus(conn, broken).text}
                      </div>
                    </div>
                    <div className="ml-auto flex shrink-0 items-center gap-1">
                      {conn.key.saved ? (
                        <Btn
                          kind="quiet"
                          disabled={broken || conn.check.kind === 'checking'}
                          title={broken ? '先让 Keyring 能读出来' : undefined}
                          onClick={() => void verify(conn.id)}
                        >
                          {conn.check.kind === 'checking' ? '正在验证…' : '验证'}
                        </Btn>
                      ) : null}
                      <Btn kind="plain" onClick={() => setOpen(open === conn.id ? null : conn.id)}>
                        {open === conn.id ? '收起' : '编辑'}
                      </Btn>
                    </div>
                  </div>
                  {open === conn.id ? (
                    <ConnectionEditor key={conn.id} conn={conn} view={view} onDone={() => setOpen(null)} onView={setView} />
                  ) : null}
                </div>
              ))}
            </div>
          )}

          {open === 'add' ? (
            <AddCard view={view} onCreated={(id, withKey, next) => void onCreated(id, withKey, next)} onCancel={() => setOpen(null)} />
          ) : (
            <Btn kind="plain" className="mt-2.5 inline-flex items-center gap-1" onClick={() => setOpen('add')}>
              <Plus className="size-3.5" />
              添加模型提供商
            </Btn>
          )}
        </div>
      </section>

      <KeyringSection view={view} onView={setView} />

    </div>
  );
}
