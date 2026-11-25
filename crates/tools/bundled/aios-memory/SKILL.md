---
name: aios-memory
description: 记忆的低频操作：马上整理/撤销整理；经确认卡删除、彻底删、恢复、更正、定下来、改做梦设置、给观察许可、处理替换卡和关于你草稿卡。平时常驻的记/搜/读/日记工具不需要加载这个 skill。
---

# 记忆

只有在下面这些低频、需要用户明确意图的操作时才用本 skill。日常记/搜/读/日记继续用常驻工具。

## 直接工具（已有）

- memory_dream_now：用户说「整理一下」时马上跑一次做梦。
- memory_undo_dream：用户明说撤销上一次整理时用。
- memory_delete_note：只有用户明确说删某条**笔记**时才用；其他层用确认卡 memory_delete。

## 确认卡动作

先加载本 skill，再用 propose_action 一次；用户点确认才生效：

- memory_delete：用户说「忘掉/删掉/删除」一条记忆。details 是 `{ "id": "<memory id>" }`。卡上写标题和「移到隔离区，7 天内可恢复」。这只进隔离区，不是彻底删。
- memory_forget：只有用户明说「彻底删/永久删」才用。details 是 `{ "id": "<memory id>" }`。卡第一条固定「彻底删，不能恢复」；没明说不要用这张卡，改用 memory_delete。
- memory_restore：从隔离区恢复。details 是 `{ "id": "<memory id>" }`。
- memory_correct：更正一条。details 是 `{ "id", "title", "summary", "body", "reason" }`，只填要改的字段。更正后的每个文字都必须在用户消息里逐字出现；对不上不能改原条，只能 memory_note 记成 Kind 自己的笔记。卡上写之前 → 之后。
- memory_pin：把一条 Kind 笔记定下来。details 是 `{ "id": "<memory id>" }`。这是判断卡，第一条是「这是你的判断：定下哪条笔记」。
- memory_dream_settings：打开/关闭做梦或改间隔。details 是 `{ "enabled": true|false }` 和/或 `{ "interval_hours": <正整数> }`。
- memory_permit：给/收回观察许可。details 是 `{ "allow": true|false }`。
- memory_resolve_replace：替换卡判断。details 是 `{ "card": "<card ref>", "replace": true|false }`；true=替换旧的留新的，false=两条都留。这是判断卡，第一条写清卡和留新/留旧。
- memory_resolve_about_draft：关于你草稿卡判断。details 是 `{ "card": "<card ref>", "save": true|false }`；true=保存进「关于你」，false=不用。这是判断卡，第一条写清哪张草稿、保存还是不用。

## 判断卡

memory_pin、memory_resolve_replace、memory_resolve_about_draft 的第一条事实由程序填「这是你的判断：<对象>」；你只给动作和对象，不要自己写这一行。卡上的判断未点不生效，结果有约束力。
