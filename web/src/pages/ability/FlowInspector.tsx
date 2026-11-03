import { useState } from 'react';
import type { ReactNode } from 'react';
import { ChevronDown, Sparkles, X } from 'lucide-react';
import type {
  Flow,
  FlowDod,
  FlowInstance,
  FlowNode,
  FlowNodeKind,
} from '@/data/flows';
import { JOIN_OPTIONS, NODE_KIND_LABEL, ROLE_OPTIONS } from '@/data/flows';
import { cn } from '@/lib/utils';
import { isShared } from './flowGraph';

type TagField = 'who' | 'members' | 'skills';

export interface FlowInspectorProps {
  flow: Flow;
  selectedIds: string[];
  preview: boolean;
  readOnly: boolean;
  archived: boolean;
  instance: FlowInstance | null;
  onPatchFlowName: (name: string) => void;
  onPointToKind: () => void;
  onClose: () => void;
  onSelect: (id: string | null) => void;
  onPatchNode: (id: string, patch: Partial<FlowNode>) => void;
  onSetNodeKind: (id: string, kind: FlowNodeKind) => void;
  onAddToArray: (id: string, field: TagField, value: string) => void;
  onRemoveFromArray: (id: string, field: TagField, index: number) => void;
  onAddDod: (id: string) => void;
  onPatchDod: (id: string, index: number, patch: Partial<FlowDod>) => void;
  onRemoveDod: (id: string, index: number) => void;
  onDeleteNode: (id: string) => void;
  onDeleteNodes: (ids: string[]) => void;
}

const fieldBase =
  'w-full rounded-btn border border-line-2 bg-raised px-2 text-2 text-ink outline-none transition-colors placeholder:text-ink-3 focus:border-run disabled:cursor-not-allowed disabled:opacity-60';
const labelBase = 'mb-1 mt-3 block text-1 font-semibold text-ink-3';
const selectBase =
  'h-7 w-full rounded-btn border border-line-2 bg-raised px-1.5 text-2 text-ink outline-none focus:border-run disabled:cursor-not-allowed disabled:opacity-60';

const KIND_SEGMENTS: { kind: FlowNodeKind; label: string }[] = [
  { kind: 'station', label: '站' },
  { kind: 'stack', label: '逐条展开' },
  { kind: 'gate', label: '门' },
  { kind: 'pool', label: '池' },
];

const DIFF_FIELD_LABEL: Record<string, string> = {
  name: '名称',
  who: '谁来做',
  give: '交出',
  recv: '收到',
  dod: '完成',
  rhythm: '节奏',
  members: '成员',
  user: '等你',
  ext: '对外',
  loop: '循环',
  rule: '算过',
  fail: '没过',
  note: '说明',
  fan: '逐条展开',
  join: '怎么汇合',
};

function lineBadge(flow: Flow, lineId: string) {
  const line = flow.lines.find((item) => item.id === lineId);
  if (!line) return null;
  return (
    <span
      key={lineId}
      className="inline-grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-bold leading-none text-white"
      style={{ background: line.color }}
      title={line.name}
    >
      {line.index}
    </span>
  );
}

function versionOf(flow: Flow): string {
  return flow.draft ? '草稿' : 'v' + flow.version;
}

function inUse(flow: Flow): number {
  return flow.instances.filter((instance) => instance.state !== 'done').length;
}

function showValue(value: unknown): string {
  if (Array.isArray(value)) {
    return value
      .map((item) => {
        if (Array.isArray(item)) return String(item[0] ?? '');
        if (item && typeof item === 'object' && 'text' in item) return String((item as FlowDod).text);
        return String(item);
      })
      .join('、');
  }
  if (value && typeof value === 'object') {
    const loop = value as { max?: number | null; until?: string };
    return '上限 ' + (loop.max ?? '?') + ' 次' + (loop.until ? '，直到' + loop.until : '');
  }
  if (value === true) return '是';
  if (value === false) return '否';
  if (value === null || value === undefined || value === '') return '（空）';
  return String(value);
}

