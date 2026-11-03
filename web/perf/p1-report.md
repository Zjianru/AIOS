# 第 1 期测速报告

生成时间：2026-10-27T18:14:19.185Z（2026-10-27T18:13:23.683Z 开始）

## 环境

- 机器：Mac16,10 · Apple M4 · 内存 16 GB
- 系统：macOS 26.6.2
- 浏览器：Playwright 驱动的 Chrome 156.0.8078.48（headless）
- 视口：1440 × 920
- 命令：`node perf.mjs`（打开 http://127.0.0.1:4184/#kind）
- 数据延迟：40ms（默认）；2000 轮用固定种子按需生成，不预载
- 测速锁：开跑前锁不存在 + `pgrep -fl 'cargo|rustc'` 为空；跑完已删除
- 流程画布那一项：先用开发开关打开压测流程（工具栏「开发 → 压测（只影响流程列表）→ 压测流程（60 节点）」，对应测试钩子 __v14.setDevStressFlow(true)），再访问 #flow:stress-60。这条流程由程序生成 60 节点 / 59 条连线，只在开关打开时出现，开关不进 localStorage。
- WebKit：WebKit 未测（本机 Playwright 没装 webkit；按需求不为此下载浏览器）

## 结果

| 项 | 数值 | 标准 | 结论 |
|---|---|---|---|
| 冷启动到首屏 | 209 ms | ≤ 2000 ms | 达标 |
| 2000 轮对话首屏 | 中位 60 ms（5 次：60 / 61 / 58 / 66 / 54） | ≤ 300 ms | 达标 |
| 首屏 DOM 节点数 | 590 | 报数 | — |
| 往上翻一页（10 次） | 中位 74 ms · 最大 85 ms | ≤ 150 ms | 达标 |
| 滚动帧率（匀速 5 秒） | 58.2 fps | ≥ 55 fps | 达标 |
| 滚动长任务（>50ms） | 0 个 | 0 个 | 达标 |
| 流式回复 30 秒长任务 | 0 个 | 0 个 | 达标 |
| 切场景（现在→Kind→能力→Kind） | 26 / 22 / 32 ms | ≤ 200 ms | 达标 |
| 内存（JS 堆） | 空闲 11.4 MB · 翻完 2000 轮 15.6 MB | 报数 | — |
| 体积 dist/ | 3.17 MB · gzip 1022 KB（首屏 1.68 MB / gzip 460 KB，其余是懒加载块） | 报数 | — |
| 流程画布（压测流程 60 节点） | 画布节点 60 个 · 长任务 0 个 | 0 个 >50ms | 达标 |

## 明细

- 冷启动：从 navigation 到 `turns:painted`（侧栏 + 工具栏 + 对话最后一页画完，含字体到位后的对齐）。DOMContentLoaded 81 ms，load 82 ms。
- 首个 30 秒流式回复期间的长任务：无。
- 滚动帧率测试里记录的长任务：无。
- 翻完 2000 轮：加载 1996 轮，用时 4487 ms，列表 2001 条；之后 DOM 节点 615 个。
- 流程画布：压测流程在 0.92 缩放（站名 13.8px / 副标 11px）下记录拖动与平移的长任务；60 个节点全部渲染。
- 最大的 5 个 JS chunk：
  - `assets/index-Dt42qywv.js`：1487 KB（gzip 433 KB，首屏）
  - `assets/elk.bundled-BgG4LqWf.js`：1397 KB（gzip 427 KB，懒加载）

## 机器占用快照（top -l 1 -n 5）

开跑前：

