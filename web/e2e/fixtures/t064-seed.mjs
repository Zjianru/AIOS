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
      binding_ref: '00000000000000000000000000000003',
      cycle_task_id: '00000000000000000000000000000003',
      cycle_max_rounds: 5,
      next_trigger_at: null,
      source_turn: null,
      created_at: Date.now() - 5 * DAY,
    },
    {
      id: 'ap_once',
      status: 'ended',
      version: 1,
      title: '交周报',
      condition: '明早九点提醒我交周报',
      rule: 'FREQ=DAILY;COUNT=1',
      timezone: 'Asia/Shanghai',
      start_at: localAt(-1, 14),
      binding_kind: 'kind',
      binding_ref: '',
      next_trigger_at: null,
      source_turn: 3,
      duration_minutes: 30,
      created_at: Date.now() - 6 * DAY,
    },
    {
      id: 'ap_overlap',
      status: 'active',
      version: 1,
      title: '撞车会',
      condition: '和早报撞时间',
      rule: 'FREQ=DAILY',
      timezone: 'Asia/Shanghai',
      start_at: run1,
      binding_kind: 'kind',
      binding_ref: '',
      next_trigger_at: next,
      source_turn: 3,
      duration_minutes: 30,
      created_at: Date.now() - 9 * DAY,
    },
    {
      id: 'ap_cancelled',
      status: 'cancelled',
      version: 1,
      title: '取消的会',
      condition: '取消也要留块',
      rule: 'FREQ=DAILY',
      timezone: 'Asia/Shanghai',
      start_at: localAt(-2, 16),
      binding_kind: 'kind',
      binding_ref: '',
      next_trigger_at: null,
      source_turn: 3,
      duration_minutes: 30,
      created_at: Date.now() - 8 * DAY,
    },
    {
      id: 'ap_standup',
      status: 'active',
      version: 1,
      title: '每日站会',
      condition: '每天九点半站会',
      rule: 'FREQ=DAILY',
      timezone: 'Asia/Shanghai',
      start_at: monday,
      binding_kind: 'kind',
      binding_ref: '',
      next_trigger_at: next,
      source_turn: 3,
      duration_minutes: 15,
      created_at: Date.now() - 2 * DAY,
    },
  ];
  const inserts = appointments
    .map((a) => {
      const columns = [
        'id', 'status', 'version', 'title', 'condition', 'rule', 'timezone', 'start_at',
        'binding_kind', 'binding_ref', 'watch_kind', 'watch_target', 'cycle_task_id',
        'cycle_max_rounds', 'next_trigger_at', 'source_turn', 'duration_minutes', 'created_at', 'updated_at',
      ];
      const values = [
        sqlQuote(a.id), sqlQuote(a.status), String(a.version), sqlQuote(a.title),
        sqlQuote(a.condition), sqlQuote(a.rule), sqlQuote(a.timezone), String(a.start_at),
        sqlQuote(a.binding_kind), sqlQuote(a.binding_ref), a.watch_kind ? sqlQuote(a.watch_kind) : 'NULL',
        a.watch_target ? sqlQuote(a.watch_target) : 'NULL', a.cycle_task_id ? sqlQuote(a.cycle_task_id) : 'NULL',
        a.cycle_max_rounds ? String(a.cycle_max_rounds) : 'NULL',
        a.next_trigger_at ? String(a.next_trigger_at) : 'NULL',
        a.source_turn ? String(a.source_turn) : 'NULL',
        a.duration_minutes ? String(a.duration_minutes) : 'NULL',
        String(a.created_at), String(a.created_at),
      ];
      return 'INSERT INTO appointment (' + columns.join(',') + ') VALUES (' + values.join(',') + ');';
    })
    .join('\n');
  const versions = [
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_morning',1,'早报','每个工作日早上 9 点，把昨天报回的任务和今天等我的事列给我','FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR','Asia/Shanghai'," + String(monday) + ",'kind','',NULL,NULL,NULL,NULL,NULL,'create'," + String(Date.now() - 12 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_watch',1,'盯 elkjs','elkjs 出新版就告诉我','FREQ=HOURLY;INTERVAL=6','Asia/Shanghai'," + String(Date.now() - 2 * DAY) + ",'kind','','url','https://example.com/elkjs',NULL,NULL,NULL,'create'," + String(Date.now() - 10 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_loop',1,'循环测试','把 rank/ 的测试一轮轮做到全绿，最多 5 轮','FREQ=DAILY','Asia/Shanghai'," + String(Date.now() - 3 * DAY) + ",'task','task-demo',NULL,NULL,'task-demo',5,NULL,'create'," + String(Date.now() - 5 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_loop',2,'循环测试','把 rank/ 的测试一轮轮做到全绿，最多 5 轮','FREQ=DAILY','Asia/Shanghai'," + String(Date.now() - 3 * DAY) + ",'task','task-demo',NULL,NULL,'task-demo',5,NULL,'循环约定暂停：连续 3 轮没有进展'," + String(Date.now() - 1 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_once',1,'交周报','明早九点提醒我交周报','FREQ=DAILY;COUNT=1','Asia/Shanghai'," + String(localAt(-1, 14)) + ",'kind','',NULL,NULL,NULL,NULL,30,'create'," + String(Date.now() - 6 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_overlap',1,'撞车会','和早报撞时间','FREQ=DAILY','Asia/Shanghai'," + String(run1) + ",'kind','',NULL,NULL,NULL,NULL,30,'create'," + String(Date.now() - 9 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_cancelled',1,'取消的会','取消也要留块','FREQ=DAILY','Asia/Shanghai'," + String(localAt(-2, 16)) + ",'kind','',NULL,NULL,NULL,NULL,30,'create'," + String(Date.now() - 8 * DAY) + ");",
    "INSERT INTO appointment_version (appointment_id,version,title,condition,rule,timezone,start_at,binding_kind,binding_ref,watch_kind,watch_target,cycle_task_id,cycle_max_rounds,duration_minutes,reason,created_at) VALUES ('ap_standup',1,'每日站会','每天九点半站会','FREQ=DAILY','Asia/Shanghai'," + String(monday) + ",'kind','',NULL,NULL,NULL,NULL,15,'create'," + String(Date.now() - 2 * DAY) + ");",
  ].join('\n');
  const runs = [
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_morning'," + String(run1) + ",0,'due','sent','',0," + String(run1) + "," + String(run1) + ",3,NULL,'t066-trigger-morning-1');",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_morning'," + String(run2) + ",0,'due','skipped','跳过：上一次 Kind 还在处理',0," + String(run2) + "," + String(run2) + ",NULL,NULL,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_morning'," + String(run3) + ",0,'due','sent','',2," + String(run3) + "," + String(run3) + ",2,NULL,'t066-trigger-morning-3');",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_watch'," + String(Date.now() - DAY) + ",0,'changed','failed','没送成：取不到网页（连接超时）',0," + String(Date.now() - DAY) + "," + String(Date.now() - DAY) + ",NULL,NULL,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_loop'," + String(Date.now() - 2 * DAY) + ",0,'report','sent','第 1 轮报回',0," + String(Date.now() - 2 * DAY) + "," + String(Date.now() - 2 * DAY) + ",2,NULL,NULL);",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_loop'," + String(Date.now() - DAY) + ",0,'report','sent','第 2 轮报回',0," + String(Date.now() - DAY) + "," + String(Date.now() - DAY) + ",3,NULL,'t066-trigger-loop-3');",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_overlap'," + String(run1) + ",0,'due','sent','',0," + String(run1) + "," + String(run1) + ",3,NULL,'t066-trigger-overlap-1');",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_cancelled'," + String(localAt(-2, 16)) + ",0,'due','sent','',0," + String(localAt(-2, 16)) + "," + String(localAt(-2, 16)) + ",2,NULL,'t066-trigger-cancelled-1');",
    "INSERT INTO run (appointment_id,nominal_at,manual,trigger_kind,result,reason,merged_count,created_at,updated_at,sent_turn,report_id,task_id) VALUES ('ap_once'," + String(localAt(-1, 14)) + ",0,'due','sent','',0," + String(localAt(-1, 14)) + "," + String(localAt(-1, 14)) + ",2,NULL,'t066-trigger-once-1');",
  ].join('\n');
  const sql = [
    'PRAGMA journal_mode=WAL;',
    'CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);',
    "INSERT INTO kv VALUES ('schema_version','5');",
    'CREATE TABLE appointment (id TEXT PRIMARY KEY,status TEXT NOT NULL,version INTEGER NOT NULL,title TEXT NOT NULL,condition TEXT NOT NULL,rule TEXT NOT NULL,timezone TEXT NOT NULL,start_at INTEGER NOT NULL,binding_kind TEXT NOT NULL,binding_ref TEXT NOT NULL,watch_kind TEXT,watch_target TEXT,cycle_task_id TEXT,cycle_max_rounds INTEGER,next_trigger_at INTEGER,source_turn INTEGER,duration_minutes INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);',
    'CREATE TABLE appointment_version (appointment_id TEXT NOT NULL,version INTEGER NOT NULL,title TEXT NOT NULL,condition TEXT NOT NULL,rule TEXT NOT NULL,timezone TEXT NOT NULL,start_at INTEGER NOT NULL,binding_kind TEXT NOT NULL,binding_ref TEXT NOT NULL,watch_kind TEXT,watch_target TEXT,cycle_task_id TEXT,cycle_max_rounds INTEGER,duration_minutes INTEGER,reason TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY (appointment_id, version));',
    'CREATE TABLE run (id INTEGER PRIMARY KEY AUTOINCREMENT,appointment_id TEXT NOT NULL,nominal_at INTEGER NOT NULL,manual INTEGER NOT NULL,trigger_kind TEXT NOT NULL,result TEXT NOT NULL,reason TEXT NOT NULL DEFAULT \'\',merged_count INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,sent_turn INTEGER,report_id TEXT,task_id TEXT,UNIQUE (appointment_id, nominal_at, manual));',
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

  // T066：触发任务/派出任务夹具；工作快照只在日历侧用。
  const taskRow = (id, title, appointmentId, runId, role, roundEnd, createdAt) =>
    JSON.stringify({
      id,
      title,
      requirement: title,
      dod: '到点办完',
      cwd: dataDir,
      conversation: 'kind-calendar-fixture',
      model: { connection: 'deepseek', model: 'deepseek-flash', effort_wanted: 'high' },
      tools: [],
      write_scope: null,
      bash: false,
      worktree: false,
      worktree_state: null,
      writes: [],
      code_mode: false,
      runs: [],
      reports: [],
      inputs: [],
      versions: [{ write_scope: null, at: createdAt, source: 'task_accept', requirement: title, dod: '到点办完' }],
      parent: null,
      needs: [],
      arrangement: null,
      gate: null,
      verdicts: [],
      missing: [],
      created_at: createdAt,
      origin: { appointmentId, runId, role },
      ...(roundEnd ? { round_end: roundEnd } : {}),
    });
  const tasks = [
    ['t066-trigger-morning-1', taskRow('t066-trigger-morning-1', '早报', 'ap_morning', 1, 'trigger', { turn: 3, at: run1 + 20 * 60_000 }, run1)],
    ['t066-trigger-morning-3', taskRow('t066-trigger-morning-3', '早报', 'ap_morning', 3, 'trigger', { turn: 2, at: run3 + 25 * 60_000 }, run3)],
    ['t066-trigger-loop-3', taskRow('t066-trigger-loop-3', '循环测试', 'ap_loop', 6, 'trigger', { turn: 3, at: Date.now() - DAY + 20 * 60_000 }, Date.now() - DAY)],
    ['t066-trigger-overlap-1', taskRow('t066-trigger-overlap-1', '撞车会', 'ap_overlap', 7, 'trigger', { turn: 3, at: run1 + 30 * 60_000 }, run1)],
    ['t066-trigger-cancelled-1', taskRow('t066-trigger-cancelled-1', '取消的会', 'ap_cancelled', 8, 'trigger', { turn: 2, at: localAt(-2, 16) + 30 * 60_000 }, localAt(-2, 16))],
    ['t066-trigger-once-1', taskRow('t066-trigger-once-1', '交周报', 'ap_once', 9, 'trigger', { turn: 2, at: localAt(-1, 14) + 30 * 60_000 }, localAt(-1, 14))],
    ['t066-dispatched-1', taskRow('t066-dispatched-1', '调查 elkjs 调查', 'ap_loop', 6, 'dispatched', null, Date.now() - DAY + 5 * 60_000)],
  ];
  const tasksSql = [
    'PRAGMA journal_mode=WAL;',
    'CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);',
    "INSERT INTO kv VALUES ('schema_version','2');",
    'CREATE TABLE task (id TEXT PRIMARY KEY, value TEXT NOT NULL);',
    ...tasks.map(([id, value]) => 'INSERT INTO task (id, value) VALUES (' + sqlQuote(id) + ',' + sqlQuote(value) + ');'),
  ].join('\n');
  const tasksDb = path.join(dataDir, 'tasks.sqlite');
  const tasksResult = spawnSync('sqlite3', [tasksDb], { input: tasksSql, encoding: 'utf8' });
  if (tasksResult.status !== 0) {
    throw new Error('sqlite3 生成任务夹具失败：' + (tasksResult.stderr || tasksResult.stdout));
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
