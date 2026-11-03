import { KernelGate } from './KernelGate';
import { ConversationPane } from './ConversationPane';
import { useKindStore } from './kindStore';

/** Kind 场景：根会话不可用时整页说明（五种状态）；正常时只有对话一栏。 */
export function KindScene({ pane }: { pane: string }) {
  const gate = useKindStore((state) => state.gate);
  const gateDetail = useKindStore((state) => state.gateDetail);
  const recheck = useKindStore((state) => state.recheck);
  if (gate !== 'ok') {
    return <KernelGate state={gate} detail={gateDetail ?? undefined} onRecheck={recheck} />;
  }
  if (pane === 'conversation') return <ConversationPane />;
  return null;
}
