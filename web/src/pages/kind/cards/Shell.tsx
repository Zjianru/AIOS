/**
 * 卡片外壳：代码块的底色、0.5px 边。
 * 工具卡和通用卡都用它；单独放这里，通用卡不再向工具卡借。
 */

import type { ReactNode } from 'react';
import { cn } from '@/lib/utils';

export function Shell({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn('overflow-hidden rounded-info border-[0.5px] border-line-2 bg-pane', className)}>
      {children}
    </div>
  );
}
