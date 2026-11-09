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
import { Ban, ChevronDown, ChevronRight, CircleAlert, Eye, EyeOff, Loader2, Plus, X } from 'lucide-react';
import { toast } from 'sonner';
import {
  addConnection, checkConnection, discoverModels, getExaKey, getModelSettings, moveKeyFile, relinkKeyFile,
  removeConnection, removeExaKey, resetKeyring, revokeKey, saveExaKey, setDisabledModels, updateConnection,
  verifyExaKey,
} from '@/data/keyring';
import { matchDisabled } from '@/data/disabled';
import {
  checkDisabledModels, displayPath, type DisabledModelUser, type ServiceQuota,
} from '@/data/service';
import { cachedQuota, loadQuota } from '@/data/quota';
import { CURRENCIES, compactPrice, formatModelTokens } from '@/lib/numbers';
import { formatLocalDateTime } from '@/lib/time';
import { useSettingsState } from '@/lib/settingsState';
import { PROTOCOL_LABEL } from '@/data/keyring';
import type { ConnectionView, ModelRow, ModelSettingsView, ToolKeyView } from '@/data/keyring';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { modelErrorReason } from '@/data/errors';
import { connectionCardOf } from '@/data/connectionCard';
import type { ConnectionSlots } from '@/data/connectionCard';
import type { CardEnvelope } from '@/lib/cardEnvelope';

/* ───────────────────────── 小部件 ───────────────────────── */

/** 用户单价（美元/百万 token）；空串表示继承目录价。 */
function parsePrice(text: string): number | undefined {
  const n = Number(text.trim());
  return text.trim() !== '' && Number.isFinite(n) && n >= 0 ? n : undefined;
}

function priceLabel(row: ModelRow): string {
  const pair: string[] = [];
  if (row.priceInput !== undefined) pair.push(compactPrice(row.priceInput));
  if (row.priceOutput !== undefined) pair.push(compactPrice(row.priceOutput));
  if (pair.length) return pair.join(' / ');
  // 只填了缓存价也让人看出来，不假装是「标价」。
  const cache: string[] = [];
  if (row.priceCacheRead !== undefined) cache.push(compactPrice(row.priceCacheRead));
  if (row.priceCacheWrite !== undefined) cache.push(compactPrice(row.priceCacheWrite));
  return cache.length ? '缓存 ' + cache.join(' / ') : '标价';
}

/** 价格按钮的 title：完整四类 + 标价/覆盖价的算法。 */
function priceTitle(row: ModelRow): string {
  const show = (value: number | undefined) => (value === undefined ? '未填' : '$' + value);
  return (
    '单价（美元/百万 token）：输入 ' +
    show(row.priceInput) +
    ' / 输出 ' +
    show(row.priceOutput) +
    ' / 缓存读 ' +
    show(row.priceCacheRead) +
    ' / 缓存写 ' +
    show(row.priceCacheWrite) +
    '。标价 = 目录价（空着就继承）；自己填 = 覆盖价。'
  );
}

function formatPrice(value: number | undefined): string {
  return value === undefined ? '' : String(value);
}

