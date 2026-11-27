/**
 * 设置 › 插件（T094.2）：官方 Office 卡 + 内容包空态。
 *
 * 卡上写插件名、说明、开关、Skill 摘要、数据目录与「打开/关掉影响」；
 * 开关走 plugin_set，和插件页开关同一条路径（settings.plugins + 共享 PluginSwitches）。
 * 内容包导入第一刀没有实现，只留位置并明说还没有导入的包。
 */

import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { Switch } from '@/components/ui/switch';
import { pluginList, pluginSet, ServiceHttpError, type PluginView } from '@/data/service';

function reasonOf(error: unknown): string {
  if (error instanceof ServiceHttpError && error.detail) return error.detail;
  if (error instanceof Error && error.message) return error.message;
  return '连不上内核';
}

export function PluginsPane() {
  const [plugins, setPlugins] = useState<PluginView[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [savingId, setSavingId] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    void pluginList()
      .then((response) => {
        if (!live) return;
        setPlugins(response.plugins);
        setLoaded(true);
      })
      .catch((error: unknown) => {
        if (!live) return;
        setLoaded(true);
        toast('插件列表没读出来', { description: reasonOf(error) });
      });
    return () => {
      live = false;
    };
  }, []);

  async function toggle(plugin: PluginView, next: boolean) {
    setSavingId(plugin.id);
    try {
      const response = await pluginSet({ id: plugin.id, enabled: next });
      setPlugins(response.plugins);
      toast(next ? plugin.name + ' 已打开' : plugin.name + ' 已关掉', {
        description: next
          ? '下一轮起 Kind/任务能看到 Office 工具。'
          : 'Office 工具一律回「已关」；已生成的文件和数据保留。',
      });
    } catch (error) {
      toast(next ? '插件没打开' : '插件没关掉', { description: reasonOf(error) });
    } finally {
      setSavingId(null);
    }
  }

  if (!loaded) {
    return (
      <div className="mt-4 text-2 text-ink-3" data-kind="plugin-loading">
        正在读插件设置…
      </div>
    );
  }

  return (
    <div className="mt-4" data-kind="plugins-pane">
      {plugins.map((plugin) => (
        <div key={plugin.id} className="border-b border-line py-4" data-kind="plugin-card">
          <div className="flex items-start gap-4">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="text-3 text-ink">{plugin.name}</span>
                <span className="rounded-btn bg-hover px-1.5 py-0.5 text-1 text-ink-3">
                  官方插件
                </span>
              </div>
              <p className="mt-1 text-2 leading-5 text-ink-2">{plugin.description}</p>
            </div>
            <div className="shrink-0 pt-0.5">
              <button
                type="button"
                role="switch"
                aria-checked={plugin.enabled}
                data-kind="plugin-toggle"
                disabled={savingId === plugin.id}
                onClick={() => void toggle(plugin, !plugin.enabled)}
                className="inline-flex items-center gap-2"
              >
                <span className="text-2 text-ink-2" data-kind="plugin-state">
                  {plugin.enabled ? '已开启' : '已关闭'}
                </span>
                <Switch checked={plugin.enabled} />
              </button>
            </div>
          </div>

          <p className="mt-3 text-2 leading-5 text-ink-3" data-kind="plugin-effect">
            {plugin.enabled
              ? '打开影响：Kind 和任务能读 xlsx/docx/pptx，写任务能新建 xlsx；关掉后这些工具一律回「已关」，已生成的文件和数据保留。'
              : '打开影响：增加 office_read（Kind/任务）和 office_create（写任务，只建 xlsx）；不会跑进程，也不带运行时。'}
          </p>

          <dl className="mt-3 grid gap-1 text-2 leading-5">
            <div className="flex gap-2">
              <dt className="shrink-0 text-ink-3">Skill 摘要</dt>
              <dd className="text-ink-2">{plugin.skill_summary}</dd>
            </div>
            <div className="flex gap-2">
              <dt className="shrink-0 text-ink-3">数据目录</dt>
              <dd className="break-all font-mono text-1 text-ink-3">{plugin.data_dir}</dd>
            </div>
          </dl>
        </div>
      ))}

      {loaded && plugins.length === 0 ? (
        <p className="py-4 text-2 text-ink-3">还没有可用的官方插件。</p>
      ) : null}

      <div className="py-4" data-kind="content-packs-empty">
        <div className="text-3 text-ink">内容包</div>
        <p className="mt-1 text-2 leading-5 text-ink-3">
          还没有导入的包。内容包导入这一版没有实现，这里先留位置。
        </p>
      </div>
    </div>
  );
}
