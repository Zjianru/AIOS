/**
 * 卡片：v10-tables §2 的固定五格模板 + 封闭类型表（14 种）。
 * 模板：[图标] 类型 · 来自哪 ⌛时间 / 标题 / 主体 / [主操作][次操作]   去看 ›
 * 规则：一张卡最多 1 个主操作（实心）；办完收成一行「✓ 类型 · 结果 · 时间」；
 *       会改变东西的动作给「撤销」提示条；不在表里的事不出卡片。
 */

import { memo, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import type { CardBody, CardType, KindCard } from '@/data/types';
import { cn } from '@/lib/utils';

const TYPE_LABEL: Record<CardType, string> = {
  routeDraft: '路线草稿',
  task: '任务',
  askDecision: '等你决定',
  askApproval: '要你允许',
  askWeb: '要你去网页完成',
  artifact: '成果',
  memory: '记忆',
  wake: '唤醒',
  roleJoin: '角色加入',
  planChanged: '计划变了',
  milestone: '里程碑判断',
  assignRole: '指派角色',
  quotaAlert: '提醒线到了',
  problem: '出了问题',
};

/** 要你动手的卡有框有底色；告知类的平铺（visual-plan-v12.md §版式）。 */
const FRAMED: ReadonlySet<CardType> = new Set<CardType>([
  'routeDraft',
  'askDecision',
  'askApproval',
  'askWeb',
  'artifact',
  'milestone',
  'assignRole',
]);

const TONE: Record<CardType, string> = {
  routeDraft: 'text-line-w',
  task: 'text-run',
  askDecision: 'text-wait',
  askApproval: 'text-wait',
  askWeb: 'text-wait',
  artifact: 'text-done',
  memory: 'text-ink-3',
  wake: 'text-line-n',
  roleJoin: 'text-ink-3',
  planChanged: 'text-ink-3',
  milestone: 'text-wait',
  assignRole: 'text-ink-3',
  quotaAlert: 'text-wait',
  problem: 'text-fail',
};

function handleAction(card: KindCard, label: string, mutates: boolean | undefined): void {
  if (mutates) {
    toast('已' + label, {
      description: card.title,
      action: { label: '撤销', onClick: () => toast('已撤销') },
    });
    return;
  }
  toast(label, { description: card.title });
}

/**
 * 成果卡的主体：默认收成一行「改了什么 N 处 · 要求 x/y 满足」，点开看明细
 * （brief §四.3 信息分层：卡片里那两段默认收成一行）。
 */
function ArtifactBody({ body }: { body: Extract<CardBody, { kind: 'artifact' }> }) {
  const [open, setOpen] = useState(false);
  const met = body.satisfied.filter((item) => item.ok).length;
  return (
    <div className="mt-2">
      <div className="flex items-baseline gap-2 text-3">
        <span className="text-ink">{body.artifact}</span>
        <span className="brand-type text-ink-3">{body.version}</span>
      </div>
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="mt-1 flex items-center gap-1.5 text-1 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
        改了什么 {body.changes.length} 处 · 要求{' '}
        <span className="brand-type">
          {met}/{body.satisfied.length}
        </span>{' '}
        满足
      </button>
      {open ? (
        <div className="mt-1 space-y-1">
          <ul className="space-y-0.5 text-2 text-ink-2">
            {body.changes.map((change) => (
              <li key={change}>· {change}</li>
            ))}
          </ul>
          <ul className="space-y-0.5 text-2">
            {body.satisfied.map((item) => (
              <li key={item.text} className="flex gap-1.5">
                <span className={item.ok ? 'text-done' : 'text-wait'}>{item.ok ? '✓' : '◐'}</span>
                <span className="text-ink-2">{item.text}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

function Origin({ card }: { card: KindCard }) {
  const { origin } = card;
  if (!origin.projectName) return <span>Kind 的对话</span>;
  return (
    <span className="flex min-w-0 items-center gap-1">
      <span
        className="grid size-[14px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white"
        style={{ background: 'var(--line-w)' }}
      >
        {origin.projectCode}
      </span>
      <span className="truncate">{origin.taskTitle ? origin.projectName + ' › ' + origin.taskTitle : origin.projectName}</span>
    </span>
  );
}

function Body({ body }: { body: CardBody }) {
  switch (body.kind) {
    case 'routeDraft':
      return (
        <div className="mt-2">
          <div className="flex flex-wrap gap-1.5">
            {body.stations.map((station) => (
              <span key={station.title} className="rounded-lg bg-pane px-2 py-1 text-2 text-ink-2">
                <b className="font-medium text-ink">{station.title}</b>
                <span className="mx-1 text-ink-3">·</span>
                {station.role}
                <span className="mx-1 text-ink-3">·</span>
                {station.what}
              </span>
            ))}
          </div>
          <dl className="mt-2 grid grid-cols-[52px_1fr] gap-x-2 gap-y-1 text-2">
            <dt className="text-ink-3">完成标准</dt>
            <dd className="text-ink-2">{body.criterion}</dd>
            <dt className="text-ink-3">开工前估算</dt>
            <dd className="est text-ink-3" title="没拿到实际数据，按同类站点的平均用时估算">
              {body.estimate}
            </dd>
          </dl>
        </div>
      );
    case 'task':
      return (
        <div className="mt-1.5 text-2 text-ink-2">
          <span className="brand-type text-ink">{body.progress}</span>
          <span className="mx-1.5 text-ink-3">·</span>
          正在做：{body.running}
        </div>
      );
    case 'askDecision':
      return (
        <div className="mt-2">
          <div className="mb-1 text-1 text-ink-3">Kind 给的选项 · 点一行就是选它</div>
          <div className="flex flex-col gap-1">
            {body.options.map((option) => (
              <button
                key={option.label}
                type="button"
                onClick={() => toast('你选了：' + option.label)}
                className="flex items-start gap-2 rounded-lg border border-line px-2.5 py-1.5 text-left transition-colors hover:bg-hover"
              >
                <span className="mt-[3px] size-3 shrink-0 rounded-full border border-ink-3" />
                <span className="min-w-0">
                  <span className="block text-3 text-ink">{option.label}</span>
                  <span className="block text-2 text-ink-3">{option.description}</span>
                </span>
              </button>
            ))}
            <button
              type="button"
              className="rounded-lg border border-dashed border-line-2 px-2.5 py-1.5 text-left text-3 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
            >
              我自己说…
            </button>
          </div>
        </div>
      );
    case 'askApproval':
      return (
        <div className="mt-2">
          <div className="rounded-lg bg-pane px-2.5 py-1.5">
            <code className="block break-all font-mono text-2 text-ink">{body.command}</code>
          </div>
          <div className="mt-1.5 text-2 text-ink-3">{body.why}</div>
        </div>
      );
    case 'askWeb':
      return (
        <div className="mt-2 text-2">
          <div className="text-ink-2">{body.why}</div>
          <div className="mt-1 font-mono text-2 text-ink-3">{body.url}</div>
        </div>
      );
    case 'artifact':
      return <ArtifactBody body={body} />;
    case 'memory':
      return (
        <div className="mt-1.5">
          <div className="text-3 text-ink">{body.note}</div>
          <div className="mt-1 text-1 text-ink-3">{body.noteKind}</div>
        </div>
      );
    case 'wake':
      return (
        <div className="mt-1.5 text-2 text-ink-2">
          <span className="brand-type text-ink">{body.when}</span>
          <span className="mx-1.5 text-ink-3">·</span>
          {body.todo}
        </div>
      );
    case 'roleJoin':
    case 'assignRole':
      return (
        <div className="mt-1.5 text-2 text-ink-2">
          <span className="text-ink">{body.role}</span>
          <span className="mx-1.5 text-ink-3">·</span>
          {body.why}
        </div>
      );
    case 'planChanged':
      return (
        <div className="mt-2 grid gap-1 text-2">
          <div className="text-ink-3">
            原来：<span className="text-ink-2">{body.before.join(' → ')}</span>
          </div>
          <div className="text-ink-3">
            现在：<span className="text-ink">{body.after.join(' ∥ ')}</span>
          </div>
          <div className="text-ink-3">{body.why}</div>
        </div>
      );
    case 'milestone':
      return (
        <ul className="mt-2 space-y-0.5 text-2">
          {body.criteria.map((item) => (
            <li key={item.text} className="flex gap-1.5">
              <span className={item.ok ? 'text-done' : 'text-wait'}>{item.ok ? '✓' : '◐'}</span>
              <span className="text-ink-2">{item.text}</span>
            </li>
          ))}
        </ul>
      );
    case 'quotaAlert':
      return (
        <div className="mt-1.5 text-2 text-ink-2">
          <div className="text-ink">{body.line}</div>
          <div className="mt-0.5 brand-type text-ink-3">{body.used}</div>
        </div>
      );
    case 'problem':
      return (
        <div className="mt-1.5 text-2 text-ink-2">
          <div>{body.what}</div>
          <div className="mt-1 text-1 text-ink-3">Kind 的处理：{body.handled}</div>
        </div>
      );
    default:
      return null;
  }
}

function DoneLine({ card }: { card: KindCard }) {
  return (
    <div className="mt-1.5 flex items-center gap-1.5 text-2 text-ink-3">
      <span className="text-done">✓</span>
      <span>
        {TYPE_LABEL[card.type]} · {card.done?.line}
      </span>
      <span className="brand-type opacity-70">{card.done?.at}</span>
    </div>
  );
}

export const KindCardView = memo(function KindCardView({ card }: { card: KindCard }) {
  if (card.done) return <DoneLine card={card} />;
  const framed = FRAMED.has(card.type);
  return (
    <div
      className={cn(
        'mt-2.5',
        framed ? 'rounded-card border border-line-2 bg-raised px-3.5 py-2.5' : 'px-0.5 py-0',
      )}
    >
      <div className="flex min-w-0 items-center gap-1.5 text-1 text-ink-3">
        <span className={cn('shrink-0', TONE[card.type])}>{TYPE_LABEL[card.type]}</span>
        <span className="shrink-0 text-ink-3/60">·</span>
        <Origin card={card} />
        <span className="ml-auto shrink-0 brand-type">{card.time}</span>
      </div>
      <div className="mt-1 text-4 font-medium leading-6 text-ink">{card.title}</div>
      {card.body ? <Body body={card.body} /> : null}
      <div className="mt-2.5 flex flex-wrap items-center gap-1.5">
        {card.actions.map((action) => (
          <button
            key={action.label}
            type="button"
            onClick={() => handleAction(card, action.label, action.mutates)}
            className={cn(
              'h-7 rounded-btn px-2.5 text-2 transition-colors',
              action.primary
                ? 'bg-ink text-content hover:opacity-90'
                : 'border border-line-2 text-ink-2 hover:bg-hover hover:text-ink',
            )}
          >
            {action.label}
          </button>
        ))}
        {card.origin.go && !card.actions.some((action) => action.label.includes('去看')) ? (
          <button
            type="button"
            onClick={() => go(card.origin.go!.replace(/^#/, ''))}
            className="ml-auto text-2 text-ink-3 transition-colors hover:text-ink"
          >
            去看 ›
          </button>
        ) : null}
      </div>
    </div>
  );
});
