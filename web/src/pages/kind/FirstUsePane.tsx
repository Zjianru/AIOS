/**
 * 首次使用的 Kind：对话面板（brief §四，用户 2026-10-27 晚定稿；10 篇第 2 节）。
 *
 * 两步都在对话里完成，不跳设置页：
 * 1) 接模型：选提供方、填密钥。密钥交给 Keyring（和「设置 › 模型与连接」同一套：
 *    同一份提供方目录、同一个保存和验证）；「已保存」和「连接可用」分两步显示；
 * 2) 接好后 Kind 说一句；起手卡由当前能力决定（不固定张数），每张对应一个真实能力，点一张它说明这次按哪档做；
 *    再可选问一句「你主要用我做什么」，按回答推荐内置流程和角色——只推荐、不自动采用。
 * 打开开发开关「首次使用（空库）」时显示，数据从 api 取，不 import 样例。
 * 原型约定：以 sk-bad 开头的密钥模拟被服务商拒绝。
 *
 * T017.4：连接卡从 Keyring 状态出信封，页面只把信封交给 <Card>；外观和文案不变。
 */

import { useEffect, useState } from 'react';
import { addConnection, checkConnection, getModelSettings, KERNEL_PROVIDERS } from '@/data/keyring';
import type { CatalogProvider, ConnectionView } from '@/data/keyring';
import { toast } from 'sonner';
import { Card } from './cards/Card';
import { connectionCardOf } from '@/data/connectionCard';
import type { ConnectionCardFact } from '@/data/connectionCard';
import type { CardHost } from './cards/model';

/** 首次使用页的事实：内置目录 + 选中的提供方 + Keyring 里已有的那个连接。 */
function connectionFactOf(
  catalog: CatalogProvider[],
  provider: string,
  conn: ConnectionView | null,
): ConnectionCardFact {
  return {
    id: conn?.id ?? 'first-use',
    name: conn?.name,
    providers: catalog.map((item) => ({ id: item.id, name: item.name })),
    provider: conn?.id ?? provider,
    ...(conn ? { key: conn.key, check: conn.check } : {}),
  };
}

export function FirstUsePane({ onConnected }: { onConnected: () => void }) {
  const [catalog, setCatalog] = useState<CatalogProvider[]>([]);
  const [provider, setProvider] = useState('');
  const [conn, setConn] = useState<ConnectionView | null>(null);

  useEffect(() => {
    let alive = true;
    void getModelSettings().then((view) => {
      if (!alive) return;
      // 只显示接上内核的部分时，目录里只有内核会说的（D36：DeepSeek）
      const items = view.catalog.filter((item) => KERNEL_PROVIDERS.has(item.id));
      setCatalog(items);
      setProvider((current) => current || items[0]?.id || '');
      // 重开页面时 Keyring 里可能已经有一个（暂时不可用的）连接：卡片照样从它出状态。
      const saved = view.connections.find((item) => KERNEL_PROVIDERS.has(item.id)) ?? null;
      setConn((current) => current ?? saved);
    });
    return () => {
      alive = false;
    };
  }, []);

  /** 保存走现有的 Keyring 保存（同设置页）；错误由页面提示，卡片只拿到成功与否。 */
  const saveConnection = async (providerId: string, key: string): Promise<string | null> => {
    const active = catalog.find((item) => item.id === providerId);
    if (!active) return null;
    try {
      const saved = await addConnection({
        source: 'catalog',
        id: active.id,
        name: active.name,
        protocol: active.protocol,
        endpoint: active.defaultEndpoint,
        models: active.defaultModels,
        key,
      });
      const next = saved.connections.find((item) => item.id === active.id) ?? null;
      setConn(next);
      return next?.id ?? null;
    } catch (error) {
      toast.error((error as Error).message);
      return null;
    }
  };

  /** 验证走现有的 Keyring 验证；结果换成新的连接状态，卡片跟着重出信封。 */
  const verifyConnection = async (connectionId: string): Promise<void> => {
    try {
      const checked = await checkConnection(connectionId);
      setConn(checked.connections.find((item) => item.id === connectionId) ?? null);
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  const host: CardHost = {
    answer: () => {},
    dismiss: () => {},
    saveConnection,
    verifyConnection,
    start: onConnected,
  };
  const envelope = connectionCardOf(connectionFactOf(catalog, provider, conn));

  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto bg-content" data-pane="conversation">
      <Card envelope={envelope} host={host} />
    </div>
  );
}
