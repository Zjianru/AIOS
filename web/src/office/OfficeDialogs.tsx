/**
 * Office 预览/插件关掉两种对话框。
 *
 * Univer 导入 xlsx 的 spike 未通过（停止条件 1）：xlsx/docx/pptx 统一「没有保真预览」卡，
 * 不拿近似图替代；坏文件写原因。打开原件下载字节，Finder 是原型提示（web 没有系统接口）。
 */

import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { FileText, X } from 'lucide-react';
import { fileReadBytes } from '@/data/service';
import { go } from '@/app/navigate';
import { officeKindLabel, officeKindOf, officeLabel, officeSizeLabel, type OfficeKind } from './format';

export interface OfficeFileRef {
  /** 附件 id（sha256:...）或路径。 */
  id: string;
  name: string;
  bytes: number;
  mime?: string | null;
}

function Overlay(props: { children: React.ReactNode; onClose: () => void }) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-ink/20 p-6"
      onClick={props.onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        className="max-h-[80vh] w-full max-w-[720px] overflow-y-auto rounded-card border border-line bg-pane p-5 shadow-xl"
        onClick={(event) => event.stopPropagation()}
      >
        {props.children}
      </div>
    </div>
  );
}

function CloseButton({ onClose }: { onClose: () => void }) {
  return (
    <button
      type="button"
      aria-label="关闭预览"
      onClick={onClose}
      className="grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink"
    >
      <X className="size-4" />
    </button>
  );
}

export function OfficeOffDialog(props: {
  open: boolean;
  file: OfficeFileRef;
  onClose: () => void;
}) {
  const kind = officeKindOf(props.file.name, props.file.mime) ?? 'xlsx';
  if (!props.open) return null;
  return (
    <Overlay onClose={props.onClose}>
      <div data-kind="office-off">
        <div className="flex items-start justify-between gap-3">
          <div>
            <div className="text-3 text-ink">
              {officeLabel(kind)} · {officeSizeLabel(props.file.bytes)}
            </div>
            <div className="mt-1 text-2 text-ink-3">{props.file.name}</div>
          </div>
          <CloseButton onClose={props.onClose} />
        </div>
        <div className="mt-4 flex items-start gap-3 rounded-card border border-line-2 bg-raised p-3" data-kind="office-off-hint">
          <FileText className="mt-0.5 size-4 shrink-0 text-ink-3" />
          <div className="text-2 text-leading-5 text-ink-2">
            Office 插件没开。打开它以后，Kind 和任务能读 xlsx/docx/pptx，写任务能新建 xlsx；
            不会跑进程，也不带运行时。
          </div>
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={props.onClose}
            className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover"
          >
            先不看
          </button>
          <button
            type="button"
            data-kind="office-go-plugins"
            onClick={() => {
              props.onClose();
              go('settings:plugins');
            }}
            className="rounded-btn bg-ink px-2.5 py-1 text-2 text-content hover:opacity-90"
          >
            去设置 › 插件打开
          </button>
        </div>
      </div>
    </Overlay>
  );
}

interface LoadState {
  loading: boolean;
  error: string | null;
  bytes: Uint8Array | null;
  mime: string;
  contentVersion: string;
  text: string | null;
  broken: boolean;
}

