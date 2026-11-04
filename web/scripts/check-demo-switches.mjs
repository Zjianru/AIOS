#!/usr/bin/env node
// F2 验收：演示开关符号在 web/src 必须为 0。
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

const src = path.resolve(process.cwd(), 'src');
const patterns = [
  'connectedOnly', 'setKernel', 'setKeyringBroken', 'setFirstUse', 'setLatency',
  'setDevStressFlow', 'devStressFlow',
  // T008.F12：死开关/空壳函数不得复活。
  'setKeyringProblem', 'syncKeyringEmpty', 'keyringBroken',
];
// T008.F12：导出的空函数（空壳开关）一律不允许。
const emptyStub = /export function \w+\([^)]*\): void \{\}/;
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
for (const target of files) {
  const lines = readFileSync(target, 'utf8').split('\n');
  lines.forEach((text, index) => {
    for (const pattern of patterns) {
      if (text.includes(pattern)) hits.push(target + ':' + (index + 1) + ' ' + pattern);
    }
    if (emptyStub.test(text)) hits.push(target + ':' + (index + 1) + ' 空函数');
  });
}
console.log('演示开关命中 ' + hits.length + ' 条');
for (const hit of hits) console.log('HIT ' + hit);
if (process.argv.includes('--report-only')) process.exit(0);
process.exit(hits.length === 0 ? 0 : 1);
