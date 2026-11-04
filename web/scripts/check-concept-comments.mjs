#!/usr/bin/env node
// F1 验收：原型 e457744 里每条「AIOS 概念 ·」的正文，在 web/src 里逐条搜；命中 0 才算过。
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';

const proto = process.env.AIOS_PROTOTYPE_DIR ?? path.join(homedir(), 'Documents/aios-design/app-v14');
const src = path.resolve(process.cwd(), 'src');
// 原型目录的 HEAD 可能已经前进；条目固定从 e457744 这个提交取（AIOS_PROTOTYPE_DIR 仍可覆盖仓库路径）。
const rows = execFileSync('git', ['-C', proto, 'grep', '-n', 'AIOS 概念', 'e457744', '--', '*.tsx', '*.ts'], {
  encoding: 'utf8',
}).trim().split('\n');
const bodies = rows.map((row) => {
  const first = row.indexOf(':');
  const second = row.indexOf(':', first + 1);
  const file = row.slice(0, first);
  const line = row.slice(first + 1, second);
  const text = row.slice(second + 1);
  return { file, line, body: text.split('AIOS 概念 · ')[1] ?? '' };
});
const files = [];
const walk = (dir) => {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full);
    else if (/\.(ts|tsx)$/.test(entry.name)) files.push(full);
  }
};
walk(src);
const hits = [];
for (const { file, line, body } of bodies) {
  const needle = body.slice(0, 24);
  for (const target of files) {
    const lines = readFileSync(target, 'utf8').split('\n');
    lines.forEach((text, index) => {
      if (text.includes(needle)) hits.push(target + ':' + (index + 1) + ' ← ' + file + ':' + line);
    });
  }
}
console.log('原型正文 ' + bodies.length + ' 条；web/src 命中 ' + hits.length + ' 条');
for (const hit of hits) console.log('HIT ' + hit);
if (hits.length > 0) {
  console.error('原型正文仍出现在 web/src：' + hits.length + ' 条');
  process.exit(1);
}
console.log('0 命中：通过');
process.exit(0);