/** xlsx/docx/pptx 都是 ZIP：本地文件头 + 结尾目录；PK 头但没结尾目录算坏文件，不猜内容。 */
function isZip(bytes: Uint8Array): boolean {
  if (bytes.length < 22) return false;
  if (
    bytes[0] !== 0x50 ||
    bytes[1] !== 0x4b ||
    bytes[2] !== 0x03 ||
    bytes[3] !== 0x04
  ) {
    return false;
  }
  const tailStart = Math.max(0, bytes.length - 65557);
  for (let index = tailStart; index + 4 <= bytes.length; index += 1) {
    if (
      bytes[index] === 0x50 &&
      bytes[index + 1] === 0x4b &&
      bytes[index + 2] === 0x05 &&
      bytes[index + 3] === 0x06
    ) {
      return true;
    }
  }
  return false;
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export function OfficePreviewDialog(props: {
  open: boolean;
  file: OfficeFileRef;
  onClose: () => void;
}) {
  const kind: OfficeKind = officeKindOf(props.file.name, props.file.mime) ?? 'xlsx';
  const [state, setState] = useState<LoadState>({
    loading: false,
    error: null,
    bytes: null,
    mime: '',
    contentVersion: '',
    text: null,
    broken: false,
  });

  useEffect(() => {
    if (!props.open) return;
    let live = true;
    setState((current) => ({ ...current, loading: true, error: null }));
    const attachmentId = props.file.id.startsWith('sha256:') ? props.file.id : undefined;
    const path = attachmentId ? undefined : props.file.id;
    void fileReadBytes({ attachmentId, path })
      .then((response) => {
        if (!live) return;
        const bytes = decodeBase64(response.bytes_base64);
        setState({
          loading: false,
          error: null,
          bytes,
          mime: response.mime,
          contentVersion: response.content_version,
          text: response.text ?? null,
          broken: !isZip(bytes),
        });
      })
      .catch((error: unknown) => {
        if (!live) return;
        setState((current) => ({
          ...current,
          loading: false,
          error: error instanceof Error ? error.message : '读不回这个文件',
          bytes: null,
          broken: true,
        }));
      });
    return () => {
      live = false;
    };
  }, [props.open, props.file.id]);

  if (!props.open) return null;

  function openOriginal() {
    if (!state.bytes) return;
    const blob = new Blob([new Uint8Array(state.bytes) as unknown as BlobPart], {
      type: state.mime || 'application/octet-stream',
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = props.file.name;
    anchor.click();
    URL.revokeObjectURL(url);
  }

  return (
    <Overlay onClose={props.onClose}>
      <div data-kind="office-preview">
        <div className="flex items-start justify-between gap-3">
          <div>
            <div className="text-3 text-ink">
              {officeLabel(kind)} · {officeSizeLabel(props.file.bytes)} · 只读副本
            </div>
            <div className="mt-1 text-2 text-ink-3">{props.file.name}</div>
          </div>
          <CloseButton onClose={props.onClose} />
        </div>

        {state.loading ? (
          <p className="mt-6 text-2 text-ink-3">正在读文件…</p>
        ) : state.broken ? (
          <div className="mt-4 rounded-card border border-line-2 bg-raised p-4" data-kind="office-broken">
            <div className="text-3 text-ink">这个文件打不开</div>
            <p className="mt-1 text-2 leading-5 text-ink-2">
              {state.error ?? '文件不是有效的 Office 压缩包（可能损坏或不是真的 Office 文件）。'}
            </p>
            <div className="mt-3 flex gap-2">
              <button
                type="button"
                onClick={openOriginal}
                className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover"
              >
                打开原件
              </button>
              <button
                type="button"
                onClick={() => toast('（原型）真实应用里会在 Finder 里显示')}
                className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover"
              >
                在 Finder 中显示
              </button>
            </div>
          </div>
        ) : (
          <div className="mt-4 rounded-card border border-line-2 bg-raised p-4" data-kind="office-no-fidelity">
            <div className="text-3 text-ink">没有保真预览</div>
            <p className="mt-1 text-2 leading-5 text-ink-2">
              {kind === 'xlsx'
                ? '这一版没有 xlsx 保真预览：开源表格引擎的导入路径没通过实测（停止条件 1），不拿近似图替代。Kind 仍能读结构与单元格；原件可以直接打开。'
                : officeKindLabel(kind) + '这一版没有保真预览：不接近似渲染，也不假装保真。Kind 能读结构与文字；原件可以直接打开。'}
            </p>
            <div className="mt-3 flex gap-2">
              <button
                type="button"
                onClick={openOriginal}
                className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover"
              >
                打开原件
              </button>
              <button
                type="button"
                onClick={() => toast('（原型）真实应用里会在 Finder 里显示')}
                className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 hover:bg-hover"
              >
                在 Finder 中显示
              </button>
            </div>
          </div>
        )}
        {state.text ? (
          <div className="mt-3 rounded-card border border-line-2 bg-raised p-3" data-kind="office-text">
            <div className="text-2 text-ink-2">读到的文字（不含版式）</div>
            <pre className="mt-1 max-h-[260px] overflow-y-auto whitespace-pre-wrap font-mono text-1 leading-5 text-ink-2">
              {state.text}
            </pre>
          </div>
        ) : null}
        {state.contentVersion ? (
          <p className="mt-3 text-1 text-ink-3">内容版本：{state.contentVersion}</p>
        ) : null}
      </div>
    </Overlay>
  );
}
