# loop.md — Loop Contract: ai-monitor-session-rotation

## Goal

The ai-monitor panel must follow its tab's session when that session is **rotated in
place** — `/clear` retires the current log and opens a new one under a new id, in the same
project directory, from the same `claude` process. Today the panel keeps tailing the
retired file forever.

**Reproduced from real logs** (`~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal/`,
one `claude` process, pid 88187):

```
a3dc815b  06:03:20.527 -> 06:31:47.313   <- the session at process start
fc326d50  06:31:54.833 -> 06:32:20.813   <- /clear at index 3 of THIS file
3150bf9c  06:32:28.016 -> 06:32:48.218   <- /clear at index 3
d554e3c6  06:32:54.034 -> 06:33:26.853   <- /clear at index 3
```

A new `<uuid>.jsonl` opens and the old one is abandoned — never truncated or reused — so
nothing in the tail path fires and the panel's "last event" clock stops with it.

> **Corrected in iteration 2.** The first draft of this contract said "each file's LAST
> record is the `/clear`". Wrong: the marker lands at **index 3 of the SUCCESSOR**, ~5 ms
> after the rotation, not at the end of the predecessor. The diagnosis is unaffected, but the
> corrected timing matters — the replacement log almost always **already exists** by the time
> a re-point runs, so the "gap" this contract asked the user to choose behaviour for is
> milliseconds, not seconds, and the empty state will rarely be seen.

**Root cause** — attachment is latched, deliberately, on both sides:

- `crates/app/src/overlay.rs:162` `open()` is the only place a log is resolved. Its callers
  are the `Cmd+Shift+M` toggle (`:806`), `RestoreMonitor` (`:863`), and `on_active_changed`
  (`:876`) — and that last one re-opens only when `repoint.changed(visible, cwd, shell_pid)`
  says the **tab** changed. `/clear` changes no cwd, no shell pid, no tab.
- `overlay.rs:309-310` — `refresh_rivals` returns immediately once `attached_flag` is set.
- `crates/ai-monitor/src/lib.rs:892` — the watcher's re-resolution is behind
  `if !attached.load(..)`.
- `overlay.rs:643-645` — `Attachment::attached` is documented "Never cleared"; the only
  reset is `Session::repoint()`, called exclusively from `open()`.

This is a **design gap, not a regression.** The latch is `specs/ai-monitor-rival-assignment`
R6.3 ("Refresh stops once attached") and exists to stop a live panel being yanked to a
*sibling's* session. It has no notion of "the file I attached to was retired and replaced by
the same process."

**The authority that already knows.** `~/.claude/sessions/<claude-pid>.json` tracks the
rotation: for pid 88187 `sessionId` had moved to `d554e3c6` while `startedAt`/`procStart`
still pointed at the 06:03:20 process start — i.e. the id moved three times within one pid,
so the registry names the **incoming** session after a clear. `tabsession::hint_for` reads
exactly this and would return the new id if anything asked it again.

## The bar this fix must clear

A re-point must key on **authority** — the tab's own process naming a different session —
never on "a newer file exists in this directory". Re-enabling free re-resolution while
attached would restore precisely the defect R6.3, `ai-monitor-session-isolation` D2, and
`ai-monitor-rival-assignment` D3 were written to remove.

## Scope & non-goals

- **In:** `crates/ai-monitor/src/lib.rs` (the `Watch` contract and the watch thread),
  `crates/app/src/overlay.rs`, `crates/app/src/tabsession.rs`, and their tests.
- **Out:** `crates/term-core/**` (grid reflow is `specs/grid-reflow/spec.md`),
  `crates/renderer/**` (this changes *which session* is shown, never how it is painted).
- **Out of scope by decision, recorded as a limit:** the *user-visible* transition. When the
  panel re-points, the counters reset to the new session's — that is correct (R10.3: totals
  are session facts). No animation, no "session rotated" banner.

---

