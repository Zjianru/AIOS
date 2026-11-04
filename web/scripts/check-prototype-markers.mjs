// T007 DoD3：产品（src + 构建产物 + index.html）里不得出现原型专属标记。
import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';

const roots = ['src', 'dist', 'index.html'];
const markers = ['原型：', 'AIOS 概念 ·', '内核状态', '__v14', 'kindDemo'];
const hits = [];
function walk(target) {
  const stat = statSync(target, { throwIfNoEntry: false });
  if (!stat) return;
  if (stat.isDirectory()) {
    for (const entry of readdirSync(target)) walk(path.join(target, entry));
    return;
  }
  if (!/\.(tsx?|jsx?|html|css|md|json)$/.test(target)) return;
  const text = readFileSync(target, 'utf8');
  for (const marker of markers) {
    if (text.includes(marker)) hits.push(`${target}: ${marker}`);
  }
}
for (const root of roots) walk(root);
if (hits.length > 0) {
  console.error(hits.join('\n'));
  console.error(`原型专属标记 ${hits.length} 处（应为 0）`);
  process.exit(1);
}
console.log('原型专属标记 0 处');
