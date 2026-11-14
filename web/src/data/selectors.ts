/**
 * 页面要用的派生计算（假数据层内部用；界面只通过 src/data/api.ts 取）。
 *
 * 这里放「从样例把它算成界面要的形状」的纯函数：地铁线的站、看板四列、任务页的账。
 * 第 1 期的 fixtures/workspace.ts 注释里说的就是这里。
 */

import { formatDurationSeconds } from '@/lib/numbers';
import { artifacts, materials, projects, tasks } from './fixtures/workspace';
import type { ProjectFixture, TaskFixture, TaskState } from './fixtures/workspace';

const RANK: Record<TaskState, number> = { fail: 0, wait: 1, run: 2, todo: 3, done: 4 };

/** 聚合任务的状态：全完成 = done，全没开始 = todo，否则取最靠前的状态。 */
export function stateOf(task: TaskFixture, all: Record<string, TaskFixture> = tasks): TaskState {
  if (!task.children?.length) return task.state ?? 'todo';
  const states = task.children.map((id) => stateOf(all[id], all));
  if (states.every((state) => state === 'done')) return 'done';
  if (states.every((state) => state === 'todo')) return 'todo';
  return states.reduce((a, b) => (RANK[a] <= RANK[b] ? a : b));
}

export function isLeaf(task: TaskFixture): boolean {
  return !task.children?.length;
}

export function leavesOf(task: TaskFixture, all: Record<string, TaskFixture> = tasks): TaskFixture[] {
  if (!task.children?.length) return [task];
  return task.children.flatMap((id) => leavesOf(all[id], all));
}

/** 任务所属项目。 */
export function projectOf(taskId: string): ProjectFixture | null {
  const task = tasks[taskId];
  return task ? projects[task.project] ?? null : null;
}

/** 从根任务到当前任务的路径（不含项目、含当前任务）。 */
export function pathOf(taskId: string): { id: string; title: string }[] {
  const out: { id: string; title: string }[] = [];
  let current: TaskFixture | undefined = tasks[taskId];
  while (current) {
    out.unshift({ id: current.id, title: current.title });
    current = current.parent ? tasks[current.parent] : undefined;
  }
  return out;
}

/* ───────────────────────── 任务页（§4.1–4.5） ───────────────────────── */

export interface TaskNodeStep {
  title: string;
  who: string;
  result: string;
}

export interface TaskDetail {
  id: string;
  title: string;
  projectId: string;
  projectCode: string;
  projectName: string;
  color: string;
  state: TaskState;
  /** 从根任务到当前任务（含当前）。 */
  path: { id: string; title: string }[];
  /** 直接子任务（地铁线的站）。 */
  children: {
    id: string;
    title: string;
    role: string;
    state: TaskState;
    note: string;
    deps: string[];
    hasChildren: boolean;
    progress: { done: number; total: number } | null;
  }[];
  /** 叶子任务的节点层（§4.2）；有子任务时为 null。 */
  nodeLayer: { tier: string; steps: TaskNodeStep[]; handover: { at: string; who: string; what: string }[]; machineCheck: string } | null;
  artifacts: { name: string; version: string; size: string; status: 'acc' | 'pend' | 'none' }[];
  ledger: { model: string; elapsed: string; eta: string; etaNote: string; tokens: string; cost: string };
  milestone: { id: string; name: string; state: string; due?: string; criteria: [string, 'ok' | 'no' | 'todo', string?][] } | null;
  /** 能暂停/取消的只有根任务和中间层；叶子完成以后不提供。 */
  canControl: boolean;
  /** 冒泡到任务页的深层事项（最深的等你 / 出问题叶子）。 */
  attention: { id: string; title: string; state: TaskState } | null;
}

/** 样例里 elapsed 的单位是秒。 */
function secondsOf(task: TaskFixture): number {
  return task.elapsed ?? 0;
}

function shortNumber(value: number): string {
  return value >= 1000 ? (value / 1000).toFixed(1) + 'k' : String(value);
}

/** 样例费率：¥0.0084 / 1k token（只用来让账上的数字对得上，不是真实报价）。 */
const COST_PER_1K = 0.0084;

