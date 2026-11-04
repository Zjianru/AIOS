/**
 * 流程编辑器样例数据。
 *
 * 界面只从这里取样例；组件里不再内联流程、版本、实例或新建草稿。
 * 位置是「整理后」的初始位置；localStorage 里有记录时以记录为准。
 */

/* ───────────────────────── 类型 ───────────────────────── */

export type FlowNodeKind = 'station' | 'stack' | 'gate' | 'pool' | 'join' | 'start' | 'end';

export type FlowJoinMode = 'all' | 'any' | 'majority';

export type FlowInstanceState = 'run' | 'wait' | 'done' | 'todo';

export type FlowDiff = 'add' | 'del' | 'chg';

export interface FlowPoint {
  x: number;
  y: number;
}

export interface FlowDod {
  text: string;
  /** true = 程序可查，false = 要人看。 */
  auto: boolean;
}

export interface FlowLoop {
  max: number | null;
  until: string;
}

export interface FlowLine {
  id: string;
  /** 线路标：1、2、3。 */
  index: number;
  name: string;
  what: string;
  color: string;
}

export interface FlowNode {
  id: string;
  kind: FlowNodeKind;
  name: string;
  /** 这个节点属于哪几条价值流；两条以上画成换乘站。 */
  lines: string[];
  position: FlowPoint;
  who?: string[];
  skills?: string[];
  recv?: string;
  give?: string;
  dod?: FlowDod[];
  /** 逐条展开：每条怎么跑。 */
  fan?: string;
  rhythm?: string;
  note?: string;
  /** 每次等你点头。 */
  user?: boolean;
  /** 对外，发出去收不回。 */
  ext?: boolean;
  loop?: FlowLoop;
  members?: string[];
  rule?: string;
  fail?: string;
  /** 汇合：怎么汇合；null = 还没定。 */
  join?: FlowJoinMode | null;
  /** 起点：手动 / 定时。 */
  start?: 'manual' | 'cron';
  /** Kind 起草时自己加的说明。 */
  auto?: string;
  /** 修改提议里的标记，只出现在预览副本里。 */
  diff?: FlowDiff;
  diffOld?: Record<string, unknown>;
}

export interface FlowEdge {
  id: string;
  source: string;
  target: string;
  lineId: string;
  /**
   * 共用路段：一段路被几条价值流同时使用。
   * 仍然是一条连线（检查、汇合、排版都按一条算），只是画的时候每条线各画一条并排的。
   */
  lineIds?: string[];
  /** 修改提议里的标记，只出现在预览副本里。 */
  diff?: FlowDiff;
}

export interface FlowVersion {
  version: number;
  at: string;
  note: string;
}

export interface FlowProjectBadge {
  name: string;
  code: string;
  color: string;
}

export interface FlowInstanceOverride {
  nodeId: string;
  label: string;
  patch: Partial<FlowNode>;
}

export interface FlowInstance {
  id: string;
  name: string;
  version: number;
  state: 'run' | 'wait' | 'done';
  stateText: string;
  overrides: FlowInstanceOverride[];
  project?: FlowProjectBadge;
  /** 实例态在图上标的进度。 */
  nodeStates?: Record<string, FlowInstanceState>;
}

export interface Flow {
  id: string;
  name: string;
  version: number;
  draft?: boolean;
  archived?: boolean;
  source: string;
  used: number;
  lines: FlowLine[];
  nodes: FlowNode[];
  edges: FlowEdge[];
  versions: FlowVersion[];
  instances: FlowInstance[];
}

export interface FlowGraph {
  nodes: FlowNode[];
  edges: FlowEdge[];
}

export interface NewFlowSource {
  id: 'blank' | 'say' | 'distil' | 'template' | 'import';
  title: string;
  description: string;
}

export interface NewFlowTemplate {
  key: string;
  title: string;
  description: string;
  /** 复制哪份草稿；内容运营 / unFIX 多线直接用现成流程的副本。 */
  flow: Flow;
}

export interface DistillOption {
  key: string;
  title: string;
  description: string;
}

export interface ImportPreview {
  title: string;
  version: string;
  summary: string;
  detail: string;
  link: string;
}

/* ───────────────────────── 选项 ───────────────────────── */

export const ROLE_OPTIONS = [
  'Kind',
  '你',
  '程序',
  '资讯员',
  '编辑',
  '写手',
  '改编',
  '审读',
  '事实核查',
  '风格',
  '平台合规',
  '工程',
  '排查员',
  '运营',
  '安全',
] as const;

export const JOIN_OPTIONS: { value: FlowJoinMode; label: string }[] = [
  { value: 'all', label: '全部到齐' },
  { value: 'any', label: '任一到' },
  { value: 'majority', label: '多数到' },
];

export const JOIN_LABEL: Record<FlowJoinMode, string> = {
  all: '全部到齐',
  any: '任一到',
  majority: '多数到',
};

export const NODE_KIND_LABEL: Record<FlowNodeKind, string> = {
  station: '站',
  stack: '逐条展开',
  gate: '门',
  pool: '池',
  join: '汇合',
  start: '起点',
  end: '终点',
};

export const INSTANCE_STATE_LABEL: Record<FlowInstanceState, string> = {
  run: '进行中',
  wait: '等你',
  done: '完成',
  todo: '还没开始',
};

export const FLOW_LINE_COLORS = ['#1AA3BA', '#7C5CF5', '#B9764A', '#C77DBA'];

