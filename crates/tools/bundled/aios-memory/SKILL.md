---
name: aios-memory
description: 记忆的低频操作：马上整理（做梦）、撤销上一次整理、用户明说删除笔记时删除。平时常驻的记/搜/读/日记工具不需要加载这个 skill。
---

# 记忆

只有在下面这些低频、需要用户明确意图的操作时才用本 skill 的工具：

- memory_dream_now：用户说「整理一下」时马上跑一次做梦。
- memory_undo_dream：用户明说撤销上一次整理时用。
- memory_delete_note：只有用户明确说删某条笔记时才用；用户没明说就不删。

删除前先用 memory_search/memory_read 确认是哪一条；删除会把笔记移到隔离区，7 天内能恢复。