export function taskDetail(taskId: string): TaskDetail | null {
  const task = tasks[taskId];
  if (!task) return null;
  const project = projects[task.project];
  if (!project) return null;
  const leaves = leavesOf(task);

  const models = [...new Set(leaves.map((leaf) => leaf.model).filter(Boolean) as string[])];
  const tokens = leaves.reduce((sum, leaf) => sum + (leaf.tokens ?? 0), 0);
  const elapsedSeconds = leaves.reduce((sum, leaf) => sum + secondsOf(leaf), 0);
  const doneLeaves = leaves.filter((leaf) => stateOf(leaf) === 'done');
  const remainingLeaves = leaves.filter((leaf) => stateOf(leaf) !== 'done');
  const doneSeconds = doneLeaves.reduce((sum, leaf) => sum + secondsOf(leaf), 0);
  const avgSeconds = doneLeaves.length ? doneSeconds / doneLeaves.length : 0;

  // §4.4：预计还需必须附依据；依据不足就写「未知」并说明。
  let eta = '未知';
  let etaNote = '已完成的项太少，没有可靠依据估算。';
  if (!remainingLeaves.length) {
    eta = '—';
    etaNote = '所有子任务都已完成。';
  } else if (doneLeaves.length >= 2) {
    eta = '≈ ' + formatDurationSeconds(Math.round(avgSeconds * remainingLeaves.length));
    etaNote =
      '按已完成 ' + doneLeaves.length + ' 项、平均每项约 ' + Math.round(avgSeconds / 60) + ' 分钟估算，不含等你的时间。';
  }

  const milestoneId = task.ms;
  const milestone = milestoneId
    ? project.milestones.find((item) => item.id === milestoneId) ?? null
    : null;

  const nodeLayer = isLeaf(task)
    ? {
        tier: task.role === 'Kind' ? '直接完成' : '结构工作',
        steps:
          task.role === 'Kind'
            ? [{ title: 'Kind 直接完成', who: 'Kind', result: task.note ?? '完成' }]
            : [
                { title: '执行', who: task.role ?? 'Kind', result: task.note ?? '完成' },
                { title: '自查', who: task.role ?? 'Kind', result: '对照要求逐条看过' },
              ],
        handover: (task.log ?? []).map(([at, what]) => ({ at, who: task.role ?? 'Kind', what })),
        machineCheck: '机器验证 2 / 2 · 都绑定了程序检查',
      }
    : null;

  const artifactNames = leaves.map((leaf) => leaf.out).filter(Boolean) as string[];
  const artifactViews = artifactNames
    .map((name) => artifacts[name])
    .filter(Boolean)
    .map((artifact) => ({
      name: artifact.name,
      version: 'v' + artifact.version,
      size: artifact.size,
      status: artifact.status as 'acc' | 'pend' | 'none',
    }));

  return {
    id: task.id,
    title: task.title,
    projectId: project.id,
    projectCode: project.code,
    projectName: project.name,
    color: project.color,
    state: stateOf(task),
    path: pathOf(task.id),
    children: (task.children ?? []).map((id) => {
      const child = tasks[id];
      const stationLeaves = leavesOf(child);
      const stationDone = stationLeaves.filter((leaf) => stateOf(leaf) === 'done').length;
      return {
        id: child.id,
        title: child.title,
        role: child.role ?? child.children?.map((cid) => tasks[cid].role ?? '').filter(Boolean).join(' · ') ?? '',
        state: stateOf(child),
        note: child.note ?? '',
        deps: child.deps ?? [],
        hasChildren: Boolean(child.children?.length),
        progress: child.children?.length ? { done: stationDone, total: stationLeaves.length } : null,
      };
    }),
    nodeLayer,
    artifacts: artifactViews,
    ledger: {
      model: models.length > 1 ? models[0] + ' 等 ' + models.length + ' 个' : models[0] ?? '—',
      elapsed: formatDurationSeconds(elapsedSeconds),
      eta,
      etaNote,
      tokens: shortNumber(tokens) + ' tokens',
      cost: '¥' + ((tokens / 1000) * COST_PER_1K).toFixed(2),
    },
    milestone: milestone
      ? {
          id: milestone.id,
          name: milestone.name,
          state: milestone.state === 'passed' ? '已通过' : milestone.state === 'judging' ? '判断中' : '进行中',
          due: milestone.due,
          criteria: (milestone.criteria ?? []) as [string, 'ok' | 'no' | 'todo', string?][],
        }
      : null,
    canControl: true,
    attention: (() => {
      const leaf = leaves.find((item) => item.id !== task.id && (stateOf(item) === 'wait' || stateOf(item) === 'fail'));
      return leaf ? { id: leaf.id, title: leaf.title, state: stateOf(leaf) } : null;
    })(),
  };
}