export const NEW_FLOW_SOURCES: NewFlowSource[] = [
  { id: 'blank', title: '空白开始', description: '从一站开始自己搭' },
  { id: 'say', title: '跟 Kind 说', description: '说一句要做什么，Kind 起草' },
  { id: 'distil', title: '从做过的事提炼', description: '挑一件做过几次的事，把做法整理出来' },
  { id: 'template', title: '从模板', description: '内置流程，复制一份再改' },
  { id: 'import', title: '导入', description: '别人分享的链接或文件，不带权限' },
];

export const DISTILL_OPTIONS: DistillOption[] = [
  { key: 'weekly', title: '周报', description: '做过 6 次，最近 10-24' },
  { key: 'competitor', title: '竞品月报', description: '做过 2 次，最近 10-02' },
  { key: 'release', title: '发版说明', description: '做过 4 次，最近 10-25' },
];

export const SAY_DEFAULT = '每天早上把我关注的 5 个开源项目的新 issue 汇总一下，发给我。';

export const IMPORT_PREVIEW: ImportPreview = {
  title: '播客剪辑',
  version: '分享时 v4',
  summary: '4 站：选段（逐条）→ 剪辑 → 审听门 → 发布。带 3 个技能说明。',
  detail: '不带：权限、本机路径、密钥、私有资料。第一次跑时会重新给你看要读写的范围。',
  link: 'aios://share/flow/podcast-cut  （或把 .aiosflow 文件拖进来）',
};

/* ───────────────────────── 内容运营（单价值流） ───────────────────────── */

const contentLines: FlowLine[] = [
  { id: 's1', index: 1, name: '内容运营', what: '每天一轮', color: FLOW_LINE_COLORS[0] },
];

const contentNodes: FlowNode[] = [
  {
    id: 'c-start',
    kind: 'start',
    name: '起点',
    lines: ['s1'],
    position: { x: 0, y: 0 },
    start: 'cron',
    rhythm: '每 2 小时',
  },
  {
    id: 'c1',
    kind: 'station',
    name: '收集',
    lines: ['s1'],
    position: { x: 108, y: 0 },
    who: ['资讯员'],
    skills: ['网页抓取', 'RSS 读取'],
    recv: '来源清单（24 个）',
    give: '原始条目若干、打不开的来源',
    dod: [
      { text: '每个来源都试过一次', auto: true },
      { text: '每条带原文链接', auto: true },
    ],
  },
  {
    id: 'c2',
    kind: 'station',
    name: '清洗排序',
    lines: ['s1'],
    position: { x: 216, y: 0 },
    who: ['资讯员'],
    skills: ['去重', '打分排序'],
    recv: '原始条目',
    give: '干净条目，放进池里',
    dod: [
      { text: '同一网址只留一条', auto: true },
      { text: '每条有分数和一句理由', auto: true },
    ],
  },
  {
    id: 'pool',
    kind: 'pool',
    name: '池',
    lines: ['s1'],
    position: { x: 324, y: 0 },
    note: '上游攒着，下游按自己的节奏来取。清洗排序每 2 小时放进来，选题每天来取一次。',
    recv: '清洗排序放进来的干净条目',
    give: '选题来取时一次给它',
  },
  {
    id: 'c3',
    kind: 'station',
    name: '选题',
    lines: ['s1'],
    position: { x: 432, y: 0 },
    who: ['编辑'],
    skills: ['选题', '流程说明：选题口味'],
    rhythm: '每天 10:00 从池里取',
    recv: '池里昨天到今天的条目',
    give: '选题单 ×1（5 个候选，各附理由）',
    dod: [
      { text: '正好 5 个候选', auto: true },
      { text: '每个都有来源链接', auto: true },
      { text: '和最近 7 天不重复', auto: true },
      { text: '口味对不对', auto: false },
    ],
  },
  {
    id: 'c4',
    kind: 'stack',
    name: '二次收集',
    lines: ['s1'],
    position: { x: 540, y: 0 },
    fan: '每条写草稿',
    who: ['写手'],
    skills: ['深度搜索', '写作'],
    recv: '一个选题和它的来源',
    give: '草稿 ×1、来源清单、未决问题',
    dod: [
      { text: '每个事实都有来源', auto: true },
      { text: '800 到 1500 字', auto: true },
      { text: '未决问题列清楚', auto: false },
    ],
  },
  {
    id: 'c5',
    kind: 'gate',
    name: '评审门',
    lines: ['s1'],
    position: { x: 648, y: 0 },
    members: ['事实核查', '风格', '平台合规'],
    rule: '全部通过',
    fail: '退回写手改；同一篇第 3 次没过，转为等你决定。',
    recv: '草稿、来源清单',
    give: '过审草稿、三份审读意见',
    dod: [
      { text: '三人都给出结论', auto: true },
      { text: '事实问题清零', auto: false },
    ],
  },
  {
    id: 'c6a',
    kind: 'station',
    name: '小红书',
    lines: ['s1'],
    position: { x: 756, y: -104 },
    who: ['改编'],
    skills: ['小红书笔记格式'],
    recv: '过审草稿',
    give: '小红书笔记 ×1',
    dod: [{ text: '字数和图片数合平台要求', auto: true }],
  },
  {
    id: 'c6b',
    kind: 'station',
    name: '抖音',
    lines: ['s1'],
    position: { x: 756, y: 0 },
    who: ['改编'],
    skills: ['短视频脚本'],
    recv: '过审草稿',
    give: '口播脚本 ×1、分镜',
    dod: [{ text: '时长 60 秒内', auto: true }],
  },
  {
    id: 'c6c',
    kind: 'station',
    name: '公众号',
    lines: ['s1'],
    position: { x: 756, y: 104 },
    who: ['改编'],
    skills: ['公众号排版'],
    recv: '过审草稿',
    give: '排好版的文章 ×1',
    dod: [{ text: '标题 30 字内', auto: true }],
  },
  {
    id: 'c-join',
    kind: 'join',
    name: '汇合',
    lines: ['s1'],
    position: { x: 864, y: 0 },
    join: 'all',
    recv: '三个平台稿',
    give: '一包三平台稿',
  },
  {
    id: 'c8',
    kind: 'station',
    name: '发布',
    lines: ['s1'],
    position: { x: 972, y: 0 },
    who: ['你', '程序'],
    user: true,
    ext: true,
    skills: ['小红书发布', '抖音发布', '公众号发布'],
    recv: '一包三平台稿',
    give: '三个平台的链接',
    dod: [
      { text: '三个平台都返回了链接', auto: true },
      { text: '你点了头', auto: false },
    ],
  },
  {
    id: 'c-end',
    kind: 'end',
    name: '终点',
    lines: ['s1'],
    position: { x: 1080, y: 0 },
  },
];

