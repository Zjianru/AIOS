// Keyring 时间展示：epoch 秒 → 本地可读时间（纯函数，真值表；TZ 固定 Asia/Shanghai）。
import assert from 'node:assert/strict';
import { formatLocalDateTime } from '../src/lib/time.ts';

process.env.TZ = 'Asia/Shanghai';

assert.equal(formatLocalDateTime(1794078720), '2026-11-08 03:12');
assert.equal(formatLocalDateTime('1794078720'), '2026-11-08 03:12', '服务端给的是字符串');
assert.equal(formatLocalDateTime(1767294240), '2026-01-02 03:04', '月/日/时/分都补零');
assert.equal(formatLocalDateTime('今天 11:42'), '今天 11:42', '演示数据已是可读文本，原样显示');
assert.equal(formatLocalDateTime(''), '—');
assert.equal(formatLocalDateTime(null), '—');
console.log('time.test.mjs: all assertions passed');
