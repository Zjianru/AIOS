import { useEffect, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { listAbilities, listBuiltinFlows, listFlowIndex } from '@/data/api';
import type { AbilityFixture, AbilityStatus, BuiltinFlow, FlowIndexEntry } from '@/data/fixtures/abilities';
import { FlowEditor } from './FlowEditor';
import { cn } from '@/lib/utils';

/** 能力场景（v13 的「能力」页 + 第 1 期新增的「流程」分组）。 */
const STATUS_TEXT: Record<AbilityStatus, string> = {
  ok: '可用',
  needAuth: '需要授权',
  needKey: '缺 API key',
  needDep: '缺依赖',
};

const STATUS_ACTION: Record<AbilityStatus, string | null> = {
  ok: null,
  needAuth: '去授权',
  needKey: '填写 key',
  needDep: '安装依赖',
};

const STATUS_TONE: Record<AbilityStatus, string> = {
  ok: 'bg-done',
  needAuth: 'bg-wait',
  needKey: 'bg-wait',
  needDep: 'bg-wait',
};

function AbilityRow({ ability }: { ability: AbilityFixture }) {
  const action = STATUS_ACTION[ability.status];
  return (
    <div className="flex items-center gap-3 border-b border-line px-4 py-2.5 last:border-b-0">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-3 text-ink">{ability.name}</span>
          <span className="shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3">{ability.type}</span>
          <span className="shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3">{ability.source}</span>
        </div>
        <div className="mt-0.5 truncate text-1 text-ink-3">配备：{ability.equippedBy.join('、')}</div>
      </div>
      <div className="flex shrink-0 items-center gap-2 text-2 text-ink-3">
        <i className={cn('size-1.5 rounded-full', STATUS_TONE[ability.status])} />
        {STATUS_TEXT[ability.status]}
        {action ? (
          <button type="button" title="第 3 期" className="rounded-btn border border-line-2 px-2 py-[2px] text-2 text-ink-2">
            {action}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function FlowRow({ flow }: { flow: FlowIndexEntry }) {
  return (
    <button
      type="button"
      onClick={() => go('flow:' + flow.id)}
      className="flex w-full items-center gap-3 border-b border-line px-4 py-2.5 text-left transition-colors last:border-b-0 hover:bg-hover"
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-3 text-ink">{flow.name}</span>
          <span className="shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3">{flow.version}</span>
          <span className="shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3">{flow.source}</span>
        </div>
        <div className="mt-0.5 truncate text-1 text-ink-3">
          用过 {flow.usedTimes} 次
          {flow.projectName ? ' · 最近用在 ' + flow.projectName : ''}
        </div>
      </div>
      <span className="flex shrink-0 items-center gap-1 text-2 text-ink-3">
        打开
        <ChevronRight className="size-3.5" />
      </span>
    </button>
  );
}

function BuiltinFlowCard({ flow }: { flow: BuiltinFlow }) {
  return (
    <div className="rounded-card border border-line bg-pane/40 p-3">
      <div className="flex items-center gap-2">
        <span className="rounded-full bg-hover px-2 py-[1px] text-1 text-ink-2">{flow.tier}</span>
        <span className="truncate text-3 text-ink">{flow.name}</span>
      </div>
      <p className="mt-1 text-1 leading-4 text-ink-3">{flow.description}</p>
      <div className="mt-1.5 flex flex-wrap items-center gap-1 text-1 text-ink-2">
        {flow.steps.map((step, index) => (
          <span key={step} className="flex items-center gap-1">
            <span>{step}</span>
            {index < flow.steps.length - 1 ? <span className="text-ink-3">→</span> : null}
          </span>
        ))}
      </div>
      <div className="mt-2 flex items-center gap-2">
        <span className="rounded-btn border border-line px-2 py-[2px] text-1 text-ink-3" title="内置流程只读">
          只读
        </span>
        <button
          type="button"
          onClick={() => toast('（原型）已复制成你的流程；副本会出现在上面的「流程」里')}
          className="rounded-btn border border-line-2 px-2 py-[2px] text-1 text-ink-2 transition-colors hover:bg-hover"
        >
          复制成自己的
        </button>
      </div>
    </div>
  );
}

function AbilityLibrary() {
  const [abilities, setAbilities] = useState<AbilityFixture[] | null>(null);
  const [flows, setFlows] = useState<FlowIndexEntry[] | null>(null);
  const [builtin, setBuiltin] = useState<BuiltinFlow[] | null>(null);

  useEffect(() => {
    let alive = true;
    void (async () => {
      const [abilityList, flowList, builtinList] = await Promise.all([listAbilities(), listFlowIndex(), listBuiltinFlows()]);
      if (!alive) return;
      setAbilities(abilityList);
      setFlows(flowList);
      setBuiltin(builtinList);
    })();
    return () => {
      alive = false;
    };
  }, []);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto w-full max-w-[1000px] px-10 py-8">
        <h1 className="text-6 font-medium leading-8 text-ink">能力</h1>
        <p className="mt-2 max-w-[620px] text-3 leading-5 text-ink-2">
          技能、工具、连接与组合工具都在这里。角色可以配备能力；需要处理的状态旁边都有按钮。
        </p>

        <section className="mt-4" data-builtin-flows="true">
          <h2 className="flex items-center gap-2 text-3 font-medium text-ink">
            <ChevronRight className="size-3.5 rotate-90 text-ink-3" />
            内置流程
            <span className="brand-type text-2 font-normal text-ink-3">{builtin ? builtin.length : 0}</span>
            <span className="ml-1 text-1 font-normal text-ink-3">
              三档：轻档 / 普通档 / 配对档 · 只读，可复制成自己的；新建流程 → 模板里也有
            </span>
          </h2>
          <div className="mt-2 grid gap-2 sm:grid-cols-3">
            {(builtin ?? []).map((flow) => (
              <BuiltinFlowCard key={flow.id} flow={flow} />
            ))}
          </div>
        </section>

        <section className="mt-4" data-flow-group="true">
          <h2 className="flex items-center gap-2 text-3 font-medium text-ink">
            <ChevronRight className="size-3.5 rotate-90 text-ink-3" />
            流程
            <span className="brand-type text-2 font-normal text-ink-3">{flows ? flows.length : 0}</span>
            <span className="ml-1 text-1 font-normal text-ink-3">一类事固定怎么做：谁做哪一步、哪里设门、每站交出什么 · 点一条打开编辑器</span>
          </h2>
          <div className="mt-2 overflow-hidden rounded-card border border-line bg-pane/40">
            {(flows ?? []).map((flow) => (
              <FlowRow key={flow.id} flow={flow} />
            ))}
            {flows && flows.length === 0 ? <div className="px-4 py-3 text-2 text-ink-3">还没有流程。</div> : null}
          </div>
        </section>
        {abilityGroupsOf(abilities).map((group) => (
          <section key={group.key} className="mt-6">
            <h2 className="flex items-center gap-2 text-3 font-medium text-ink">
              <ChevronRight className="size-3.5 rotate-90 text-ink-3" />
              {group.name}
              <span className="brand-type text-2 font-normal text-ink-3">{group.items.length}</span>
            </h2>
            <div className="mt-2 overflow-hidden rounded-card border border-line bg-pane/40">
              {group.items.map((ability) => (
                <AbilityRow key={ability.id} ability={ability} />
              ))}
              {group.items.length === 0 ? <div className="px-4 py-3 text-2 text-ink-3">正在取…</div> : null}
            </div>
          </section>
        ))}

      </div>
    </div>
  );
}

function abilityGroupsOf(abilities: AbilityFixture[] | null) {
  const list = abilities ?? [];
  return [
    { key: 'daily', name: '日常', items: list.filter((ability) => ability.group === 'daily') },
    { key: 'pro', name: '专业', items: list.filter((ability) => ability.group === 'pro') },
  ];
}

export function AbilityScene() {
  const route = useRouteStore((state) => state.route);
  if (route.kind === 'flow') return <FlowEditor key={route.id} flowId={route.id} />;
  return <AbilityLibrary />;
}
