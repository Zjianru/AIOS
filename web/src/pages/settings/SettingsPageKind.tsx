/**
 * 设置页的 Kind（D118）：点右上角 Kind，给三条这一页最常见的写动作。
 * 每条都先出确认卡；点确认后走和页面按钮同一条校验/写入口：
 * - 加看图模型 / 开关截桌面 -> settings-update；
 * - 装本机读字 -> install_package 动作（action-act，和对话里的安装卡同一动作）。
 */

import { useEffect, useState } from 'react';
import { Sparkles, X } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
import { actionAct, updateSettings, visionPackageStatus } from '@/data/service';
import type { ModelSettingsView, ModelRow } from '@/data/keyring';

type Suggestion = 'add-vision' | 'toggle-screen' | 'install-ocr';

interface Reply {
  text: string;
  title: string;
  facts: { name: string; value: string }[];
  confirmLabel: string;
  action: Suggestion;
}

function modelsOf(connection: ModelSettingsView['connections'][number]): ModelRow[] {
  return connection.models.length ? connection.models : connection.availableModels ?? [];
}

function installCandidate(view: ModelSettingsView): { connection: string; model: string } | null {
  for (const connection of view.connections) {
    if (!connection.key.saved) continue;
    for (const row of modelsOf(connection)) {
      if (row.input?.includes('image') !== true) continue;
      if (!view.visionModels.some((link) => link.connection === connection.id && link.model === row.id)) {
        return { connection: connection.id, model: row.id };
      }
    }
  }
  return null;
}

