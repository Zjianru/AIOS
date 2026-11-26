import assert from 'node:assert/strict';
import fs from 'node:fs';
import {
  STAGE_LABEL,
  columnOf,
  filterWorkForProject,
  problemsOf,
  weekDone,
} from '../src/pages/work/workModel.ts';

const fixture = JSON.parse(fs.readFileSync(new URL('../src/pages/work/work-grouping.fixture.json', import.meta.url)));
const wait = new Set(fixture.waitTaskIds);
const groups = { wait: [], queue: [], doing: [], done: [] };
for (const task of fixture.tasks) {
  groups[columnOf(task, wait)].push(task.id);
}
assert.deepEqual(groups, fixture.groups);
assert.deepEqual(STAGE_LABEL, fixture.stageLabels);

// 没绑任务的等你项算个人——全部/个人下显示，选具体项目才藏。
const filterTasks = [
  { id: 'p1', projectId: 'proj' },
  { id: 'mine', projectId: null },
];
const filterWait = [
  { key: 'a', taskId: 'p1' },
  { key: 'b', taskId: 'mine' },
  { key: 'c' },
  { key: 'd', taskId: null },
];
const all = filterWorkForProject(filterTasks, filterWait, null);
assert.deepEqual(all.wait.map((item) => item.key), ['a', 'b', 'c', 'd']);
const personal = filterWorkForProject(filterTasks, filterWait, 'personal');
assert.deepEqual(personal.tasks.map((task) => task.id), ['mine']);
assert.deepEqual(personal.wait.map((item) => item.key), ['b', 'c', 'd']);
const oneProject = filterWorkForProject(filterTasks, filterWait, 'proj');
assert.deepEqual(oneProject.tasks.map((task) => task.id), ['p1']);
assert.deepEqual(oneProject.wait.map((item) => item.key), ['a']);
const waitItems = fixture.waitTaskIds.map((taskId) => ({ taskId }));
assert.deepEqual(
  problemsOf(fixture.tasks, waitItems).map((task) => task.id),
  fixture.problems,
);
const done = weekDone(
  fixture.tasks.map((task) => ({
    id: task.id,
    title: task.id,
    stage: task.stage,
    gate: task.gate,
    closedAt: task.closedAt ?? null,
    cancelledAt: task.cancelledAt ?? null,
  })),
  fixture.now,
);
assert.deepEqual(done.names, fixture.doneThisWeek);
assert.equal(done.count, fixture.doneThisWeek.length);
console.log('work grouping fixture: ok');
