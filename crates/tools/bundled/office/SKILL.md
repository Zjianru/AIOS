---
name: aios-office
description: 读写 Office 文件（xlsx/docx/pptx）：xlsx 可生成；读只给结构与文字，不承诺版式。
---

# Office 插件

插件开着时才可用；关着时 office_read 回「Office 插件已关，去插件页打开」。

- 读已有文件：调 office_read，传文件路径；xlsx 给 sheet/非空行/A1 单元格与公式，docx 给段落号与表格，pptx 给 slide/shape 锚点。超出默认窗口时用 offset/limit 再读。
- 生成的限制：第一刀只有 office_create 建 xlsx（写任务里）；docx/pptx 不生成，也不承诺保真预览。
- 读不到（坏文件/不支持/超出上限）照实说，不编内容；不做视觉自检。
