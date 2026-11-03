# AIOS 前端预演 v14

设计原型：用将来产品打算用的技术栈，把 v13 整个原型重做一遍，流程编辑器嵌在里面。
**不进 AIOS 仓库，不走单元门禁**；不替换 web/。需求见 ../prototype/app-v14-brief.md。

## 跑起来

    npm install
    npm run dev                 # 开发：http://localhost:5184
    npm run build               # 出纯静态页到 dist/
    npx vite preview --port 4184 --strictPort   # 用产物（测速/截图都走这个）

## 测速与截图

    node perf.mjs     # 第 1 期测速；跑前看测速锁 ~/develop/aios-d14-audit/perf-lock.md
    node shoot.mjs    # 第 1 期截图到 ../proto-shots/v14-p1/，旁边带同状态的 v13 对照图
    node perf-p2.mjs  # 第 2 期测速（第 1 期 11 项 + 任务页/原始记录打开）；同样要锁
    node shoot-p2.mjs # 第 2 期全量重拍到 ../proto-shots/v14-p2/（94 组：第 1 期 55 组重拍 + 新增 39 组；另 2 张特写）

## 目录

    ROUTES.md       v13 全部路由 + data-go 目标，每条标在第几期（唯一的路由清单）
    DEPS.md         依赖清单：版本、发布日、LICENSE、为什么加
    QUESTIONS.md    没定下来的、以及我自己做的取舍
    PHASE2-DELTA.md 第 2 期差异记录：需求覆盖、页面变化、缺口、提交列表
    src/app/        路由解析、场景定义、界面状态、测速 hook
    src/components/ 外壳（侧栏 / 工具栏 / 每个场景的 dock 根）+ shadcn/ui + subway（地铁线小部件）
    src/pages/      kind / ability / now / work / task / project / read / placeholder
    src/data/       假数据层：types.ts 照真实 RPC 写；api.ts 是界面唯一的取数入口；selectors.ts 放派生计算
    perf/           测速报告与截图清单（p1-*、p2-*）

## 第 1 期做了什么

外壳（侧栏 220/56、工具栏 44、每场景一个 dock 根、窄窗自动收成标签页、深浅两套主题、减少动效）、
Kind 场景（2000 轮对话：分页 / 虚拟列表 / 阅读位置保持 / 回到最新 / 流式）、
能力 → 流程编辑器、假数据层、第 2/3 期占位页、测速与截图。
详见 ROUTES.md 的期数表和 perf/p1-report.md。

## 第 2 期做了什么（2026-10-28）

Kind 右栏改成「聚焦 + Kind 现在带着」、现在页（含空态）、首次使用的 Kind、工作页（看板 \| 等你）、
任务页（地铁线 / 账 / 里程碑 / 控制）、原始记录（520 轮分页虚拟列表）、项目页（项目页 \| 对话）、
阅读页（目录 / 原件 / 伴读 + 验收面板 + diff + 一起写）、内置流程三档、流程编辑器 ≥4 线图例淡出、
信息分层三处、三档露脸与每页一句 AIOS 概念。
截图清单 perf/p2-shots.md（94 组，含第 1 期全量重拍）；差异记录 [PHASE2-DELTA.md](PHASE2-DELTA.md)。
预览：`npx vite preview --port 4184 --strictPort` → http://127.0.0.1:4184