export function SettingsPageKind({ view, onChanged }: { view: ModelSettingsView; onChanged: () => void }) {
  const [open, setOpen] = useState(false);
  const [reply, setReply] = useState<Reply | null>(null);
  const [busy, setBusy] = useState(false);
  const [receipt, setReceipt] = useState<string | null>(null);
  const [installProgress, setInstallProgress] = useState<string>('');
  const visionCapable = (() => {
    const candidate = installCandidate(view);
    if (!candidate) return null;
    const connection = view.connections.find((item) => item.id === candidate.connection);
    const row = connection ? modelsOf(connection).find((item) => item.id === candidate.model) : undefined;
    return connection && row && row.input?.includes('image') ? { connection, row } : null;
  })();

  const suggest = (kind: Suggestion) => {
    setReceipt(null);
    if (kind === 'add-vision') {
      if (!visionCapable) {
        setReply(null);
        toast('已接的模型里没有能看图的，先去添加连接或把模型的「输入」改成文字+图片');
        return;
      }
      setReply({
        text: '把 ' + (visionCapable.row.name ?? visionCapable.row.id) + ' 加进看图模型' + (view.visionModels.length === 0 ? '，作为第一个' : '，作为备用') + '。',
        title: '加一个看图模型',
        facts: [
          { name: '看图模型', value: (visionCapable.row.name ?? visionCapable.row.id) + ' · ' + visionCapable.connection.name },
          { name: '现在', value: view.visionModels.length === 0 ? '没有' : view.visionModels.length + ' 个' },
          { name: '之后', value: view.visionModels.length + 1 + ' 个' },
        ],
        confirmLabel: '加为' + (view.visionModels.length === 0 ? '第一个' : '备用'),
        action: kind,
      });
      return;
    }
    if (kind === 'toggle-screen') {
      setReply({
        text: view.screenCapture ? '关掉「让 Kind 截桌面」后，Kind 要截会被拒并告诉你来这里打开。' : '打开后 Kind 需要时能截一张整个屏幕的图来看，只是看，不点也不按。',
        title: view.screenCapture ? '关掉截桌面' : '打开截桌面',
        facts: [{ name: '让 Kind 截桌面', value: (view.screenCapture ? '开' : '关') + ' → ' + (view.screenCapture ? '关' : '开') }],
        confirmLabel: view.screenCapture ? '关掉' : '打开',
        action: kind,
      });
      return;
    }
    setReply({
      text: '装了以后图里的字先在本机读，图不出本机；读不出、读不准才交给看图模型。下载和校验走安装卡同一条动作。',
      title: '装本机读字',
      facts: [
        { name: '包', value: '本机 OCR（PP-OCRv6 small）' },
        { name: '大小', value: '约 29.7 MiB' },
        { name: '来源', value: 'PaddlePaddle 官方（Apache-2.0）' },
        { name: '核对', value: '三个文件逐个 sha256' },
      ],
      confirmLabel: '下载并安装',
      action: kind,
    });
  };

  useEffect(() => {
    if (!busy || reply?.action !== 'install-ocr') return;
    let alive = true;
    const tick = async () => {
      try {
        const response = await visionPackageStatus();
        const pkg = response.packages.find((item) => item.id === 'pp-ocrv6-small');
        if (!alive || !pkg) return;
        if (pkg.state === 'downloading') setInstallProgress('正在下载 ' + Math.round((pkg.doneBytes / Math.max(1, pkg.totalBytes)) * 100) + '%');
        else if (pkg.state === 'verifying') setInstallProgress('正在核对三个文件的 sha256…');
        else setInstallProgress('正在安装…');
      } catch {
        // 读不到就保持普通忙状态。
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 500);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [busy, reply]);

  const confirm = async () => {
    if (!reply || busy) return;
    setBusy(true);
    try {
      if (reply.action === 'add-vision' && visionCapable) {
        await updateSettings({
          visionModels: [...view.visionModels, { connection: visionCapable.connection.id, model: visionCapable.row.id }],
        });
        setReceipt('看图模型加好了。');
      } else if (reply.action === 'toggle-screen') {
        await updateSettings({ screenCapture: !view.screenCapture });
        setReceipt(view.screenCapture ? '截桌面关掉了。' : '截桌面打开了，权限在下面这一节查。');
      } else {
        const result = await actionAct('install_package', { id: 'pp-ocrv6-small' });
        setReceipt(result.summary);
      }
      onChanged();
      window.dispatchEvent(new CustomEvent('aios:vision-settings'));
    } catch (error) {
      setReceipt('没做成：' + (error as Error).message);
    } finally {
      setBusy(false);
      setInstallProgress('');
    }
  };

  return (
    <>
      <button
        type="button"
        data-kind="page-kind-open"
        onClick={() => {
          setOpen(true);
          setReply(null);
          setReceipt(null);
        }}
        className="mt-3 inline-flex items-center gap-1.5 rounded-btn border border-line px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
        title="让 Kind 替你做这一页上的事：它出卡，你点了才改"
      >
        <Sparkles className="size-3.5" />
        页 Kind
      </button>
      {open ? (
        <aside
          data-kind="page-kind"
          className="fixed bottom-5 right-5 z-30 flex max-h-[min(680px,86vh)] w-[360px] flex-col rounded-card border border-line-2 bg-raised shadow-[var(--shadow-pop)]"
        >
          <header className="flex items-center gap-2 border-b border-line px-3.5 py-2.5">
            <Sparkles className="size-4 text-run" />
            <div className="min-w-0 flex-1">
              <div className="text-3 font-medium leading-5 text-ink">Kind</div>
              <div className="truncate text-1 leading-4 text-ink-3">在看：设置 › 模型与连接 › 看图</div>
            </div>
            <button
              type="button"
              onClick={() => go('kind')}
              className="rounded-btn px-1.5 py-1 text-1 text-ink-3 hover:bg-hover hover:text-ink"
              title="到 Kind 对话里看"
            >
              到对话里看
            </button>
            <button type="button" onClick={() => setOpen(false)} className="grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink" aria-label="关上">
              <X className="size-3.5" />
            </button>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto px-3.5 py-3">
            {reply ? (
              <div data-kind="page-kind-card">
                <div className="text-2 leading-6 text-ink-2">{reply.text}</div>
                <div className="mt-2 rounded-card border border-line p-3" data-kind="page-kind-card-brand">
                  <div className="text-3 text-ink">{reply.title}</div>
                  <ul className="mt-1 space-y-0.5">
                    {reply.facts.map((fact) => (
                      <li key={fact.name} className="flex gap-2 text-1 leading-4">
                        <span className="text-ink-3">{fact.name}</span>
                        <span className="text-ink-2">{fact.value}</span>
                      </li>
                    ))}
                  </ul>
                  {busy ? <div className="mt-2 text-1 text-run" data-kind="page-kind-progress">{installProgress || '正在处理…'}</div> : null}
                  {receipt && !busy ? <div className="mt-2 text-1 leading-4 text-ink-2" data-kind="page-kind-receipt">{receipt}</div> : null}
                  <div className="mt-2 flex justify-end gap-2">
                    <button type="button" disabled={busy} onClick={() => setReply(null)} className="rounded-btn px-2.5 py-1 text-2 text-ink-2 hover:bg-hover disabled:opacity-50">暂不</button>
                    <button type="button" disabled={busy || Boolean(receipt)} onClick={() => void confirm()} className="rounded-btn bg-ink px-3 py-1 text-2 text-content hover:opacity-90 disabled:opacity-50">
                      {busy ? '处理中…' : reply.confirmLabel}
                    </button>
                  </div>
                </div>
              </div>
            ) : (
              <>
                <p className="text-2 leading-6 text-ink-2">这一页上你能点的，都可以直接跟我说。要改东西我出一张卡，你点了才改。</p>
                <div className="mt-3 text-1 text-ink-3">常说的</div>
                <div className="mt-2 flex flex-col items-start gap-1.5">
                  {[
                    { kind: 'add-vision' as const, say: '加一个看图模型' },
                    { kind: 'toggle-screen' as const, say: view.screenCapture ? '关掉截桌面' : '打开截桌面' },
                    { kind: 'install-ocr' as const, say: '装本机读字' },
                  ].map((item) => (
                    <button
                      key={item.kind}
                      type="button"
                      data-kind="page-kind-suggestion"
                      onClick={() => suggest(item.kind)}
                      className={cn('rounded-btn border border-line px-2.5 py-1 text-left text-2 leading-5 text-ink-2 transition-colors hover:border-line-2 hover:bg-hover hover:text-ink')}
                    >
                      {item.say}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        </aside>
      ) : null}
    </>
  );
}
