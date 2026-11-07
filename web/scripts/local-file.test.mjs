// T015 fix B2：Markdown 里的本地绝对路径 → /local-file 链接（纯函数，真值表）。
import assert from 'node:assert/strict';
import {
  fileNameOf,
  isLocalFilePath,
  localFileHref,
  openCommandPath,
} from '../src/pages/kind/localFile.ts';

// 绝对路径才算；相对路径、URL、命令、多行都不算。
assert.equal(isLocalFilePath('/private/tmp/aios/savings-30y.html'), true);
assert.equal(isLocalFilePath('/private/tmp/aios/报告 第 1 版.html'), true, '含空格的路径也算');
assert.equal(isLocalFilePath('  /private/tmp/x.html  '), true, '前后空白不算内容');
assert.equal(isLocalFilePath('relative/x.html'), false);
assert.equal(isLocalFilePath('./x.html'), false);
assert.equal(isLocalFilePath('https://example.com/x'), false);
assert.equal(isLocalFilePath('open /tmp/x.html'), false);
assert.equal(isLocalFilePath('/tmp/a\nb.html'), false, '换行不是一条路径');

// href：路径整体进 query，空格等字符要编码。
assert.equal(
  localFileHref('/private/tmp/aios/报告 第 1 版.html'),
  '/local-file?path=' + encodeURIComponent('/private/tmp/aios/报告 第 1 版.html'),
);
assert.match(localFileHref('/private/tmp/a.html'), /^\/local-file\?path=%2Fprivate%2Ftmp%2Fa\.html$/);

// fenced code 只有一行 open <绝对路径> 才换成链接；别的命令保持原样。
assert.equal(openCommandPath('open /private/tmp/aios/savings-30y.html\n'), '/private/tmp/aios/savings-30y.html');
assert.equal(openCommandPath('  open   /private/tmp/报告 第 1 版.html  '), '/private/tmp/报告 第 1 版.html');
assert.equal(openCommandPath('open relative.html'), null);
assert.equal(openCommandPath('cat /private/tmp/a.html'), null);
assert.equal(openCommandPath('open /tmp/a.html; echo x'), null);
assert.equal(openCommandPath('open /tmp/a.html\nopen /tmp/b.html'), null, '多行不是只含一条 open');

// 文件名用于链接文字，完整显示（含空格）。
assert.equal(fileNameOf('/private/tmp/aios/savings-30y.html'), 'savings-30y.html');
assert.equal(fileNameOf('/private/tmp/aios/报告 第 1 版.html'), '报告 第 1 版.html');
assert.equal(fileNameOf('/private/tmp/aios/'), 'aios');
console.log('local-file: ok');
