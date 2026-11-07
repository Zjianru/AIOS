import assert from 'node:assert/strict';
import { globMatch, matchDisabled, namesOf } from '../src/data/disabled.ts';

// 逐条照抄 Magpie TestDisabledGlob 的通配用例（和 service 的 disabled_tests 同一份表）。
const cases = [
  ['*fable*', 'claude-fable-5', true],
  ['*fable*', 'anthropic/claude-fable-5', true],
  ['claude-fable-5', 'claude-fable-5', true],
  ['claude-fable-5', 'claude-fable-5-mini', false],
  ['claude-fable-?', 'claude-fable-5', true],
  ['anthropic/*', 'anthropic/claude-fable-5', true],
  ['anthropic/*', 'openrouter/anthropic/claude-fable-5', false],
  ['*', '', true],
  ['a*b*c', 'aXbYc', true],
  ['a*b*c', 'aXbY', false],
];
for (const [pat, value, expected] of cases) {
  assert.equal(globMatch(pat, value), expected, pat + ' ~ ' + value);
}

assert.deepEqual(namesOf('openrouter', 'anthropic/claude-fable-5'), [
  'anthropic/claude-fable-5',
  'openrouter/anthropic/claude-fable-5',
  'claude-fable-5',
]);

assert.equal(matchDisabled([' ', '*FABLE*'], 'openrouter', 'anthropic/claude-fable-5'), '*FABLE*');
for (const pattern of [
  'claude-fable-5',
  'openrouter/anthropic/claude-fable-5',
  'anthropic/claude-fable-5',
]) {
  assert.equal(matchDisabled([pattern], 'openrouter', 'anthropic/claude-fable-5'), pattern);
}
assert.equal(matchDisabled(['anthropic/*'], 'openrouter', 'x'), undefined);
assert.equal(matchDisabled(['DEEPSEEK/DEEPSEEK-FLASH'], 'deepseek', 'deepseek-flash'), 'DEEPSEEK/DEEPSEEK-FLASH');
assert.equal(matchDisabled(['m9'], 'deepseek', 'deepseek-flash'), undefined);
console.log('disabled ok');
