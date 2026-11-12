import assert from 'node:assert/strict';
import { modelErrorReason, verifyFailureReason } from '../src/data/errors.ts';

// T009 故事 3/4、测试 5：验证失败原因不显示内部码/状态数字；未知码给通用中文原因。
const cases = [
  [
    'HTTP_404: model MiniMax-M2 not found',
    'MiniMax（国内站）',
    'MiniMax（国内站） 返回了意外的错误：model MiniMax-M2 not found',
  ],
  ['HTTP_404', 'MiniMax（国内站）', 'MiniMax（国内站） 返回了意外的错误'],
  ['PROVIDER_BUSY: try later', 'DeepSeek', 'DeepSeek 返回了意外的错误：try later'],
  ['AUTH', 'DeepSeek', 'DeepSeek 拒绝了这把密钥'],
  ['AUTH: invalid x-api-key', 'DeepSeek', 'DeepSeek 拒绝了这把密钥：invalid x-api-key'],
  ['', 'DeepSeek', 'DeepSeek 返回了意外的错误'],
  // 没有「码: 」形状的是厂商/浏览器原文，原样保留。
  ['Failed to fetch', 'DeepSeek', 'Failed to fetch'],
  ['没有给出原因', 'DeepSeek', '没有给出原因'],
];
for (const [reason, name, expected] of cases) {
  const actual = verifyFailureReason(reason, name);
  assert.equal(actual, expected, reason);
  assert.ok(!actual.includes('HTTP_'), `不能显示内部码：${actual}`);
}
assert.ok(
  !verifyFailureReason('HTTP_404: model MiniMax-M2 not found', 'DeepSeek').includes('404'),
  '不能显示状态数字',
);
// 连接行按码查表：HTTP_* 走通用原因，不显示原始码。
assert.equal(modelErrorReason('HTTP_404', 'DeepSeek'), 'DeepSeek 返回了意外的错误');

console.log('errors ok');
