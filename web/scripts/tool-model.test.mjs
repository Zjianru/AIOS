// T013.5：工作过程的纯计算（清单 11 的 node 部分；原型 toolModel 用例翻改）。
import assert from 'node:assert/strict';
import { formatDurationMs } from '../src/lib/numbers.ts';
import {
  categoryOf,
  diffOf,
  diffTotals,
  displayPath,
  doneSummary,
  parseExitStatus,
  toolTitle,
  rowSummary,
} from '../src/pages/kind/process/toolModel.ts';

const step = (name, args = {}, output = '', state = 'done', extra = {}) => ({
  kind: 'tool',
  id: name + '-1',
  name,
  args,
  output,
  exitCode: 0,
  signal: null,
  durationMs: 1000,
  state,
  meta: null,
  ...extra,
});

// parseExitStatus：末尾标记去掉；没有标记就是 0；中间同样的字不算。
assert.deepEqual(parseExitStatus('done\n\n[exit code: 2]'), { text: 'done', exitCode: 2, signal: null });
assert.deepEqual(parseExitStatus('killed\n[killed by signal: SIGTERM]'), {
  text: 'killed',
  exitCode: null,
  signal: 'SIGTERM',
});
assert.deepEqual(parseExitStatus('all good\n'), { text: 'all good', exitCode: 0, signal: null });
assert.deepEqual(parseExitStatus('a\n[exit code: 2]\nb'), {
  text: 'a\n[exit code: 2]\nb',
  exitCode: 0,
  signal: null,
});

// categoryOf：AIOS 实际注册的十三种工具。
const categories = {
  read: 'read',
  glob: 'search',
  grep: 'search',
  write: 'write',
  edit: 'edit',
  apply_patch: 'edit',
  bash: 'commands',
  ask_user_question: 'questions',
  todo_write: 'plan',
  skill: 'tools',
  job_list: 'tools',
  job_output: 'tools',
  job_kill: 'tools',
};
for (const [name, expected] of Object.entries(categories)) {
  assert.equal(categoryOf(name), expected, name);
}
assert.equal(toolTitle('bash'), '运行命令');
assert.equal(toolTitle('apply_patch'), '编辑');

// diffOf：跑时按参数推，跑完优先用结果 meta，失败不出，停下只出 meta 里的实际改动。
const writing = step('write', { file_path: '/w/a.txt', content: 'one\ntwo\n' }, '', 'running');
const pending = diffOf(writing, '/w');
assert.equal(pending.length, 1);
assert.equal(pending[0].path, 'a.txt');
assert.equal(pending[0].added, 2);

const applied = step(
  'write',
  { file_path: '/w/a.txt', content: 'one\ntwo\n' },
  'Updated file',
  'done',
  { meta: { diffs: [{ path: '/w/a.txt', oldText: 'one\n', newText: 'one\ntwo\n' }], operation: 'update' } },
);
const fromMeta = diffOf(applied, '/w');
assert.equal(fromMeta.length, 1);
assert.equal(fromMeta[0].added, 1, '用 meta 的实际改动 +1');

const stopped = step('apply_patch', { input: '*** Begin Patch\n*** Add File: x.txt\n+x\n*** End Patch' }, 'Patch stopped', 'stopped', {
  meta: { diffs: [{ path: '/w/x.txt', oldText: null, newText: 'x\n' }], stopped: true },
});
const stoppedDiff = diffOf(stopped, '/w');
assert.equal(stoppedDiff.length, 1, '中途停只显示已完成的改动');

assert.equal(diffOf(step('write', { file_path: '/w/a.txt' }, '', 'error'), '/w'), null, '失败不出 diff');
const totals = diffTotals(fromMeta);
assert.deepEqual(totals, { added: 1, removed: 0 });

// 行摘要与路径相对化、时长文案。
assert.equal(displayPath('/w/src/a.ts', '/w'), 'src/a.ts');
assert.equal(displayPath('/w/src/a.ts', ''), '/w/src/a.ts', '没有 cwd（旧状态消息）时原样显示');
assert.equal(rowSummary(step('read', { file_path: '/w/src/a.ts' }, '', 'done', {}), '/w'), 'src/a.ts');
assert.equal(formatDurationMs(1000), '1秒');
assert.equal(formatDurationMs(61_000), '1分1秒');
assert.equal(formatDurationMs(3_600_000), '1小时0分');

// 组头文案：两类用「并」连；空组是「已完成分析」。
const read = step('read', { file_path: '/w/a.ts' });
const write = step('write', { file_path: '/w/b.ts', content: 'x' });
assert.equal(doneSummary([read, write, write]).text, '已写入文件并读取文件');
assert.equal(doneSummary([]).text, '已完成分析');
console.log('tool-model.test.mjs: all assertions passed');
