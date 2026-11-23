/**
 * service 的程序消息前缀表（与 crates/service/src/program_messages.rs 同步）。
 * fixed-block 是固定块补发；它不是用户说的话，不进对话气泡。
 */
export const PROGRAM_MESSAGE_PREFIXES = [
  'task-report:',
  'task-stop:',
  'task-cc:',
  'gate-verdict:',
  'upstream-changed:',
  'appointment-wake:',
  'restart:',
  'fixed-block:',
  'date-now:',
  'dynamic-block:',
] as const;

export function isProgramRpc(rpcId: string | null | undefined): boolean {
  return typeof rpcId === 'string' && PROGRAM_MESSAGE_PREFIXES.some((prefix) => rpcId.startsWith(prefix));
}