function edge(id: string, source: string, target: string, lineId: string): FlowEdge {
  return { id, source, target, lineId };
}

const contentEdges: FlowEdge[] = [
  edge('ce1', 'c-start', 'c1', 's1'),
  edge('ce2', 'c1', 'c2', 's1'),
  edge('ce3', 'c2', 'pool', 's1'),
  edge('ce4', 'pool', 'c3', 's1'),
  edge('ce5', 'c3', 'c4', 's1'),
  edge('ce6', 'c4', 'c5', 's1'),
  edge('ce7', 'c5', 'c6a', 's1'),
  edge('ce8', 'c5', 'c6b', 's1'),
  edge('ce9', 'c5', 'c6c', 's1'),
  edge('ce10', 'c6a', 'c-join', 's1'),
  edge('ce11', 'c6b', 'c-join', 's1'),
  edge('ce12', 'c6c', 'c-join', 's1'),
  edge('ce13', 'c-join', 'c8', 's1'),
  edge('ce14', 'c8', 'c-end', 's1'),
];

export const CONTENT_FLOW: Flow = {
  id: 'content-ops',
  name: '内容运营',
  version: 3,
  source: '你自建',
  used: 12,
  lines: contentLines,
  nodes: contentNodes,
  edges: contentEdges,
  versions: [
    { version: 3, at: '10-21', note: '选题改为每天 10:00 从池里取' },
    { version: 2, at: '10-12', note: '加了评审门，三人全过' },
    { version: 1, at: '10-03', note: '从做过的 4 次提炼' },
  ],
  instances: [
    {
      id: 'i1',
      name: '10-27 这一轮',
      version: 3,
      state: 'run',
      stateText: '进行中  在评审门',
      project: { name: 'news-intelligence', code: 'N', color: FLOW_LINE_COLORS[0] },
      nodeStates: {
        'c-start': 'done',
        c1: 'done',
        c2: 'done',
        pool: 'done',
        c3: 'done',
        c4: 'done',
        c5: 'run',
        c6a: 'todo',
        c6b: 'todo',
        c6c: 'todo',
        'c-join': 'todo',
        c8: 'todo',
        'c-end': 'todo',
      },
      overrides: [
        {
          nodeId: 'c3',
          label: '选题：候选 5 个 → 3 个',
          patch: {
            give: '选题单 ×1（3 个候选，各附理由）',
            dod: [
              { text: '正好 3 个候选', auto: true },
              { text: '每个都有来源链接', auto: true },
              { text: '和最近 7 天不重复', auto: true },
              { text: '口味对不对', auto: false },
            ],
          },
        },
      ],
    },
    {
      id: 'i2',
      name: '科技号试运营',
      version: 3,
      state: 'wait',
      stateText: '等你  发布',
      project: { name: 'tech-weekly', code: 'T', color: FLOW_LINE_COLORS[2] },
      nodeStates: {
        'c-start': 'done',
        c1: 'done',
        c2: 'done',
        pool: 'done',
        c3: 'done',
        c4: 'done',
        c5: 'done',
        c6a: 'done',
        c6b: 'done',
        c6c: 'done',
        'c-join': 'done',
        c8: 'wait',
        'c-end': 'todo',
      },
      overrides: [],
    },
    {
      id: 'i3',
      name: '10-20 那一轮',
      version: 2,
      state: 'done',
      stateText: '已完成',
      project: { name: 'news-intelligence', code: 'N', color: FLOW_LINE_COLORS[0] },
      nodeStates: {
        'c-start': 'done',
        c1: 'done',
        c2: 'done',
        pool: 'done',
        c3: 'done',
        c4: 'done',
        c5: 'done',
        c6a: 'done',
        c6b: 'done',
        c6c: 'done',
        'c-join': 'done',
        c8: 'done',
        'c-end': 'done',
      },
      overrides: [],
    },
  ],
};

/* ───────────────────────── AIOS 产品（三条价值流，共享门） ───────────────────────── */

const aiosLines: FlowLine[] = [
  { id: 's1', index: 1, name: 'Kind 体验', what: '新功能', color: FLOW_LINE_COLORS[0] },
  { id: 's2', index: 2, name: '内核稳定', what: '问题和技术债', color: FLOW_LINE_COLORS[1] },
  { id: 's3', index: 3, name: '线上运维', what: '一直开着', color: FLOW_LINE_COLORS[2] },
];

