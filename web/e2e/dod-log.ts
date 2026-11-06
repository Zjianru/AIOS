import { execFileSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import path from 'node:path';

/**
 * DoD2 的日志事实（T013 F2）：页面上的断言放在最后一个 [data-kind="answer"] 里；
 * 「回答之前有没有 read 那个路径」从会话日志用 jq 算，不靠页面文字猜。
 * 同一个 probe 也给不带 key 的守护用例用（标记只在你的消息里时，它必须报「没有回答」）。
 */

/** 一次按会话日志算：带标记的回答在第几条；read 过某个路径的调用在第几条。 */
export const READ_PROBE = [
  '[ .[] | select(.type=="tool/call") | {seq, name: .data.name, arguments: .data.arguments} ] as $calls',
  '| [ .[] | select(.type=="assistant/message")',
  '    | {seq, text: ([.data.message.content[]? | select(.type=="text") | .text] | join(""))} ] as $answers',
  '| { answerSeq: ([ $answers[] | select(.text | contains($marker)) | .seq ] | min),',
  '    readSeqs: [ $calls[] | select(.name=="read") | select(.arguments | contains($needle)) | .seq ] }',
].join(String.fromCharCode(10));

export interface ReadProbe {
  /** 带标记的那条回答的 seq；没有就是 null。 */
  answerSeq: number | null;
  /** read 过那个路径的调用 seq。 */
  readSeqs: number[];
}

/** 证据里抄一份能重跑的命令（参数带空格或换行就加单引号）。 */
export function renderCommand(args: string[]): string {
  return args
    .map((arg) => (arg.includes(' ') || arg.includes(String.fromCharCode(10)) ? "'" + arg + "'" : arg))
    .join(' ');
}

/** 会话日志（最新的那一份）。 */
export function sessionLog(dataDir: string): string {
  const dir = path.join(dataDir, 'sessions');
  const file = readdirSync(dir).filter((name) => name.endsWith('.jsonl')).sort().pop();
  if (!file) throw new Error('没有会话日志');
  return path.join(dir, file);
}

/** 跑一次 probe：返回 jq 命令、原始输出和解析结果（原始输出写进证据）。 */
export function runProbe(
  log: string,
  marker: string,
  needle: string,
): { args: string[]; raw: string; probe: ReadProbe } {
  const args = ['-c', '-s', '--arg', 'marker', marker, '--arg', 'needle', needle, READ_PROBE, log];
  const raw = execFileSync('jq', args, { encoding: 'utf8' }).trim();
  return { args, raw, probe: JSON.parse(raw) as ReadProbe };
}

/** 带标记的回答存在，而且它之前有过 read。 */
export function answeredAfterRead(probe: ReadProbe): boolean {
  return probe.answerSeq !== null && probe.readSeqs.some((seq) => seq < probe.answerSeq!);
}
