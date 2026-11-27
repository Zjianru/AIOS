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

/** 跨页把一段选中引用带进 Kind 输入框（阅读页 -> Kind）。 */
const PENDING_QUOTE_KEY = 'aios.kind.pending-quote';

export interface PendingQuote {
  text: string;
  source: {
    kind: string;
    id: string;
    title: string;
    version?: number | null;
    taskId?: string | null;
  };
}

export function setPendingQuote(quote: PendingQuote): void {
  localStorage.setItem(PENDING_QUOTE_KEY, JSON.stringify(quote));
}

/** 取出并清掉；没有就返回 null（只消费一次）。 */
export function takePendingQuote(): PendingQuote | null {
  const raw = localStorage.getItem(PENDING_QUOTE_KEY);
  if (raw === null) return null;
  localStorage.removeItem(PENDING_QUOTE_KEY);
  try {
    const value = JSON.parse(raw) as PendingQuote;
    if (typeof value.text === 'string' && value.source && typeof value.source.kind === 'string') {
      return value;
    }
  } catch {
    // 坏值当没有。
  }
  return null;
}