/** 余量结果的时间，只到分钟。 */
function formatQuotaTime(at: number): string {
  return new Date(at).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

/**  F1：用户单价四格（照原型 PriceEditor）；空=继承目录标价，「恢复标价」清空四格。 */
function PriceEditor({ price, onChange }: { price: ModelRow; onChange: (patch: Partial<ModelRow>) => void }) {
  const fields: { key: 'priceInput' | 'priceCacheRead' | 'priceCacheWrite' | 'priceOutput'; label: string }[] = [
    { key: 'priceInput', label: '输入' },
    { key: 'priceCacheRead', label: '缓存读' },
    { key: 'priceCacheWrite', label: '缓存写' },
    { key: 'priceOutput', label: '输出' },
  ];
  const hasAny =
    price.priceInput !== undefined ||
    price.priceOutput !== undefined ||
    price.priceCacheRead !== undefined ||
    price.priceCacheWrite !== undefined;
  return (
    <div className="mb-1 ml-0.5 rounded-btn bg-content px-2.5 py-2" data-settings="price">
      <div className="grid grid-cols-4 gap-1.5">
        {fields.map((field) => (
          <label key={field.key} className="text-1 text-ink-3">
            {field.label}
            <input
              key={field.key + ':' + String(price[field.key])}
              className={cn(inputClass, 'mt-0.5 brand-type')}
              data-settings={'price-' + field.key}
              defaultValue={formatPrice(price[field.key])}
              placeholder="继承"
              inputMode="decimal"
              onBlur={(event) => onChange({ [field.key]: parsePrice(event.target.value) })}
            />
          </label>
        ))}
      </div>
      <div className="mt-1.5 flex items-start gap-2 text-1 leading-5 text-ink-3">
        <span>美元 / 百万 token。空着按目录标价算。以厂商账单为准。</span>
        {hasAny ? (
          <button
            type="button"
            className="ml-auto shrink-0 text-ink-2 hover:text-ink"
            data-settings="price-reset"
            onClick={() =>
              onChange({
                priceInput: undefined,
                priceOutput: undefined,
                priceCacheRead: undefined,
                priceCacheWrite: undefined,
              })
            }
          >
            恢复标价
          </button>
        ) : null}
      </div>
    </div>
  );
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
function StatusDot({ card }: { card: CardEnvelope<ConnectionSlots> }) {
  const slots = card.slots;
  const color = slots.broken && slots.saved
    ? 'var(--fail)'
    : !slots.saved
    ? 'var(--wait)'
    : slots.check === 'ok'
      ? 'var(--done)'
      : slots.check === 'fail'
        ? 'var(--fail)'
        : slots.check === 'checking'
          ? 'var(--run)'
          : 'var(--ink-3)';
  return (
    <span
      className={cn('block size-2 shrink-0 rounded-full', slots.check === 'checking' && 'animate-pulse')}
      style={{ backgroundColor: color }}
    />
  );
}

/** 连接卡信封 → 密钥和连接状态的一句话（「已保存」「连接可用」分开说）。 */
function keyStatus(card: CardEnvelope<ConnectionSlots> | null): { text: string; tone?: 'warn' | 'fail' | 'ok' } {
  const slots = card?.slots;
  if (!slots || !slots.saved) return { text: '还没有密钥。填上才能用这个提供商。', tone: 'warn' };
  const saved = `已保存在 Keyring（${formatLocalDateTime(slots.savedAt)}）`;
  if (slots.broken) return { text: `${saved} · 现在读不出来，暂时不能用`, tone: 'fail' };
  switch (slots.check) {
    case 'ok':
      return { text: `${saved} · 连接可用（${formatLocalDateTime(slots.checkedAt)} 验证）`, tone: 'ok' };
    case 'fail':
      return { text: `${saved} · 不可用：${modelErrorReason(slots.reason)}`, tone: 'fail' };
    case 'checking':
      return { text: `${saved} · 正在验证…` };
    default:
      return { text: `${saved} · 还没验证过` };
  }
}

/** 设置页的连接状态也从连接卡信封取（和首次使用同一份事实和派生）。 */
function cardOf(conn: ConnectionView | null, broken: boolean): CardEnvelope<ConnectionSlots> | null {
  if (!conn) return null;
  return connectionCardOf({
    id: conn.id,
    name: conn.name,
    provider: conn.id,
    key: conn.key,
    check: conn.check,
    broken,
  });
}

function KeyInput({ id, value, onChange, card }: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  card: CardEnvelope<ConnectionSlots> | null;
}) {
  const [shown, setShown] = useState(false);
  const status = keyStatus(card);
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
          placeholder={card?.slots.saved ? '已保存一把；留空就继续用它' : '输入 API 密钥'}
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

function parseTokens(text: string): number | undefined {
  const t = text.trim().toUpperCase();
  if (!t) return undefined;
  const m = /^(\d+(?:\.\d+)?)([KM]?)$/.exec(t);
  if (!m) return undefined;
  const base = parseFloat(m[1]);
  return Math.round(m[2] === 'M' ? base * 1000000 : m[2] === 'K' ? base * 1000 : base);
}

/* ───────────────────────── 模型目录 ───────────────────────── */

function ModelCatalog({ models, onChange, defaults, discover, required, connectionId, masked, onMask }: {
  models: ModelRow[];
  onChange: (models: ModelRow[]) => void;
  /** 跟着厂商走时列出来的模型（存下的厂商列表；没存过用内置目录）。 */
  defaults?: ModelRow[];
  /** 已保存的连接才能向厂商要列表；新连接没有 keyring 记录，不给这个函数。 */
  discover?: () => Promise<ModelRow[]>;
  required?: boolean;
  /** 这批模型属于哪个连接（行上的屏蔽按钮用；新建连接还没有 id 时不给）。 */
  connectionId?: string;
  /** 已屏蔽的规则整表（服务端保存的）。 */
  masked?: string[];
  /** 点行上的屏蔽按钮（带简单确认）。 */
  onMask?: (pattern: string) => void;
}) {
  const [candidates, setCandidates] = useState<ModelRow[] | null>(null);
  const [priceOpen, setPriceOpen] = useState<number | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState('');
  const [asking, setAsking] = useState(false);
  const blockedBy = (id: string) => (connectionId ? matchDisabled(masked ?? [], connectionId, id) : undefined);

  const ask = async () => {
    if (!discover) return;
    setAsking(true);
    try {
      const list = await discover();
      setCandidates(list);
      // 跟着厂商走（一个没勾）的打开还是一个没勾；勾过的打开是原来勾的。
      const configured = new Set(models.map((row) => row.id));
      setPicked(new Set(list.filter((row) => configured.has(row.id)).map((row) => row.id)));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setAsking(false);
    }
  };

  const needle = query.trim().toLowerCase();
  const visible = (candidates ?? []).filter(
    (row) => !needle || row.id.toLowerCase().includes(needle) || (row.name ?? '').toLowerCase().includes(needle),
  );
  const freshCount = (candidates ?? []).filter((row) => row.fresh).length;
  const followModels = models.length ? [] : defaults ?? [];

  const applyPicked = () => {
    if (!candidates) return;
    // 一个都不勾：跟着厂商的列表走（照 Magpie：Models 为空）。
    if (picked.size === 0) {
      onChange([]);
      setCandidates(null);
      setQuery('');
      return;
    }
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
            {models.length ? `只用勾的 ${models.length} 个` : followModels.length ? '跟着厂商的列表走' : '还没有模型'}
          </div>
        </div>
        <Btn
          kind="quiet"
          className="ml-auto"
          disabled={asking || !discover}
          title={discover ? undefined : '保存连接后，才能向厂商要真实列表'}
          onClick={() => void ask()}
        >
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
          <div className="mt-2 flex items-center gap-2 text-1 text-ink-3" data-settings="discover-count">
            <span>
              列出 {candidates.length} 个
              {freshCount ? <span className="font-medium text-ink">，{freshCount} 个是新的</span> : null}
              {picked.size ? `，勾了 ${picked.size} 个` : '，一个没勾'}
            </span>
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
                {row.fresh ? (
                  <span className="shrink-0 rounded-btn bg-done-soft px-1 text-1 font-medium text-done" data-settings="fresh">新</span>
                ) : null}
                {row.contextWindow ? <span className="ml-auto text-1 text-ink-3">{formatModelTokens(row.contextWindow)}</span> : null}
              </label>
            ))}
          </div>
          <div className="mt-2 flex items-center gap-2">
            <span className="text-1 leading-4 text-ink-3">一个都不勾：跟着厂商的列表走，新出的自动算进来。勾了就只用勾的。</span>
            <Btn kind="quiet" className="ml-auto" onClick={() => setCandidates(null)}>取消</Btn>
            <Btn kind="primary" disabled={Boolean(required) && picked.size === 0} onClick={applyPicked}>更新列表</Btn>
          </div>
        </div>
      ) : null}

      {models.length ? (
        <div className="mt-2 space-y-1.5">
          <div className="grid grid-cols-[1fr_1fr_64px_64px_112px_24px] gap-1.5 px-0.5 text-1 text-ink-3">
            <span>模型 ID</span>
            <span>显示名称</span>
            <span>上下文</span>
            <span>最大输出</span>
            <span>单价 $/M</span>
            <span />
          </div>
          <div className="px-0.5 text-1 leading-4 text-ink-3" data-settings="price-help">
            标价 = 目录价（空着就继承）；自己填 = 覆盖价。
          </div>
          {models.map((row, index) => (
            <div key={index}>
            <div className="grid grid-cols-[1fr_1fr_64px_64px_112px_24px] items-center gap-1.5">
              <input
                className={cn(inputClass, 'brand-type', blockedBy(row.id) && 'text-ink-3 line-through')}
                value={row.id}
                placeholder="model-id"
                onChange={(e) => setRow(index, { id: e.target.value })}
              />
              <input className={inputClass} value={row.name ?? ''} placeholder="可不填" onChange={(e) => setRow(index, { name: e.target.value || undefined })} />
              <input key={'cw' + index + ':' + row.contextWindow} className={inputClass} defaultValue={formatModelTokens(row.contextWindow)} placeholder="继承" onBlur={(e) => setRow(index, { contextWindow: parseTokens(e.target.value) })} />
              <input key={'mt' + index + ':' + row.maxTokens} className={inputClass} defaultValue={formatModelTokens(row.maxTokens)} placeholder="继承" onBlur={(e) => setRow(index, { maxTokens: parseTokens(e.target.value) })} />
              <button
                type="button"
                data-settings="model-price"
                title={priceTitle(row)}
                onClick={() => setPriceOpen(priceOpen === index ? null : index)}
                className={cn(
                  'h-7 overflow-hidden text-ellipsis whitespace-nowrap rounded-btn border border-line-2 px-1 text-2 transition-colors hover:bg-hover',
                  priceOpen === index ? 'text-ink' : 'text-ink-2',
                )}
              >
                {priceLabel(row)}
              </button>
              <div className="flex items-center">
                {onMask && connectionId && row.id.trim() ? (
                  blockedBy(row.id) ? (
                    <span
                      className="grid size-6 place-items-center text-fail"
                      title={'已屏蔽（规则「' + blockedBy(row.id) + '」）；在下面「已屏蔽的模型」里解除'}
                      data-settings="row-masked"
                    >
                      <Ban className="size-3.5" />
                    </span>
                  ) : (
                    <button
                      type="button"
                      aria-label={'屏蔽 ' + row.id}
                      title="屏蔽：立即不让用，不删除"
                      onClick={() => onMask(connectionId + '/' + row.id)}
                      className="grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-[var(--fail-soft)] hover:text-[var(--fail)]"
                      data-settings="mask"
                    >
                      <Ban className="size-3.5" />
                    </button>
                  )
                ) : (
                  <span className="size-6" />
                )}
                <button
                  type="button"
                  aria-label="删掉这个模型"
                  onClick={() => onChange(models.filter((_, i) => i !== index))}
                  className="grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink"
                >
                  <X className="size-3.5" />
                </button>
              </div>
            </div>
            {priceOpen === index ? (
              <PriceEditor
                price={row}
                onChange={(patch) => setRow(index, patch)}
              />
            ) : null}
            </div>
          ))}
        </div>
      ) : (
        <div
          className="mt-2 rounded-card border border-dashed border-line-2 px-3 py-3 text-center text-2 leading-5 text-ink-3"
          style={required ? { color: 'var(--wait)' } : undefined}
        >
          {followModels.length ? (
            <>
              <div>跟着厂商的列表走，厂商新出的自动算进来。只想用其中几个，就「获取可用模型」勾上。</div>
              <div className="mt-2 flex flex-wrap justify-center gap-1.5">
                {followModels.map((row) => {
                  const rule = blockedBy(row.id);
                  return (
                    <span
                      key={row.id}
                      className="inline-flex items-center gap-1 rounded-btn border border-line-2 py-0.5 pl-2 pr-0.5 brand-type text-1 text-ink-2"
                      data-settings="vendor-model"
                      title={row.name ?? row.id}
                    >
                      <span className={cn(rule && "line-through")}>{row.id}</span>
                      {onMask && connectionId ? (
                        rule ? (
                          <span
                            className="grid size-5 place-items-center text-fail"
                            title={"已屏蔽（规则「" + rule + "」）"}
                            data-settings="vendor-masked"
                          >
                            <Ban className="size-3" />
                          </span>
                        ) : (
                          <button
                            type="button"
                            aria-label={"屏蔽 " + row.id}
                            title="屏蔽：立即不让用，不删除"
                            onClick={() => onMask(connectionId + "/" + row.id)}
                            className="grid size-5 place-items-center rounded-btn text-ink-3 hover:bg-[var(--fail-soft)] hover:text-[var(--fail)]"
                            data-settings="vendor-mask"
                          >
                            <Ban className="size-3" />
                          </button>
                        )
                      ) : null}
                    </span>
                  );
                })}
              </div>
              {!discover ? (
                <div className="mt-2 text-1">保存连接后，才能向厂商要真实列表；现在列的是内置目录。</div>
              ) : null}
            </>
          ) : required ? (
            '自定义接口至少要写一个模型。'
          ) : (
            '模型选择器里不会列出模型；目录外的 ID 仍可直接发送。'
          )}
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

function ConnectionEditor({ conn, view, onDone, onView, onMask }: {
  conn: ConnectionView;
  view: ModelSettingsView;
  onDone: () => void;
  onView: (view: ModelSettingsView) => void;
  /** 行上的屏蔽按钮：打开确认框，确认后立即保存（不经过这张卡片的保存）。 */
  onMask: (pattern: string) => void;
}) {
  const [key, setKey] = useState('');
  const [endpoint, setEndpoint] = useState(conn.endpoint);
  const [models, setModels] = useState<ModelRow[]>(conn.models);
  const [quota, setQuota] = useState<ServiceQuota | null>(() => cachedQuota(conn.id)?.quota ?? null);
  const [quotaAt, setQuotaAt] = useState<number | null>(() => cachedQuota(conn.id)?.at ?? null);
  const [quotaError, setQuotaError] = useState<string | null>(null);
  const [askingQuota, setAskingQuota] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirm, setConfirm] = useState<'revoke' | 'remove' | null>(null);
  const catalog = view.catalog.find((item) => item.id === conn.id);

  const askQuota = useCallback(async (force = false) => {
    setAskingQuota(true);
    setQuotaError(null);
    try {
      setQuota(await loadQuota(conn.id, { force }));
      setQuotaAt(cachedQuota(conn.id)?.at ?? Date.now());
    } catch (error) {
      setQuotaError((error as Error).message);
    } finally {
      setAskingQuota(false);
    }
  }, [conn.id]);

  // 打开编辑器自动查一次；缓存/最小间隔内再打开直接用上次结果，不重复打厂商接口。
  // 没保存密钥时查不了，别自动打出一个失败提示。
  useEffect(() => {
    if (!conn.key.saved) return;
    void askQuota(false);
  }, [askQuota, conn.key.saved]);

  const save = async () => {
    setSaving(true);
    try {
      const withKey = Boolean(key.trim());
      onView(
        await updateConnection(conn.id, {
          endpoint,
          models,
          key,
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
      <KeyInput id={'key-' + conn.id} value={key} onChange={setKey} card={cardOf(conn, false)} />
      <Collapsible title="自定义设置" defaultOpen={conn.source === 'custom'}>
        <Field label="API 地址" hint={`协议：${PROTOCOL_LABEL[conn.protocol]}`}>
          <input
            className={inputClass}
            data-settings="endpoint"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder={conn.defaultEndpoint}
          />
        </Field>
        <ModelCatalog
          models={models}
          onChange={setModels}
          // 跟着厂商走时的小标签/选择器用存下的厂商列表；没存过用内置目录。
          defaults={view.listed?.[conn.id]?.models ?? catalog?.defaultModels}
          required={conn.source === 'custom'}
          discover={async () => {
            const list = await discoverModels(conn.id);
            // 获取后服务端存下了厂商列表：把设置视图刷新一次，小标签/选择器立刻用它。
            onView(await getModelSettings());
            return list;
          }}
          connectionId={conn.id}
          masked={view.disabledModels}
          onMask={onMask}
        />
      </Collapsible>
      <div className="mt-4 border-t border-line pt-3" data-settings="quota">
        <div className="flex items-center gap-2">
          <span className="text-2 text-ink-2">余额与套餐余量</span>
          <Btn kind="quiet" data-settings="quota-refresh" disabled={askingQuota} onClick={() => void askQuota(true)}>
            {askingQuota ? (
              <>
                <Loader2 data-settings="quota-spinner" className="size-3.5 animate-spin" aria-hidden />
                正在查询…
              </>
            ) : (
              '刷新'
            )}
          </Btn>
          {quotaAt && !askingQuota ? (
            <span className="ml-auto text-1 text-ink-3">更新于 {formatQuotaTime(quotaAt)}</span>
          ) : null}
        </div>
        {quotaError ? (
          <div className="mt-2 text-2 text-fail" data-settings="quota-error">
            查询失败：{quotaError}
            <button
              type="button"
              className="ml-2 underline hover:text-ink"
              onClick={() => void askQuota(true)}
            >
              重试
            </button>
          </div>
        ) : quota ? (
          <div className="mt-2 space-y-0.5 text-2 text-ink-2" data-settings="quota-value">
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
        ) : !conn.key.saved ? (
          <div className="mt-2 text-2 text-ink-3">保存密钥后自动查询余额/余量。</div>
        ) : (
          <div className="mt-2 text-2 text-ink-3">正在查询…</div>
        )}
      </div>
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
      <KeyInput id="add-catalog-key" value={key} onChange={setKey} card={null} />
      <Collapsible title="自定义设置">
        <Field label="API 地址" hint={`协议：${PROTOCOL_LABEL[provider.protocol]}`}>
          <input className={inputClass} value={endpoint} onChange={(e) => setEndpoint(e.target.value)} placeholder={provider.defaultEndpoint} />
        </Field>
        <ModelCatalog
          models={models}
          onChange={setModels}
          defaults={provider.defaultModels}
          // 新连接还没有 id/Keyring 记录：拿到列表的只可能是本地目录，所以不提供「获取」，
          // 保存后在「编辑」里向厂商要真实列表。
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

/* ───────────────────────── 联网搜索（D73） ───────────────────────── */

/**
 * Exa 的工具凭证：只写不读，存进 Keyring；「已保存」和「能用」分开说。
 *
 * - 没存：Kind 的能力目录里没有 `web`，问它要联网，它让你来这里填；
 * - 保存以后自动验证一次；验证跑在服务端，页面只拿结果；
 * - 不读环境变量；页面、日志和对话里都不出现 key 正文。
 */
function WebSearchSection({ broken, onChanged }: { broken: boolean; onChanged: () => void }) {
  const [view, setView] = useState<ToolKeyView | null>(null);
  const [draft, setDraft] = useState('');
  const [editing, setEditing] = useState(false);
  const [shown, setShown] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (broken) return;
    void getExaKey().then(setView).catch(() => setView(null));
  }, [broken]);

  const saved = view?.key.saved ?? false;
  const checking = view?.check.kind === 'checking';
  const savedAt = view && view.key.saved ? formatLocalDateTime(view.key.savedAt) : '';

  const verify = async () => {
    if (!view) return;
    setView({ ...view, check: { kind: 'checking' } });
    try {
      setView(await verifyExaKey());
      onChanged();
    } catch (error) {
      toast.error((error as Error).message);
      setView(await getExaKey().catch(() => null));
    }
  };

  const save = async () => {
    setBusy(true);
    try {
      const next = await saveExaKey(draft);
      setDraft('');
      setEditing(false);
      onChanged();
      toast.success(saved ? 'key 换好了，进了 Keyring；正在验证' : 'key 进了 Keyring；Kind 下一轮就能联网，正在验证');
      setView({ ...next, check: { kind: 'checking' } });
      setView(await verifyExaKey());
      onChanged();
    } catch (error) {
      toast.error((error as Error).message);
      setView(await getExaKey().catch(() => null));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    try {
      setView(await removeExaKey());
      setConfirmDelete(false);
      onChanged();
      toast('Exa 的 key 删了；Kind 从下一轮起不能联网');
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  const status: { text: string; tone?: 'warn' | 'fail' | 'ok' } = broken
    ? { text: 'Keyring 现在读不出来，暂时不能联网', tone: 'fail' }
    : !view
      ? { text: '正在读取…' }
      : !saved
        ? { text: '还没有 key。Kind 现在不能搜网页、读链接。', tone: 'warn' }
        : checking
          ? { text: '已保存在 Keyring（' + savedAt + '）· 正在验证…' }
          : view.check.kind === 'ok'
            ? { text: '已保存在 Keyring（' + savedAt + '）· 能用（' + formatLocalDateTime(view.check.at) + ' 验证）', tone: 'ok' }
            : view.check.kind === 'fail'
              ? { text: '已保存在 Keyring（' + savedAt + '）· 不能用：' + view.check.reason, tone: 'fail' }
              : { text: '已保存在 Keyring（' + savedAt + '）· 还没验证过' };

  const dot = broken
    ? 'var(--fail)'
    : !saved
      ? 'var(--wait)'
      : checking
        ? 'var(--run)'
        : view?.check.kind === 'ok'
          ? 'var(--done)'
          : view?.check.kind === 'fail'
            ? 'var(--fail)'
            : 'var(--ink-3)';

  return (
    <section className="mt-10" data-settings="web-search">
      <h2 className="text-3 font-medium text-ink">联网搜索</h2>
      <p className="mt-1 text-2 leading-5 text-ink-2">
        填上你自己的 Exa key，Kind 才能搜网页、读链接。网页的内容当作资料，不当作指令；读网页不访问本机和内网地址。
      </p>

      <div className="mt-3 rounded-card border border-line">
        <div className="flex items-center gap-3 px-4 py-3">
          <span
            className={cn('block size-2 shrink-0 rounded-full', checking && 'animate-pulse')}
            style={{ backgroundColor: dot }}
          />
          <div className="min-w-0">
            <div className="text-3 text-ink">Exa</div>
            <div
              className="mt-0.5 truncate text-1"
              data-settings="web-search-status"
              style={status.tone ? { color: `var(--${status.tone === 'warn' ? 'wait' : status.tone === 'ok' ? 'done' : 'fail'})` } : undefined}
            >
              <span className={status.tone ? '' : 'text-ink-3'}>{status.text}</span>
            </div>
          </div>
          <div className="ml-auto flex shrink-0 items-center gap-1">
            {saved && !broken ? (
              <Btn kind="quiet" disabled={checking} onClick={() => void verify()}>
                {checking ? '正在验证…' : '验证'}
              </Btn>
            ) : null}
            {saved && !editing && !broken ? (
              <Btn kind="danger" onClick={() => setConfirmDelete(true)}>
                删除
              </Btn>
            ) : null}
            {(saved && !broken) || editing ? (
              <Btn
                kind="plain"
                onClick={() => {
                  setEditing((on) => !on);
                  setDraft('');
                }}
              >
                {editing ? '收起' : '换一把'}
              </Btn>
            ) : null}
          </div>
        </div>

        {(!saved || editing) && !broken ? (
          <div className="border-t border-line px-4 py-3" data-settings="exa-key">
            <div className="flex items-center gap-2">
              <div className="relative flex-1">
                <input
                  type={shown ? 'text' : 'password'}
                  autoComplete="new-password"
                  spellCheck={false}
                  value={draft}
                  onChange={(event) => {
                    setDraft(event.target.value);
                    setShown(false);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' && draft.trim() && !busy) void save();
                  }}
                  placeholder={saved ? '新的 key；保存后替换掉旧的' : '输入 Exa 的 API key'}
                  className={cn(inputClass, draft && 'pr-9')}
                  data-settings="exa-key-input"
                />
                {draft ? (
                  <button
                    type="button"
                    onClick={() => setShown((on) => !on)}
                    aria-label={shown ? '隐藏 key' : '显示 key'}
                    className="absolute right-1.5 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink"
                  >
                    {shown ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
                  </button>
                ) : null}
              </div>
              <Btn kind="primary" disabled={!draft.trim() || busy} onClick={() => void save()}>
                {busy ? '正在保存…' : '保存'}
              </Btn>
            </div>
            <div className="mt-1.5 text-1 leading-4 text-ink-3">
              在 <span className="brand-type">dashboard.exa.ai</span> 的 API Keys 里拿。保存以后页面、日志和对话里都不再出现；不从环境变量读。
            </div>
          </div>
        ) : null}
      </div>

      <Dialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>删除 Exa 的 key？</DialogTitle>
            <DialogDescription>
              立刻生效：Kind 从下一轮起不能搜网页、读链接，正在搜的那一次会失败。删除会留下记录，key 本身不留。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Btn kind="plain" onClick={() => setConfirmDelete(false)}>
              取消
            </Btn>
            <Btn kind="danger" className="border border-[var(--fail)]/40" onClick={() => void remove()}>
              删除
            </Btn>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

/* ───────────────────────── Keyring 一节 ───────────────────────── */

/** ：显示币种（settings.json 的 currency；价格和花费一律按美元存，显示时换算）。 */
function CurrencySection() {
  const money = useSettingsState((state) => state.money);
  const setCurrency = useSettingsState((state) => state.setCurrency);
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
        上面存的模型密钥和联网搜索的 key 都在这里，加密保存。Kind 和模型从不拿到密钥本身：每次请求由内核向 Keyring 申请一次，只在发出的那一刻展开。
      </p>

      <div className="mt-3 divide-y divide-line rounded-card border border-line">
        <div className="px-4 py-3">
          <div className="flex items-center gap-2">
            <div className="min-w-0">
              <div className="text-3 text-ink">加密钥匙</div>
              <div className="mt-0.5 text-1 text-ink-3">放在哪由你定。现在是一个文件，{formatLocalDateTime(view.keyFile.since)} 起放在这里。</div>
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
                {view.issuedToday ? `今天签发 ${view.issuedToday} 次，最近一次 ${formatLocalDateTime(view.issues[0]?.at)}。只记谁用、做什么，不记密钥。` : '今天还没有签发过。'}
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
                    <td className="brand-type py-1">{formatLocalDateTime(row.at)}</td>
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

/* ───────────────────────── 屏蔽（D67） ───────────────────────── */

/**
 * 「已屏蔽的模型」：照原型 MaskSection。
 *
 * - 上面一行写规则：从已知模型里挑（datalist），也能写通配；
 * - 每条规则下面写着「现在匹配 n 个：…」；一个都不匹配也留着（出现了照样拒）；
 * - 先过确认框（MaskDialog）再保存；「解除屏蔽」立即生效；
 * - 固定说明「读不出来时沿用上一次读到的；从没读到过就都不发。」；
 *   从没读到过时动态写「settings.json 读不出来」（不写路径/服务端原因）。
 *
 * 匹配数只读服务给的 `disabledHits`（与规则等长同序），这里不自己匹配。
 */
function MaskSection({ view, onView, onMask }: {
  view: ModelSettingsView;
  onView: (view: ModelSettingsView) => void;
  onMask: (pattern: string) => void;
}) {
  const refreshMasks = useSettingsState((state) => state.refreshMasks);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const duplicate = view.disabledModels.some(
    (pattern) => pattern.toLowerCase() === draft.trim().toLowerCase(),
  );
  // 提示从已知模型里选：和选择器同一个口径（勾了的；没勾就是厂商列表/内置目录）。
  const known = view.connections.flatMap((conn) =>
    (conn.models.length
      ? conn.models
      : conn.availableModels ?? view.catalog.find((item) => item.id === conn.id)?.defaultModels ?? []
    ).map((row) => conn.id + '/' + row.id),
  );

  const submit = () => {
    const pattern = draft.trim();
    if (!pattern || duplicate) return;
    // 先过确认框；真正保存由确认框决定。
    onMask(pattern);
    setDraft('');
  };

  const unmask = async (pattern: string) => {
    setBusy(pattern);
    try {
      onView(await setDisabledModels(view.disabledModels.filter((item) => item !== pattern)));
      await refreshMasks();
      toast('已解除屏蔽「' + pattern + '」');
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="mt-10" data-settings="masked">
      <h2 className="text-3 font-medium text-ink">已屏蔽的模型</h2>
      <p className="mt-1.5 text-2 leading-5 text-ink-2">
        屏蔽的模型不让用：每次请求前查一遍，正在跑的一轮也会停下。模型选择器里照样列出，标「已屏蔽」，选不了。
        屏蔽不是删除，历史和用量照常显示。
      </p>
      <p className="mt-1.5 text-1 leading-5 text-ink-3" data-settings="mask-note">
        读不出来时沿用上一次读到的；从没读到过就都不发。
      </p>
      <div className="mt-3 rounded-card border border-line" data-settings="mask-list">
        <div className="flex items-center gap-2 px-4 py-3">
          <input
            list="mask-models"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') submit();
            }}
            placeholder="挑一个模型，或者写通配，比如 *preview*"
            className={cn(inputClass, 'brand-type')}
            data-settings="mask-input"
          />
          <datalist id="mask-models">
            {known.map((id) => (
              <option key={id} value={id} />
            ))}
          </datalist>
          <Btn
            kind="plain"
            disabled={!draft.trim() || duplicate}
            title={duplicate ? '已经有这一条了' : undefined}
            onClick={submit}
          >
            屏蔽…
          </Btn>
        </div>
        {view.disabledUnknown ? (
          <div className="border-t border-line px-4 py-3 text-2 text-fail" data-settings="mask-unknown">
            settings.json 读不出来
          </div>
        ) : null}
        {view.disabledModels.length === 0 ? (
          view.disabledUnknown ? null : (
            <div className="border-t border-line px-4 py-3 text-2 text-ink-3">还没有屏蔽任何模型。</div>
          )
        ) : (
          view.disabledModels.map((pattern, index) => {
            const hits = view.disabledHits[index] ?? [];
            return (
              <div
                key={pattern}
                className="flex items-center gap-3 border-t border-line px-4 py-3"
                data-settings="mask-row"
              >
                <div className="min-w-0">
                  <div className="brand-type truncate text-3 text-ink">{pattern}</div>
                  <div className="mt-0.5 truncate text-1 text-ink-3">
                    {hits.length
                      ? '现在匹配 ' + hits.length + ' 个：' + hits.slice(0, 4).join('、') + (hits.length > 4 ? '…' : '')
                      : '现在不匹配任何模型；出现了也会被拒'}
                  </div>
                </div>
                <Btn
                  kind="quiet"
                  className="ml-auto"
                  disabled={busy === pattern}
                  onClick={() => void unmask(pattern)}
                >
                  解除屏蔽
                </Btn>
              </div>
            );
          })
        )}
      </div>
    </section>
  );
}

/**
 * 屏蔽前先列出谁在用（照原型 MaskDialog）：命中的模型和使用者都由服务的
 * disabled-models-check 给，页面不自己匹配。
 */
function MaskDialog({ pattern, busy, onCancel, onConfirm }: {
  pattern: string;
  busy: boolean;
  onCancel: () => void;
  onConfirm: (wasRunning: boolean) => void;
}) {
  const [check, setCheck] = useState<{ hits: string[]; users: DisabledModelUser[] } | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let live = true;
    setCheck(null);
    setFailed(false);
    void checkDisabledModels(pattern)
      .then((next) => {
        if (live) setCheck(next);
      })
      .catch(() => {
        if (live) setFailed(true);
      });
    return () => {
      live = false;
    };
  }, [pattern]);
  const users = check?.users ?? [];
  const running = users.some((item) => item.kind === 'kind-chat' && item.running);
  const inUse = users.some((item) => item.kind === 'kind-chat');
  return (
    <Dialog open onOpenChange={(open) => (open ? null : onCancel())}>
      <DialogContent data-settings="mask-confirm">
        <DialogHeader>
          <DialogTitle>屏蔽「{pattern}」？</DialogTitle>
          <DialogDescription>
            {check
              ? check.hits.length
                ? '现在匹配 ' + check.hits.length + ' 个：' + check.hits.join('、') + '。'
                : '现在不匹配任何模型；以后出现了也会被拒。'
              : failed
                ? '读不到谁在用；先别动这条规则。'
                : '正在查谁在用…'}
            {' '}确认后立即不让用，不用再点保存。
          </DialogDescription>
        </DialogHeader>
        <div className="text-2">
          <div className="mb-1.5 text-ink-2">谁在用</div>
          {failed ? (
            <div className="text-ink-3">读不到谁在用；关掉重试。</div>
          ) : !check ? (
            <div className="text-ink-3">正在查…</div>
          ) : users.length ? (
            <div className="rounded-card border border-line">
              {users.map((item, index) => (
                <div
                  key={item.kind}
                  className={cn('flex gap-3 px-3 py-2', index > 0 && 'border-t border-line')}
                  data-settings="mask-user"
                >
                  <span className="w-20 shrink-0 text-ink">{item.kind === 'kind-chat' ? 'Kind 对话' : '新会话'}</span>
                  <span style={{ color: 'var(--' + (item.kind === 'kind-chat' && item.running ? 'fail' : 'wait') + ')' }}>
                    {item.kind === 'kind-chat'
                      ? item.running
                        ? '这一轮还在跑，会立即停下'
                        : '下一轮发不出去'
                      : '会用上一次选的'}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <div className="text-ink-3">现在没有地方在用。</div>
          )}
          {inUse ? (
            <p className="mt-2 text-1 leading-4 text-ink-3">
              {running
                ? '停下的那一轮已经做的留着；接着用哪个模型、从哪接，由你在对话里定。AIOS 不会自己换模型。'
                : '接着用哪个模型由你在对话里选。AIOS 不会自己换模型。'}
            </p>
          ) : null}
        </div>
        <DialogFooter>
          <Btn kind="plain" onClick={onCancel}>取消</Btn>
          <Btn
            kind="primary"
            className="!bg-[var(--fail)] !text-white"
            disabled={busy || !check}
            onClick={() => onConfirm(running)}
          >
            {busy ? '正在保存…' : '屏蔽'}
          </Btn>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/* ───────────────────────── 整组 ───────────────────────── */

export function ModelSettings() {
  useEffect(() => {
    // 直接打开设置页（不经 Kind 页）时也要拿币种/汇率。
    void useSettingsState.getState().refreshMoney();
  }, []);
  const [view, setView] = useState<ModelSettingsView | null>(null);
  /** 展开的是哪张卡片：连接 id、'add'，或者都不展开。一次只展开一张。 */
  const [open, setOpen] = useState<string | null>(null);
  /** 正在确认屏蔽的规则；null = 没有对话框。 */
  const [masking, setMasking] = useState<string | null>(null);
  const [maskBusy, setMaskBusy] = useState(false);
  const refreshMasks = useSettingsState((state) => state.refreshMasks);

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

  const mask = async (wasRunning: boolean) => {
    if (!masking) return;
    const pattern = masking;
    setMaskBusy(true);
    try {
      setView(await setDisabledModels(view ? [...view.disabledModels, pattern] : [pattern]));
      // 页面不用刷新：选择器上的「已屏蔽」立刻跟上。
      await refreshMasks();
      toast('已屏蔽「' + pattern + '」' + (wasRunning ? '；Kind 对话里在跑的一轮已经停下' : ''));
      setMasking(null);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setMaskBusy(false);
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
              {rows.map((conn, index) => {
                const card = cardOf(conn, broken);
                return (
                <div key={conn.id} className={cn(index > 0 && 'border-t border-line')}>
                  <div className="flex items-center gap-3 px-4 py-3">
                    {card ? <StatusDot card={card} /> : null}
                    <div className="min-w-0">
                      <div className="text-3 text-ink">{conn.name}</div>
                      <div className="mt-0.5 truncate text-1 text-ink-3" data-card-state={card?.state}>
                        {keyStatus(card).text}
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
                    <ConnectionEditor
                      key={conn.id}
                      conn={conn}
                      view={view}
                      onDone={() => setOpen(null)}
                      onView={setView}
                      onMask={(pattern) => setMasking(pattern)}
                    />
                  ) : null}
                </div>
                );
              })}
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

      <MaskSection view={view} onView={setView} onMask={(pattern) => setMasking(pattern)} />

      <WebSearchSection broken={broken} onChanged={() => void load()} />

      <KeyringSection view={view} onView={setView} />

      {masking ? (
        <MaskDialog
          pattern={masking}
          busy={maskBusy}
          onCancel={() => setMasking(null)}
          onConfirm={(wasRunning) => void mask(wasRunning)}
        />
      ) : null}
    </div>
  );
}
