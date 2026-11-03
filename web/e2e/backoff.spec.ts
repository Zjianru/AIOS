import { test, expect } from '@playwright/test';
import { reconnectDelayMs } from '../src/data/reconnect';

/// T007.9：重连退避是 300/600/1200/2400/4800 递增，最后封顶 5000。
test('断线重连退避指数递增且封顶', () => {
  expect([1, 2, 3, 4, 5, 6, 7].map(reconnectDelayMs)).toEqual([
    300, 600, 1200, 2400, 4800, 5000, 5000,
  ]);
  expect(reconnectDelayMs(0)).toBe(0);
  expect(reconnectDelayMs(10)).toBe(5000);
});