function InputRow(props: {
  label: string;
  value: string;
  placeholder?: string;
  multiline?: boolean;
  bad?: boolean;
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  const { label, value, placeholder, multiline, bad, disabled, onChange } = props;
  return (
    <label className="block">
      <span className={labelBase}>{label}</span>
      {multiline ? (
        <textarea
          className={cn(fieldBase, 'min-h-[50px] resize-y py-1 leading-5', bad && 'border-fail')}
          rows={2}
          value={value}
          placeholder={placeholder}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
        />
      ) : (
        <input
          className={cn(fieldBase, 'h-7', bad && 'border-fail')}
          value={value}
          placeholder={placeholder}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
        />
      )}
    </label>
  );
}

function TagList(props: {
  values: string[];
  disabled?: boolean;
  onRemove: (index: number) => void;
}) {
  const { values, disabled, onRemove } = props;
  if (!values.length) return <span className="text-2 text-ink-3">（空）</span>;
  return (
    <span className="flex flex-wrap items-center gap-1">
      {values.map((value, index) => (
        <span key={value + index} className="inline-flex h-6 items-center gap-0.5 rounded-btn bg-hover pl-2 pr-0.5 text-2 text-ink">
          {value}
          <button
            type="button"
            onClick={() => onRemove(index)}
            disabled={disabled}
            className="grid size-4 place-items-center rounded-[3px] text-ink-3 hover:bg-line-2 hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
            aria-label={'去掉' + value}
          >
            <X className="size-2.5" />
          </button>
        </span>
      ))}
    </span>
  );
}

function RoleSelect(props: {
  have: string[];
  keyName: string;
  disabled?: boolean;
  onAdd: (value: string) => void;
}) {
  const { have, keyName, disabled, onAdd } = props;
  return (
    <select
      className="e-add h-6 rounded-btn border border-dashed border-line-2 bg-transparent px-1 text-1 text-ink-3 outline-none hover:text-ink focus:border-run"
      value=""
      disabled={disabled}
      aria-label={'加一位' + keyName}
      onChange={(event) => {
        const value = event.target.value;
        event.currentTarget.value = '';
        if (value) onAdd(value);
      }}
    >
      <option value="">＋ 加</option>
      {ROLE_OPTIONS.filter((role) => !have.includes(role)).map((role) => (
        <option key={role} value={role}>
          {role}
        </option>
      ))}
    </select>
  );
}

function VersionList({ flow }: { flow: Flow }) {
  const byVersion = new Map<number, number>();
  for (const instance of flow.instances) {
    if (instance.state === 'done') continue;
    byVersion.set(instance.version, (byVersion.get(instance.version) ?? 0) + 1);
  }
  if (!flow.versions.length) return <p className="text-2 text-ink-3">还没保存过。</p>;
  return (
    <ul className="flex flex-col gap-0.5">
      {flow.versions.map((version) => (
        <li key={version.version} className="grid grid-cols-[34px_36px_1fr] items-baseline gap-1.5 py-0.5 text-2 text-ink-2">
          <b className="brand-type text-2 font-bold text-ink">v{version.version}</b>
          <span className="brand-type text-1 text-ink-3">{version.at}</span>
          <span className="min-w-0">
            {version.note}
            {byVersion.get(version.version) ? (
              <span className="ml-1.5 text-1 text-ink-3">· 在用 {byVersion.get(version.version)}</span>
            ) : null}
          </span>
        </li>
      ))}
    </ul>
  );
}

function Overview({ flow, preview, readOnly, instance, onPatchFlowName }: {
  flow: Flow;
  preview: boolean;
  readOnly: boolean;
  instance: FlowInstance | null;
  onPatchFlowName: (name: string) => void;
}) {
  const locked = preview || readOnly;
  return (
    <div>
      <div className="flex items-center gap-1">
        <span className="text-1 text-ink-3">流程</span>
        <span className="ml-auto rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2">
          {versionOf(flow)}
        </span>
      </div>
      <input
        className="-mx-1 mt-1 w-full rounded-btn border border-transparent bg-transparent px-1 py-0.5 text-5 font-semibold text-ink outline-none hover:border-line-2 focus:border-run focus:bg-raised disabled:cursor-not-allowed"
        value={flow.name}
        aria-label="流程名"
        disabled={locked}
        onChange={(event) => onPatchFlowName(event.target.value)}
      />
      <p className="mt-2 text-2 text-ink-3">{flow.source}</p>

      {instance ? (
        <div className="mt-3 rounded-card border border-line bg-pane px-2.5 py-2">
          <div className="flex items-center gap-1.5">
            <span className={cn('size-1.5 rounded-full', instance.state === 'run' ? 'bg-run' : instance.state === 'wait' ? 'bg-wait' : 'bg-done')} />
            <span className="text-2 font-medium text-ink">{instance.name}</span>
            <span className="ml-auto rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2">v{instance.version}</span>
          </div>
          <div className="mt-1 text-2 text-ink-2">
            {instance.stateText}
            {instance.project ? ' · ' + instance.project.name : ''}
          </div>
          <div className="mt-1 text-1 text-ink-3">
            {instance.overrides.length ? '本地改了 ' + instance.overrides.length + ' 处' : '照流程跑，没改'}
          </div>
        </div>
      ) : null}

      {preview ? (
        <div className="mt-3 rounded-card border border-run/40 bg-run/[0.06] px-2.5 py-2 text-2 text-ink-2">
          正在看修改提议：绿 = 新加，红 = 去掉，蓝 = 改了。接受或不要之后再改。
        </div>
      ) : readOnly ? (
        <div className="mt-3 rounded-card border border-line bg-pane px-2.5 py-2 text-2 text-ink-2">
          这个流程现在只读；回到「流程」或恢复后再改。
        </div>
      ) : (
        <div className="mt-3 space-y-1 text-2 text-ink-2">
          <p>拖零件到线上，能放的位置会亮；点一站改它，也可以指着一站跟 Kind 说。</p>
          <p>保存时先检查：有要补的就存不了，每条都有一键补法。</p>
        </div>
      )}

      <div className={labelBase}>价值流</div>
      <div className="flex flex-col gap-1">
        {flow.lines.map((line) => (
          <div key={line.id} className="flex items-center gap-1.5 text-2 text-ink-2">
            <span
              className="inline-grid size-[15px] place-items-center rounded-badge brand-type text-[9px] font-bold text-white"
              style={{ background: line.color }}
            >
              {line.index}
            </span>
            <span className="text-ink">{line.name}</span>
            <span className="ml-auto text-1 text-ink-3">{line.what}</span>
          </div>
        ))}
      </div>

      <div className={labelBase}>版本</div>
      <VersionList flow={flow} />
      <div className="mt-3 text-1 leading-4 text-ink-3">
        {inUse(flow)} 个实例在用 · 版本固定，升级要逐个点
      </div>
    </div>
  );
}

function DiffDetail({ flow, node }: { flow: Flow; node: FlowNode }) {
  const diff = node.diff;
  const shared = isShared(flow, node);
  return (
    <div>
      <div className="flex items-center gap-1">
        <span className="text-1 text-ink-3">{NODE_KIND_LABEL[node.kind]}{shared ? '  多条线共用' : ''}</span>
        <span className="ml-auto text-1 text-ink-3">只读</span>
      </div>
      <div className="mt-1 text-5 font-semibold text-ink">{node.name}</div>
      {diff === 'add' ? (
        <div className="mt-2 rounded-card border border-done/40 bg-done/[0.06] px-2.5 py-2 text-2 text-ink-2">
          <b className="text-done">新加的一站</b>
        </div>
      ) : null}
      {diff === 'del' ? (
        <div className="mt-2 rounded-card border border-fail/40 bg-fail/[0.06] px-2.5 py-2 text-2 text-ink-2">
          <b className="text-fail">这一站会去掉</b>，前后两站直接接上。
        </div>
      ) : null}
      {diff === 'chg' ? (
        <div className="mt-2 rounded-card border border-run/40 bg-run/[0.06] px-2.5 py-2 text-2">
          <b className="text-run">改了这些</b>
          <dl className="mt-1 grid grid-cols-[44px_1fr] gap-x-2 gap-y-1 text-2">
            {Object.entries(node.diffOld ?? {}).map(([key, oldValue]) => {
              let next: unknown = '';
              if (key === 'name') next = node.name;
              else if (key === 'who') next = node.who ?? [];
              else if (key === 'give') next = node.give ?? '';
              else if (key === 'recv') next = node.recv ?? '';
              else if (key === 'dod') next = node.dod ?? [];
              else if (key === 'rhythm') next = node.rhythm ?? '';
              else if (key === 'members') next = node.members ?? [];
              else if (key === 'rule') next = node.rule ?? '';
              else if (key === 'fail') next = node.fail ?? '';
              else if (key === 'loop') next = node.loop ?? null;
              else if (key === 'user') next = node.user ?? false;
              else next = '';
              return (
                <div key={key} className="contents">
                  <dt className="text-ink-3">{DIFF_FIELD_LABEL[key] ?? key}</dt>
                  <dd className="min-w-0 text-ink-2">
                    <s className="text-fail">{showValue(oldValue)}</s>
                    <br />
                    <span className="text-run">{showValue(next)}</span>
                  </dd>
                </div>
              );
            })}
          </dl>
        </div>
      ) : null}

      <div className={labelBase}>交接</div>
      <dl className="grid grid-cols-[34px_1fr] gap-x-2 gap-y-1 text-2">
        <dt className="text-ink-3">收到</dt>
        <dd className="text-ink-2">{node.recv || '（空）'}</dd>
        <dt className="text-ink-3">交出</dt>
        <dd className="text-ink-2">{node.give || '（空）'}</dd>
      </dl>
      {node.dod?.length ? (
        <>
          <div className={labelBase}>完成标准</div>
          <ul className="space-y-0.5 text-2 text-ink-2">
            {node.dod.map((item) => (
              <li key={item.text}>{item.text}</li>
            ))}
          </ul>
        </>
      ) : null}
      <p className="mt-4 text-1 text-ink-3">看修改时不能编辑；接受或不要之后再改。</p>
    </div>
  );
}

function Detail(props: {
  flow: Flow;
  node: FlowNode;
  readOnly: boolean;
  archived: boolean;
  onPointToKind: () => void;
  onClose: () => void;
  onPatchNode: (id: string, patch: Partial<FlowNode>) => void;
  onSetNodeKind: (id: string, kind: FlowNodeKind) => void;
  onAddToArray: (id: string, field: TagField, value: string) => void;
  onRemoveFromArray: (id: string, field: TagField, index: number) => void;
  onAddDod: (id: string) => void;
  onPatchDod: (id: string, index: number, patch: Partial<FlowDod>) => void;
  onRemoveDod: (id: string, index: number) => void;
  onDeleteNode: (id: string) => void;
}) {
  const { flow, node, readOnly, archived } = props;
  const [skillDraft, setSkillDraft] = useState('');
  const shared = isShared(flow, node);
  const kindLabel = NODE_KIND_LABEL[node.kind];
  const supportsType = node.kind === 'station' || node.kind === 'stack' || node.kind === 'gate' || node.kind === 'pool';
  const showHandoff = node.kind === 'pool' || node.kind === 'gate' || node.kind === 'station' || node.kind === 'stack';
  const showWorks = node.kind === 'gate' || node.kind === 'station' || node.kind === 'stack';

  const patch = (value: Partial<FlowNode>) => props.onPatchNode(node.id, value);
  const addSkill = () => {
    const value = skillDraft.trim();
    if (!value) return;
    props.onAddToArray(node.id, 'skills', value);
    setSkillDraft('');
  };

  return (
    <div>
      <div className="flex items-center gap-1">
        <span className="text-1 text-ink-3">{kindLabel}{shared ? '  多条线共用' : ''}</span>
        {!readOnly ? (
          <button
            type="button"
            onClick={props.onPointToKind}
            className="ml-auto inline-flex h-6 items-center gap-1 rounded-btn px-1.5 text-1 text-run hover:bg-run/10"
          >
            <Sparkles className="size-3" />
            指给 Kind
          </button>
        ) : null}
        <button
          type="button"
          onClick={props.onClose}
          className={cn(
            'grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink',
            readOnly ? 'ml-auto' : '',
          )}
          aria-label="关上详情"
        >
          <X className="size-4" />
        </button>
      </div>

      {readOnly ? (
        <p className="mt-1 rounded-btn bg-hover px-2 py-1 text-1 text-ink-3">
          {archived ? '已归档只读；恢复后才能改。' : '实例态只读；回到「流程」再改。'}
        </p>
      ) : null}

      <input
        className="-mx-1 mt-1 w-full rounded-btn border border-transparent bg-transparent px-1 py-0.5 text-5 font-semibold text-ink outline-none hover:border-line-2 focus:border-run focus:bg-raised disabled:cursor-not-allowed"
        value={node.name}
        aria-label="这一站的名字"
        disabled={readOnly}
        onChange={(event) => patch({ name: event.target.value })}
      />
      <div className="mt-1 flex flex-wrap items-center gap-1.5">
        {node.lines.map((lineId) => lineBadge(flow, lineId))}
        {node.user ? <span className="rounded-badge bg-wait-soft px-1.5 py-0.5 text-1 font-medium text-wait">每次等你</span> : null}
        {node.ext ? <span className="rounded-badge bg-fail-soft px-1.5 py-0.5 text-1 font-medium text-fail">对外</span> : null}
        {node.loop ? <span className="rounded-badge bg-hover px-1.5 py-0.5 brand-type text-1 text-ink-2">≤{node.loop.max ?? '?'} 次</span> : null}
      </div>

      {supportsType ? (
        <fieldset className="mt-2 flex rounded-btn bg-hover p-0.5" disabled={readOnly}>
          {KIND_SEGMENTS.map((segment) => (
            <button
              key={segment.kind}
              type="button"
              onClick={() => props.onSetNodeKind(node.id, segment.kind)}
              aria-pressed={node.kind === segment.kind}
              className={cn(
                'h-6 flex-1 rounded-[4px] text-1 text-ink-2 transition-colors disabled:cursor-not-allowed',
                node.kind === segment.kind ? 'bg-ink font-semibold text-content' : 'hover:text-ink',
              )}
            >
              {segment.label}
            </button>
          ))}
        </fieldset>
      ) : null}

      {node.kind === 'pool' ? (
        <InputRow
          label="说明"
          value={node.note ?? ''}
          multiline
          disabled={readOnly}
          onChange={(value) => patch({ note: value })}
        />
      ) : null}

      {node.kind === 'gate' ? (
        <>
          <div className={labelBase}>门里的人</div>
          <div className="flex flex-wrap items-center gap-1">
            <TagList
              values={node.members ?? []}
              disabled={readOnly}
              onRemove={(index) => props.onRemoveFromArray(node.id, 'members', index)}
            />
            {!readOnly ? (
              <RoleSelect
                have={node.members ?? []}
                keyName="审读"
                onAdd={(value) => props.onAddToArray(node.id, 'members', value)}
              />
            ) : null}
          </div>
          <label className="block">
            <span className={labelBase}>怎么算过</span>
            <select
              className={selectBase}
              value={node.rule ?? '全部通过'}
              disabled={readOnly}
              onChange={(event) => patch({ rule: event.target.value })}
            >
              {['全部通过', '一票否决', '多数通过'].map((rule) => (
                <option key={rule} value={rule}>
                  {rule}
                </option>
              ))}
            </select>
          </label>
          <InputRow
            label="没过怎么办"
            value={node.fail ?? ''}
            multiline
            disabled={readOnly}
            onChange={(value) => patch({ fail: value })}
          />
        </>
      ) : null}

      {node.kind === 'station' || node.kind === 'stack' ? (
        <>
          <div className={labelBase}>谁来做</div>
          <div className="flex flex-wrap items-center gap-1">
            <TagList
              values={node.who ?? []}
              disabled={readOnly}
              onRemove={(index) => props.onRemoveFromArray(node.id, 'who', index)}
            />
            {!readOnly ? (
              <RoleSelect
                have={node.who ?? []}
                keyName="谁来"
                onAdd={(value) => props.onAddToArray(node.id, 'who', value)}
              />
            ) : null}
          </div>
          <div className={labelBase}>用到的技能</div>
          <div className="flex flex-wrap items-center gap-1">
            <TagList
              values={node.skills ?? []}
              disabled={readOnly}
              onRemove={(index) => props.onRemoveFromArray(node.id, 'skills', index)}
            />
          </div>
          {!readOnly ? (
            <input
              className={cn(fieldBase, 'mt-1.5 h-7')}
              value={skillDraft}
              placeholder="加一个技能，回车"
              onChange={(event) => setSkillDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  addSkill();
                }
              }}
            />
          ) : null}
        </>
      ) : null}

      {node.kind === 'stack' ? (
        <InputRow
          label="逐条展开"
          value={node.fan ?? ''}
          placeholder="比如：每条写草稿"
          disabled={readOnly}
          onChange={(value) => patch({ fan: value })}
        />
      ) : null}

      {showHandoff ? (
        <>
          <div className={labelBase}>交接</div>
          <div className="grid grid-cols-[34px_1fr] items-start gap-x-2 gap-y-1">
            <span className="pt-1.5 text-2 text-ink-3">收到</span>
            <textarea
              className={cn(fieldBase, 'min-h-[42px] resize-y py-1 leading-5')}
              value={node.recv ?? ''}
              placeholder="从上一站拿到什么"
              rows={2}
              disabled={readOnly}
              onChange={(event) => patch({ recv: event.target.value })}
            />
            <span className="pt-1.5 text-2 text-ink-3">交出</span>
            <textarea
              className={cn(fieldBase, 'min-h-[42px] resize-y py-1 leading-5', showWorks && !(node.give ?? '').trim() && 'border-fail')}
              value={node.give ?? ''}
              placeholder={node.kind === 'pool' ? '下游来取时给什么' : '交给下一站什么'}
              rows={2}
              disabled={readOnly}
              onChange={(event) => patch({ give: event.target.value })}
            />
          </div>
        </>
      ) : null}

      {showWorks ? (
        <>
          <div className={labelBase}>完成标准</div>
          <div className="space-y-1">
            {(node.dod ?? []).map((item, index) => (
              <div key={index} className="flex items-center gap-1">
                <input
                  className={cn(fieldBase, 'h-7 min-w-0 flex-1')}
                  value={item.text}
                  disabled={readOnly}
                  onChange={(event) => props.onPatchDod(node.id, index, { text: event.target.value })}
                />
                <button
                  type="button"
                  onClick={() => props.onPatchDod(node.id, index, { auto: !item.auto })}
                  disabled={readOnly}
                  className={cn(
                    'h-6 shrink-0 rounded-[3px] px-1 text-[10px]',
                    item.auto
                      ? 'text-done ring-1 ring-done/40'
                      : 'text-ink-3 ring-1 ring-line-2',
                  )}
                >
                  {item.auto ? '程序可查' : '要人看'}
                </button>
                <button
                  type="button"
                  onClick={() => props.onRemoveDod(node.id, index)}
                  disabled={readOnly}
                  className="grid size-5 shrink-0 place-items-center rounded-[3px] text-ink-3 hover:bg-hover hover:text-ink disabled:cursor-not-allowed"
                  aria-label="去掉这条完成标准"
                >
                  <X className="size-3" />
                </button>
              </div>
            ))}
          </div>
          {!readOnly ? (
            <button type="button" onClick={() => props.onAddDod(node.id)} className="mt-1 h-6 text-1 text-run hover:underline">
              ＋ 加一条
            </button>
          ) : null}

          <InputRow
            label="节奏"
            value={node.rhythm ?? ''}
            placeholder="可空；比如：每天 10:00 从池里取"
            disabled={readOnly}
            onChange={(value) => patch({ rhythm: value })}
          />

          <div className={labelBase}>规矩</div>
          <div className="space-y-1 text-2">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                className="size-3.5 accent-[var(--run)]"
                checked={Boolean(node.user)}
                disabled={readOnly}
                onChange={(event) => patch({ user: event.target.checked })}
              />
              每次等你点头
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                className="size-3.5 accent-[var(--fail)]"
                checked={Boolean(node.ext)}
                disabled={readOnly}
                onChange={(event) => patch({ ext: event.target.checked })}
              />
              对外，发出去收不回
              <span className="text-1 text-ink-3">勾上就必须每次等你</span>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                className="size-3.5 accent-[var(--done)]"
                checked={Boolean(node.loop)}
                disabled={readOnly}
                onChange={(event) =>
                  patch({ loop: event.target.checked ? { max: 3, until: '' } : undefined })
                }
              />
              循环到满意
            </label>
            {node.loop ? (
              <div className="ml-5 flex items-center gap-1 text-2 text-ink-2">
                上限
                <input
                  type="number"
                  min={1}
                  max={9}
                  className={cn(fieldBase, 'h-6 w-12 px-1')}
                  value={node.loop.max ?? ''}
                  disabled={readOnly}
                  onChange={(event) =>
                    patch({
                      loop: {
                        max: event.target.value ? Number(event.target.value) : null,
                        until: node.loop?.until ?? '',
                      },
                    })
                  }
                />
                次，直到
                <input
                  className={cn(fieldBase, 'h-6 min-w-0 flex-1 px-1')}
                  value={node.loop.until}
                  placeholder="下一道门过了"
                  disabled={readOnly}
                  onChange={(event) =>
                    patch({ loop: { max: node.loop?.max ?? null, until: event.target.value } })
                  }
                />
              </div>
            ) : null}
          </div>
        </>
      ) : null}

      {node.kind === 'join' ? (
        <>
          <label className="block">
            <span className={labelBase}>怎么汇合</span>
            <select
              className={selectBase}
              value={node.join ?? ''}
              disabled={readOnly}
              onChange={(event) => patch({ join: (event.target.value || null) as FlowNode['join'] })}
            >
              <option value="">还没定</option>
              {JOIN_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <div className={labelBase}>从这里进来的</div>
          <div className="text-2 text-ink-2">
            {flow.edges
              .filter((edge) => edge.target === node.id)
              .map((edge) => flow.nodes.find((item) => item.id === edge.source)?.name ?? '？')
              .join(' → ') || '还没有线接进来'}
          </div>
          <p className="mt-2 text-1 leading-4 text-ink-3">
            要从上一站拉一条线到汇合的左边把手；怎么汇合写清全部、任一或多数。
          </p>
        </>
      ) : null}

      {node.kind === 'start' ? (
        <>
          <div className={labelBase}>怎么开一轮</div>
          <div className="flex rounded-btn bg-hover p-0.5">
            <button
              type="button"
              onClick={() => patch({ start: 'manual' })}
              disabled={readOnly}
              aria-pressed={node.start === 'manual'}
              className={cn(
                'h-6 flex-1 rounded-[4px] text-1',
                node.start === 'manual' ? 'bg-ink font-semibold text-content' : 'text-ink-2',
              )}
            >
              你说开始
            </button>
            <button
              type="button"
              onClick={() => patch({ start: 'cron', rhythm: node.rhythm || '每天 9:00' })}
              disabled={readOnly}
              aria-pressed={node.start === 'cron'}
              className={cn(
                'h-6 flex-1 rounded-[4px] text-1',
                node.start === 'cron' ? 'bg-ink font-semibold text-content' : 'text-ink-2',
              )}
            >
              到点开一轮
            </button>
          </div>
          {node.start === 'cron' ? (
            <InputRow
              label="节奏"
              value={node.rhythm ?? ''}
              placeholder="比如：每天 8:00"
              disabled={readOnly}
              onChange={(value) => patch({ rhythm: value })}
            />
          ) : null}
        </>
      ) : null}

      {node.kind === 'end' ? <p className="mt-3 text-2 text-ink-2">这一条线在这里结束。</p> : null}

      {!readOnly && node.kind !== 'start' && node.kind !== 'end' ? (
        <button
          type="button"
          onClick={() => props.onDeleteNode(node.id)}
          className="mt-5 -ml-1 rounded-btn px-1 py-1 text-2 text-fail hover:bg-fail/10"
        >
          删掉这一站{shared ? '（每条线都去掉）' : ''}
        </button>
      ) : null}
    </div>
  );
}

