import assert from 'node:assert/strict';
import { eventSelectionEffort, serviceSelectionEffort } from '../src/pages/kind/effortSelection.ts';

// K1 回归：session/model 事件只带 wanted；实际档只能来自服务，事件处理不能拿 wanted 顶替。
assert.deepEqual(eventSelectionEffort('max'), { wanted: 'max', actual: undefined });
assert.equal(eventSelectionEffort('max').actual, undefined, '事件不能把 wanted 当 actual');
assert.equal(eventSelectionEffort(undefined).wanted, undefined);

// 服务回执/status：actual 有就用，没有就是未知（不回落 wanted）。
assert.deepEqual(serviceSelectionEffort('max', 'high'), { wanted: 'max', actual: 'high' });
assert.equal(serviceSelectionEffort('max', undefined).actual, undefined);
