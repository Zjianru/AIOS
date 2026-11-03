# 第 2 期测速报告

生成时间：2026-10-28T09:56:40.670Z（2026-10-28T09:55:43.558Z 开始）

## 环境

- 机器：Mac16,10 · Apple M4 · 内存 16 GB
- 系统：macOS 26.6.2
- 浏览器：Playwright 驱动的 Chrome 156.0.8078.73（headless）
- 视口：1440 × 920
- 命令：`node perf-p2.mjs`（打开 http://127.0.0.1:4184/#kind）
- 覆盖范围：第 1 期 11 项重测 + 第 2 期新增 2 项（任务页打开、原始记录打开）。
- 数据延迟：40ms（默认）；2000 轮用固定种子按需生成，不预载
- 测速锁：开跑前锁不存在 + `pgrep -fl 'cargo|rustc'` 为空；跑完已删除
- 流程画布那一项：先用开发开关打开压测流程（工具栏「开发 → 压测（只影响流程列表）→ 压测流程（60 节点）」，对应测试钩子 __v14.setDevStressFlow(true)），再访问 #flow:stress-60。这条流程由程序生成 60 节点 / 58 条连线 / 4 条线，只在开关打开时出现，开关不进 localStorage。
- WebKit：WebKit 未测（本机 Playwright 没装 webkit；按需求不为此下载浏览器）

## 结果

| 项 | 数值 | 标准 | 结论 |
|---|---|---|---|
| 冷启动到首屏 | 282 ms | ≤ 2000 ms | 达标 |
| 2000 轮对话首屏 | 中位 55 ms（5 次：53 / 55 / 55 / 55 / 59） | ≤ 300 ms | 达标 |
| 首屏 DOM 节点数 | 638 | 报数 | — |
| 往上翻一页（10 次） | 中位 73 ms · 最大 79 ms | ≤ 150 ms | 达标 |
| 滚动帧率（匀速 5 秒） | 60.3 fps | ≥ 55 fps | 达标 |
| 滚动长任务（>50ms） | 0 个 | 0 个 | 达标 |
| 流式回复 30 秒长任务 | 0 个 | 0 个 | 达标 |
| 切场景（现在→Kind→能力→Kind） | 30 / 27 / 34 ms | ≤ 200 ms | 达标 |
| 内存（JS 堆） | 空闲 10.3 MB · 翻完 2000 轮 18.2 MB | 报数 | — |
| 体积 dist/ | 3.31 MB · gzip 1057 KB（首屏 1.81 MB / gzip 495 KB，其余是懒加载块） | 报数 | — |
| 流程画布（压测流程 60 节点 / 4 条线） | 画布节点 60 个 · 长任务 0 个 | 0 个 >50ms | 达标 |
| 任务页打开（第 2 期：地铁线 / 账 / 里程碑） | 76 ms | ≤ 500 ms | 达标 |
| 原始记录打开（第 2 期：520 轮） | 67 ms | ≤ 500 ms（同任务详情） | 达标 |

## 明细

- 冷启动：从 navigation 到 `turns:painted`（侧栏 + 工具栏 + 对话最后一页画完，含字体到位后的对齐）。DOMContentLoaded 154 ms，load 154 ms。
- 首个 30 秒流式回复期间的长任务：无。
- 滚动帧率测试里记录的长任务：无。
- 翻完 2000 轮：加载 1996 轮，用时 4289 ms，列表 2001 条；之后 DOM 节点 663 个。
- 流程画布：压测流程在 0.92 缩放（站名 13.8px / 副标 11px）下记录拖动与平移的长任务；60 个节点全部渲染（4 条线）。
- 任务页 / 原始记录打开：从点进路由（`route:start:<route>`）到数据画完（任务页 `task:data-painted`、原始记录 `trace:painted`）；只量打开，不量后续交互。现在页 / 项目页的打开时间只作参考：24 / 21 ms。
- 最大的 5 个 JS chunk：
  - `assets/index-B81xwCrq.js`：1618 KB（gzip 466 KB，首屏）
  - `assets/elk.bundled-YvhVMncb.js`：1397 KB（gzip 427 KB，懒加载）

## 机器占用快照（top -l 1 -n 5）

开跑前：