/* ───────────────────────── 项目页（§4.5 里程碑 / §4.6 成果） ───────────────────────── */

export interface ProjectTaskRow {
  id: string;
  title: string;
  state: TaskState;
  role: string;
  note: string;
  progress: { done: number; total: number } | null;
}

export interface ProjectMilestoneView {
  id: string;
  name: string;
  state: 'passed' | 'open' | 'judging';
  when?: string;
  due?: string;
  criteria: [string, 'ok' | 'no' | 'todo', string?][];
  tasks: ProjectTaskRow[];
}

export interface ProjectArtifactView {
  name: string;
  version: string;
  size: string;
  status: 'acc' | 'pend' | 'none';
  from: string;
}

export interface ProjectMaterialView {
  name: string;
  type: string;
  status: 'ok' | 'busy' | 'bad';
  version: string;
  /** 带 = 每次整份带上；查 = 要用时来查。 */
  mode: string;
}

export interface ProjectDetail {
  id: string;
  name: string;
  code: string;
  color: string;
  brief: string;
  counts: { done: number; total: number };
  milestones: ProjectMilestoneView[];
  artifacts: ProjectArtifactView[];
  /** 放在这个项目里的资料（§8.5）。 */
  materials: ProjectMaterialView[];
  /** 还能查全局的几份。 */
  globalMaterials: number;
}

export function projectDetail(projectId: string): ProjectDetail | null {
  const project = projects[projectId];
  if (!project) return null;
  const projectTasks = Object.values(tasks).filter((task) => task.project === projectId);
  const roots = projectTasks.filter((task) => !task.parent);
  const leaves = roots.flatMap((root) => leavesOf(root));

  const milestones: ProjectMilestoneView[] = project.milestones.map((milestone) => {
    const rows: ProjectTaskRow[] = roots
      .filter((root) => root.ms === milestone.id)
      .map((root) => {
        const stationLeaves = leavesOf(root);
        const done = stationLeaves.filter((leaf) => stateOf(leaf) === 'done').length;
        const roles = root.children?.length
          ? [...new Set(root.children.map((id) => tasks[id].role ?? '').filter(Boolean))].join(' · ')
          : root.role ?? '';
        return {
          id: root.id,
          title: root.title,
          state: stateOf(root),
          role: roles,
          note: root.note ?? '',
          progress: root.children?.length ? { done, total: stationLeaves.length } : null,
        };
      });
    return {
      id: milestone.id,
      name: milestone.name,
      state: milestone.state,
      when: milestone.when,
      due: milestone.due,
      criteria: (milestone.criteria ?? []) as [string, 'ok' | 'no' | 'todo', string?][],
      tasks: rows,
    };
  });

  const artifactRows: ProjectArtifactView[] = [];
  for (const task of projectTasks) {
    if (!task.out) continue;
    const artifact = artifacts[task.out];
    if (!artifact) continue;
    artifactRows.push({
      name: artifact.name,
      version: 'v' + artifact.version,
      size: artifact.size,
      status: artifact.status as 'acc' | 'pend' | 'none',
      from: task.title,
    });
  }

  const projectMaterials: ProjectMaterialView[] = [];
  for (const material of materials) {
    const placement = material.placedAt.find(([where]) => where === '项目 · ' + project.name);
    if (!placement) continue;
    projectMaterials.push({
      name: material.name,
      type: material.type,
      status: material.status,
      version: material.version,
      mode: placement[1],
    });
  }
  const globalMaterials = materials.filter((material) => material.placedAt.some(([where]) => where === '全局')).length;

  return {
    id: project.id,
    name: project.name,
    code: project.code,
    color: project.color,
    brief: project.brief,
    counts: { done: leaves.filter((leaf) => stateOf(leaf) === 'done').length, total: leaves.length },
    milestones,
    artifacts: artifactRows,
    materials: projectMaterials,
    globalMaterials,
  };
}

/* ───────────────────────── 阅读页（§8.6 资料与成果共用） ───────────────────────── */

