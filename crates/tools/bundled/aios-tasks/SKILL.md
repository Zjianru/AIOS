---
name: aios-tasks
description: Stop a running task when the user explicitly asks. mode=now cancels the current Attempt immediately; mode=after_report lets it finish the current step, report once, then end.
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

Call `task_stop` with `mode: "after_report"` when the user wants the task to finish what it is doing and report once instead of being cut off.

- The task gets a fixed stop note: finish the current step, report once with `task_report`, then end. Once the note is in, only `task_report` is allowed; other tool calls are refused.
- The Attempt ends with whatever it reports; if it ends the turn without reporting, it is recorded as stopped with the waiting state.
- The user can still switch to `mode: "now"` afterwards; that cancels the Attempt immediately.
- Say in one sentence that the stop is in and that it will report before ending. If the result says the task is not running, say that plainly and do not retry.

## Pause

The global pause is a separate gate: new Attempts wait while the running turn finishes. It is not available in this version; do not promise it. When the user wants one task to stop, use `task_stop`.
