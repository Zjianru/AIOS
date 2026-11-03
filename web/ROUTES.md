# ROUTES.md — v13 全部路由 / data-go 目标，与第 1/2/3 期归属

依据：`prototype/v13.html` 的 `V9.route(...)` 注册表（`settings` `usage` `projset` `newproj`
`welcome` `empty` `memory` `health` `notify` `monitor`）、`go7()/go8()` 的分支页面
（`now` `kind` `work` `project` `task` `trace` `team` `role` `ability` `lib` `read`）、
以及全文 `data-go="…"` 目标。v14 新增 `flow:<id>`（即 v0 的「策略编辑器」：用户 2026-10-27 裁定界面改叫「流程」、代码标识用 flow，见 `strategy-editor-v1-brief.md` §一）。

路由形态沿用 v13：hash 路由，名字照旧（`#now`、`#kind`、`#task:B2`）。
查询参数 `?wv=board|cal` 等沿用 v13 的用法。期数为「做这页的那一期」；第 1 期只做标记 p1 的。

**第 2 期交付（2026-10-28）**：p2 的路由已全部实现——`#now` `#work` `#project:<id>` `#task:<id>` `#trace:<id>`
`#read:<文件名>` `#empty:now`；`#kind` 与 `#ability` 的第 2 期改动（右栏、内置流程）见 `PHASE2-DELTA.md`。
`#read` / `#trace` 在 v14 里各自单独一个场景（dock 面板在同一场景内换路由不会重渲染）。

**设置 › 模型与连接（2026-11-01）**：用户定 Keyring 设置页现在就做，先补进原型。`#settings:*` 进设置场景（左栏 8 组），
只有 `#settings:model` 做出来了，其余分组仍写「第 3 期」。首次使用（开发开关「首次使用（空库）」）的接模型一步
和这页是同一套假 Keyring（`src/data/api.ts`）：同一份提供方目录，存进去的连接两边都看得到。
原型约定：以 `sk-bad` 开头的密钥模拟被服务商拒绝。

**接上内核的部分（2026-11-01，T007/T008 的参照）**：工具栏「内核状态」菜单（`data-dev="kernel-menu"`）。
- 「只显示接上内核的部分」（`connectedOnly`）：侧栏只剩 Kind 和设置；`#kind` 只有对话一栏（无右栏、无带上芯片、无「指一下」、无费用）；
  其他路由跳回 `#kind`；设置只剩「模型与连接」，提供方目录只有 DeepSeek（D36），首次使用同样只有 DeepSeek、不提中转站。
  **T007/T008 交付的就是这个视图。**
- 内核状态单选：正常 / 连不上服务 / 令牌失效 / 对话找不到 / 日志读不懂 / 被占用 → `#kind` 整页换成对应的说明页（`KernelGate.tsx`）。
- 演示一轮：提问卡（整批作答 / 不答）、模型重试、失败（模型 / 密钥被拒 / Keyring 读不出）、自动压缩、断线、消息没送到、空对话、恢复样例。
  平常发消息就会演工具一条条跑、流式回答；运行中发送键换成停止（Esc 同效）。底栏：模型选择、占用 % 与手动压缩（悬停可见）、等你回答的提示。
- 「Keyring 读不出来」：设置页顶部出横幅（指到新位置 / 重新开始），各提供商标红、不能验证；对话里的失败块指去设置。
- 测试钩子：`window.__v14.setConnectedOnly(b)`、`setKernel(state)`、`kindDemo(name)`、`setKeyringBroken(b)`。
- 只属于原型的东西不进产品：「原型：」开头的说明、「AIOS 概念 ·」注释行、「内核状态」「开发」两个菜单、工具栏原型徽标。

## 一、页面路由

