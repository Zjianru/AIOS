/**
 * 监控页小件：大数字、环、堆叠条、排行行、缺数据斜线。颜色只用主题 token，
 * 与消耗/在跑/诊断三块共用。
 */

import { useEffect, useState } from 'react';
import { cn } from '@/lib/utils';

export const btn =
  'rounded-btn px-2 py-0.5 text-1 text-ink-2 transition-colors hover:bg-hover hover:text-ink';

export function useTick(): number {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(() => setTick((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return tick;
}

export function fmtClock(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  const pad = (value: number) => String(value).padStart(2, '0');
  const hours = Math.floor(total / 3600);
  return (
    (hours ? hours + ':' + pad(Math.floor((total % 3600) / 60)) : pad(Math.floor(total / 60))) +
    ':' +
    pad(total % 60)
  );
}

export function fmtSpan(ms: number | null | undefined): string {
  if (ms == null || ms < 0) return '没采到';
  return fmtClock(ms / 1000);
}

export function fmtTime(ms: number | null | undefined): string {
  if (!ms) return '没采到';
  return new Date(ms).toLocaleString('zh-CN', {
    month: 'numeric',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  });
}

/** 缺数据：画斜线写「没采到」，不当 0。 */
export function Missing({ what = '没采到' }: { what?: string }) {
  return (
    <span
      className="inline-flex items-center gap-1 text-ink-3"
      data-kind="missing"
      title={what}
    >
      <span
        aria-hidden="true"
        className="inline-block h-3 w-5 rounded-[2px]"
        style={{ background: 'repeating-linear-gradient(135deg, var(--line-2) 0 1.5px, transparent 1.5px 5px)' }}
      />
      <span>{what}</span>
    </span>
  );
}

export function Card({
  title,
  note,
  right,
  className,
  children,
  ...rest
}: {
  title?: string;
  note?: string;
  right?: React.ReactNode;
  className?: string;
  children: React.ReactNode;
} & React.HTMLAttributes<HTMLDivElement>) {
  return (
    <section
      className={cn('min-w-0 rounded-card border border-line bg-raised/40 p-4', className)}
      {...rest}
    >
      {title ? (
        <div className="mb-3 flex items-baseline gap-2">
          <h2 className="text-3 font-semibold text-ink">{title}</h2>
          {note ? <span className="truncate text-1 text-ink-3">{note}</span> : null}
          <span className="flex-1" />
          {right}
        </div>
      ) : null}
      {children}
    </section>
  );
}

export function Kpi({
  label,
  value,
  sub,
  tone,
}: {
  label: string;
  value: React.ReactNode;
  sub?: React.ReactNode;
  tone?: 'run' | 'wait' | 'fail';
}) {
  return (
    <div className="min-w-0 rounded-card border border-line bg-raised/40 px-4 py-3" data-kind="kpi">
      <div className="truncate text-1 text-ink-3">{label}</div>
      <div
        className={cn(
          'brand-type mt-1 truncate text-6 leading-7',
          tone === 'run' ? 'text-run' : tone === 'wait' ? 'text-wait' : tone === 'fail' ? 'text-fail' : 'text-ink',
        )}
      >
        {value}
      </div>
      {sub ? <div className="mt-1 truncate text-1 text-ink-3">{sub}</div> : null}
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: readonly (readonly [T, React.ReactNode])[];
  onChange: (value: T) => void;
}) {
  return (
    <div className="inline-flex rounded-btn border border-line p-0.5">
      {options.map(([key, label]) => (
        <button
          key={key}
          type="button"
          className={cn(
            'rounded-[5px] px-2 py-0.5 text-1 transition-colors',
            value === key ? 'bg-active text-ink' : 'text-ink-3 hover:text-ink-2',
          )}
          onClick={() => onChange(key)}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

export function RankRow({
  name,
  count,
  max,
  color,
  right,
  mono,
  onClick,
}: {
  name: React.ReactNode;
  count: number;
  max: number;
  color: string;
  right?: React.ReactNode;
  mono?: boolean;
  onClick?: () => void;
}) {
  const content = (
    <>
      <span className="flex min-w-0 items-baseline gap-2">
        <span className={cn('min-w-0 truncate text-2 text-ink', mono && 'font-mono text-1')}>{name}</span>
        <span className="brand-type shrink-0 text-1 text-ink-3">×{count}</span>
        <span className="flex-1" />
        {right ? <span className="shrink-0 text-1 text-ink-3">{right}</span> : null}
      </span>
      <span className="mt-1 block h-1 overflow-hidden rounded-full bg-line">
        <span
          className="block h-full rounded-full"
          style={{ width: Math.min(100, Math.max(2, (count / Math.max(1, max)) * 100)) + '%', background: color }}
        />
      </span>
    </>
  );
  return onClick ? (
    <button type="button" data-kind="rank-row" className="block w-full py-1.5 text-left" onClick={onClick}>
      {content}
    </button>
  ) : (
    <div data-kind="rank-row" className="block w-full py-1.5 text-left">
      {content}
    </div>
  );
}

export function Donut({
  title,
  parts,
  missing,
  center,
}: {
  title: string;
  parts: { key: string; name: string; value: number; color: string }[];
  missing?: React.ReactNode;
  center?: React.ReactNode;
}) {
  const total = parts.reduce((sum, part) => sum + part.value, 0);
  if (!total) {
    return (
      <Card title={title} data-kind="donut">
        <div className="grid h-[150px] place-items-center">
          {missing ?? <Missing what="没采到" />}
        </div>
      </Card>
    );
  }
  let offset = 0;
  const stops: string[] = [];
  for (const part of parts) {
    if (part.value <= 0) continue;
    const end = offset + (part.value / total) * 100;
    stops.push(part.color + ' ' + offset + '% ' + end + '%');
    offset = end;
  }
  return (
    <Card title={title} data-kind="donut">
      <div className="flex items-center gap-5">
        <div
          className="relative grid size-[118px] shrink-0 place-items-center rounded-full"
          style={{ background: 'conic-gradient(' + stops.join(',') + ')' }}
        >
          <div className="grid size-[78px] place-items-center rounded-full bg-content text-center">
            <div>
              <div className="brand-type text-4 text-ink">{center}</div>
              <div className="text-1 text-ink-3">合计</div>
            </div>
          </div>
        </div>
        <div className="min-w-0 flex-1 space-y-1">
          {parts.map((part) => (
            <div key={part.key} className="flex items-center gap-1.5 text-1 text-ink-2">
              <i className="size-2 shrink-0 rounded-[2px]" style={{ background: part.color }} />
              <span className="min-w-0 truncate">{part.name}</span>
              <span className="flex-1" />
              <span className="brand-type shrink-0">{Math.round((part.value / total) * 100)}%</span>
            </div>
          ))}
        </div>
      </div>
    </Card>
  );
}

export function StackBar({
  parts,
  used,
  max,
  reserve,
  height = 8,
}: {
  parts: { key: string; name: string; value: number; color: string }[];
  used: number;
  max: number;
  reserve?: number;
  height?: number;
}) {
  if (!max) return <Missing what="窗口没采到" />;
  return (
    <div>
      <div className="flex overflow-hidden rounded-full bg-line" style={{ height }}>
        {parts.map((part) => (
          <span
            key={part.key}
            title={part.name}
            style={{ width: (part.value / max) * 100 + '%', background: part.color }}
          />
        ))}
        {reserve && reserve > 0 ? (
          <span
            title="压缩余量"
            className="flex-1"
            style={{
              background: 'repeating-linear-gradient(135deg, var(--line-2) 0 1.5px, transparent 1.5px 5px)',
            }}
          />
        ) : null}
      </div>
      <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-1 text-ink-3">
        {parts.map((part) => (
          <span key={part.key} className="flex items-center gap-1">
            <i className="size-2 rounded-[2px]" style={{ background: part.color }} />
            {part.name}
            <span className="text-ink-3">约</span>
          </span>
        ))}
        <span className="ml-auto brand-type">
          实报 {Math.round(used).toLocaleString()} · 窗口 {max.toLocaleString()}
        </span>
      </div>
    </div>
  );
}
