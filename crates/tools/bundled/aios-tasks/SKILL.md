---
name: aios-tasks
description: Stop a running task, or hold and resume new tasks globally, when the user explicitly asks. mode=now cancels the current Attempt immediately; mode=after_report lets it finish the current step, report once, then end.
---

# AIOS tasks

Use this when the user asks to stop a task they had you dispatch — "stop that task", "别查了", "把它停了" — or to hold new tasks — "暂停新任务", "先别开新的", "继续跑". Do not call it otherwise; work that is still useful should keep going.

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

## Hold and resume new tasks

The global pause is a separate gate: while it is on, new Attempts wait in the queue and the running turn finishes; reports and notices still reach you. Nothing queued is cancelled or dropped; turning it off runs the queue in order.

Offer it only when the user asks to pause or resume new tasks. To change it, call `propose_action` once with one option:

- `set_paused`: `details` is `{ "paused": true }` to hold new tasks, or `{ "paused": false }` to let the queue run. The card fills in the before and after line; do not write that yourself.
- Name the choice in the user's words. Say in one sentence that new tasks will wait while the running one finishes, or that the queue runs again. If the result says the pause is already in that state, say so; do not propose again.
- This does not stop a running task. When the user wants one task to stop, use `task_stop`.
