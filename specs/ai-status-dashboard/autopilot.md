# autopilot.md — ai-status-dashboard

> Durable run-control ledger for the unattended build of the ai-monitor status dashboard.
> Config human-approved 2026-07-28. The governor's source of truth across every tick;
> written atomically (temp + rename). The single-flight lock is a separate atomic lockdir
> (`specs/ai-status-dashboard/.tick.lock`), NOT a field in this file.

## Config   (human-approved before the run started)
- build_order: specs/ai-status-dashboard/build_order.md
- cadence_seconds: 1800               # primary ScheduleWakeup delay; MUST be < per_tick_timeout
- per_tick_timeout_seconds: 5400      # a tick is ONE whole feature's /loop; sized to telemetry-model
- wall_clock_cap_hours: 10
- max_ticks: 24                       # cost cap; 4 features realistically need 4-12 ticks
- consecutive_park_cap: 1
- crash_loop: { max: 2, window_minutes: 240 }
- killswitch_path: specs/ai-status-dashboard/STOP

## State   (the durable authority for halting — re-derived at every tick entry)
- status: complete
- halt_reason: work complete — all 4 features done, gates green, 4 commits on build/ai-status-dashboard
- run_started_at: 2026-07-28T15:20:00Z
- tick_count: 4
- last_tick_started_at: 2026-07-28T17:03:39Z
- lease_renewed_at: 2026-07-28T17:03:39Z
- last_tick_completed_at: 2026-07-28T17:20:05Z
- consecutive_parks: 0
- recent_crashes: []
- backstop_cron_id: d19eeb00   # session-only (no durable cron in this environment)

<!--
Invariants:
- cadence_seconds (1800) < per_tick_timeout_seconds (5400) — validated at start.
- A tick acquires specs/ai-status-dashboard/.tick.lock via `mkdir` (atomic); a stale lock is
  reclaimed via `mv` (atomic), never rmdir+mkdir.
- Halt is authoritative via these counters, not the status flag alone.
- autopilot never drives a live session: no tmux send-keys, no /compact injection.

Config deviations from the template default, and why:
- per_tick_timeout 900 -> 5400. A ② unit is one feature's ENTIRE /loop. telemetry-model is 8
  tasks / 47 sub-requirements with checked-in fixtures and will exceed 15 minutes; under the
  default a later tick would reclaim the lock, record a crash, and 3 of those would halt a
  perfectly healthy run.
- cadence 240 -> 1800. Kept well under per_tick_timeout while not waking every 4 minutes into a
  lock it cannot acquire.
- max_ticks 200 -> 24. 200 is no cost cap for a 4-feature chain.
- consecutive_park_cap 3 -> 1. The feature graph is a strict chain, so the first park empties the
  ready set with work remaining — build-order deadlocks and stops. A cap above 1 only burns ticks
  rediscovering that.
- crash_loop window 10 -> 240 minutes. Two 90-minute ticks cannot both land inside 10 minutes, so
  the default window is unreachable and the limit would never fire.

ENVIRONMENT LIMIT — recorded because the design leans on it:
  CronCreate's `durable` flag has NO EFFECT here; all cron jobs are session-only and die with this
  session. Autopilot's backstop is specified as a durable cron that survives a session death and
  resurrects the run. That property is UNAVAILABLE. The backstop degrades to a second in-session
  timer: it still covers a dropped primary ScheduleWakeup, but it CANNOT resurrect the run across a
  session death. This run survives compaction, not session death.
-->
