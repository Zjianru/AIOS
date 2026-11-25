---
name: aios-tasks
description: Stop or interrupt a running task, hold and resume new tasks globally, create projects, assign a task or graph to a project, speak in a task conversation, accept delivered results, judge a user milestone, offer to continue tasks a restart interrupted, choose or widen a writing task's write scope, and propose ordered task graphs. mode=now cancels the current Attempt immediately; mode=after_report lets it finish the current step, report once, then end.
---

# AIOS tasks

Use this when the user asks to stop a task they had you dispatch — "stop that task", "别查了", "把它停了" — or to hold new tasks — "暂停新任务", "先别开新的", "继续跑" — or when you dispatch a writing task or its write scope needs to change. Do not call it otherwise; work that is still useful should keep going.

## Task actions from a confirmation card

The user decides on the card; you only propose it. Load this skill first, then call `propose_action` once with the exact action and details:

- `task_interrupt`: `{ "task_id": "<id>" }`. Stops the task's current turn without closing the task or writing an Attempt end.
- `task_say`: `{ "task_id": "<id>", "text": "<the user's words, verbatim>" }`. The program checks `text` against the user's typed messages; if it is not there, do not invent or rewrite it — tell the user that changing the requirement needs `update_task_requirement`.
- `task_set_project`: `{ "task_id": "<id>", "project_id": "<project id>" | null }` (`null` = personal). The card shows the before → after project.
- `accept_deliveries`: `{ "task_ids": ["<id>", ...] }`. This is a judgment card: its first fact is 「这是你的判断：接受…的交付」. Only the user's confirmation accepts; you never accept for them.
- `gate_judge`: `{ "gate_task": "<id>", "gate_report": "<candidate report id>", "conclusion": "pass" | "needs_fix", "note": "<user's words when fixing>" }`. Use this only for a user-judged milestone, never a program gate; the first fact is 「这是你的判断：判…通过/不通过」.

## Writing tasks and the write scope

`task_accept` takes `write_scope`: one path. A file means only that file may change; a directory means anything under it may change. The scope is frozen into the task, and the task's write and edit calls are refused outside it. Omit it only for a read-only investigation.

- If the user's message already contains a path starting with / or ~/ (including a `~/...` path) and says what to write, pass that exact raw path as `write_scope` and dispatch directly. If they gave a path but not yet what to write, ask what to write before dispatching.
- If the user wants a file written but no path appears, do not call `ask_user_question` and do not dispatch. First load `aios-tasks` if needed, then call `propose_action` once with one `confirm_write_scope` option whose `details` are `{ "write_scope": "<one concrete / or ~/ path you propose>", "task": { ...full task_accept arguments... } }`; let the user confirm where to write. Use the project's default directory or the user's home plus a filename when no path is given.
- The Kind conversation's directory is not the write scope. Never pass the cwd just because the work happens there.
- Set `worktree: true` only when the user explicitly asks to work in a git worktree.
- When a task hands back because it needs a wider scope, do not call `task_update` until the user agrees. When their message contains the new / or ~/ path, call `task_update` with `{ "task_id": "<the id task_accept returned>", "write_scope": "<path>" }`: it records a new requirement version and resumes the task with the new scope. If they agreed but did not say the exact path, propose `confirm_write_scope` with the `task_id`; do not use `ask_user_question` for the path.

## 用户原话（quote）

Always pass `quote` on `task_accept` and `task_graph_propose`: copy the user's request verbatim from a user message. The program checks it is a contiguous substring (after trimming) of a message the user typed and rejects the whole call otherwise: 「派任务要用用户原话：『…』不在你的消息里；照原样引用，或者问用户。」 Do not paraphrase or put Kind's wording in quote. Put Kind's understanding in `requirement`, and any assumption or addition from Kind in `kind_note` — never claim it as the user's request. If you cannot quote the user, ask them to say it again instead of inventing words.

## Constraints（用户的硬要求）

When the user states a hard must/never rule that ThinAgent must follow ("必须…", "不要…", "不准…"), copy that sentence into `constraints` — verbatim, without rewriting, summarizing, or merging it with your own words. Pass it on `task_accept` (single task) or `task_graph_propose` (whole graph, at the root); when the user changes the requirement and restates a rule, also pass `constraints` on `update_task_requirement`. If the user stated no such rule, omit `constraints`; on an update, omitting it keeps the previous constraints.

The program checks every string against the user messages in this Kind conversation and rejects the whole call if one is not verbatim: 「约束要用用户原话：『…』不在用户的消息里」. Do not try to bypass it by trimming a sentence into a different wording; ask the user to say the rule again if you cannot quote it.

## Ordered work: task graphs

When one step needs an earlier step's result (「先查 A，再按查到的改 B」), do not dispatch the steps one by one and do not copy results between tasks by hand. Build one task graph:

1. Call `task_graph_propose` once with the whole graph: root `title` / `quote` / `requirement` / `dod` / `cwd?`, and `subtasks` — at least two, each with `title` / `requirement` / `dod` / `write_scope?` / `worktree?` / `after`. `after` lists the indexes (0-based) of the subtasks whose result this step needs. Steps without a link may run at the same time, so their write scopes must not overlap.
2. Then call `propose_action` once: one option, `action: "start_task_graph"`, `details: { "task_id": "<the graph id task_graph_propose returned>" }`, label it with something like 确认并开跑. The card fills in one line per subtask (title, what it waits for, write scope). The user confirms on the card; you have no tool that confirms or starts a graph.
3. The user confirms once: steps with no upstream start right away; when an upstream reports a ready result, its downstream starts automatically and is handed that result. Do not dispatch those downstream steps again, and do not ask the user to accept intermediate results.

