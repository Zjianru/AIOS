import assert from 'node:assert/strict';
import { plainText } from '../src/lib/plain.ts';

// 已知 Markdown 输入：标题、粗体、斜体、行内代码、代码块、链接、引用、列表都变成纯文字。
const input = [
  '## 结论',
  '**先按旧源继续**，等来源恢复再补。',
  '- 第一项',
  '- 第二项带 [链接](https://example.com/a)',
  '> 引用一句',
  '行内 `code` 也要去掉。',
].join('\n');
const output = plainText(input);
for (const leaked of ['#', '**', '`', '](', '> ']) {
  assert.equal(output.includes(leaked), false, '不应残留 ' + leaked + '：' + output);
}
assert.equal(output.includes('结论'), true);
assert.equal(output.includes('先按旧源继续'), true);
assert.equal(output.includes('第二项带 链接'), true, '链接文字保留、URL 去掉');
assert.equal(output.includes('第一项 第二项'), true, '列表标记去掉后接成一行');

// 代码围栏整段去掉内容？只去标记、保留正文；连续空白压成一个空格。
const fenced = plainText('前\n\n```\nconst a = 1;\n```\n后');
assert.equal(fenced.includes('`'), false);
assert.equal(fenced.includes('const a = 1;'), true);
assert.equal(fenced, '前 const a = 1; 后');

// 空输入 / 只有标记。
assert.equal(plainText(''), '');
assert.equal(plainText('## 标题'), '标题');
assert.equal(plainText('## '), '');

console.log('plain-text: all assertions passed');