| # | 路由 | v13 页面 | 期 |
|---|---|---|---|
| 1 | `#now` | 现在（首页） | p2 |
| 2 | `#kind` | Kind 全页（对话 \| 在做的事） | **p1** |
| 3 | `#work` | 工作：看板 / 日历（`?wv=board` / `?wv=cal`） | p2 |
| 4 | `#project:<projectId>` | 项目页（wsp / news / daily） | p2 |
| 5 | `#task:<taskId>` | 任务详情：地铁线、三层深度、这件事的账、里程碑 | p2 |
| 6 | `#trace:<taskId>` | 原始记录（第三层） | p2 |
| 7 | `#team` | 团队（角色名册） | p3 |
| 8 | `#role:<index>` | 角色详情（概况/工作方式/装备/知识/成长/履历） | p3 |
| 9 | `#ability` | 能力库（含「流程」分组入口） | **p1**（页面骨架 + 流程分组 + 编辑器入口）；p3 补齐「能力库其余部分」 |
| 10 | `#lib` | 资料 | p3 |
| 11 | `#read:<文件名>` | 阅读页（资料与成果共用） | p2 |
| 12 | `#memory` | 记忆 | p3 |
| 13 | `#health` | AIOS 健康状况 | p3 |
| 14 | `#notify` | 什么时候会通知你 | p3 |
| 15 | `#monitor` | 监控（全部 AIOS） | p3 |
| 16 | `#monitor:kind` | 监控（这次 Kind 对话） | p3 |
| 17 | `#monitor:task:<taskId>` | 监控（一件任务） | p3 |
| 18 | `#settings` | 设置（默认落到「外观」） | p3 |
| 19 | `#settings:look` | 设置 · 外观 | p3 |
| 20 | `#settings:model` | 设置 · 模型与连接（Keyring） | **已做**（2026-11-01，提前于 p3） |
| 21 | `#settings:kind` | 设置 · Kind | p3 |
| 22 | `#settings:exec` | 设置 · 执行 | p3 |
| 23 | `#settings:notify` | 设置 · 通知 | p3 |
| 24 | `#settings:privacy` | 设置 · 隐私与更新 | p3 |
| 25 | `#settings:monitor` | 设置 · 监控 | p3 |
| 26 | `#settings:about` | 设置 · 关于 | p3 |
| 27 | `#usage` | 用量与费用 | p3 |
| 28 | `#usage:<taskId>` | 用量与费用（某件任务） | p3 |
| 29 | `#projset:<projectId>` | 项目设置 | p3 |
| 30 | `#newproj` | 新建项目 | p3 |
| 31 | `#welcome` | 第一次打开 · 引导 | p3 |
| 32 | `#welcome:model` | 引导 · 连模型 | p3 |
| 33 | `#welcome:project` | 引导 · 建第一个项目 | p3 |
| 34 | `#welcome:done` | 引导 · 完成 | p3 |
| 35 | `#empty:now` | 空状态预览 · 现在 | p2（「现在」的空态） |
| 36 | `#empty:project` | 空状态预览 · 新项目 | p3 |
| 37 | `#empty:lib` | 空状态预览 · 资料 | p3 |
| 38 | `#empty:team` | 空状态预览 · 团队 | p3 |
| 39 | `#flow:<flowId>` | 流程编辑器（v14 新增；入口在 `#ability` 的「流程」分组） | **p1** |

## 二、不是路由、但要算进去的界面

| 界面 | v13 形态 | 期 |
|---|---|---|
| 全局搜索 `⌘K` | 覆盖层（`data-e-search`，无 hash 路由） | p3 |
| 问 Kind 召唤栏 `⌥Space` | 覆盖层（`#summon`） | p3（§三不含它） |
| 右侧抽屉 sheet | `#sheet` 覆盖层 | p3 |
| 布局切换器（工具栏） | v13-o `经典 / 专注 / 工作台 / 我的布局` | **p1 只做「默认布局 / 重置我的布局」**；四套预设与 `切预设` 动效（动效 22）归 p3 |
| 面板 Dock / 浮窗 / 五区吸附 | v13-o（`data-o-lay`、`V9.layout.*`） | p3 |
| 场景默认布局 | v13 各页自带固定分栏 | **p1**（每场景一个 dock 根 + 默认布局 + 按场景存） |

## 三、p1 里第 2/3 期的页面怎么处理

`#now` `#work` `#team` `#lib` `#memory` `#health` `#notify` `#settings:*` `#usage:*`
`#newproj` `#projset:*` `#welcome:*` `#project:*` `#task:*` `#trace:*` `#role:*` `#read:*`
`#monitor:*` `#empty:*` 在第 1 期都**注册路由**，渲染成占位页：写明「第 2 期」或「第 3 期」，
并保留面包屑、侧栏当前项与 `data-go` 目标，保证路由表完整、切场景可测。
