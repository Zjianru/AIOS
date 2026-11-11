/**
 * 设置 › 执行：席位、全局暂停、默认角色的 bash 开关。
 *
 * 值来自 settings.json（settings-update 全空请求只读），保存走同一个入口；暂停/席位
 * 在服务端同时更新任务运行时的准入闸门，页面显示的就是生效值。席位只收 1–16（D66），
 * 0/17 和非法输入当场拒绝，不写文件；写盘失败保留旧值并说原因。
 * bash 是单独的一项大授权（D85）：默认关，只影响以后派的任务；开了以后写入范围管不住
 * bash，后果由用户承担，写任务的卡上照实标明。
 */

import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { Switch } from '@/components/ui/switch';
import { loadExecSettings, saveExecSettings, ServiceHttpError } from '@/data/service';

const MIN_SEATS = 1;
const MAX_SEATS = 16;

function reasonOf(error: unknown): string {
  if (error instanceof ServiceHttpError && error.detail) return error.detail;
  if (error instanceof Error && error.message) return error.message;
  return '连不上内核';
}

function Row({ title, hint, children }: { title: string; hint: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-6 border-b border-line py-4">
      <div className="min-w-0 flex-1">
        <div className="text-3 text-ink">{title}</div>
        <div className="mt-0.5 text-2 leading-5 text-ink-3">{hint}</div>
      </div>
      <div className="shrink-0 pt-0.5">{children}</div>
    </div>
  );
}

export function ExecSettings() {
  const [seats, setSeats] = useState(3);
  const [draft, setDraft] = useState('3');
  const [paused, setPaused] = useState(false);
  const [savingPause, setSavingPause] = useState(false);
  const [bash, setBash] = useState(false);
  const [savingBash, setSavingBash] = useState(false);

  useEffect(() => {
    let live = true;
    void loadExecSettings()
      .then((view) => {
        if (!live) return;
        setSeats(view.seats);
        setDraft(String(view.seats));
        setPaused(view.paused);
        setBash(view.bash);
      })
      .catch((error: unknown) => {
        if (live) toast('执行设置没读出来', { description: reasonOf(error) });
      });
    return () => {
      live = false;
    };
  }, []);

  async function commitSeats() {
    const value = Number(draft);
    if (!Number.isInteger(value) || value < MIN_SEATS || value > MAX_SEATS) {
      toast('席位要在 1–16 之间', {
        description: '填 1 到 16 之间的整数；0 和 17 存不进去',
      });
      setDraft(String(seats));
      return;
    }
    if (value === seats) return;
    try {
      const saved = await saveExecSettings({ seats: value });
      setSeats(saved.seats);
      setDraft(String(saved.seats));
    } catch (error) {
      toast('席位没保存', { description: reasonOf(error) });
      setDraft(String(seats));
    }
  }

  async function togglePause(next: boolean) {
    setSavingPause(true);
    try {
      const saved = await saveExecSettings({ paused: next });
      setPaused(saved.paused);
    } catch (error) {
      toast(next ? '暂停没打开' : '暂停没关掉', { description: reasonOf(error) });
    } finally {
      setSavingPause(false);
    }
  }

  async function toggleBash(next: boolean) {
    setSavingBash(true);
    try {
      const saved = await saveExecSettings({ bash: next });
      setBash(saved.bash);
    } catch (error) {
      toast(next ? 'bash 开关没打开' : 'bash 开关没关掉', { description: reasonOf(error) });
    } finally {
      setSavingBash(false);
    }
  }

  return (
    <div className="mt-4" data-kind="exec-settings">
      <Row
        title="同时跑几个任务（席位）"
        hint="默认 3 个，最多 16 个。多出来的排队；改小不掐断已经在跑的，只影响之后开跑的。"
      >
        <input
          type="number"
          min={MIN_SEATS}
          max={MAX_SEATS}
          inputMode="numeric"
          value={draft}
          disabled={savingPause}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={() => void commitSeats()}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              void commitSeats();
            }
          }}
          className="w-16 rounded-btn border border-line-2 bg-raised px-2 py-1 text-right text-3 text-ink outline-none focus:border-ink-3/50"
          data-kind="seats"
        />
      </Row>
      <Row
        title="全局暂停"
        hint="关上闸门：新派的任务排队，在跑的那一轮跑完；关掉就是开闸。要马上全停，到各个任务对话里立即停。"
      >
        <Switch
          checked={paused}
          disabled={savingPause}
          onCheckedChange={(next) => void togglePause(next)}
          data-kind="global-pause"
        />
      </Row>
      <Row
        title="默认角色可以跑命令（bash）"
        hint="默认关。打开以后，写任务的 ThinAgent 能跑任意命令，写入范围管不住 bash。改动和后果由你自己承担。只影响以后派的任务。"
      >
        <Switch
          checked={bash}
          disabled={savingBash}
          onCheckedChange={(next) => void toggleBash(next)}
          data-kind="bash-toggle"
        />
      </Row>
    </div>
  );
}
