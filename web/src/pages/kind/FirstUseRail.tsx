/**
 * 首次使用时的右栏：空库状态。
 * 不加假的演示数据（brief §四「首次使用的截图要用空库」），只说明接下去这里会长出什么。
 */

export function FirstUseRail() {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-content" data-pane="rail" data-first-use-rail="true">
      <section className="shrink-0 border-b border-line px-4 pb-3 pt-3">
        <h3 className="text-3 font-medium text-ink">聚焦</h3>
        <p className="mt-2 text-2 text-ink-3">都在正常推进</p>
      </section>
      <section className="min-h-0 flex-1 px-4 py-3">
        <h3 className="text-3 font-medium text-ink">Kind 现在带着</h3>
        <p className="mt-2 text-2 leading-5 text-ink-3">
          还没有上下文。开始一件事以后，这里会列：基本设定 · 能力说明 · 角色 · 记忆 · 资料 · 对话历史 · 工具结果。
        </p>
      </section>
      <footer className="shrink-0 border-t border-line px-4 py-2 text-1 text-ink-3">空库状态 · 开发开关「首次使用（空库）」打开时这样显示</footer>
    </div>
  );
}
