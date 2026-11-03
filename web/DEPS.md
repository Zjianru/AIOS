# DEPS.md — 依赖清单（第 1 期）

规则（app-v14-brief.md §一）：版本写死、package.json 不用 ^、选发布满两周的版本、装前看 LICENSE。
「满两周」按 2026-10-27 算，即发布日期 ≤ 2026-10-13。下面每行的日期是 npm 上的发布时间。

**依赖总数：运行时 20 个 + 构建期 9 个 = 29 个**（npm install 装了 170 个包，其余是这 29 个的直接/间接依赖）。

## 一、需求表里点名的（不算额外依赖）

| 包 | 版本 | 发布日 | LICENSE | 用途 |
|---|---|---|---|---|
| @xyflow/react | 12.12.0 | 2026-09-24 | MIT | 流程画布 |
| dockview | 8.5.1 | 2026-10-10 | MIT | 分栏 / Dock 根 / 浮窗基础 |
| elkjs | 0.12.0 | 2026-07-17 | EPL-2.0 OR GPL-3.0-or-later | 流程整理排版 |
| lucide-react | 1.56.0 | 2026-10-13 | ISC | 图标 |
| motion | 14.2.0 | 2026-10-13 | MIT | 动效（LazyMotion） |
| react | 19.3.0 | 2026-09-09 | MIT | UI 运行时 |
| react-dom | 19.3.0 | 2026-09-09 | MIT | UI 运行时 |
| react-markdown | 10.1.0 | 2025-03-07 | MIT | Markdown |
| react-router | 8.4.0 | 2026-09-15 | MIT | hash 路由（v13 路由名照旧） |
| react-virtuoso | 4.18.19 | 2026-10-13 | MIT | 长列表（Virtuoso） |
| remark-gfm | 4.0.1 | 2025-02-10 | MIT | 表格 / 删除线 |
| zustand | 5.0.15 | 2026-08-13 | MIT | 状态 |

## 二、表以外新增的（按 §一 要求逐条说明为什么、多大、有没有替代）

| 包 | 版本 | 发布日 | LICENSE | 用途 |
|---|---|---|---|---|
| @fontsource-variable/overpass | 5.3.0 | 2026-07-19 | OFL-1.1 | 标识字体 Overpass（visual-plan-v12 的线路代码/数字字体） |
| class-variance-authority | 0.7.1 | 2024-11-26 | Apache-2.0 | shadcn 变体 |
| clsx | 2.1.1 | 2024-04-23 | MIT | cn() |
| cmdk | 1.1.1 | 2025-03-14 | MIT | 命令面板（第 3 期） |
| dockview-react | 8.5.1 | 2026-10-10 | MIT | dockview 的 React 绑定（dockview 8 起核心与 React 分包） |
| radix-ui | 1.8.1 | 2026-10-12 | MIT | shadcn/ui 底层（v4 版统一包） |
| sonner | 2.0.8 | 2026-08-09 | MIT | 撤销提示条 / toast |
| tailwind-merge | 3.7.0 | 2026-09-12 | MIT | cn() |

新增的这几个的取舍：

- **@fontsource-variable/overpass**：visual-plan-v12.md 指定标识字体 Overpass，只用在数字/时间/线路代码上；打包进产物，不走外网字体。
- **class-variance-authority**：shadcn 生成的按钮等组件用它写变体；改它就得改组件源码，而需求要求只改样式 token。
- **clsx**：shadcn 的 cn() 依赖。
- **cmdk**：shadcn 的 command 组件依赖；第 1 期没用到（全局搜索是第 3 期），先用 shadcn 生成时一起装上。
- **dockview-react**：dockview 8.x 的核心包不再带 React 组件，React 绑定单独发在 dockview-react；两者同版本（8.5.1）。
- **radix-ui**：shadcn/ui 的 Tailwind-4 组件按官方 registry（new-york-v4）生成，底层用统一包 radix-ui，不再逐个装 @radix-ui/react-*。
- **sonner**：v10-tables §3.4「任何会改变东西的动作都给撤销提示条」。
- **tailwind-merge**：shadcn 的 cn() 依赖。

构建期依赖（不进产物）：

| 包 | 版本 | 发布日 | LICENSE | 用途 |
|---|---|---|---|---|
| @tailwindcss/vite | 4.3.3 | 2026-07-16 | MIT | Tailwind 4 的 Vite 插件 |
| tailwindcss | 4.3.3 | 2026-07-16 | MIT | 样式 |
| typescript | 7.0.2 | 2026-07-08 | Apache-2.0 | TS strict |
| vite | 8.3.4 | 2026-10-08 | MIT | 构建；npm run build 出纯静态页 |
| @types/node | 26.6.5 | 2026-10-09 | MIT | TS 类型 |
| @types/react | 19.3.0 | 2026-09-09 | MIT | TS 类型 |
| @types/react-dom | 19.3.0 | 2026-09-09 | MIT | TS 类型 |
| @vitejs/plugin-react | 6.1.2 | 2026-10-05 | MIT | Vite 跑 React |
| tw-animate-css | 1.4.0 | 2025-09-24 | MIT | Tailwind 4 的 animate 预设（shadcn 要求） |

## 三、体积

产物 dist/ 的总大小与 gzip 大小见 perf/p1-report.md / perf/p2-report.md（perf.mjs、perf-p2.mjs 每次都重新算）。

**第 2 期没有新增依赖**：新增页面和组件（地铁线小部件、看板、任务页、项目页、阅读页、diff、内置流程）都只用上表已有的包。

## 四、没有采用的东西

- @vitejs/plugin-react 的 peer 里出现的 oxc-transform-react / @rolldown/plugin-babel / babel-plugin-react-compiler 都是可选 peer，本项目没装，vite build 正常。
- 没有引入 @tailwindcss/typography：markdown 元素的样式写在 src/index.css 的 base 层（十几条），不值得再加一个插件。
- 没有引入图表库 / 状态机库：第 1 期用不到。
- shadcn/ui 的组件按官方 registry 的 new-york-v4 取（与 Tailwind 4 对应），之后没有手改行为，只把颜色/圆角/宽度接到本项目的 token 上。

