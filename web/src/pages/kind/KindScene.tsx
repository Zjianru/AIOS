import { ConversationPane } from './ConversationPane';

/** Kind 场景：T007 连接视图固定只有对话一栏（无右栏、无首次使用）。 */
export function KindScene({ pane }: { pane: string }) {
  if (pane === 'conversation') return <ConversationPane />;
  return null;
}
