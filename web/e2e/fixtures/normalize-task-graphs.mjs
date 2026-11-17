#!/usr/bin/env node
// 把 Store 生成的任务图夹具重写成确定性 id：同一逻辑图每次重生成得到同一份字节。
// 任务 id 按 (created_at,title) 排；报告/输入 id 按任务内数组顺序；会话 UUID 按 Attempt 顺序。
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';

const file = process.argv[2];
if (!file) {
  console.error('usage: normalize-task-graphs.mjs <tasks.sqlite>');
  process.exit(2);
}

const db = new DatabaseSync(file);
const rows = db.prepare('SELECT id, value FROM task').all().map((row) => ({
  id: String(row.id),
  task: JSON.parse(String(row.value)),
}));
rows.sort((left, right) => {
  const created = (left.task.created_at ?? 0) - (right.task.created_at ?? 0);
  if (created !== 0) return created;
  return String(left.task.title).localeCompare(String(right.task.title));
});

const taskIdMap = new Map();
const reportIdMap = new Map();
const inputIdMap = new Map();
const conversationIdMap = new Map();

const hex = (value) => value.toString(16).padStart(32, '0');
rows.forEach((row, index) => taskIdMap.set(row.id, hex(index + 1)));

let reportCounter = 0;
let inputCounter = 0;
let conversationCounter = 0;
for (const row of rows) {
  for (const report of row.task.reports ?? []) {
    reportCounter += 1;
    reportIdMap.set(report.id, hex(0x100000 + reportCounter));
  }
  for (const input of row.task.inputs ?? []) {
    inputCounter += 1;
    inputIdMap.set(input.id, hex(0x200000 + inputCounter));
  }
  for (const run of row.task.runs ?? []) {
    for (const attempt of run.attempts ?? []) {
      if (attempt.conversation && !conversationIdMap.has(attempt.conversation)) {
        conversationCounter += 1;
        conversationIdMap.set(
          attempt.conversation,
          '00000000-0000-4000-8000-' + String(conversationCounter).padStart(12, '0'),
        );
      }
    }
  }
}

function remap(value) {
  if (typeof value === 'string') {
    return (
      taskIdMap.get(value) ??
      reportIdMap.get(value) ??
      inputIdMap.get(value) ??
      conversationIdMap.get(value) ??
      value
    );
  }
  if (Array.isArray(value)) return value.map(remap);
  if (value && typeof value === 'object') {
    const out = {};
    for (const [key, item] of Object.entries(value)) out[key] = remap(item);
    return out;
  }
  return value;
}

const schema = db.prepare("SELECT value FROM kv WHERE key = 'schema_version'").get();
const rewritten = rows
  .map((row) => ({ id: taskIdMap.get(row.id), value: JSON.stringify(remap(row.task)) }))
  .sort((left, right) => (left.id < right.id ? -1 : 1));

db.exec('DELETE FROM task; DELETE FROM effect; DELETE FROM ledger_note; DELETE FROM kv;');
db.prepare('INSERT INTO kv (key, value) VALUES (?, ?)').run(
  'schema_version',
  schema ? String(schema.value) : '2',
);
const insert = db.prepare('INSERT INTO task (id, value) VALUES (?, ?)');
for (const row of rewritten) insert.run(row.id, row.value);
db.exec('VACUUM;');
db.close();
console.log('normalized', path.basename(file), 'tasks=' + rewritten.length);
