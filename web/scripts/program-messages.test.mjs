import assert from 'node:assert/strict';
import { isProgramRpc, PROGRAM_MESSAGE_PREFIXES } from '../src/data/programMessages.ts';

assert.equal(PROGRAM_MESSAGE_PREFIXES.length, 8);
assert.equal(isProgramRpc('fixed-block:0123456789abcdef'), true);
assert.equal(isProgramRpc('restart:boot-1'), true);
assert.equal(isProgramRpc('confirm:card-1'), false);
assert.equal(isProgramRpc('user-rpc-1'), false);
assert.equal(isProgramRpc(null), false);
console.log('program messages: fixed-block hidden from user bubbles');
