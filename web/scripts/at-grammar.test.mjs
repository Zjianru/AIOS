// T013.3：@ 文件语法（清单 6；DSH context/file-reference-local/tests/search.spec.ts 的 grammar 用例翻过来）。
import assert from 'node:assert/strict';
import { activeAtToken, formatFileMention } from '../src/pages/kind/atGrammar.ts';

// activeAtToken：边界、引号、邮箱不算。
assert.deepEqual(activeAtToken('@src/tu', 7), { prefix: '@src/tu', query: 'src/tu', quoted: false });
assert.deepEqual(activeAtToken('read @"docs/design n', 20), {
  prefix: '@"docs/design n',
  query: 'docs/design n',
  quoted: true,
});
assert.equal(activeAtToken('mail a@b.test', 13), undefined);
assert.equal(activeAtToken('done @src/x" next', 17), undefined);

// formatFileMention：文件/目录/引号/不安全字符。
assert.equal(formatFileMention('src/index.ts', 'file', false), '@src/index.ts');
assert.equal(formatFileMention('src', 'directory', false), '@src/');
assert.equal(formatFileMention('docs/design notes.md', 'file', false), '@"docs/design notes.md"');
assert.equal(formatFileMention('docs/design notes', 'directory', false), '@"docs/design notes/');
assert.equal(formatFileMention('README.md', 'file', true), '@"README.md"');
assert.equal(formatFileMention('bad\nname', 'file', false), undefined);
assert.equal(formatFileMention('bad "name".md', 'file', false), undefined);
assert.equal(formatFileMention('bad"name.md', 'file', false), undefined);
console.log('at-grammar.test.mjs: all assertions passed');
