---
name: aios-calendar
description: "Use AIOS appointments: read the calendar, propose creating or changing an appointment on a confirmation card, or cancel one directly. Load before any calendar action."
---

# AIOS calendar

Use this skill when the user asks to be reminded, to watch a page/file/task fact, to run a task in rounds, or to read/change/cancel their calendar.

- Read first: call aios_query with topic "calendar". It returns every appointment (id, status, version, trigger, binding, watch, cycle, next trigger) and recent runs.
- Create: call propose_action with one option whose action is create_appointment. Details: title; condition (the user's own words); rule (iCal RRULE, for example FREQ=DAILY or FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR); timezone (IANA, for example Asia/Shanghai); the first trigger as either startAt (epoch milliseconds, a numeric string of milliseconds, an RFC3339 timestamp, or a local "YYYY-MM-DD HH:MM:SS") or delaySeconds (integer seconds from now; use this when the user says "in one minute"); binding as either "kind" or "task", or an object {"kind":"chat"|"kind"|"task","taskId":"..."} (chat means the Kind conversation); optional watch {kind: "url"|"file"|"taskFacts", target}; optional cycleTaskId and maxRounds.
- Change: call propose_action with update_appointment. Details: appointmentId, expectedVersion (the version you just read), and only the fields that change. If someone changed it in between, confirming the card is refused with "这条约定刚被改过".
- Cancel directly without a card: call appointment_cancel with appointmentId and reason. Use user when the user asked to cancel; use stopped when the appointment stops itself by its own condition (marks it ended).
- Do not create four separate tools. One read query, two card actions, one direct cancel are the whole surface.
- An incoming appointment_wake message carries the appointment and run facts; read it, then either remind the user, dispatch a task, or call task_continue when it is a cycle wake.
