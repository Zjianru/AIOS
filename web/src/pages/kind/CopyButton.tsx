/**
 * T013.6：复制按钮（照原型 a5d6f58 的 CopyButton）——回答、你的消息、代码块共用。
 * T013 验收 F1：写入成功才显示对勾；被拒就走隐藏 textarea + execCommand 兜底，还失败就显示「复制失败」，
 * 不装作成功（真实浏览器没给剪贴板写权限时，原来的写法会把失败吞掉还打对勾）。
 */

import { useEffect, useRef, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { cn } from '@/lib/utils';

type CopyState = 'idle' | 'copied' | 'failed';

/** 写入剪贴板：先走异步 API；被拒或没有这个 API 时走 execCommand 兜底。 */
async function writeClipboard(value: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(value);
      return true;
    }
  } catch {
    // 没授权/被拒：继续走兜底。
  }
  return legacyCopy(value);
}

function legacyCopy(value: string): boolean {
  const area = document.createElement('textarea');
  area.value = value;
  area.setAttribute('readonly', '');
  area.style.position = 'fixed';
  area.style.top = '0';
  area.style.left = '-9999px';
  document.body.appendChild(area);
  try {
    area.focus();
    area.select();
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    document.body.removeChild(area);
  }
}

interface CopyButtonProps {
  /** 要复制的文字；给函数就是点的时候再取（代码块从 DOM 里取）。 */
  text: string | (() => string);
  /** 显示文字（终端卡片上写「复制」）；不给就只有图标。 */
  label?: string;
  /** data-kind（e2e 找：消息复制 copy、代码块 code-copy）。 */
  kind?: string;
  className?: string;
}

export function CopyButton({ text, label, kind = 'copy', className }: CopyButtonProps) {
  const [state, setState] = useState<CopyState>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  const onClick = () => {
    if (timer.current) clearTimeout(timer.current);
    const value = typeof text === 'function' ? text() : text;
    // 空内容没有可复制的：直接算失败，也不去写空串。
    void (async () => {
      const ok = value !== '' && (await writeClipboard(value));
      setState(ok ? 'copied' : 'failed');
      timer.current = setTimeout(() => setState('idle'), 1500);
    })();
  };
  const labelText = state === 'copied' ? '已复制' : state === 'failed' ? '复制失败' : '复制';
  const Icon = state === 'copied' ? Check : Copy;
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={labelText}
      title={labelText}
      data-kind={kind}
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded-btn text-ink-3 transition-colors hover:bg-hover hover:text-ink',
        label ? 'h-6 px-1.5 text-1' : 'size-6 justify-center',
        className,
      )}
    >
      <Icon
        className={cn(
          'size-3.5',
          state === 'copied' && 'text-done',
          state === 'failed' && 'text-fail',
        )}
      />
      {label ? <span>{state === 'idle' ? label : labelText}</span> : null}
    </button>
  );
}
