//! 根连接状态落到页头 gate。

import type { RootState } from '@/data/service';
import type { GateState } from './KernelGate';
import { useKindStore } from './kindStore';

export function applyRootState(root: RootState): void {
  if (root.state === 'ok') {
    const gate = useKindStore.getState().gate;
    if (gate !== 'unreachable' && gate !== 'token') {
      useKindStore.setState({ gate: 'ok', gateDetail: null });
    }
    return;
  }
  const mapped: GateState =
    root.kind === 'lost'
      ? 'lost'
      : root.kind === 'corrupt'
        ? 'broken'
        : root.kind === 'busy'
          ? 'busy'
          : 'unreachable';
  useKindStore.setState({ gate: mapped, gateDetail: { path: root.path ?? undefined, line: root.line ?? undefined } });
}
