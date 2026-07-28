# loop.md — Loop Contract: overlay-session-attach

## Goal
Stop the ai-monitor overlay presenting an unrelated session's history as the active tab's telemetry, and stop it reporting a context percentage above 100.

## The defect (measured, 2026-07-28)
`resolve_log` → `newest_jsonl` (`crates/ai-monitor/src/lib.rs:63`) picks the most-recently-modified `*.jsonl` in the cwd-encoded project directory. That is a **directory**-level heuristic with no binding to the process running in the tab. Measured on the reported case: the app launched at 11:38 local; nothing in `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal/` had been written since 11:02, because the tab's brand-new Claude Code session had not yet produced a log at all. The overlay therefore attached to `54e576c2` — last event `2026-07-28T16:52:57Z`, first event ~5 days earlier — backfilled its whole history, and rendered `up 114h37m · idle 1h50m` as if it were the new session.

Two facts bound the fix:
- **There is no exact binding available.** A live `claude` process holds no open fd on its own `.jsonl` (`lsof -c claude` → 0 matches; it opens, appends, closes), so the log cannot be attributed to a PTY's descendant from outside.
- **`idle 1h50m` was already on screen.** The data needed to disbelieve the panel was being rendered and nothing acted on it.

Separately, `context 128%` is real: that session's last usage line totals 256,551 tokens against an assumed 200k window. The fold is correct; the window is wrong (a 1M-context session logs as a bare `claude-opus-4-8`, the Known Gap in `specs/overlay-dashboard-frame/spec.md`).

## Approach (human-approved 2026-07-28)
1. **Re-resolve, don't latch.** Watch the project *directory* rather than one file, and switch to a newer `*.jsonl` the moment one appears — so the tab's own session is picked up as soon as it writes its first line.
2. **A session that stopped writing before this window existed is not this window's session.** The floor is the app's own start time, injected like `now_ms`. It is a *fact*, not a threshold: it never hides a session that has been active at any point since launch — including one idle for hours — which a plain idle-timeout would.
3. **Clamp the percentage and allow an override.** Over 100% renders `>100%`, and `[ai_monitor] context_window` in the TOML config overrides the per-model table.

## Scope & non-goals
- In: `crates/ai-monitor/src/lib.rs`, `crates/app/src/overlay.rs`, `crates/app/src/dashboard.rs`, `crates/common/src/config.rs`
- Out: `crates/app/src/terminal_app.rs`, `seam.rs`, `settings.rs`, `crates/renderer/**`. No `OverlayModel` change, no colour.
- Known limitation to document, not fix: a *foreign* session that has been active since app start can still be picked. Only per-tab binding would fix that, and it is unavailable (above).

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| stale log blocked | a log whose last event precedes the floor renders the detached state, **not** its session data | named test, seen as `... ok` in run output |
| live log kept | a log whose last event is at/after the floor renders normally, even when idle for hours | named test |
| re-resolve | a newer `*.jsonl` appearing in the watched dir switches the watcher and emits a snapshot from the **new** file | named test |
| percentage clamp | `context_used` above the window renders `>100%` — no numeral above 100 anywhere | named test |
| config override | `[ai_monitor] context_window` beats the model table | named test |
| no regression | the 2 frame + 7 boxes snapshots unchanged; whole suite green | `cargo test --workspace` |

## Gate 3 — Qualitative
- **Policy/mechanism split (SD4)** — the floor, the clamp and the window override are app-side; only directory watching and re-resolution live in `ai-monitor`.
- **Dependency direction** — `ai-monitor` still imports no `wgpu`/`winit`/`renderer`/`app`.
- **Honest degradation** — the detached state says *why* it is detached (and how stale the newest log is); it never silently blanks and never presents a foreign session as live.
- **Purity** — `dashboard::compose` still reads no clock and no filesystem; the floor is injected exactly like `now_ms`.
- **No panic path** — a missing project directory, an unreadable log, or a log deleted mid-watch degrades, never aborts the terminal's own process.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the four declared files, including changing `compose`'s signature and re-accepting snapshots whose content is unchanged.
**🛑 STOP — escalate:**
- Editing `terminal_app.rs`, `seam.rs`, `settings.rs` or `crates/renderer/**` (out of surface) — including adding a per-tab spawn-time channel, which is the *better* floor but needs seam plumbing.
- Adding any crate dependency.
- Any write to a session log, or any change to `~/.claude`.
- Changing `OverlayModel` (SD3).
- Anything that would make the overlay guess which session a tab owns rather than state what it knows.

## Exit criteria
Every gate passes and the Evidence Report is emitted.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Recorded 2026-07-28 after a 3-iteration run. `source: human-approved` — the
     human was asked and approved recording these. -->

### L1 — two fixes to the same fail-open must agree on their polarity
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: When a rule is enforced in two places (here: the watcher's latch and the display's floor), state the shared predicate once in words, then check each site against that sentence. "Cannot be shown to postdate the window" must mean the same thing on both sides.
- note: the display layer was taught to REFUSE an undated log (`Detached::Undated`) in the same iteration the watcher was taught to LATCH on one (`is_none_or` where `is_some_and` was meant). The result was worse than either bug alone: the watcher pinned itself to a log the display would never render, disabling the re-resolution that could have rescued it. Both edits were made minutes apart by the same author and neither test caught it — the checker did.

### L2 — a "last seen" field is not a "maximum" field
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Before comparing an accumulated timestamp against a threshold, confirm how it is written. `SessionTelemetry` assigns `last_event_ms` unconditionally, so it can move BACKWARD on an out-of-order line. Any latch or gate keyed on it must be monotonic on its own (`latched = latched || …`).
- note: the first latch recomputed itself on every fold, so a single backdated timestamp would un-latch a live session mid-flight, hand the attachment to another file, and discard everything accumulated. The comment above it promised "after it we STAY"; the code did not.

### L3 — a fallback that walks up a path must have a stated stopping point
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Any "nearest existing ancestor" walk needs an explicit boundary, and a test asserting it declines rather than escaping. Name the last acceptable directory in the code.
- note: falling back to the nearest existing directory in order to watch for a project dir that does not exist yet climbed to `$HOME` for a user who has never run Claude Code — and then watched it RECURSIVELY for the panel's lifetime. The fix climbs exactly one level, to the projects root, and returns `None` beyond it.

### L4 — the exact-binding idea was refuted by one command; run it first
- cause: other
- source: human-approved
- form: context
- status: active
- check: —
- note: the obvious fix for "which log belongs to this tab" is to find the tab's `claude` process and ask which file it has open. `lsof -c claude | grep .jsonl` returns 0 matches — Claude Code opens, appends and closes, so there is no fd to attribute. Thirty seconds of measurement removed the whole design branch. Everything downstream of it (the floor rule, the latch, the documented limitation) exists *because* that binding is unavailable.

### L5 — the panel was already showing the evidence it needed
- cause: other
- source: human-approved
- form: context
- status: active
- check: —
- note: the misattributed session rendered `idle 1h50m` on a window seconds old. The data that disproved the attachment was on screen and nothing acted on it. When a display can compute a fact that contradicts its own framing, the fact should gate the framing — not sit next to it.
