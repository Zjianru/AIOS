import { useEffect } from 'react';

import { FirstUsePane } from './FirstUsePane';
import { KernelGate } from './KernelGate';
import { ConversationPane } from './ConversationPane';
import { useKindStore } from './kindStore';

/** Kind 场景：根会话不可用时整页说明（五种状态）；正常时只有对话一栏。 */
export function KindScene({ pane }: { pane: string }) {
  const firstUse = useKindStore((state) => state.firstUse);
  const bootstrap = useKindStore((state) => state.bootstrap);
  const disconnect = useKindStore((state) => state.disconnect);
  const gate = useKindStore((state) => state.gate);
  const gateDetail = useKindStore((state) => state.gateDetail);
  const recheck = useKindStore((state) => state.recheck);

  useEffect(() => {
    void bootstrap();
    // 离开 Kind 场景（设置页往返、切场景）时关掉连接：旧 socket 留在后台会和新 mount
    // 打开的那条一起收 agent 帧，实时文字就被追加两遍。
    return () => disconnect();
  }, [bootstrap, disconnect]);
  // T008 r4 #1：401 已由 service 请求层统一置 gate=token；令牌说明页先于第一次使用页。
  if (gate !== 'ok') {
    return <KernelGate state={gate} detail={gateDetail ?? undefined} onRecheck={recheck} />;
  }
  if (firstUse) {
    return (
      <FirstUsePane
        onConnected={() => {
          useKindStore.setState({ firstUse: false });
          void useKindStore.getState().recheck();
        }}
      />
    );
  }
  if (pane === 'conversation') return <ConversationPane />;
  return null;
}