## Gate 1 — Pass/Fail (binary, 100% required)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| term-core untouched | `sh specs/ai-monitor-right-panel/check-no-term-core.sh` |
| renderer untouched (scope) | `test -z "$(git diff --name-only; git diff --cached --name-only \| grep '^crates/renderer/')"` |
| no stray subagent artifacts (L4) | `test -z "$(git status --porcelain --untracked-files=all -- crates/ \| grep -v '^ M')"` |
| committed baseline before verify (L3) | `git rev-parse HEAD` recorded in the report |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| suites green | `>= 18` | count of `test result: ok` (baseline 18 at `c89ee4f`) |
| tests passing | `>= 1119` | sum of `test result: ok` counts (baseline 1119) |
| the `/clear` repro | re-points | a test: attached to log A; the registry for our pid now names B; B exists in the dir → the watcher/overlay switches to B |
| totals rebuilt, not continued | new session's counts only | the same test asserts the accumulator was reset, not carried over (R10.3) |
| **rival protection intact** | no re-point | a test: a *foreign* `<uuid>.jsonl` appears in the same dir and the registry does NOT name it → still attached to A |
| **stale-registry protection** | no re-point | a test: the registry record fails `validated_session_id` (pid reuse) → still attached to A |
| R6.3's own tests unmodified | `== 0` changed | `attaching_mid_flight_stops_the_re_resolution` and siblings pass byte-identical |
| pre-existing assertions **weakened or deleted** | `== 0` | diff test bodies; an assert count that RISES is fine |
| new `/bin/ps` forks on the attached path | `== 0` | inspection: the attached-path check may read the registry FILE for a pid cached at `open()`, never re-run `hint_for` |

## Gate 3 — Qualitative (checker subagent — maker ≠ checker)

- **Authority, not recency** — does the re-point fire only when the tab's own process names a
  different session? A fix that re-enables free re-resolution, or that switches to "the newest
  jsonl", is a regression of R6.3 / session-isolation D2 / rival-assignment D3 even if every
  test passes. Score this against the code, not the comments.
- **pid reuse** — the pid is cached at `open()`. `~/.claude/sessions/` files outlive their
  processes (5 files for 4 live processes, when measured). Is `validated_session_id` applied
  to the re-read record, against the process start recorded at `open()`?
- **Re-point completeness** — after a switch, is the accumulator rebuilt from scratch, the
  offset reset, and the attachment's `session_id` updated? A half-switch (new file, old
  totals) is worse than the bug.
- **Idle cost (G1)** — what does this add to the idle path? The watcher fires per fs event in
  a directory that is silent when nothing is happening; the overlay path fires per metadata
  sync. State the added work per tick concretely.
- **The honest limit** — quitting `claude` and starting a fresh one in the same tab has the
  same shape (shell pid unchanged → `repoint.changed` false) but a *different* pid, so a
  cached-pid check cannot see it. Whether the fix covers it or records it as a gap, the
  statement must be accurate. An overstated limit is a finding; an accurately stated one is not.
- **Re-point latency is stated, not assumed** — the fix depends on when `claude` rewrites its
  registry record after `/clear`. The evidence proves the id *does* move; it does not prove
  how fast. The bound must be written down as measured-or-unknown, never implied.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the re-point signal and its plumbing through `Watch`,
the overlay's attached-path check, tests for all of it, doc/comment corrections, lint/format.

**🛑 STOP — escalate:**
- Weakening R6.3 / session-isolation D2 / rival-assignment D3 — i.e. any change that lets an
  unnamed or merely-newer log be adopted while attached.
- Any edit under `crates/term-core/**` or `crates/renderer/**`.
- Changing `Attachment::admit`'s "never revoked" rule for reasons other than an authoritative
  rotation.
- Weakening or deleting an existing assertion to make a gate pass.
- Adding a dependency, or a `/bin/ps` fork on the attached path.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.

## Measured facts (iteration 2)

Recorded so later work argues with numbers rather than with adjectives.

| Fact | Value | How |
|------|-------|-----|
| `registered_session` cost | **14 µs** warm | 20k iterations, release, real `~/.claude/sessions/` |
| registry record size | 429 B mean | the 8 live records |
| successor log appears after `/clear` | **~5 ms** | 88187 chain, `/clear` at index 3 of the successor |
| wake path cadence | **no floor, no ceiling** | `OverlayPending` fires per append burst; `tabmeta::worker::publish_if_changed` wakes only on a *changed* `TabMeta`, so an idle tab produces none |
| registry rotation latency | **the registry LEADS the new log by 120 ms** | caught live on a real `/clear` (pid 50936, 2026-08-08): old log last write `12:15:16.343`, registry rewritten `12:15:20.677` naming the new session, new log created `12:15:20.797`. One sample, but it settles the direction: the answer is already correct before there is a file to attach to. |
| wake availability after `/clear` | **none, until iteration 5** | the terminal is quiet; the attached watcher dropped sibling-file events; `publish_if_changed` wakes only on a changed `TabMeta`. The signal was prompt and correct and nothing read it. |

## Known gaps (accurate, not exhaustive-by-assumption)

1. **`claude` restarted in the same tab** — different pid, and `Repoint::changed` keys on
   (cwd, shell_pid), which a restart leaves untouched. Worse than merely undetected: the
   *dead* pid's registry file survives, still validates against the old process start, and
   still names the old session — so the check answers "no rotation" with apparent confidence.
