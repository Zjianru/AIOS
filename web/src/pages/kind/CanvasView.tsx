/// 画布（T015.1；D58，样子照原型 app-v14 CanvasView.tsx）。
///
/// 外壳在 /canvas-frame：一个 sandbox="allow-scripts allow-forms" 的 iframe，
/// 父窗口把这一版的 HTML 用 postMessage 交给它，它用 document.write 写进自己的文档。
/// 画布只做一件事：把主题变量和尺寸报给父窗口。

import type { CanvasViewDoc } from './turnViews';

export function CanvasView({ doc }: { doc: CanvasViewDoc }) {
  void doc;
  return null;
}