export interface ReadSection {
  heading: string;
  paragraphs: string[];
  bullets?: string[];
}

export interface ReadDoc {
  name: string;
  type: string;
  /** 来源：资料 / 成果。 */
  origin: 'material' | 'artifact';
  source: string;
  version: string;
  status: 'ok' | 'busy' | 'bad' | 'acc' | 'pend' | 'none';
  /** 来自哪个任务（成果）。 */
  fromTask: string | null;
  projectId: string | null;
  projectCode: string | null;
  projectColor: string | null;
  /** 被哪些任务用过 / 涉及。 */
  usedBy: { task: string; where: string }[];
  /** 验收的完成标准（成果才有）。 */
  criteria: string[];
  sections: ReadSection[];
  /** 类型特有的样子：docx/pdf = 分页、xlsx = 网格、md = 纯文本、png = 图片、html = 快照。 */
  layout: 'pages' | 'grid' | 'text' | 'image' | 'snapshot' | 'code';
  capturedAt?: string;
  /** 和上一版的逐行对比（v13-o O2 的 diff 查看器）；没有就是 null。 */
  diff: { left: string[]; right: string[]; status: ('same' | 'chg' | 'add' | 'del')[] } | null;
}

const READ_DIFFS: Record<string, { left: string[]; right: string[]; status: ('same' | 'chg' | 'add' | 'del')[] }> = {
  '评测记录.md': {
    left: [
      '多引擎实测 · 评测记录 v1',
      '范围：Brave / Bing / Exa 三个引擎；同一批 20 个查询。',
      '去重后命中率：21 / 20。',
      '结论：三个引擎都可用，Bing 结果最好。',
      '附录 · 口径：命中 = 前三条里有目标页面。',
    ],
    right: [
      '多引擎实测 · 评测记录 v2',
      '范围：Brave / Bing / Exa 三个引擎；同一批 20 个查询，口径见附录。',
      '去重后命中率：18 / 20。',
      '结论：三个引擎都可用；Bing 缺 key 未测，结论里注明缺这一项。',
      '附录 · 口径：命中 = 前三条里有目标页面。',
    ],
    status: ['chg', 'chg', 'chg', 'chg', 'same'],
  },
};

const ARTIFACT_SECTIONS: Record<string, ReadSection[]> = {
  '结构说明.docx': [
    {
      heading: '',
      paragraphs: [
        'news-intelligence 是一个本地运行的新闻情报工具：按来源清单定时抓取新闻，去重、打分排序后，在浏览器里给你看。只有你自己用，不对外提供服务。',
      ],
    },
    { heading: '1. 技术栈', paragraphs: ['Python 3.12；Web 框架 FastAPI；存储 SQLite；抓取用 httpx 和 selectolax。依赖以 pyproject.toml 为准，共 12 个。'] },
    {
      heading: '2. 模块划分',
      paragraphs: [],
      bullets: [
        'ingest：抓取与清洗，按来源清单定时抓取，去掉广告和导航。',
        'dedupe：去重，目前只按标题比对。',
        'rank：打分与排序，按来源权重、发布时间和关键词。',
        'store：存储，SQLite 里一张文章表、一张来源表。',
        'serve：本地 Web 界面，有列表页和详情页。',
        'cli：命令行入口，启动服务、手动抓取一次。',
      ],
    },
    {
      heading: '3. 怎么运行',
      paragraphs: ['先装依赖：uv sync；再启动：python -m news_intel --serve 8080；然后在浏览器打开 localhost:8080。仓库里的 run.sh 把这三步写在了一起。'],
    },
    {
      heading: '4. 完成度与可改进点',
      paragraphs: ['抓取、去重、打分、存储、页面都已经能跑；缺定时调度的图形界面和告警。可改进点：标题去重漏掉同文改写；来源权重写死在代码里；没有错误重试。'],
    },
  ],
  '评测记录.md': [
    { heading: '', paragraphs: ['多引擎实测 · 评测记录 v2。范围：Brave / Bing / Exa 三个引擎；同一批 20 个查询，口径见附录。'] },
    { heading: '结论', paragraphs: ['去重之后命中率 18 / 20。Bing 缺 key 未测，结论里注明缺这一项。'] },
    { heading: '附录 · 口径', paragraphs: ['命中 = 前三条里有目标页面；超时超过 8 秒算未命中。'] },
  ],
};

