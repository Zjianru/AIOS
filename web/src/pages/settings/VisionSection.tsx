/**
 * 设置 › 模型与连接 › 看图：看图模型链、本机读字、截桌面开关。
 * 协议事实来自 ModelSettingsResponse 的 visionModels / screenCapture；动作走 settings-update、
 * vision-test、screen-permission、package-status、package-uninstall。
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import { ArrowDown, ArrowUp, Plus } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
import { Switch } from '@/components/ui/switch';
import {
  screenPermission, updateSettings, visionPackageStatus, visionPackageUninstall, visionTest,
} from '@/data/service';
import type { ModelSettingsView, ModelRow, VisionModelRefView } from '@/data/keyring';
import type { PackageStatusView } from '@/data/generated/PackageStatusView';
import type { ScreenPermissionResponse } from '@/data/generated/ScreenPermissionResponse';
import type { VisionTestResponse } from '@/data/generated/VisionTestResponse';

function notifyVisionSettings() {
  window.dispatchEvent(new CustomEvent('aios:vision-settings'));
}

const inputClass =
  'w-full rounded-btn border border-line-2 bg-content px-2.5 py-1.5 text-3 text-ink outline-none placeholder:text-ink-3 focus:border-ink-3';

function Btn({
  kind = 'plain', className, ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { kind?: 'primary' | 'plain' | 'quiet' | 'danger' }) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        'shrink-0 whitespace-nowrap rounded-btn px-3 py-1.5 text-2 transition-colors disabled:cursor-not-allowed disabled:opacity-45',
        kind === 'primary' && 'bg-ink text-content hover:opacity-90',
        kind === 'plain' && 'border border-line-2 text-ink-2 hover:bg-hover hover:text-ink',
        kind === 'quiet' && 'text-ink-3 hover:bg-hover hover:text-ink',
        kind === 'danger' && 'text-[var(--fail)] hover:bg-[var(--fail-soft)]',
        className,
      )}
    />
  );
}

function modelLabel(connectionName: string, row: ModelRow): string {
  return (row.name ?? row.id) + ' · ' + connectionName;
}

function modelsOf(connection: ModelSettingsView['connections'][number]): ModelRow[] {
  return connection.models.length ? connection.models : connection.availableModels ?? [];
}

function imageCapable(row: ModelRow): boolean {
  return row.input?.includes('image') === true;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return bytes + ' B';
  const mib = bytes / 1_048_576;
  if (mib >= 1) return mib.toFixed(1) + ' MiB';
  return (bytes / 1024).toFixed(0) + ' KB';
}

const TONE = { ok: 'var(--done)', fail: 'var(--fail)', warn: 'var(--wait)', run: 'var(--run)', idle: 'var(--ink-3)' } as const;

function Dot({ tone, pulse }: { tone: keyof typeof TONE; pulse?: boolean }) {
  return (
    <span
      className={cn('block size-2 shrink-0 rounded-full', pulse && 'animate-pulse')}
      style={{ backgroundColor: TONE[tone] }}
    />
  );
}

interface TestState {
  kind: 'idle' | 'testing' | 'ok' | 'textOnly' | 'rateLimited' | 'timeout' | 'error';
  elapsedMs?: number;
  message?: string;
}

function testLine(test: TestState): { text: string; tone: keyof typeof TONE } {
  switch (test.kind) {
    case 'idle':
      return { text: '还没测过。「测试看图」：1 次请求，只看这个模型能不能看图', tone: 'idle' };
    case 'testing':
      return { text: '正在测试 · 1 张内置测试图 · 不走备用', tone: 'run' };
    case 'ok':
      return { text: '测过 · ' + (test.elapsedMs ?? 0) + ' 毫秒 · 看图正常', tone: 'ok' };
    case 'textOnly':
      return { text: '测过 · 实测只认文字：图片请求被模型拒绝。换一个，或者把它的「输入」改回 文字', tone: 'fail' };
    case 'rateLimited':
      return { text: '请求太频繁，稍后再测', tone: 'warn' };
    case 'timeout':
      return { text: '测试超时，已自动结束', tone: 'warn' };
    case 'error':
      return { text: '测试失败：' + (test.message ?? '后端出错'), tone: 'fail' };
  }
}

function testOf(response: VisionTestResponse): TestState {
  switch (response.state) {
    case 'normal':
      return { kind: 'ok', elapsedMs: response.elapsedMs ?? 0 };
    case 'text-only':
      return { kind: 'textOnly' };
    case 'rate-limited':
      return { kind: 'rateLimited' };
    case 'timeout':
      return { kind: 'timeout' };
    default:
      return { kind: 'error', message: response.message ?? undefined };
  }
}

function ChainRow({
  view, link, index, count, test, onTest, onMove, onRemove,
}: {
  view: ModelSettingsView;
  link: VisionModelRefView;
  index: number;
  count: number;
  test: TestState;
  onTest: () => void;
  onMove: (delta: number) => void;
  onRemove: () => void;
}) {
  const connection = view.connections.find((item) => item.id === link.connection);
  const row = connection ? modelsOf(connection).find((item) => item.id === link.model) : undefined;
  const line = testLine(test);
  return (
    <div className={cn('flex items-center gap-3 px-4 py-3', index > 0 && 'border-t border-line')} data-kind="vision-model">
      <Dot tone={line.tone} pulse={test.kind === 'testing'} />
      <div className="min-w-0">
        <div className="flex items-center gap-2 text-3 text-ink">
          <span className="grid size-[18px] shrink-0 place-items-center rounded-full bg-hover text-1 text-ink-2">{index + 1}</span>
          {row?.name ?? link.model}
          <span className="text-2 text-ink-3">{(connection?.name ?? link.connection) + (index === 0 ? ' · 先用' : ' · 备用')}</span>
        </div>
        <div
          className="mt-0.5 truncate text-1"
          style={{ color: line.tone === 'idle' || line.tone === 'run' ? undefined : TONE[line.tone] }}
        >
          <span className={line.tone === 'idle' || line.tone === 'run' ? 'text-ink-3' : ''}>{line.text}</span>
        </div>
      </div>
      <div className="ml-auto flex shrink-0 items-center gap-1">
        <Btn kind="quiet" disabled={test.kind === 'testing'} onClick={onTest}>
          {test.kind === 'testing' ? '正在测试…' : test.kind === 'idle' ? '测试看图' : '重新测试'}
        </Btn>
        <Btn kind="quiet" className="px-1.5" disabled={index === 0} aria-label="往前挪" title="往前挪：先试它" onClick={() => onMove(-1)}>
          <ArrowUp className="size-3.5" />
        </Btn>
        <Btn kind="quiet" className="px-1.5" disabled={index === count - 1} aria-label="往后挪" title="往后挪" onClick={() => onMove(1)}>
          <ArrowDown className="size-3.5" />
        </Btn>
        <Btn
          kind="danger"
          onClick={() => {
            onRemove();
            toast(count === 1 ? '看图模型清空了：主模型看不了图时，Kind 会照实说看不到' : '看图模型移除了一个');
          }}
        >
          移除
        </Btn>
      </div>
    </div>
  );
}

function OcrRow({ onChanged }: { onChanged: () => void }) {
  const [pkg, setPkg] = useState<PackageStatusView | null>(null);
  const load = useCallback(async () => {
    try {
      const response = await visionPackageStatus();
      setPkg(response.packages.find((item) => item.id === 'pp-ocrv6-small') ?? null);
    } catch {
      // 读不到包状态时按没装显示，不阻塞设置页。
      setPkg(null);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    if (!pkg || (pkg.state !== 'downloading' && pkg.state !== 'verifying')) return;
    const timer = window.setInterval(() => void load(), 800);
    return () => window.clearInterval(timer);
  }, [pkg, load]);

  const phase = pkg?.state ?? 'not-installed';
  const total = pkg?.totalBytes || (9_880_512 + 21_159_378 + 74_947);
  const status: { text: string; tone: keyof typeof TONE } =
    phase === 'not-installed'
      ? { text: '没装：图里的字现在交给看图模型读。装了以后先在本机读，图不出本机；约 ' + formatBytes(total) + '，点了才下载', tone: 'idle' }
      : phase === 'downloading'
        ? { text: '正在下载 ' + formatBytes(pkg?.doneBytes ?? 0) + ' / ' + formatBytes(total), tone: 'run' }
        : phase === 'verifying'
          ? { text: '下载完了，正在核对三个文件的 sha256…', tone: 'run' }
          : phase === 'installed'
            ? { text: '已装' + (pkg?.installedAt ? '（' + new Date(pkg.installedAt * 1000).toLocaleString('zh-CN') + '）' : '') + ' · 三个文件 sha256 都对上了 · 读不出、读不准时才交给看图模型', tone: 'ok' }
            : { text: '没装上：' + (pkg?.reason ?? '下载或核对失败') + '。原来的没动', tone: 'fail' };
  const busy = phase === 'downloading' || phase === 'verifying';
  return (
    <div className="flex items-center gap-3 px-4 py-3" data-kind="vision-ocr" data-phase={phase}>
      <Dot tone={status.tone} pulse={busy} />
      <div className="min-w-0">
        <div className="text-3 text-ink">
          PP-OCRv6 small
          <span className="ml-2 text-2 text-ink-3">Apache-2.0 · 来自 PaddlePaddle 官方</span>
        </div>
        <div
          className="mt-0.5 text-1 leading-4"
          style={{ color: status.tone === 'idle' || status.tone === 'run' ? undefined : TONE[status.tone] }}
        >
          <span className={status.tone === 'idle' || status.tone === 'run' ? 'text-ink-3' : ''}>{status.text}</span>
        </div>
        {busy && phase === 'downloading' ? (
          <div className="mt-1.5 h-1 w-56 overflow-hidden rounded-full bg-hover">
            <div
              className="h-full rounded-full bg-[var(--run)] transition-[width]"
              style={{ width: Math.round(((pkg?.doneBytes ?? 0) / Math.max(1, total)) * 100) + '%' }}
            />
          </div>
        ) : null}
      </div>
      <div className="ml-auto flex shrink-0 items-center gap-1">
        {phase === 'not-installed' || phase === 'failed' ? (
          <Btn
            kind="plain"
            onClick={() => {
              go('kind');
              toast('到和 Kind 的对话里说「装一下本机读字」', { description: '它会出安装卡；点了才下载。' });
            }}
          >
            让 Kind 装
          </Btn>
        ) : phase === 'installed' ? (
          <Btn
            kind="danger"
            onClick={() => {
              void visionPackageUninstall({ id: 'pp-ocrv6-small' })
                .then(() => {
                  toast('卸载了：只删了包目录', { description: '以后图里的字交给看图模型读' });
                  notifyVisionSettings();
                  onChanged();
                  void load();
                })
                .catch((error: Error) => toast.error(error.message));
            }}
          >
            卸载
          </Btn>
        ) : null}
      </div>
    </div>
  );
}

function ScreenRow({ view, onChanged }: { view: ModelSettingsView; onChanged: () => void }) {
  const [on, setOn] = useState(view.screenCapture);
  const [permission, setPermission] = useState<ScreenPermissionResponse | null>(null);
  const [checking, setChecking] = useState(false);
  const check = useCallback(async () => {
    setChecking(true);
    try {
      setPermission(await screenPermission());
    } catch {
      setPermission(null);
    } finally {
      setChecking(false);
    }
  }, []);
  useEffect(() => {
    if (on) void check();
  }, [on, check]);
  return (
    <div data-kind="vision-screen">
      <div className="flex items-start gap-6 px-4 py-3">
        <div className="min-w-0 flex-1">
          <div className="text-3 text-ink">让 Kind 截桌面</div>
          <div className="mt-0.5 text-2 leading-5 text-ink-3">
            默认关。打开后，Kind 需要时能截一张整个屏幕的图来看，只是看，不点也不按。关着时它要截会被拒，并告诉你来这里打开。把一段网页代码渲染成图不归这个开关管，那不碰你的屏幕。
          </div>
        </div>
        <div className="shrink-0 pt-0.5">
          <Switch
            checked={on}
            data-kind="screen-toggle"
            onCheckedChange={(next) => {
              setOn(next);
              void updateSettings({ screenCapture: next })
                .then(() => {
                  notifyVisionSettings();
                  onChanged();
                  toast(next ? '截桌面打开了：先查一次 macOS 的录屏权限' : '截桌面关了：Kind 再要截会被拒', {
                    description: '工具表不变，只是执行时看这个开关',
                  });
                })
                .catch((error: Error) => {
                  setOn(!next);
                  toast.error(error.message);
                });
            }}
          />
        </div>
      </div>
      {on ? (
        <div
          className="flex items-center gap-3 border-t border-line px-4 py-2.5"
          data-kind="screen-permission"
          data-phase={checking ? 'checking' : permission?.state ?? 'unknown'}
        >
          <Dot
            tone={permission?.state === 'granted' ? 'ok' : permission?.state === 'denied' ? 'warn' : 'run'}
            pulse={checking}
          />
          <div
            className="min-w-0 text-1 leading-4"
            style={{ color: permission?.state === 'granted' ? TONE.ok : permission?.state === 'denied' ? TONE.warn : undefined }}
          >
            {permission?.state === 'granted' ? (
              'macOS 已给 AIOS 屏幕录制权限'
            ) : permission?.state === 'denied' ? (
              'macOS 还没给 AIOS 屏幕录制权限，现在截不了。到「系统设置 › 隐私与安全性 › 录屏与系统录音」打开 AIOS。'
            ) : (
              <span className="text-ink-3">正在查 macOS 的屏幕录制权限…</span>
            )}
          </div>
          {permission?.state === 'denied' ? (
            <div className="ml-auto flex shrink-0 items-center gap-1">
              <Btn kind="quiet" onClick={() => window.open(permission.settingsUrl, '_blank')}>
                打开系统设置
              </Btn>
              <Btn kind="plain" onClick={() => void check()}>
                再查一次
              </Btn>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function VisionSection({ view, onChanged }: { view: ModelSettingsView; onChanged: () => void }) {
  const [chain, setChain] = useState<VisionModelRefView[]>(view.visionModels);
  const [tests, setTests] = useState<Record<string, TestState>>({});
  useEffect(() => {
    setChain(view.visionModels);
  }, [view.visionModels]);
  const save = async (next: VisionModelRefView[]) => {
    setChain(next);
    try {
      await updateSettings({ visionModels: next });
      notifyVisionSettings();
      onChanged();
    } catch (error) {
      setChain(view.visionModels);
      toast.error((error as Error).message);
    }
  };
  const addOptions = useMemo(
    () =>
      view.connections
        .filter((connection) => connection.key.saved)
        .flatMap((connection) => modelsOf(connection).map((row) => ({ connection, row })))
        .filter(({ row }) => imageCapable(row))
        .filter(({ connection, row }) => !chain.some((link) => link.connection === connection.id && link.model === row.id)),
    [view, chain],
  );
  const [picked, setPicked] = useState('');
  const value = addOptions.some(({ connection, row }) => connection.id + '/' + row.id === picked)
    ? picked
    : addOptions[0]
      ? addOptions[0].connection.id + '/' + addOptions[0].row.id
      : '';
  const empty = chain.length === 0;
  const test = async (link: VisionModelRefView) => {
    const key = link.connection + '/' + link.model;
    setTests((current) => ({ ...current, [key]: { kind: 'testing' } }));
    try {
      const response = await visionTest({ connection: link.connection, model: link.model });
      setTests((current) => ({ ...current, [key]: testOf(response) }));
    } catch (error) {
      setTests((current) => ({ ...current, [key]: { kind: 'error', message: (error as Error).message } }));
    }
  };
  return (
    <section className="mt-10 scroll-mt-6" data-settings="vision">
      <h2 className="text-3 font-medium text-ink">看图</h2>
      <p className="mt-1 text-2 leading-5 text-ink-2">
        这里只设置谁负责看图，聊天用的模型仍在对话里选。主模型看不了图时，发出去之前由这里的模型先把图转述成一段文字；Kind 找位置、读字、裁剪放大时也用它。云端的看图模型会收到你的图片。
      </p>

      <h3 className="mt-4 text-2 font-medium text-ink-2">看图模型</h3>
      <div className="mt-1.5 rounded-card border border-line" data-kind="vision-chain" data-count={chain.length}>
        {empty ? (
          <div className="flex items-center gap-3 border-b border-line px-4 py-3" data-kind="vision-empty">
            <Dot tone="warn" />
            <div className="min-w-0">
              <div className="text-3 text-ink">还没有看图模型</div>
              <div className="mt-0.5 text-1 leading-4 text-wait">
                现在没有看图能力：主模型看不了图时，图发不进去，Kind 会照实说它看不到。
              </div>
            </div>
          </div>
        ) : (
          chain.map((link, index) => (
            <ChainRow
              key={link.connection + '/' + link.model}
              view={view}
              link={link}
              index={index}
              count={chain.length}
              test={tests[link.connection + '/' + link.model] ?? { kind: 'idle' }}
              onTest={() => void test(link)}
              onMove={(delta) => {
                const next = [...chain];
                const target = index + delta;
                if (target < 0 || target >= next.length) return;
                [next[index], next[target]] = [next[target], next[index]];
                void save(next);
              }}
              onRemove={() => void save(chain.filter((_, at) => at !== index))}
            />
          ))
        )}
        <div className={cn('px-4 py-3', !empty && 'border-t border-line')} data-kind="vision-add">
          {addOptions.length > 0 ? (
            <div className="flex items-center gap-2">
              <select
                value={value}
                onChange={(event) => setPicked(event.target.value)}
                className={cn(inputClass, 'flex-1')}
                aria-label="选一个能看图的模型"
              >
                {addOptions.map(({ connection, row }) => (
                  <option key={connection.id + '/' + row.id} value={connection.id + '/' + row.id}>
                    {modelLabel(connection.name, row)}
                  </option>
                ))}
              </select>
              <Btn
                kind={empty ? 'primary' : 'plain'}
                className="inline-flex items-center gap-1"
                onClick={() => {
                  const pickedOption = addOptions.find(({ connection, row }) => connection.id + '/' + row.id === value);
                  if (!pickedOption) return;
                  const next = [...chain, { connection: pickedOption.connection.id, model: pickedOption.row.id }];
                  void save(next);
                  setPicked('');
                  toast(
                    (pickedOption.row.name ?? pickedOption.row.id) +
                      (empty ? ' 填好了：主模型看不了图时，由它转述' : ' 加在后面：前面的被限流时试它'),
                    { description: '点「测试看图」确认它真能看图' },
                  );
                }}
              >
                <Plus className="size-3.5" />
                {empty ? '添加看图模型' : '添加备用看图模型'}
              </Btn>
            </div>
          ) : (
            <div className="text-2 leading-5 text-ink-2">
              {empty ? '已接的模型里没有能看图的。' : '已接的、能看图的模型都加进来了。'}
            </div>
          )}
          <div className="mt-1.5 text-1 leading-4 text-ink-3">
            只列「输入」是 文字+图片 的模型（在提供商的「编辑 › 模型」里能改）。本机的 Ollama、LM Studio 先在上面「添加模型提供商 › 自定义模型 API」接上，再从这里选。
          </div>
        </div>
      </div>
      <p className="mt-1.5 text-1 leading-4 text-ink-3">
        按顺序试：前一个被限流就换下一个，它先歇 60 秒（服务商说了等多久就照它的）。这一轮都不行，Kind 说这一轮看不了图，不换说法重试。同一张图转述过一次就记住，不重复花钱。
      </p>

      <h3 className="mt-5 text-2 font-medium text-ink-2">本机读字</h3>
      <div className="mt-1.5 rounded-card border border-line">
        <OcrRow onChanged={onChanged} />
      </div>

      <h3 className="mt-5 text-2 font-medium text-ink-2">截桌面</h3>
      <div className="mt-1.5 rounded-card border border-line">
        <ScreenRow view={view} onChanged={onChanged} />
      </div>
    </section>
  );
}
