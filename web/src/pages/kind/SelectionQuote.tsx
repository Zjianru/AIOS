/**
 * 选中一段文字后浮「引用」按钮的共享件。
 *
 * 来源由调用方给（任务对话/报告/阅读页）；选中范围必须在本件里面。
 */
import { useRef, useState } from 'react';
import { createPortal } from 'react-dom';

export interface SelectionQuoteSource {
  kind: string;
  id: string;
  title: string;
  version?: number | null;
  taskId?: string | null;
}

interface SelectionQuoteProps {
  source: SelectionQuoteSource;
  onQuote: (text: string, source: SelectionQuoteSource) => void;
  children: React.ReactNode;
  className?: string;
}

export function SelectionQuote({ source, onQuote, children, className }: SelectionQuoteProps) {
  const areaRef = useRef<HTMLDivElement | null>(null);
  const [spot, setSpot] = useState<{ text: string; left: number; top: number } | null>(null);

  const capture = () => {
    const area = areaRef.current;
    const selection = window.getSelection();
    if (!area || !selection || selection.isCollapsed) {
      setSpot(null);
      return;
    }
    const text = selection.toString().trim();
    if (!text) {
      setSpot(null);
      return;
    }
    const range = selection.getRangeAt(0);
    if (!area.contains(range.commonAncestorContainer)) {
      setSpot(null);
      return;
    }
    const rect = range.getBoundingClientRect();
    setSpot({
      text,
      left: Math.min(Math.max(8, rect.left), window.innerWidth - 80),
      top: Math.min(rect.bottom + 6, window.innerHeight - 40),
    });
  };

  const quote = () => {
    if (!spot) return;
    onQuote(spot.text, source);
    setSpot(null);
    window.getSelection()?.removeAllRanges();
  };

  return (
    <div ref={areaRef} className={className} onMouseUp={capture} onKeyUp={capture}>
      {children}
      {spot
        ? createPortal(
            <button
              type="button"
              data-kind="quote-selection"
              onMouseDown={(event) => event.preventDefault()}
              onClick={quote}
              style={{ position: 'fixed', left: spot.left, top: spot.top }}
              className="z-50 rounded-btn border border-line-2 bg-raised px-2 py-1 text-1 text-ink shadow-[var(--shadow-pop)] hover:bg-hover"
            >
              引用
            </button>,
            document.body,
          )
        : null}
    </div>
  );
}