```
Processes: 533 total, 2 running, 531 sleeping, 3172 threads 
2026/10/28 02:13:23
Load Avg: 4.20, 3.38, 3.18 
CPU usage: 9.52% user, 10.58% sys, 79.89% idle 
SharedLibs: 734M resident, 140M data, 164M linkedit.
MemRegions: 289090 total, 4600M resident, 159M private, 1067M shared.
PhysMem: 14G used (1998M wired, 1923M compressor), 1058M unused.
VM: 233T vsize, 6144M framework vsize, 1494015(0) swapins, 2071311(0) swapouts.
Networks: packets: 74069105/101G in, 51971241/75G out.
Disks: 50597825/1142G read, 40978206/1424G written.

PID    COMMAND          %CPU TIME     #TH #WQ #PORTS MEM   PURG CMPRS PGRP  PPID STATE    BOOSTS    %CPU_ME %CPU_OTHRS UID FAULTS COW MSGSENT MSGRECV SYSBSD SYSMACH CSW   PAGEINS IDLEW POWER INSTRS CYCLES JETPRI USER  #MREGS RPRVT VPRVT VSIZE KPRVT KSHRD
98575  talagentd        0.0  00:01.04 2   1   142    8576K 0B   6848K 98575 1    sleeping *0[818]   0.00000 0.00000    501 30684  161 2832    2062    15848  10145   10646 108     1     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98425  networkservicepr 0.0  00:00.95 2   1   101    9360K 0B   8656K 98425 1    sleeping *0[10]    0.00000 0.00000    501 19020  187 2196    998     18511  3684    7721  616     1     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98409  intelligencecont 0.0  00:00.41 2   1   77     7969K 0B   5568K 98409 1    sleeping *0[469]   0.00000 0.00000    501 14276  234 1954    2234    13788  5125    6323  576     1     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98348  contentlinkingd  0.0  00:00.35 2   1   57     3568K 0B   1776K 98348 1    sleeping *0[162]   0.00000 0.00000    501 14154  114 1286    1187    12012  7121    6738  77      13    0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98058  assistant_servic 0.0  00:03.54 2   1   162    10M   0B   9392K 98058 1    sleeping *0[12244] 0.00000 0.00000    501 34211  243 2694    15600   193458 21350   63513 940     5     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A
```

跑完后：

```
Processes: 525 total, 4 running, 521 sleeping, 3176 threads 
2026/10/28 02:14:19
Load Avg: 4.06, 3.48, 3.23 
CPU usage: 12.16% user, 12.16% sys, 75.66% idle 
SharedLibs: 734M resident, 141M data, 164M linkedit.
MemRegions: 288871 total, 4678M resident, 160M private, 1068M shared.
PhysMem: 15G used (1978M wired, 1906M compressor), 990M unused.
VM: 230T vsize, 6144M framework vsize, 1494021(0) swapins, 2071311(0) swapouts.
Networks: packets: 74105575/101G in, 52000473/75G out.
Disks: 50607758/1142G read, 40982082/1424G written.

PID    COMMAND          %CPU TIME     #TH #WQ #PORTS MEM   PURG CMPRS PGRP  PPID STATE    BOOSTS    %CPU_ME %CPU_OTHRS UID FAULTS COW MSGSENT MSGRECV SYSBSD SYSMACH CSW   PAGEINS IDLEW POWER INSTRS CYCLES JETPRI USER  #MREGS RPRVT VPRVT VSIZE KPRVT KSHRD
98575  talagentd        0.0  00:01.04 2   1   142    8576K 0B   6848K 98575 1    sleeping *0[818]   0.00000 0.00000    501 30684  161 2832    2062    15848  10145   10646 108     1     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98425  networkservicepr 0.0  00:00.95 2   1   101    9360K 0B   8656K 98425 1    sleeping *0[10]    0.00000 0.00000    501 19020  187 2196    998     18511  3684    7721  616     1     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98409  intelligencecont 0.0  00:00.41 2   1   78     7969K 0B   5536K 98409 1    sleeping *0[470]   0.00000 0.00000    501 14281  234 1958    2239    13812  5136    6329  576     1     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98348  contentlinkingd  0.0  00:00.35 3   2   58     3648K 0B   1728K 98348 1    sleeping *0[162]   0.00000 0.00000    501 14187  114 1290    1192    12075  7161    6766  77      13    0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A  
98058  assistant_servic 0.0  00:03.54 2   1   162    10M   0B   9392K 98058 1    sleeping *0[12244] 0.00000 0.00000    501 34211  243 2694    15600   193458 21350   63513 940     5     0.0   0      0      0      codez N/A    N/A   N/A   N/A   N/A   N/A
```

## 备注

- WebKit 未测（本机 Playwright 没装 webkit；按需求不为此下载浏览器）
- 流程画布按 brief §四.2 地铁线规则改过（线 4px / 间距 6px；站中性浅底深描边；换乘站跨线胶囊、门跨线菱形），本期这一项量的是改后版本。
- 地铁线「≥4 条线时点图例其余淡到 25%」本期未实现：现有流程最多 3 条线，没有触发场景，等第 2 期有 4 线数据再落（协调线 2026-10-28 同意先登记）。
- 控制台错误：[]