const aiosNodes: FlowNode[] = [
  {
    id: 'a-start-1',
    kind: 'start',
    name: '起点',
    lines: ['s1'],
    position: { x: 0, y: -120 },
    start: 'manual',
  },
  {
    id: 'n1',
    kind: 'station',
    name: '需求澄清',
    lines: ['s1'],
    position: { x: 108, y: -120 },
    who: ['Kind', '你'],
    skills: ['需求访谈'],
    recv: '你的一句话或一个问题单',
    give: '需求说明、验收标准、不做清单',
    dod: [
      { text: '每条需求都有验收标准', auto: true },
      { text: '你确认过', auto: false },
    ],
  },
  {
    id: 'n2',
    kind: 'stack',
    name: '实现',
    lines: ['s1'],
    position: { x: 216, y: -120 },
    fan: '每件子任务一个',
    who: ['工程'],
    skills: ['代码规范', '测试先行'],
    recv: '一件子任务和它的写集',
    give: '改动、测试、交接说明',
    dod: [
      { text: '只改写集里的文件', auto: true },
      { text: '新测试先红后绿', auto: true },
    ],
  },
  {
    id: 'a-start-2',
    kind: 'start',
    name: '起点',
    lines: ['s2'],
    position: { x: 0, y: 0 },
    start: 'cron',
    rhythm: '每天 9:00',
  },
  {
    id: 'm1',
    kind: 'station',
    name: '问题分拣',
    lines: ['s2', 's3'],
    position: { x: 324, y: 0 },
    who: ['排查员'],
    skills: ['问题分拣'],
    recv: '问题池',
    give: '修复单 ×N，每件附复现步骤',
    dod: [{ text: '每件都能复现', auto: true }],
  },
  {
    id: 'm2',
    kind: 'stack',
    name: '修复',
    lines: ['s2', 's3'],
    position: { x: 432, y: 0 },
    fan: '每件一个',
    who: ['工程'],
    skills: ['代码规范', '测试先行'],
    loop: { max: 5, until: '质量门全过' },
    recv: '一件修复单',
    give: '改动、回归测试',
    dod: [{ text: '回归测试先红后绿', auto: true }],
  },
  {
    id: 'a-start-3',
    kind: 'start',
    name: '起点',
    lines: ['s3'],
    position: { x: 0, y: 120 },
    start: 'cron',
    rhythm: '一直开着',
  },
  {
    id: 'o1',
    kind: 'station',
    name: '发布后观察',
    lines: ['s3'],
    position: { x: 108, y: 120 },
    who: ['运维', '程序'],
    skills: ['看错误率', '看关键指标'],
    recv: '每次发布的版本号',
    give: '发现的问题，放进问题池',
    dod: [{ text: '每次发布后 24 小时内看过一遍', auto: true }],
  },
  {
    id: 'pool2',
    kind: 'pool',
    name: '问题池',
    lines: ['s3', 's2'],
    position: { x: 216, y: 120 },
    note: '线上运维发现的问题先攒在池里；内核稳定线的问题分拣每天 9:00 来取。',
    recv: '发布后观察放进来的问题',
    give: '问题分拣来取时一次给它',
  },
  {
    id: 'g1',
    kind: 'gate',
    name: '评审门',
    lines: ['s1', 's2', 's3'],
    position: { x: 540, y: 0 },
    members: ['工程', '审读', '安全'],
    rule: '一票否决',
    fail: '退回实现或修复，三人的意见一起带回。',
    recv: '改动、测试、交接说明',
    give: '过审的改动',
    dod: [{ text: '三人都给出结论', auto: true }],
  },
  {
    id: 'g2',
    kind: 'gate',
    name: '质量门',
    lines: ['s1', 's2', 's3'],
    position: { x: 648, y: 0 },
    members: ['程序'],
    rule: '全部通过',
    fail: '退回实现；修复线回到修复，上限 5 次。',
    recv: '过审的改动',
    give: '测试报告、DoD 证据',
    dod: [
      { text: '全部测试通过', auto: true },
      { text: '每条 DoD 都有证据', auto: true },
    ],
  },
  {
    id: 'g3',
    kind: 'gate',
    name: '运维门',
    lines: ['s1', 's2', 's3'],
    position: { x: 756, y: 0 },
    members: ['程序', '你'],
    rule: '发布检查全过，再等你点头',
    fail: '停下，等你决定。',
    recv: '测试报告、改动',
    give: '可以发布的版本',
    dod: [
      { text: '发布检查全过', auto: true },
      { text: '你点了头', auto: false },
    ],
  },
  {
    id: 'r1',
    kind: 'station',
    name: '发布',
    lines: ['s1', 's2', 's3'],
    position: { x: 864, y: 0 },
    who: ['程序', '你'],
    user: true,
    ext: true,
    skills: ['发版'],
    recv: '可以发布的版本',
    give: '版本号、更新说明',
    dod: [
      { text: '版本号和更新说明都在', auto: true },
      { text: '你点了头', auto: false },
    ],
  },
  {
    id: 'a-end',
    kind: 'end',
    name: '终点',
    lines: ['s1', 's2', 's3'],
    position: { x: 972, y: 0 },
  },
];

