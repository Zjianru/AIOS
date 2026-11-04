#!/usr/bin/env node
// F3 验收：kindStore 里不能有演示动作/演示脚本/假压缩。
import { readFileSync } from 'node:fs';
const file = 'src/pages/kind/kindStore.ts';
const text = readFileSync(file, 'utf8');
const patterns = ['liveDemo', 'compactionSummary', 'demo:', 'startRun', 'LiveScript', 'LiveDemo', 'pendingScripts', 'nextSendFails'];
const hits = [];
text.split('\n').forEach((line, index) => {
  for (const pattern of patterns) {
    if (line.includes(pattern)) hits.push(file + ':' + (index + 1) + ' ' + pattern);
  }
});
console.log('kindStore 演示符号命中 ' + hits.length + ' 条');
for (const hit of hits) console.log('HIT ' + hit);
process.exit(hits.length === 0 ? 0 : 1);