function genericSections(_name: string, type: string): ReadSection[] {
  return [
    { heading: '', paragraphs: ['这是原型里的样例内容：真实实现会按 ' + type + ' 类型做保真渲染。'] },
    {
      heading: '第一部分',
      paragraphs: ['这里放正文段落。原型用占位文本，保证版式、字号、行高和真实内容一致。'],
      bullets: ['要点一：说明它是什么。', '要点二：说明它怎么用。', '要点三：说明还有什么限制。'],
    },
    { heading: '第二部分', paragraphs: ['更多正文。阅读页右侧是 Kind 伴读，进入验收时换成验收面板。'] },
  ];
}

export function readDoc(fileName: string): ReadDoc | null {
  const artifact = artifacts[fileName];
  if (artifact) {
    const owner = Object.values(tasks).find((task) => task.out === fileName);
    const project = owner ? projects[owner.project] : null;
    // 成果挂在叶子上，里程碑挂在它祖先的根任务上：沿 parent 往上找第一个带 ms 的。
    let milestoneOwner = owner;
    while (milestoneOwner && !milestoneOwner.ms && milestoneOwner.parent) milestoneOwner = tasks[milestoneOwner.parent];
    const milestone =
      milestoneOwner?.ms && project ? project.milestones.find((item) => item.id === milestoneOwner?.ms) : null;
    const sections = ARTIFACT_SECTIONS[fileName] ?? genericSections(fileName, artifact.type);
    return {
      name: artifact.name,
      type: artifact.type,
      origin: 'artifact',
      source: '来自「' + (owner?.title ?? '任务') + '」',
      version: 'v' + artifact.version,
      status: artifact.status,
      fromTask: owner?.title ?? null,
      projectId: owner?.project ?? null,
      projectCode: project?.code ?? null,
      projectColor: project?.color ?? null,
      usedBy: owner
        ? [
            { task: '验收', where: owner.title },
            { task: '后续引用', where: artifact.use ?? '还没被别的任务用过' },
          ]
        : [],
      criteria: ((milestone?.criteria ?? []) as [string, 'ok' | 'no' | 'todo', string?][]).map(([text]) => text),
      sections,
      layout:
        artifact.type === 'md' || artifact.type === 'txt'
          ? 'text'
          : artifact.type === 'xlsx'
            ? 'grid'
            : artifact.type === 'png' || artifact.type === 'graph'
              ? 'image'
              : artifact.type === 'html'
                ? 'snapshot'
                : artifact.type === 'json' || artifact.type === 'sh'
                  ? 'code'
                  : 'pages',
      capturedAt: artifact.type === 'html' ? '10月21日抓取' : undefined,
      diff: READ_DIFFS[fileName] ?? null,
    };
  }

  const material = materials.find((item) => item.name === fileName);
  if (material) {
    return {
      name: material.name,
      type: material.type,
      origin: 'material',
      source: material.source,
      version: material.version,
      status: material.status,
      fromTask: null,
      projectId: null,
      projectCode: null,
      projectColor: null,
      usedBy: material.usedBy.map(([task, where, version]) => ({ task, where: where + (version ? ' · ' + version : '') })),
      criteria: [],
      sections: genericSections(material.name, material.type),
      layout:
        material.type === 'xlsx'
          ? 'grid'
          : material.type === 'png'
            ? 'image'
            : material.type === 'html'
              ? 'snapshot'
              : material.type === 'pdf'
                ? 'pages'
                : 'text',
      capturedAt: material.type === 'html' ? material.version : undefined,
      diff: null,
    };
  }
  return null;
}

/* ───────────────────────── 工作页的「等你」小列表 ───────────────────────── */

export interface WaitingRow {
  id: string;
  title: string;
  projectName: string | null;
  projectCode: string | null;
  typeLabel: string;
  time: string;
  href: string;
}

export function waitingRows(decisions: { id: string; title: string; projectName: string | null; projectCode: string | null; typeLabel: string; time: string; href: string }[]): WaitingRow[] {
  return decisions.map((item) => ({
    id: item.id,
    title: item.title,
    projectName: item.projectName,
    projectCode: item.projectCode,
    typeLabel: item.typeLabel,
    time: item.time,
    href: item.href,
  }));
}
