/**
 * Kind 右栏：上「聚焦」+ 下「Kind 现在带着」，两块同时在。
 */

import { FocusPane } from './FocusPane';
import { ContextPane } from './ContextPane';

export function RightRail() {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-content" data-pane="rail">
      <FocusPane />
      <ContextPane />
    </div>
  );
}
