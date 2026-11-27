// T093：阅读页引用来源形状一处拼；node --experimental-strip-types 跑。
import assert from 'node:assert/strict';
import { readQuoteSource } from '../src/pages/read/quoteSource.ts';

assert.deepEqual(readQuoteSource('评测记录.md', '3'), {
  kind: 'file',
  id: '评测记录.md',
  title: '评测记录.md',
  version: 3,
});
assert.deepEqual(readQuoteSource('a/b.md', '1'), {
  kind: 'file',
  id: 'a/b.md',
  title: 'a/b.md',
  version: 1,
});
for (const bad of ['', 'v1', '0', '-2']) {
  assert.deepEqual(readQuoteSource('x.md', bad), {
    kind: 'file',
    id: 'x.md',
    title: 'x.md',
    version: 1,
  });
}
console.log('read-quote-source.test.mjs: all assertions passed');
