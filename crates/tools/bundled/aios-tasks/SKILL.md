---
name: aios-tasks
description: Stop a running task when the user explicitly asks. mode=now records the stop and cancels the current Attempt; the after-report stop is not supported yet.
---

# AIOS tasks

Use this when the user asks to stop a task they had you dispatch — "stop that task", "别查了", "把它停了". Do not call it otherwise; a task that is still useful should keep running.

## Stop now

Call `task_stop` once with `{"task_id": "<the id task_accept returned>", "mode": "now"}`.

- It records the stop in the task store and cancels the task's running Attempt immediately. The user sees the stop in the task conversation.
- You receive a `task_report` message with `state: "stopped"` as a notice; do not wait for it before answering.
- Say in one sentence that the task is stopped now and which stage it is in. Do not claim the result changed: stopping keeps the work already done.
- If the result says the task is not running, say that plainly and do not retry.

## After-report stop

`mode: "after_report"` is not supported in this version; the tool answers with a fixed "not supported" error. If the user asks for it, tell them it is not supported yet and that the only stop available now is stopping immediately. Do not pretend it worked.

## Pause

The global pause is a separate gate: new Attempts wait while the running turn finishes. It is not available in this version; do not promise it. When the user wants one task to stop, use `task_stop` with `mode: "now"`.