const aiosEdges: FlowEdge[] = [
  edge('ae1', 'a-start-1', 'n1', 's1'),
  edge('ae2', 'n1', 'n2', 's1'),
  edge('ae3', 'n2', 'g1', 's1'),
  edge('ae4', 'a-start-2', 'm1', 's2'),
  edge('ae5', 'm1', 'm2', 's2'),
  edge('ae6', 'm2', 'g1', 's2'),
  edge('ae7', 'a-start-3', 'o1', 's3'),
  edge('ae8', 'o1', 'pool2', 's3'),
  edge('ae9', 'pool2', 'm1', 's3'),
  // 评审门之后三条价值流共用同一段路：一条连线、三条线并排画（修复 3）
  { id: 'ae10', source: 'g1', target: 'g2', lineId: 's1', lineIds: ['s1', 's2', 's3'] },
  { id: 'ae11', source: 'g2', target: 'g3', lineId: 's1', lineIds: ['s1', 's2', 's3'] },
  { id: 'ae12', source: 'g3', target: 'r1', lineId: 's1', lineIds: ['s1', 's2', 's3'] },
  { id: 'ae13', source: 'r1', target: 'a-end', lineId: 's1', lineIds: ['s1', 's2', 's3'] },
];

export const AIOS_FLOW: Flow = {
  id: 'aios-product',
  name: 'AIOS 产品',
  version: 2,
  source: '导入自 unFIX 社区模板，未带权限',
  used: 3,
  lines: aiosLines,
  nodes: aiosNodes,
  edges: aiosEdges,
  versions: [
    { version: 2, at: '10-25', note: '三条线共用评审门、质量门和运维门' },
    { version: 1, at: '10-19', note: '从 unFIX 模板导入' },
  ],
  instances: [
    {
      id: 'a1',
      name: 'AIOS 产品  此刻',
      version: 2,
      state: 'run',
      stateText: '在途 8 件',
      project: { name: 'AIOS 产品', code: 'A', color: FLOW_LINE_COLORS[1] },
      nodeStates: {
        'a-start-1': 'done',
        n1: 'wait',
        n2: 'run',
        'a-start-2': 'done',
        m1: 'run',
        m2: 'run',
        'a-start-3': 'done',
        o1: 'run',
        pool2: 'done',
        g1: 'run',
        g2: 'run',
        g3: 'wait',
        r1: 'todo',
        'a-end': 'todo',
      },
      overrides: [],
    },
  ],
};

export const SEED_FLOWS: Flow[] = [CONTENT_FLOW, AIOS_FLOW];

/* ───────────────────────── 新建流程的样例草稿 ───────────────────────── */

const draftLine = (): FlowLine[] => [{ id: 's1', index: 1, name: '未命名', what: '草稿', color: FLOW_LINE_COLORS[0] }];

const blankFlow: Flow = {
  id: 'draft-blank',
  name: '未命名流程',
  version: 0,
  draft: true,
  source: '空白开始',
  used: 0,
  lines: draftLine(),
  nodes: [
    { id: 'b-start', kind: 'start', name: '起点', lines: ['s1'], position: { x: 0, y: 0 }, start: 'manual' },
    {
      id: 'b1',
      kind: 'station',
      name: '新的一站',
      lines: ['s1'],
      position: { x: 108, y: 0 },
      who: [],
      skills: [],
      recv: '',
      give: '',
      dod: [],
    },
    { id: 'b-end', kind: 'end', name: '终点', lines: ['s1'], position: { x: 216, y: 0 } },
  ],
  edges: [edge('be1', 'b-start', 'b1', 's1'), edge('be2', 'b1', 'b-end', 's1')],
  versions: [],
  instances: [],
};

const sayFlow: Flow = {
  id: 'draft-say',
  name: '开源动态早报',
  version: 0,
  draft: true,
  source: 'Kind 按你的一句话起草',
  used: 0,
  lines: [{ id: 's1', index: 1, name: '早报', what: '每天 8:00', color: FLOW_LINE_COLORS[0] }],
  nodes: [
    {
      id: 'o-start',
      kind: 'start',
      name: '起点',
      lines: ['s1'],
      position: { x: 0, y: 0 },
      start: 'cron',
      rhythm: '每天 8:00',
    },
    {
      id: 'o1',
      kind: 'stack',
      name: '看每个项目',
      lines: ['s1'],
      position: { x: 108, y: 0 },
      fan: '每个项目一遍',
      who: ['资讯员'],
      skills: ['GitHub 读取'],
      recv: '你关注的 5 个开源项目',
      give: '每个项目昨天以来的新 issue',
      dod: [{ text: '5 个项目都看过', auto: true }],
      auto: 'Kind：「看每个项目」做成了逐条展开，以后你加项目不用改流程。',
    },
    {
      id: 'o2',
      kind: 'station',
      name: '汇总',
      lines: ['s1'],
      position: { x: 216, y: 0 },
      who: ['Kind'],
      skills: ['摘要'],
      recv: '各项目的新 issue',
      give: '早报 ×1（每个项目三行以内）',
      dod: [
        { text: '每个项目都有一段', auto: true },
        { text: '写清哪些和你有关', auto: false },
      ],
    },
    {
      id: 'o3',
      kind: 'station',
      name: '发给我',
      lines: ['s1'],
      position: { x: 324, y: 0 },
      who: ['程序'],
      skills: ['站内消息'],
      recv: '早报',
      give: '一条站内消息',
      dod: [{ text: '消息已送达', auto: true }],
    },
    { id: 'o-end', kind: 'end', name: '终点', lines: ['s1'], position: { x: 432, y: 0 } },
  ],
  edges: [
    edge('oe1', 'o-start', 'o1', 's1'),
    edge('oe2', 'o1', 'o2', 's1'),
    edge('oe3', 'o2', 'o3', 's1'),
    edge('oe4', 'o3', 'o-end', 's1'),
  ],
  versions: [],
  instances: [],
};

