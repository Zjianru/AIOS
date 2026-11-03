/**
 * T008.5 DoD5：key 不外泄扫描（从运行时 env 取 key，只打印命中文件/次数，不打印 key）。
 * 扫描：WORK/T008/evidence 下文本证据 + /tmp/aios-t008-dod 下明文文件（跳过 .age 密文）。
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const key = process.env.DEEPSEEK_API_KEY;
if (!key) {
  console.error('缺少 DEEPSEEK_API_KEY（D34）');
  process.exit(2);
}
const probes = [key.slice(0, 8), key.slice(-8)];
const work = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T008/evidence');
const dod = '/tmp/aios-t008-dod';
const files = [];
function walk(root) {
  let entries;
  try {
    entries = readdirSync(root);
  } catch {
    return;
  }
  for (const entry of entries) {
    const full = path.join(root, entry);
    const stat = statSync(full);
    if (stat.isDirectory()) walk(full);
    else if (!full.endsWith('.age')) files.push(full);
  }
}
walk(work);
walk(dod);
let hits = 0;
for (const file of files) {
  let text;
  try {
    text = readFileSync(file, 'utf8');
  } catch {
    continue;
  }
  for (const probe of probes) {
    if (probe && text.includes(probe)) {
      hits += 1;
      console.log('命中：' + path.relative(work, file));
    }
  }
}
console.log('扫描文件数：' + files.length + '；命中：' + hits);
process.exit(hits === 0 ? 0 : 1);