Independent things in one message are still separate `task_accept` calls, not a graph. If the user dismisses the card the graph stays without running; to change it, propose a new graph after they agree.

## Look before continuing: milestones

When the user wants to check something before the next step (「改完找个独立的核查看一眼，我来拍板，过了再…」), add one read-only milestone to the graph:

- one subtask with `milestone: true`, no `write_scope`, and `after` exactly one index: the step it checks;
- the checked step's other downstream steps wait until the user passes the milestone;
- only the user decides on the milestone result card (通过 / 需修正); when they tell you their decision, propose `gate_judge` with their conclusion and words. Never choose the verdict yourself.

When the user sends a milestone back needing fixes, call `task_continue` once with the checked task's id and the user's words as `note`. Do not dispatch the checked task again and do not change its requirement or write scope: it continues in the original requirement as a new Attempt, and the milestone reruns automatically when the new result arrives. The user still decides the new result.

If the user's words go beyond the original requirement (a new scope, another goal, a different outcome), do not call `task_continue` — ask the user to change the requirement first (`task_update`), then continue.

## Program gates (commands decide pass/fail)

When the user names commands that decide whether a step passed ("改完跑 cargo test -q 和 npm test，全绿才算过"), add a program gate to that graph step instead of a user-judged milestone: on the subtask set `program_gate: { "commands": [{ "command": "<verbatim>", "timeout_ms"?: <ms> }] }` and do not set `milestone: true` (they are mutually exclusive). Copy each command verbatim from the user's words: do not rewrite it, add arguments, or guess a package. If the command is ambiguous, or the user did not say the working directory or package, ask before proposing the graph. Add `timeout_ms` only when the user named a duration (default 120 seconds, max 600000).

The confirmation card lists every command and the user's confirmation authorizes exactly those commands. After that the program runs them; there are no judge buttons on a program gate result card and you never judge it. If a check is undetermined or was interrupted, tell the user what happened (timeout/spawn failure/stopped/restart) and ask whether to re-run or change the commands. If they say re-run, call `gate_check_again` once with the gate task id; do not call `task_continue` and do not propose a card. To change commands, call `propose_action` once with action `update_gate_commands` and `details: { "gate_task_id": "<id>", "commands": [...] }`; the card shows before → after, the command version, and that the current candidate will be re-run and the old verdict voided.

## Delivery records

When the user asks what a task delivered, what it is based on, what is still missing, or whether they accepted it, call `task_delivery` once with the task id and repeat the record as written. Read-only, no card. Do not turn unproven or undetermined into passed.

## Upstream results changed

When a notice says an upstream task has a new result and names a downstream task, call `task_continue` once for that downstream task. It keeps its original requirement and write scope and checks its existing result against the new upstream result; the user is not asked. Do not dispatch it again and do not change its requirement.

## Changing a task's requirement

When the user's message carries a result card and asks for a change, call `propose_action` once with one option:

- `update_task_requirement`: `details` is `{ "task_id": "<the card's task id>", "quote": "<the user's words for this change, verbatim>", "requirement": "<Kind's understanding of the whole new requirement>", "dod": "<optional new dod>" }`. `quote` is required and is checked against the user's typed messages just like dispatch; `requirement` is Kind's understanding, not a copy of the user's words. Keep the rest of the original requirement; change nothing else. Load `aios-tasks` first.
- Label the option "改任务要求". The card fills in the before → after and version lines; do not write those yourself.
- After the user confirms, the same task starts a new Attempt (version N+1); the old acceptance stays in history.
- If the message only asks a question about the result, answer it directly and do not show a card.
- When the user asks you to accept a delivered result, propose `accept_deliveries`; never accept for them or say it has been accepted before the card is confirmed.

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

## Projects

The fixed block lists the projects by id, name, goal and default directory. A task or task graph may belong to at most one project; the task card and the task conversation show it.

- Decide the project from what the user said, not from the directory. Never infer a project from cwd.
- If it is genuinely unclear, ask one short question and include a personal option (不归项目（个人）) before dispatching.
- Pass project (the project id) on task_accept or task_graph_propose only when you are sure; omit it for a personal task. A graph's subtasks inherit the root project.
- When the user names a directory, pass it as cwd; otherwise omit cwd and the project's default directory is used, then the Kind conversation directory. A project default directory that does not exist is an error; report it and ask, do not fall back silently.
- When the user explicitly asks to create a project, call project_create with name, goal, optional dir. It creates immediately and returns a receipt; a duplicate name returns an error and creates nothing.
- When you infer a new project from repeated conversation, call propose_action once with one option, action: "create_project", and details: { "name": ..., "goal": ..., "dir": ... }. The user confirms on the card; if the name already exists when they confirm, the action says it was already created.
- When the user asks to re-assign a task to another project (or back to personal), propose_action once with action `task_set_project` and details `{ "task_id": "<id>", "project_id": "<project id>" | null }`; the card shows the before → after project and only the user's confirmation changes it.
- When the user asks to edit a project's name/goal/default directory, propose_action once with action `project_update` and details `{ "id": "<project id>", "name": "<new name>", "goal": "<new goal>", "dir": "<new dir>" | null }`; the card shows before → after for each changed field.