const weeklyFlow: Flow = {
  id: 'draft-weekly',
  name: '周报',
  version: 0,
  draft: true,
  source: '从做过的 6 次提炼（10-03 起）',
  used: 0,
  lines: [{ id: 's1', index: 1, name: '周报', what: '每周五 16:00', color: FLOW_LINE_COLORS[0] }],
  nodes: [
    {
      id: 'w-start',
      kind: 'start',
      name: '起点',
      lines: ['s1'],
      position: { x: 0, y: 0 },
      start: 'cron',
      rhythm: '每周五 16:00',
    },
    {
      id: 'w1',
      kind: 'station',
      name: '收本周进展',
      lines: ['s1'],
      position: { x: 108, y: 0 },
      who: ['Kind'],
      skills: ['读任务记录'],
      recv: '本周关掉的任务和提交',
      give: '进展清单',
      dod: [{ text: '每个项目都有一条', auto: true }],
    },
    {
      id: 'w2',
      kind: 'station',
      name: '工程汇总',
      lines: ['s1'],
      position: { x: 216, y: -104 },
      who: ['工程'],
      recv: '进展清单里的工程部分',
      give: '工程小结',
      dod: [],
    },
    {
      id: 'w3',
      kind: 'station',
      name: '运营汇总',
      lines: ['s1'],
      position: { x: 216, y: 104 },
      who: ['运营'],
      recv: '进展清单里的运营部分',
      give: '运营小结',
      dod: [],
    },
    {
      id: 'w-join',
      kind: 'join',
      name: '汇合',
      lines: ['s1'],
      position: { x: 324, y: 0 },
      join: null,
      recv: '两份小结',
      give: '可以写的周报素材',
    },
    {
      id: 'w4',
      kind: 'station',
      name: '写周报',
      lines: ['s1'],
      position: { x: 432, y: 0 },
      who: ['写手'],
      skills: ['周报格式'],
      recv: '两份小结',
      give: '',
      dod: [
        { text: '不超过一页', auto: true },
        { text: '每条有数字', auto: false },
      ],
    },
    {
      id: 'w5',
      kind: 'station',
      name: '发给老板',
      lines: ['s1'],
      position: { x: 540, y: 0 },
      who: ['程序'],
      ext: true,
      user: true,
      skills: ['邮件'],
      recv: '周报',
      give: '已发出的邮件',
      dod: [{ text: '邮件已发出', auto: true }],
      auto: 'Kind 自动加上「每次等你」：对外发出去收不回。前 6 次你也都是看过才发的。',
    },
    { id: 'w-end', kind: 'end', name: '终点', lines: ['s1'], position: { x: 648, y: 0 } },
  ],
  edges: [
    edge('we1', 'w-start', 'w1', 's1'),
    edge('we2', 'w1', 'w2', 's1'),
    edge('we3', 'w1', 'w3', 's1'),
    edge('we4', 'w2', 'w-join', 's1'),
    edge('we5', 'w3', 'w-join', 's1'),
    edge('we6', 'w-join', 'w4', 's1'),
    edge('we7', 'w4', 'w5', 's1'),
    edge('we8', 'w5', 'w-end', 's1'),
  ],
  versions: [],
  instances: [],
};

const writingFlow: Flow = {
  id: 'draft-writing',
  name: '写作与审校',
  version: 0,
  draft: true,
  source: '来自内置模板「写作与审校」',
  used: 0,
  lines: [{ id: 's1', index: 1, name: '写作', what: '一篇一篇', color: FLOW_LINE_COLORS[0] }],
  nodes: [
    { id: 'x-start', kind: 'start', name: '起点', lines: ['s1'], position: { x: 0, y: 0 }, start: 'manual' },
    {
      id: 'x1',
      kind: 'station',
      name: '写初稿',
      lines: ['s1'],
      position: { x: 108, y: 0 },
      who: ['写手'],
      skills: ['写作'],
      recv: '题目和要求',
      give: '初稿 ×1',
      dod: [{ text: '字数在要求内', auto: true }],
    },
    {
      id: 'x2',
      kind: 'gate',
      name: '审读门',
      lines: ['s1'],
      position: { x: 216, y: 0 },
      members: ['审读', '事实核查'],
      rule: '全部通过',
      fail: '退回修改。',
      recv: '初稿',
      give: '过审稿、审读意见',
      dod: [{ text: '两人都给出结论', auto: true }],
    },
    {
      id: 'x3',
      kind: 'station',
      name: '修改',
      lines: ['s1'],
      position: { x: 324, y: 0 },
      who: ['写手'],
      loop: { max: 3, until: '审读门全过' },
      recv: '审读意见',
      give: '定稿 ×1',
      dod: [{ text: '每条意见都有回应', auto: false }],
    },
    { id: 'x-end', kind: 'end', name: '终点', lines: ['s1'], position: { x: 432, y: 0 } },
  ],
  edges: [
    edge('xe1', 'x-start', 'x1', 's1'),
    edge('xe2', 'x1', 'x2', 's1'),
    edge('xe3', 'x2', 'x3', 's1'),
    edge('xe4', 'x3', 'x-end', 's1'),
  ],
  versions: [],
  instances: [],
};