2. **Two `claude` under one tab** — `tabsession::find_claude` BFS-picks one; a `/clear` in
   the other is invisible.
3. ~~**When the re-point fires is not established.**~~ **Measured** (see the table): the
   registry leads the new log by 120 ms, so the answer is ready before the replacement exists.
   What was missing was not promptness but a *wake* — fixed in iteration 5 by letting a
   sibling `.jsonl` event wake the caller without being adopted. Left visible rather than
   deleted: this gap was recorded as "unmeasured" for four iterations while being the reason
   the user's first two tests failed.
4. **Arming does not update `Watch.want`.** A panel opened before its session was named arms
   its baseline but keeps resolving on the weaker tier-2 dating path for the rest of that
   open. This follows from "arming must not re-point" rather than from an oversight; the next
   `/clear` re-opens and repairs it.
5. Covered, and worth naming because it was not designed for: an in-session `/resume` rotates
   the session id **within one pid**, so it is followed by the same mechanism.

## Decisions this narrows

`ai-monitor-rival-assignment` **R6.3** is written as "no registry reads and no membership
reads occur" once attached. Its **anti-yank half is intact** — nothing here lets an unnamed
or merely-newer log be adopted, and both latches (`refresh_rivals`' early return, the
watcher's `if !attached`) are byte-identical. Its **cost half is now false on the app side**:
an attached, visible panel performs one 14 µs registry read per wake. That was pre-authorised
by this contract's Gate 2 and is recorded in `session_rotated_with`'s doc so a reader of R6.3
finds the exception. The watcher's reads still stop dead.

## Lessons

_Seeded from this author's prior runs on adjacent contracts. They are re-recorded here
because lessons do not propagate between contracts (skill rule), and all three were paid for._

### L1 — never `git checkout --` an uncommitted working tree
- cause: escalation
- source: human-approved
- form: context
- status: active
- check: —
- note: a prior run mutation-tested a gate script then reverted with `git checkout -- <file>`
  on a tree where the whole feature was uncommitted — reverting to HEAD wiped ~1400 lines.
  Mutate a scratchpad copy, or commit first.

### L3 — commit before verifying
- cause: escalation
- source: human-approved
- form: context
- status: active
- check: —
- note: a run that can modify files must start from a committed baseline, so any destructive
  step is recoverable by `git restore` rather than by transcript archaeology. Baseline for
  this run: `c89ee4f`, clean tree, 18 suites / 1119 tests green.

### L5 — mutation-test from a file backup, never from `git restore`
- cause: escalation
- source: (pending human approval)
- form: context
- status: active
- check: —
- note: this run tripped L1 again in a milder form. Iterations 1 and 2 were committed before
  mutating, iteration 3 was not, and the `git restore` between two mutations silently reverted
  ~200 lines of uncommitted work back to the last commit — visible only as a test count that
  dropped from 521 to 515. Committing first is necessary but not sufficient, because the loop
  keeps producing uncommitted work between commits. The harness must `cp` the file to the
  scratchpad and restore from THAT, so the restore is scoped to the mutation regardless of
  what else is in flight.

### L6 — pin the lines that CONNECT a tested rule to production
- cause: gate-fail
- source: (pending human approval)
- form: gate3-item
- status: active
- check: "Wiring: for every pure rule added, is the call site that invokes it, and the impure
  input that feeds it, pinned by something? A rule can be exhaustively unit-tested and still
  be dead code."
- note: three checker rounds each found the same class in a new place — the arming write-back,
  `open()`'s baseline assignment, then `|| self.session_rotated()` and the `registered_session`
  argument. Each mutated to a constant with the entire suite green. Well-covered code on both
  sides of a seam is what HIDES the seam: the rule's tests pass because the rule is right, and
  the call site's tests pass because nothing tests the call site.

### L7 — do not assert a compiler constraint you have not compiled
- cause: gate-fail
- source: (pending human approval)
- form: context
- status: active
- check: —
- note: the same call site got two successive comments claiming a borrow error that does not
  exist — first to justify a `RefCell`, then to justify a local binding. Both were disproved by
  a checker simply writing the "impossible" form and building it. If a comment explains why the
  obvious shape cannot work, compile the obvious shape first.

### L4 — a terminated subagent leaves its scaffolding behind
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `test -z "$(git status --porcelain --untracked-files=all -- crates/ | grep -v '^ M')"`
- note: a checker killed by a session limit left `crates/renderer/tests/zz_gate3_probe.rs`
  behind; the stray integration test broke `cargo test` and `cargo fmt --check` on an
  otherwise green tree, and the failure looked like a code regression.