```
Processes: 507 total, 3 running, 504 sleeping, 3761 threads 
2026/10/28 17:55:43
Load Avg: 7.33, 5.27, 4.30 
CPU usage: 18.65% user, 17.16% sys, 64.17% idle 
SharedLibs: 570M resident, 124M data, 88M linkedit.
MemRegions: 592264 total, 4573M resident, 77M private, 1284M shared.
PhysMem: 15G used (2500M wired, 6479M compressor), 102M unused.
VM: 249T vsize, 6144M framework vsize, 1694373(0) swapins, 2387563(0) swapouts.
Networks: packets: 93743184/124G in, 67683188/95G out.
Disks: 59127447/1325G read, 47388889/1558G written.

PID    COMMAND          %CPU TIME     #TH #WQ #PORTS MEM   PURG CMPRS PGRP  PPID STATE    BOOSTS    %CPU_ME %CPU_OTHRS UID FAULTS COW MSGSENT MSGRECV SYSBSD SYSMACH CSW    PAGEINS IDLEW POWER INSTRS CYCLES JETPRI USER  #MREGS RPRVT VPRVT VSIZE KPRVT KSHRD
98575  talagentd        0.0  00:08.01 2   1   148    9088K 0B   8624K 98575 1    sleeping *0[3971]  0.00000 0.00000    501 179911 163 10282   8927    67055  40038   50750  146     5     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98058  assistant_servic 0.0  00:06.35 2   1   153    13M   0B   13M   98058 1    sleeping *0[16117] 0.00000 0.00000    501 78593  243 6243    20939   283968 35406   97057  1013    54    0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
93518  suggestd         0.0  00:36.37 3   2   548    22M   0B   17M   93518 1    sleeping *0[5424]  0.00000 0.00000    501 464886 292 45412   26495   800688 493723  300085 7972    499   0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
92218  UsageTrackingAge 0.0  00:36.77 2   1   123    7169K 0B   6720K 92218 1    sleeping *0[11348] 0.00000 0.00000    501 41390  176 16316   9420    231992 35762   72905  2160    40    0.0   0      0      40     codez N/A    N/A   N/A   N/A   N/A   N/A  
90916  IMAutomaticHisto 0.0  00:00.22 2   1   66     5040K 0B   4688K 90916 1    sleeping  0[0]     0.00000 0.00000    501 7704   150 576     402     4377   1812    2583   219     1     0.0   0      0      40     codez N/A    N/A   N/A   N/A   N/A   N/A
```

跑完后：

```
Processes: 504 total, 2 running, 502 sleeping, 3753 threads 
2026/10/28 17:56:40
Load Avg: 6.49, 5.35, 4.38 
CPU usage: 20.54% user, 15.52% sys, 63.92% idle 
SharedLibs: 527M resident, 113M data, 83M linkedit.
MemRegions: 633016 total, 3830M resident, 55M private, 1317M shared.
PhysMem: 15G used (2527M wired, 7213M compressor), 191M unused.
VM: 252T vsize, 6144M framework vsize, 1703353(0) swapins, 2431863(0) swapouts.
Networks: packets: 93836715/124G in, 67759267/95G out.
Disks: 59160792/1326G read, 47422574/1558G written.

PID    COMMAND          %CPU TIME     #TH #WQ #PORTS MEM   PURG CMPRS PGRP  PPID STATE    BOOSTS    %CPU_ME %CPU_OTHRS UID FAULTS COW MSGSENT MSGRECV SYSBSD SYSMACH CSW    PAGEINS IDLEW POWER INSTRS CYCLES JETPRI USER  #MREGS RPRVT VPRVT VSIZE KPRVT KSHRD
98575  talagentd        0.0  00:08.02 2   1   148    9088K 0B   8720K 98575 1    sleeping *0[3979]  0.00000 0.00000    501 180460 163 10291   8939    67140  40083   50896  146     5     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98058  assistant_servic 0.0  00:06.35 2   1   153    13M   0B   13M   98058 1    sleeping *0[16117] 0.00000 0.00000    501 78593  243 6243    20939   283968 35406   97057  1013    54    0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
93518  suggestd         0.0  00:36.42 2   1   547    22M   0B   21M   93518 1    sleeping *0[5432]  0.00000 0.00000    501 467269 292 45428   26511   801703 494327  300658 7975    500   0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
92218  UsageTrackingAge 0.0  00:36.77 2   1   124    7249K 0B   6816K 92218 1    sleeping *0[11354] 0.00000 0.00000    501 41557  176 16321   9425    232037 35775   72993  2160    40    0.0   0      0      40     codez N/A    N/A   N/A   N/A   N/A   N/A  
90916  IMAutomaticHisto 0.0  00:00.22 2   1   66     5040K 0B   4688K 90916 1    sleeping  0[0]     0.00000 0.00000    501 7704   150 576     402     4377   1812    2583   219     1     0.0   0      0      40     codez N/A    N/A   N/A   N/A   N/A   N/A
```

## 备注

- WebKit 未测（本机 Playwright 没装 webkit；按需求不为此下载浏览器）
- 口径与第 1 期一致：所有数字都是在代理实现上量的（React 原型 + 假数据层，走 api.ts 的 40ms 模拟延迟），不是真实 App；「真实 App」的指标这一期没有。
- 流程画布按 brief §四.2 地铁线规则改过（线 4px / 间距 6px；站中性浅底深描边；换乘站跨线胶囊、门跨线菱形）；第 2 期压测流程是 4 条线 / 60 个节点（为了触发「≥4 条线」的图例淡出）。
- 「≥4 条线时点图例其余淡到 25%」第 2 期已实现：压测流程（开发开关下）4 条线，画布左上角「价值流」图例点一条、其余淡到 25%。
- 控制台错误：[]