const podcastFlow: Flow = {
  id: 'draft-podcast',
  name: '播客剪辑',
  version: 0,
  draft: true,
  source: '导入自分享链接  未带权限、未带本机路径',
  used: 0,
  lines: [{ id: 's1', index: 1, name: '播客', what: '一期一期', color: FLOW_LINE_COLORS[0] }],
  nodes: [
    { id: 'p-start', kind: 'start', name: '起点', lines: ['s1'], position: { x: 0, y: 0 }, start: 'manual' },
    {
      id: 'p1',
      kind: 'stack',
      name: '选段',
      lines: ['s1'],
      position: { x: 108, y: 0 },
      fan: '每期一遍',
      who: ['编辑'],
      skills: ['听写', '找高光'],
      recv: '一期录音',
      give: '高光片段清单',
      dod: [{ text: '每段都有起止时间', auto: true }],
      auto: '导入的流程不带权限：第一次跑时，会把要读写的范围重新给你看一遍。',
    },
    {
      id: 'p2',
      kind: 'station',
      name: '剪辑',
      lines: ['s1'],
      position: { x: 216, y: 0 },
      who: ['改编'],
      skills: ['音频剪辑'],
      recv: '高光片段清单',
      give: '成片 ×1',
      dod: [{ text: '时长 20 分钟内', auto: true }],
    },
    {
      id: 'p3',
      kind: 'gate',
      name: '审听门',
      lines: ['s1'],
      position: { x: 324, y: 0 },
      members: ['审读'],
      rule: '全部通过',
      fail: '退回剪辑。',
      recv: '成片',
      give: '过审成片',
      dod: [{ text: '审听过', auto: true }],
    },
    {
      id: 'p4',
      kind: 'station',
      name: '发布',
      lines: ['s1'],
      position: { x: 432, y: 0 },
      who: ['程序', '你'],
      ext: true,
      user: true,
      skills: ['播客平台发布'],
      recv: '过审成片',
      give: '节目链接',
      dod: [{ text: '平台返回了链接', auto: true }],
    },
    { id: 'p-end', kind: 'end', name: '终点', lines: ['s1'], position: { x: 540, y: 0 } },
  ],
  edges: [
    edge('pe1', 'p-start', 'p1', 's1'),
    edge('pe2', 'p1', 'p2', 's1'),
    edge('pe3', 'p2', 'p3', 's1'),
    edge('pe4', 'p3', 'p4', 's1'),
    edge('pe5', 'p4', 'p-end', 's1'),
  ],
  versions: [],
  instances: [],
};

function flowCopy(base: Flow, patch: Partial<Flow>): Flow {
  return JSON.parse(JSON.stringify({ ...base, ...patch })) as Flow;
}

const contentTemplate = flowCopy(CONTENT_FLOW, {
  id: 'draft-tpl-content',
  name: '内容运营（副本）',
  version: 0,
  draft: true,
  source: '来自内置模板「内容运营」',
  used: 0,
  versions: [],
  instances: [],
});

const unfixTemplate = flowCopy(AIOS_FLOW, {
  id: 'draft-tpl-unfix',
  name: 'unFIX 多线（副本）',
  version: 0,
  draft: true,
  source: '来自社区模板，导入时不带权限',
  used: 0,
  versions: [],
  instances: [],
});


/* ───────────────────────── 压测流程（只在开发开关下出现，不进正常列表） ───────────────────────── */

/**
 * 程序生成的 60 节点压测流程：四条线（够触发 ≥4 条线的图例淡出）、两处分叉汇合，不写业务文案。
 * 只用于测速表里的「流程画布 60 节点拖动与平移」；正常列表里看不到它
 * （FlowEditor 只在开发开关打开时才向 api 要它）。
 */
export function makeStressFlow(): Flow {
  const lines: FlowLine[] = [
    { id: 'st1', index: 1, name: '压测线 1', what: '压测', color: FLOW_LINE_COLORS[0] },
    { id: 'st2', index: 2, name: '压测线 2', what: '压测', color: FLOW_LINE_COLORS[1] },
    { id: 'st3', index: 3, name: '压测线 3', what: '压测', color: FLOW_LINE_COLORS[2] },
    { id: 'st4', index: 4, name: '压测线 4', what: '压测', color: FLOW_LINE_COLORS[3] },
  ];
  const nodes: FlowNode[] = [];
  const edges: FlowEdge[] = [];
  const link = (source: string, target: string, lineId: string): void => {
    edges.push({ id: 'se' + edges.length, source, target, lineId });
  };

  let serial = 0;
  const pushNode = (id: string, kind: FlowNodeKind, name: string, x: number, y: number, lineId: string): void => {
    nodes.push({ id, kind, name, lines: [lineId], position: { x, y } });
    serial += 1;
  };

  // 四条线（图例淡出要有 ≥4 条线才能触发），节点数仍是 60：15 + 15 + 14 + 14 + 2 个旁路。
  const rowSizes = [15, 15, 14, 14];
  const rowY = [0, 220, 440, 660];
  for (let row = 0; row < rowSizes.length; row += 1) {
    const line = lines[row];
    const ids: string[] = [];
    for (let col = 0; col < rowSizes[row]; col += 1) {
      const id = 'st' + (row + 1) + '-' + col;
      ids.push(id);
      const kind: FlowNodeKind = col === 0 ? 'start' : col === rowSizes[row] - 1 ? 'end' : 'station';
      pushNode(id, kind, kind === 'start' ? '起点' : kind === 'end' ? '终点' : '站 ' + (row + 1) + '-' + (col + 1), col * 150, rowY[row], line.id);
    }
    for (let col = 0; col < ids.length - 1; col += 1) link(ids[col], ids[col + 1], line.id);
  }

  // 两处分叉 + 汇合（各插一个旁路站，两条线各一处）
  const branches: [number, number, number][] = [
    [1, 5, 9],
    [2, 6, 10],
  ];
  for (const [row, startCol, joinCol] of branches) {
    const line = lines[row - 1];
    const bypassId = 'st' + row + '-bypass';
    pushNode(bypassId, 'station', '旁路 ' + row, joinCol * 150 - 150, rowY[row - 1] + 110, line.id);
    link('st' + row + '-' + startCol, bypassId, line.id);
    link(bypassId, 'st' + row + '-' + joinCol, line.id);
  }

  return {
    id: 'stress-60',
    name: '压测流程 · 60 节点',
    version: 0,
    source: '开发开关生成',
    used: 0,
    lines,
    nodes,
    edges,
    versions: [],
    instances: [],
    draft: true,
  };
}

