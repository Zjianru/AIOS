import { useEffect } from 'react';

import { FirstUsePane } from './FirstUsePane';
import { KernelGate } from './KernelGate';
import { ConversationPane } from './ConversationPane';
import { RightRail } from './RightRail';
import { TaskCardStateSync } from './TaskCardStateSync';
import { useKindStore } from './kindStore';

/** Kind 场景：根会话不可用时整页说明（五种状态）；正常时只有对话一栏。 */
export function KindScene({ pane }: { pane: string }) {
  const firstUse = useKindStore((state) => state.firstUse);
  const bootstrap = useKindStore((state) => state.bootstrap);
  const disconnect = useKindStore((state) => state.disconnect);
  const gate = useKindStore((state) => state.gate);
  const gateDetail = useKindStore((state) => state.gateDetail);
  const recheck = useKindStore((state) => state.recheck);

  // Kind 恢复两栏后 bootstrap/断开只由对话栏做一次；右栏不再各开一条连接
  // （两条连接会互相顶掉 activeSocket，首用/门说明页也会重复渲染一份）。
  const active = pane === 'conversation';
  useEffect(() => {
    if (!active) return;
    void bootstrap();
    // 离开 Kind 场景（设置页往返、切场景）时关掉连接：旧 socket 留在后台会和新 mount
    // 打开的那条一起收 agent 帧，实时文字就被追加两遍。
    return () => disconnect();
  }, [active, bootstrap, disconnect]);

  // 记忆页搜索命中「Kind 对话」：回到 Kind 并滚到那一轮。
  useEffect(() => {
    const raw = sessionStorage.getItem('aios:memory-jump');
    if (!raw) return;
    sessionStorage.removeItem('aios:memory-jump');
    try {
      const jump = JSON.parse(raw) as { turn?: unknown };
      if (typeof jump.turn === 'number' && jump.turn > 0) {
        void useKindStore.getState().revealTurnNumber(jump.turn);
      }
    } catch {
      // 坏了就当没有跳转要求。
    }
  }, []);
  // T008 r4 #1：401 已由 service 请求层统一置 gate=token；令牌说明页先于第一次使用页。
  if (gate !== 'ok') {
    if (!active) return null;
    return <KernelGate state={gate} detail={gateDetail ?? undefined} onRecheck={recheck} />;
  }
  if (firstUse) {
    if (!active) return null;
    return (
      <FirstUsePane
        onConnected={() => {
          useKindStore.setState({ firstUse: false });
          void useKindStore.getState().recheck();
        }}
      />
    );
  }
  if (pane === 'conversation') {
    return (
      <>
        <TaskCardStateSync />
        <ConversationPane />
      </>
    );
  }
  return <RightRail />;
}
