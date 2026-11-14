---
name: aios-tasks
description: Stop a running task, hold and resume new tasks globally, offer to continue tasks a restart interrupted, choose or widen a writing task's write scope, and propose ordered task graphs. mode=now cancels the current Attempt immediately; mode=after_report lets it finish the current step, report once, then end.
---

# AIOS tasks

Use this when the user asks to stop a task they had you dispatch — "stop that task", "别查了", "把它停了" — or to hold new tasks — "暂停新任务", "先别开新的", "继续跑" — or when you dispatch a writing task or its write scope needs to change. Do not call it otherwise; work that is still useful should keep going.

## Writing tasks and the write scope

`task_accept` takes `write_scope`: one path. A file means only that file may change; a directory means anything under it may change. The scope is frozen into the task, and the task's write and edit calls are refused outside it. Omit it only for a read-only investigation.

- If the user named the file or directory, pass it as `write_scope` and dispatch directly.
- If you inferred the path or cannot find it, ask the user first and give one concrete path to approve. Do not dispatch until they answer.
- The Kind conversation's directory is not the write scope. Never pass the cwd just because the work happens there.
- Set `worktree: true` only when the user explicitly asks to work in a git worktree.
- When a task hands back because it needs a wider scope, ask the user. After they agree, call `task_update` with `{ "task_id": "<the id task_accept returned>", "write_scope": "<path>" }`: it records a new requirement version and resumes the task with the new scope.

## Ordered work: task graphs

When one step needs an earlier step's result (「先查 A，再按查到的改 B」), do not dispatch the steps one by one and do not copy results between tasks by hand. Build one task graph:

1. Call `task_graph_propose` once with the whole graph: root `title` / `requirement` / `dod` / `cwd?`, and `subtasks` — at least two, each with `title` / `requirement` / `dod` / `write_scope?` / `worktree?` / `after`. `after` lists the indexes (0-based) of the subtasks whose result this step needs. Steps without a link may run at the same time, so their write scopes must not overlap.
2. Then call `propose_action` once: one option, `action: "start_task_graph"`, `details: { "task_id": "<the graph id task_graph_propose returned>" }`, label it with something like 确认并开跑. The card fills in one line per subtask (title, what it waits for, write scope). The user confirms on the card; you have no tool that confirms or starts a graph.
3. The user confirms once: steps with no upstream start right away; when an upstream reports a ready result, its downstream starts automatically and is handed that result. Do not dispatch those downstream steps again, and do not ask the user to accept intermediate results.

Independent things in one message are still separate `task_accept` calls, not a graph. If the user dismisses the card the graph stays without running; to change it, propose a new graph after they agree.

## Look before continuing: milestones

When the user wants to check something before the next step (「改完找个独立的核查看一眼，我来拍板，过了再…」), add one read-only milestone to the graph:

- one subtask with `milestone: true`, no `write_scope`, and `after` exactly one index: the step it checks;
- the checked step's other downstream steps wait until the user passes the milestone;
- only the user decides on the milestone result card (通过 / 需修正); you have no tool that passes or fixes it.

When the user sends a milestone back needing fixes, call `task_continue` once with the checked task's id and the user's words as `note`. Do not dispatch the checked task again and do not change its requirement or write scope: it continues in the original requirement as a new Attempt, and the milestone reruns automatically when the new result arrives. The user still decides the new result.

If the user's words go beyond the original requirement (a new scope, another goal, a different outcome), do not call `task_continue` — ask the user to change the requirement first (`task_update`), then continue.

## Upstream results changed

When a notice says an upstream task has a new result and names a downstream task, call `task_continue` once for that downstream task. It keeps its original requirement and write scope and checks its existing result against the new upstream result; the user is not asked. Do not dispatch it again and do not change its requirement.

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

## After a restart

When a message with `kind: "tasks_interrupted"` arrives, the service restarted while those tasks were running. `tasks` lists each one with its id, title, the last report (if there was one, possibly shortened) and how many Attempts it had.

- Say what happened and where each task got to, using only what the list says. Do not make up a result it did not report.
- Say what is kept: the task conversation, the work already done, and any question that was waiting for the user.
- Suggest how to pick up: a task continues in its own conversation as a new Attempt and is told to look at the current files and state before doing anything.
- Then call `propose_action` once with one option per task:
  - `resume_task`: `details` is `{ "task_id": "<the task id>" }`; label the option with the task title. The card fills in the status line; do not write that yourself.
  - "Not now" is the card's dismiss. Do not add an option for it.
- If the result says the task already resumed, say so in one sentence and do not propose again. If the user dismisses the card, the tasks stay interrupted: do not ask again on your own.

## Hold and resume new tasks

The global pause is a separate gate: while it is on, new Attempts wait in the queue and the running turn finishes; reports and notices still reach you. Nothing queued is cancelled or dropped; turning it off runs the queue in order.

Offer it only when the user asks to pause or resume new tasks. To change it, call `propose_action` once with one option:

- `set_paused`: `details` is `{ "paused": true }` to hold new tasks, or `{ "paused": false }` to let the queue run. The card fills in the before and after line; do not write that yourself.
- Name the choice in the user's words. Say in one sentence that new tasks will wait while the running one finishes, or that the queue runs again. If the result says the pause is already in that state, say so; do not propose again.
- This does not stop a running task. When the user wants one task to stop, use `task_stop`.
