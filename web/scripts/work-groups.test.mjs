import assert from 'node:assert/strict';
import fs from 'node:fs';
import { columnOf } from '../src/pages/work/workModel.ts';

const fixture = JSON.parse(fs.readFileSync(new URL('../src/pages/work/work-grouping.fixture.json', import.meta.url)));
const wait = new Set(fixture.waitTaskIds);
const groups = { wait: [], queue: [], doing: [], done: [] };
for (const task of fixture.tasks) {
  groups[columnOf(task, wait)].push(task.id);
}
assert.deepEqual(groups, fixture.groups);
console.log('work grouping fixture: ok');
