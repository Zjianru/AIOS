/** 交付清单里的 Office 文件：可点开预览 + 固定「未做视觉自检」。 */

import { useState } from 'react';
import { FileSpreadsheet } from 'lucide-react';
import { officeKindOf, officeLabel } from './format';
import { OfficePreviewDialog } from './OfficeDialogs';

function baseName(path: string): string {
  const parts = path.split('/');
  return parts[parts.length - 1] || path;
}

export function OfficeDeliverable({ path }: { path: string }) {
  const [open, setOpen] = useState(false);
  const kind = officeKindOf(path);
  const name = baseName(path);
  return (
    <span className="mr-2 inline-flex min-w-0 items-center gap-1" data-kind="office-deliverable">
      <button
        type="button"
        onClick={() => setOpen(true)}
        title={'点击预览：' + path + '（只读副本）'}
        className="inline-flex min-w-0 items-center gap-1 text-ink-2 hover:text-ink"
      >
        <FileSpreadsheet className="size-3 shrink-0 text-ink-3" />
        <span className="process-path truncate">{name}</span>
        <span className="shrink-0 text-1 text-ink-3">{kind ? officeLabel(kind) : 'Office'} · 只读副本</span>
      </button>
      <span data-kind="no-visual-check" title="只做了程序读回与容器/单元格核对，没有做视觉自检" className="shrink-0 text-1 text-ink-3">
        未做视觉自检
      </span>
      <OfficePreviewDialog
        open={open}
        file={{ id: path, name, bytes: 0, mime: kind === 'xlsx' ? 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet' : null }}
        onClose={() => setOpen(false)}
      />
    </span>
  );
}
