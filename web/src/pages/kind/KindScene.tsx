import { useAppStore } from '@/app/store';
import { ConversationPane } from './ConversationPane';
import { RightRail } from './RightRail';
import { FirstUsePane } from './FirstUsePane';
import { FirstUseRail } from './FirstUseRail';
import { KernelGate } from './KernelGate';

/**
 * Kind 场景默认布局：对话 | 右栏。
 * 右栏 = 上「聚焦」+ 下「Kind 现在带着」（用户 2026-10-28 追加第 1 条）；
 * 原「在做的事」线路图挪到工作页的完整线路图。
 * 开发开关「首次使用（空库）」打开时，换成首次使用的 Kind：接模型 → 起手卡片 → 可选推荐，
 * 右栏也是空库状态（brief §四：不放假的演示数据）。
 */
export function KindScene({ pane }: { pane: string }) {
  const firstUse = useAppStore((state) => state.firstUse);
  const kernel = useAppStore((state) => state.kernel);
  // 接不上内核时整页说明原因（右栏此时不显示，见 AppShell 的 soloKind）。
  if (kernel !== 'ok') return pane === 'conversation' ? <KernelGate state={kernel} /> : null;
  if (firstUse) return pane === 'conversation' ? <FirstUsePane /> : <FirstUseRail />;
  if (pane === 'conversation') return <ConversationPane />;
  return <RightRail />;
}
