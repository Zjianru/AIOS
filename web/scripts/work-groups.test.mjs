import assert from 'node:assert/strict';
import fs from 'node:fs';
import { columnOf, weekDone } from '../src/pages/work/workModel.ts';

const fixture = JSON.parse(fs.readFileSync(new URL('../src/pages/work/work-grouping.fixture.json', import.meta.url)));
const wait = new Set(fixture.waitTaskIds);
const groups = { wait: [], queue: [], doing: [], done: [] };
for (const task of fixture.tasks) {
  groups[columnOf(task, wait)].push(task.id);
}
assert.deepEqual(groups, fixture.groups);
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
