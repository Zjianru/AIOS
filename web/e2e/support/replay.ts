//! 起/停 aios-replay 进程；路由文件优先，没有就按场景选 jsonl + override。

import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import type { FixtureOptions } from './options';
import {
  replaySelection,
  restartRouteFile,
  taskResultRouteFile,
  t013RouteFile,
  t073RouteFile,
  t030RouteFile,
  t052RouteFile,
  t053RouteFile,
  t054RouteFile,
  t055RouteFile,
  t056RouteFile,
  t057RouteFile,
} from './scenario-routes';

const FIXTURE_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), '../fixtures');


/** 起 aios-replay（打印一行 aios-replay: http://…）；每夹具一个进程。
 *
 * ：显式 routesFile 和 port 用于服务重启前换回放脚本（同一个端口，连接不用改）。
 */
export function startReplay(
  options: FixtureOptions,
  replayFile?: string,
  routesFile?: string | null,
): { child: ReturnType<typeof spawn>; url: Promise<string> } {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
  const bin = process.env.AIOS_REPLAY_BIN ?? path.join(repoRoot, 'target/debug/aios-replay');
 // 双会话（Kind + ThinAgent）按请求体各回各的脚本；路由文件里帧间隔逐条给。
  const routeFile =
    routesFile === undefined
      ? (t030RouteFile(options) ??
        restartRouteFile(options) ??
        t057RouteFile(options) ??
        t056RouteFile(options) ??
        t055RouteFile(options) ??
        t073RouteFile(options) ??
        t013RouteFile(options) ??
        t052RouteFile(options) ??
        t053RouteFile(options) ??
        t054RouteFile(options) ??
        taskResultRouteFile(options))
      : routesFile;
  const args: string[] = [];
  if (routeFile) {
    args.push('--routes', path.isAbsolute(routeFile) ? routeFile : path.join(FIXTURE_DIR, routeFile));
  } else {
    const selection = replaySelection(options);
    args.push('--file', replayFile ?? path.join(FIXTURE_DIR, selection.file + '.jsonl'));
    if (selection.override) args.push('--override', path.join(FIXTURE_DIR, selection.override + '.json'));
    if (selection.paceMs) args.push('--pace-ms', String(selection.paceMs));
  }
 // 假厂商也回一份 /v1/models：保存连接会来拉一次（默认按 DeepSeek 的真实模型配）。
  const vendorModels = options.vendorModels ?? ['deepseek-flash', 'deepseek-v4-pro'];
  if (vendorModels.length > 0) args.push('--vendor-models', vendorModels.join(','));
  args.push('--host', '127.0.0.1', '--port', '0');
  const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const url = new Promise<string>((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => reject(new Error('aios-replay 启动超时: ' + output)), 20_000);
    child.stdout?.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      const match = /aios-replay: (http:\/\/\S+)/.exec(output);
      if (match) {
        clearTimeout(timer);
        resolve(match[1]);
      }
    });
    child.stderr?.on('data', (chunk: Buffer) => {
      output += chunk.toString();
    });
  });
  return { child, url };
}