export function FlowInspector(props: FlowInspectorProps) {
  const { flow, selectedIds, preview, readOnly, archived, instance } = props;
  let body: ReactNode;

  if (!selectedIds.length) {
    body = (
      <Overview
        flow={flow}
        preview={preview}
        readOnly={readOnly}
        instance={instance}
        onPatchFlowName={props.onPatchFlowName}
      />
    );
  } else if (selectedIds.length > 1) {
    const selected = selectedIds
      .map((id) => flow.nodes.find((node) => node.id === id))
      .filter((node): node is FlowNode => Boolean(node));
    body = (
      <div>
        <div className="flex items-center gap-1">
          <span className="text-1 text-ink-3">已选 {selected.length} 个</span>
          <button
            type="button"
            onClick={props.onClose}
            className="ml-auto grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink"
            aria-label="取消选择"
          >
            <X className="size-4" />
          </button>
        </div>
        <div className="mt-2 text-5 font-semibold text-ink">框选的站</div>
        <ul className="mt-2 space-y-1 text-2 text-ink-2">
          {selected.map((node) => (
            <li key={node.id}>
              <button
                type="button"
                onClick={() => props.onSelect(node.id)}
                className="flex w-full items-center gap-1.5 rounded-btn px-1 py-0.5 text-left hover:bg-hover"
              >
                <span className="size-1.5 rounded-full bg-ink-3" />
                {node.name}
                <span className="ml-auto text-1 text-ink-3">{NODE_KIND_LABEL[node.kind]}</span>
              </button>
            </li>
          ))}
        </ul>
        {!readOnly && !preview ? (
          <button
            type="button"
            onClick={() => props.onDeleteNodes(selected.map((node) => node.id))}
            className="mt-4 rounded-btn px-1 py-1 text-2 text-fail hover:bg-fail/10"
          >
            删掉选中的站
          </button>
        ) : null}
        <p className="mt-3 text-1 text-ink-3">框选用于批量删；点单个站可以改表单。</p>
      </div>
    );
  } else {
    const node = flow.nodes.find((item) => item.id === selectedIds[0]);
    if (!node) {
      body = (
        <Overview
          flow={flow}
          preview={preview}
          readOnly={readOnly}
          instance={instance}
          onPatchFlowName={props.onPatchFlowName}
        />
      );
    } else if (preview) {
      body = <DiffDetail flow={flow} node={node} />;
    } else {
      body = (
        <Detail
          flow={flow}
          node={node}
          readOnly={readOnly}
          archived={archived}
          onPointToKind={props.onPointToKind}
          onClose={props.onClose}
          onPatchNode={props.onPatchNode}
          onSetNodeKind={props.onSetNodeKind}
          onAddToArray={props.onAddToArray}
          onRemoveFromArray={props.onRemoveFromArray}
          onAddDod={props.onAddDod}
          onPatchDod={props.onPatchDod}
          onRemoveDod={props.onRemoveDod}
          onDeleteNode={props.onDeleteNode}
        />
      );
    }
  }

  return (
    <aside className="flex h-full min-h-0 w-[312px] shrink-0 flex-col border-l border-line bg-content" aria-live="polite">
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">{body}</div>
      <div className="shrink-0 border-t border-line px-4 py-2 text-1 text-ink-3">
        <span className="inline-flex items-center gap-1">
          <ChevronDown className="size-3" />
          {preview ? '看修改时只读' : readOnly ? (archived ? '已归档只读' : '实例态只读') : '点站改它 · 空白处取消'}
        </span>
      </div>
    </aside>
  );
}
