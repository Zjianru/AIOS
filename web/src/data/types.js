/**
 * 假数据层的类型 = AIOS 真实协议的形状（第 1 期只取界面用得到的字段）。
 *
 * 依据：\`origin/main\` \`4a201c182\`
 *   - aios-rs/app-server-protocol/src/protocol/v2/thread.rs（ThreadResumeParams/Response、TurnsPage、ThreadTurnsListParams/Response）
 *   - aios-rs/app-server-protocol/src/protocol/v2/thread_data.rs（Turn）
 *   - aios-rs/app-server-protocol/src/protocol/v2/item.rs（ThreadItem，serde tag="type"，rename_all="camelCase"）
 *   - aios-rs/app-server-protocol/src/protocol/v2/turn.rs（UserInput、TurnStatus）
 *   - web/server/bridge.mjs:887、:1619-1626（真实前端怎么取首页、怎么 reverse）
 *
 * 字段名一律 camelCase（协议就是这样）。协议里查不到的，注释写「未核实」。
 * 界面只通过 src/data/api.ts 取数据；组件里不许直接 import 样例数据。
 */
export {};
