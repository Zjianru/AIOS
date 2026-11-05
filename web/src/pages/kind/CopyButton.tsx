/**
 * T013.6：复制按钮（照原型 a5d6f58 的 CopyButton）——回答、你的消息、代码块共用。
 * 点了换成对勾 1.5 秒；复制的永远是原文。
 */

import { useEffect, useRef, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { cn } from '@/lib/utils';

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
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  const onClick = () => {
    const value = typeof text === 'function' ? text() : text;
    void navigator.clipboard?.writeText(value).catch(() => undefined);
    setCopied(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  };
  const Icon = copied ? Check : Copy;
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={copied ? '已复制' : '复制'}
      title={copied ? '已复制' : '复制'}
      data-kind={kind}
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded-btn text-ink-3 transition-colors hover:bg-hover hover:text-ink',
        label ? 'h-6 px-1.5 text-1' : 'size-6 justify-center',
        className,
      )}
    >
      <Icon className={cn('size-3.5', copied && 'text-done')} />
      {label ? <span>{copied ? '已复制' : label}</span> : null}
    </button>
  );
}