export const NEW_FLOW_DRAFTS: Record<'blank' | 'say' | 'weekly' | 'writing' | 'podcast', Flow> = {
  blank: blankFlow,
  say: sayFlow,
  weekly: weeklyFlow,
  writing: writingFlow,
  podcast: podcastFlow,
};

/** 三档内置流程的最小模板：站就是这几步（谁做/门槛第 3 期再补，先给结构）。 */
function builtinTemplateFlow(id: string, name: string, steps: string[]): Flow {
  const nodes: FlowNode[] = [
    { id: id + '-start', kind: 'start', name: '起点', lines: ['s1'], position: { x: 0, y: 40 }, start: 'manual' },
  ];
  steps.forEach((title, index) => {
    nodes.push({ id: id + '-n' + index, kind: 'station', name: title, lines: ['s1'], position: { x: (index + 1) * 150, y: 40 } });
  });
  nodes.push({ id: id + '-end', kind: 'end', name: '终点', lines: ['s1'], position: { x: (steps.length + 1) * 150, y: 40 } });
  const edges: FlowEdge[] = [];
  for (let index = 0; index < nodes.length - 1; index += 1) {
    edges.push({ id: id + '-e' + index, source: nodes[index].id, target: nodes[index + 1].id, lineId: 's1' });
  }
  return {
    id,
    name,
    version: 0,
    source: '内置流程（只读模板）',
    used: 0,
    draft: true,
    lines: [{ id: 's1', index: 1, name, what: '内置流程', color: FLOW_LINE_COLORS[0] }],
    nodes,
    edges,
    versions: [],
    instances: [],
  };
}

export const TEMPLATE_OPTIONS: NewFlowTemplate[] = [
  {
    key: 'builtin-light',
    title: '轻档 · 直接做完',
    description: '内置流程（只读） 小事：Kind 直接完成 → 交回成果，不设门',
    flow: builtinTemplateFlow('tpl-light', '轻档 · 直接做完', ['Kind 直接完成', '交回成果']),
  },
  {
    key: 'builtin-normal',
    title: '普通档 · 先给任务图',
    description: '内置流程（只读） 路线草稿 → 你确认 → 执行 → 交回成果',
    flow: builtinTemplateFlow('tpl-normal', '普通档 · 先给任务图', ['路线草稿', '你确认', '执行', '交回成果']),
  },
  {
    key: 'builtin-pair',
    title: '配对档 · 有人验',
    description: '内置流程（只读） 执行 → 审查门 → 修正 → 复核 → 交回成果',
    flow: builtinTemplateFlow('tpl-pair', '配对档 · 有人验', ['执行', '审查门', '修正', '复核', '交回成果']),
  },
  {
    key: 'content',
    title: '内容运营',
    description: '内置  单价值流：收集 → 选题 → 逐条写 → 评审门 → 三平台 → 发布',
    flow: contentTemplate,
  },
  {
    key: 'writing',
    title: '写作与审校',
    description: '内置  写 → 审读门 → 改，循环到过',
    flow: writingFlow,
  },
  {
    key: 'unfix',
    title: 'unFIX 多线',
    description: '社区模板  三条线共用评审门和质量门；导入不带权限',
    flow: unfixTemplate,
  },
];

/** 拖到画布上时的默认零件。start / end 不放进零件栏，由新建流程自动给。 */
export interface PalettePart {
  kind: 'station' | 'user' | 'stack' | 'gate' | 'pool' | 'join';
  title: string;
  description: string;
  nodeKind: FlowNodeKind;
}

export const PALETTE_PARTS: PalettePart[] = [
  { kind: 'station', title: '站', description: '一个角色做一件事', nodeKind: 'station' },
  { kind: 'user', title: '等你确认', description: '轮到时停下等你点头', nodeKind: 'station' },
  { kind: 'stack', title: '逐条展开', description: '每条跑一遍，条数运行时才知道', nodeKind: 'stack' },
  { kind: 'gate', title: '门', description: '几个人一起审，写明怎么算过', nodeKind: 'gate' },
  { kind: 'pool', title: '池', description: '上游攒着，下游按节奏来取', nodeKind: 'pool' },
  { kind: 'join', title: '汇合', description: '几支在这里并回来，写明怎么汇合', nodeKind: 'join' },
];
