/** 记忆页「加一条」没有页面直写协议：把一句草稿经 localStorage 带进 Kind 输入框。 */
const PENDING_DRAFT_KEY = 'aios.kind.pending-draft';

export function setPendingDraft(text: string): void {
  localStorage.setItem(PENDING_DRAFT_KEY, text);
}

/** 取出并清掉；没有就返回 null（只消费一次）。 */
export function takePendingDraft(): string | null {
  const text = localStorage.getItem(PENDING_DRAFT_KEY);
  if (text !== null) localStorage.removeItem(PENDING_DRAFT_KEY);
  return text;
}
