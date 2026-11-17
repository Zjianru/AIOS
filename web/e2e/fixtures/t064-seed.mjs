// T064.6 e2e：用 sqlite3 生成一份日历夹具（时间按运行当天算），并写三条唤醒的根会话日志。
// 这里只造页面断言需要的事实；服务端的规则/周计划仍由真服务算。
import { spawnSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

const DAY = 86_400_000;
const HOUR = 3_600_000;

function localAt(offsetDays, hour, minute = 0) {
  const date = new Date();
  date.setDate(date.getDate() + offsetDays);
  date.setHours(hour, minute, 0, 0);
  return date.getTime();
}

function nextWeekdayAt(hour) {
  for (let offset = 0; offset < 8; offset += 1) {
    const stamp = localAt(offset, hour);
    const day = new Date(stamp).getDay();
    if (stamp > Date.now() + 60_000 && day !== 0 && day !== 6) return stamp;
  }
  return localAt(1, hour);
}

function previousWeekday(index) {
  const days = [];
  for (let offset = -1; days.length < index + 1 && offset > -20; offset -= 1) {
    const stamp = localAt(offset, 9);
    const day = new Date(stamp).getDay();
    if (stamp < Date.now() - 60_000 && day !== 0 && day !== 6) days.push(stamp);
  }
  return days[index] ?? localAt(-1, 9);
}

function sqlQuote(value) {
  return "'" + String(value).replaceAll("'", "''") + "'";
}

export async function seedT064Calendar(dataDir) {
  const monday = localAt(-7, 9);
  const next = nextWeekdayAt(9);
  const run1 = previousWeekday(0);
  const run2 = previousWeekday(1);
  const run3 = previousWeekday(2);
  const appointments = [
    {
      id: 'ap_morning',
      status: 'active',
      version: 1,
      title: '早报',
      condition: '每个工作日早上 9 点，把昨天报回的任务和今天等我的事列给我',
      rule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR',
      timezone: 'Asia/Shanghai',
      start_at: monday,
      binding_kind: 'kind',
      binding_ref: '',
      next_trigger_at: next,
      source_turn: 3,
      created_at: Date.now() - 12 * DAY,
    },
    {
      id: 'ap_watch',
      status: 'active',
      version: 2,
      title: '盯 elkjs',
      condition: 'elkjs 出新版就告诉我',
      rule: 'FREQ=HOURLY;INTERVAL=6',
      timezone: 'Asia/Shanghai',
      start_at: Date.now() - 2 * DAY,
      binding_kind: 'kind',
      binding_ref: '',
      next_trigger_at: Date.now() + 2 * HOUR,
      source_turn: 2,
      watch_kind: 'url',
      watch_target: 'https://example.com/elkjs',
      created_at: Date.now() - 10 * DAY,
    },
    {
      id: 'ap_loop',
      status: 'paused',
      version: 2,
      title: '循环测试',
      condition: '把 rank/ 的测试一轮轮做到全绿，最多 5 轮',
      rule: 'FREQ=DAILY',
      timezone: 'Asia/Shanghai',
      start_at: Date.now() - 3 * DAY,
      binding_kind: 'task',
      binding_ref: 'task-demo',
      cycle_task_id: 'task-demo',
      cycle_max_rounds: 5,
      next_trigger_at: null,
      source_turn: null,
      created_at: Date.now() - 5 * DAY,
    },
  ];
  const inserts = appointments
    .map((a) => {
      const columns = [
        'id', 'status', 'version', 'title', 'condition', 'rule', 'timezone', 'start_at',
        'binding_kind', 'binding_ref', 'watch_kind', 'watch_target', 'cycle_task_id',
        'cycle_max_rounds', 'next_trigger_at', 'source_turn', 'created_at', 'updated_at',
      ];
      const values = [
        sqlQuote(a.id), sqlQuote(a.status), String(a.version), sqlQuote(a.title),
        sqlQuote(a.condition), sqlQuote(a.rule), sqlQuote(a.timezone), String(a.start_at),
        sqlQuote(a.binding_kind), sqlQuote(a.binding_ref), a.watch_kind ? sqlQuote(a.watch_kind) : 'NULL',
        a.watch_target ? sqlQuote(a.watch_target) : 'NULL', a.cycle_task_id ? sqlQuote(a.cycle_task_id) : 'NULL',
        a.cycle_max_rounds ? String(a.cycle_max_rounds) : 'NULL',
        a.next_trigger_at ? String(a.next_trigger_at) : 'NULL',
        a.source_turn ? String(a.source_turn) : 'NULL',
        String(a.created_at), String(a.created_at),
      ];
      return 'INSERT INTO appointment (' + columns.join(',') + ') VALUES (' + values.join(',') + ');';
    })
    .join('\n');
  const versions = [
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,reason,created_at) VALUES ('ap_morning',1,'早报','每个工作日早上 9 点，把昨天报回的任务和今天等我的事列给我','FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR','Asia/Shanghai'," + String(monday) + ",'kind','',NULL,NULL,NULL,NULL,'create'," + String(Date.now() - 12 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,reason,created_at) VALUES ('ap_watch',1,'盯 elkjs','elkjs 出新版就告诉我','FREQ=HOURLY;INTERVAL=6','Asia/Shanghai'," + String(Date.now() - 2 * DAY) + ",'kind','','url','https://example.com/elkjs',NULL,NULL,'create'," + String(Date.now() - 10 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,reason,created_at) VALUES ('ap_loop',1,'循环测试','把 rank/ 的测试一轮轮做到全绿，最多 5 轮','FREQ=DAILY','Asia/Shanghai'," + String(Date.now() - 3 * DAY) + ",'task','task-demo',NULL,NULL,'task-demo',5,'create'," + String(Date.now() - 5 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,reason,created_at) VALUES ('ap_loop',2,'循环测试','把 rank/ 的测试一轮轮做到全绿，最多 5 轮','FREQ=DAILY','Asia/Shanghai'," + String(Date.now() - 3 * DAY) + ",'task','task-demo',NULL,NULL,'task-demo',5,'循环约定暂停：连续 3 轮没有进展'," + String(Date.now() - 1 * DAY) + ");",
  ].join('\n');
  const runs = [
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id) VALUES ('ap_morning'," + String(run1) + ",0,'due','sent','',0," + String(run1) + "," + String(run1) + ",3,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id) VALUES ('ap_morning'," + String(run2) + ",0,'due','skipped','跳过：上一次 Kind 还在处理',0," + String(run2) + "," + String(run2) + ",NULL,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id) VALUES ('ap_morning'," + String(run3) + ",0,'due','sent','',2," + String(run3) + "," + String(run3) + ",2,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id) VALUES ('ap_watch'," + String(Date.now() - DAY) + ",0,'changed','failed','没送成：取不到网页（连接超时）',0," + String(Date.now() - DAY) + "," + String(Date.now() - DAY) + ",NULL,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id) VALUES ('ap_loop'," + String(Date.now() - 2 * DAY) + ",0,'report','sent','第 1 轮报回',0," + String(Date.now() - 2 * DAY) + "," + String(Date.now() - 2 * DAY) + ",2,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id) VALUES ('ap_loop'," + String(Date.now() - DAY) + ",0,'report','sent','第 2 轮报回',0," + String(Date.now() - DAY) + "," + String(Date.now() - DAY) + ",3,NULL);",
  ].join('\n');
  const sql = [
    'PRAGMA journal_mode=WAL;',
    'CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);',
    "INSERT INTO kv VALUES ('schema_version','4');",
    'CREATE TABLE appointment (id TEXT PRIMARY KEY,status TEXT NOT NULL,version INTEGER NOT NULL,title TEXT NOT NULL,condition TEXT NOT NULL,rule TEXT NOT NULL,timezone TEXT NOT NULL,start_at INTEGER NOT NULL,binding_kind TEXT NOT NULL,binding_ref TEXT NOT NULL,watch_kind TEXT,watch_target TEXT,cycle_task_id TEXT,cycle_max_rounds INTEGER,next_trigger_at INTEGER,source_turn INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);',
    'CREATE TABLE appointment_version (appointment_id TEXT NOT NULL,version INTEGER NOT NULL,title TEXT NOT NULL,condition TEXT NOT NULL,rule TEXT NOT NULL,timezone TEXT NOT NULL,start_at INTEGER NOT NULL,binding_kind TEXT NOT NULL,binding_ref TEXT NOT NULL,watch_kind TEXT,watch_target TEXT,cycle_task_id TEXT,cycle_max_rounds INTEGER,reason TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY (appointment_id, version));',
    'CREATE TABLE run (id INTEGER PRIMARY KEY AUTOINCREMENT,appointment_id TEXT NOT NULL,nominal_at INTEGER NOT NULL,manual INTEGER NOT NULL,trigger_kind TEXT NOT NULL,result TEXT NOT NULL,reason TEXT NOT NULL DEFAULT \'\',merged_count INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,sent_turn INTEGER,report_id TEXT,UNIQUE (appointment_id, nominal_at, manual));',
    'CREATE INDEX run_appointment ON run(appointment_id, nominal_at);',
    'CREATE TABLE watch_state (appointment_id TEXT PRIMARY KEY,content TEXT NOT NULL,hash TEXT NOT NULL,attachment_id TEXT,updated_at INTEGER NOT NULL);',
    inserts,
    versions,
    runs,
  ].join('\n');
  const pathToDb = path.join(dataDir, 'calendar.sqlite');
  const result = spawnSync('sqlite3', [pathToDb], { input: sql, encoding: 'utf8' });
  if (result.status !== 0) {
    throw new Error('sqlite3 生成日历夹具失败：' + (result.stderr || result.stdout));
  }

  const sessionId = '00000000-0000-4000-8000-000000000464';
  const wake = (turn, rpc, body) => [
    JSON.stringify({ type: 'turn/start', seq: turn * 4 - 3, time: Date.now() - (4 - turn) * 60_000, data: { turn } }),
    JSON.stringify({ type: 'user/message', seq: turn * 4 - 2, time: Date.now() - (4 - turn) * 60_000 + 1, data: { turn, step: 1, message: { id: 'u-' + String(turn), role: 'user', source: { kind: 'user', rpcId: rpc }, content: [{ type: 'text', text: JSON.stringify(body) }] } } }),
    JSON.stringify({ type: 'assistant/message', seq: turn * 4 - 1, time: Date.now() - (4 - turn) * 60_000 + 2, data: { turn, step: 1, message: { id: 'a-' + String(turn), role: 'assistant', source: { kind: 'model', provider: 'deepseek', model: 'deepseek-flash' }, content: [{ type: 'text', text: '收到，先看事实。' }] }, stream: [], usage: { inputTokens: 10, outputTokens: 3 } } }),
    JSON.stringify({ type: 'turn/end', seq: turn * 4, time: Date.now() - (4 - turn) * 60_000 + 3, data: { turn, reason: { kind: 'completed' } } }),
  ];
  const wakeBody = (appointment, runId, trigger, extra) => ({
    kind: 'appointment_wake',
    appointment,
    run_id: runId,
    trigger,
    nominal_at: Date.now() - 60_000,
    merged_count: 0,
    manual: false,
    ...extra,
  });
  const rows = [
    JSON.stringify({ type: 'session', version: 18, id: sessionId, createdAt: Date.now() - 300_000, cwd: dataDir }),
    ...wake(1, 'appointment-wake:ap_morning:1', wakeBody({ id: 'ap_morning', condition: '每个工作日早上 9 点，把昨天报回的任务和今天等我的事列给我', version: 1, binding: 'kind' }, 1, 'due', { merged_count: 2 })),
    ...wake(2, 'appointment-wake:ap_watch:2', wakeBody({ id: 'ap_watch', condition: 'elkjs 出新版就告诉我', version: 2, binding: 'kind' }, 2, 'changed', { change: { summary: '变了：\n--- 上次\n+++ 这次\n+version 3.2.0\n-version 3.1.0', attachment_id: 'sha256:0000000000000000000000000000000000000000000000000000000000000464' } })),
    ...wake(3, 'appointment-wake:ap_loop:3', wakeBody({ id: 'ap_loop', condition: '把 rank/ 的测试一轮轮做到全绿，最多 5 轮', version: 2, binding: 'task' }, 3, 'report', { reason: '第 2 轮报回' })),
  ].join('\n');
  await mkdir(path.join(dataDir, 'sessions'), { recursive: true });
  await writeFile(path.join(dataDir, 'sessions', sessionId + '.jsonl'), rows + '\n');
  await writeFile(path.join(dataDir, 'kind-root.json'), JSON.stringify({ version: 1, sessionId }));
}
