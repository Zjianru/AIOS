/**
 * 测速用的测试钩子：perf.mjs / shoot.mjs 通过 window.__v14 驱动界面。
 * 只放「点不到、但要量」的动作（滚到顶、开始流式、读 mark）。生产里没影响。
 */
const hooks = {};
export function registerHook(name, fn) {
    hooks[name] = fn;
}
export function installTestHooks() {
    window.__v14 = hooks;
}
