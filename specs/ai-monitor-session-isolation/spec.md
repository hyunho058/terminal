# Spec: ai-monitor-session-isolation

## Meta
- **Created**: 2026-07-31
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-01
- **Source**: `HANDOFF.md` (root, untracked) — symptom, confirmed root cause, human decision

## Goal
A tab's ai-monitor panel must never render another session's data. Enforce strict
per-tab isolation: show this tab's *confirmed* session, or an explicit empty state
that says why there is nothing to show.

## Non-goals
- Fixing `CLAUDE_CODE_CHILD_SESSION` inheritance (why the reporter's tab wrote no
  transcript). Environmental, not a code defect — `HANDOFF.md:22-24` says so explicitly.
  It stays the cheapest reproduction of the real gap.
- The two rejected alternatives: labelling the guess on screen, and a multi-session
  list view. The human chose strict isolation over both.
- `crates/renderer/**`, `crates/common/**`, `crates/term-core/**` — no change needed.
- Deleting `resolve_session` / `most_recent_session`. The ranked fallback stays as a
  public function; only its *production callers* change.

## Confirmed Goal
Under strict resolution:

1. `crates/ai-monitor/src/lib.rs` gains `resolve_owned_session(dir, want)` — tiers 1-2
   only (id match, then first-event-dates-the-session), `None` when nothing confirms.
   Every `Resolved` it returns has `owned: true` by construction. `resolve_session`
   becomes that function `.or_else(...)` the tier-3 ranked fallback, so its existing
   behaviour and tests are unchanged.
2. Both production callers move to the strict function: `app/src/overlay.rs:150` (on
   open) and the watcher's re-resolution (`ai-monitor/src/lib.rs:459`). Missing the
   watcher would leave it free to adopt a foreign log before attach.
3. `dashboard::Env` gains `tab_has_claude`, so the collapsed-to-`NoLog` empty state
   distinguishes "no Claude session in this tab" from "this tab's session has not
   written a log yet" — the latter being exactly the reported case.
4. `specs/overlay-tab-binding/loop.md:24` is rewritten. This work *reverses* the rule
   documented there; the reversal is recorded, not left as a silent contradiction.

**Done when**: `cargo build/fmt/clippy/test --workspace` are green; the existing
`resolve_session` fallback tests pass *unchanged*; `specs/overlay-tab-binding/loop.md`
reads the new rule; and these new tests pass —
- a directory holding *only* a foreign log, with a hint that matches nothing, resolves to
  `None` rather than to the foreign log;
- the watcher does not adopt an unowned log before attach;
- tier 2 rejects a log whose first event is far *after* the process start (D3's new upper
  bound);
- a confirmed but empty log leaves the panel unattached and the watcher free (D4);
- an attachment holding no identity adopts the first one a snapshot names, and still
  refuses a snapshot whose id differs from one already held (D4b — no existing test
  reaches `attached && session_id.is_none()`);
- both empty-state wordings, keyed on `tab_has_claude`.

## Research

### The defect, in code
- `resolve_session` (`crates/ai-monitor/src/lib.rs:130-168`) has three tiers: id
  (`:132`), first-event-dates-it within `FIRST_EVENT_SLACK_MS = 120_000` (`:144-162`),
  then the unconditional ranked fallback `most_recent_session` tagged `owned: false`
  (`:163-167`). Tier 3 fires whenever tiers 1-2 miss — including the everyday case of a
  tab not running `claude` at all.
- The `owned` flag never reaches the screen. `dashboard::Env`
  (`crates/app/src/dashboard.rs:96-107`) carries `now_ms`, `attached`,
  `context_window` — no `owned`. A guessed attachment renders pixel-identical to a
  confirmed one.
- `owned` is consumed in exactly one place: `Attachment::admit`
  (`crates/app/src/overlay.rs:349-384`), where `owned && !self.owned` (`:362`) lets a
  confirmed session *displace* an unconfirmed one. Displacement only helps if a
  confirmed session ever shows up; nothing stops the unconfirmed one being displayed
  meanwhile (`overlay.rs:705` asserts exactly that: "attached, but only as a guess").

### The two call sites that must change
- `crates/app/src/overlay.rs:141-152` — builds `Wanted` from
  `tabsession::hint_for(shell_pid)`, empty hint → `Wanted::default()`, then
  `resolve_session(&dir, &want)`.
- `crates/ai-monitor/src/lib.rs:451-469` — the watcher re-resolves on every fs event
  until `attached` is set, via the same `resolve_session(&dir, &want)`.

### The hint the empty state needs
- `tabsession::hint_for(shell_pid) -> Option<SessionHint>`
  (`crates/app/src/tabsession.rs:54`) returns `None` when the tab is not running Claude
  Code at all; `SessionHint::is_empty()` (`:36`) is true when a claude process exists
  but named neither an id nor a start time. `tab_has_claude` is the `is_some()` of the
  former, which the overlay already computes and then discards via `unwrap_or_default()`
  (`overlay.rs:129`).

### The empty state machinery already exists
- `Detached::{StaleSince, Undated, NoLog}` (`crates/app/src/dashboard.rs:167-197`),
  worded in `summary()` (`:384-408`). `NoLog`'s current wording is `(no active
  session)` (`:392`) — misleading once strict resolution collapses the foreign-log path
  into it, because it implies no session exists anywhere.

### Blast radius, measured
- `Env` is built as an exhaustive struct literal in **9** places: **8 in `dashboard.rs`**
  — the `env(now)` test helper (`:708`) plus `:1440, 1466, 1487, 1520, 1536, 1568, 1587`
  — and **1 in `crates/app/src/overlay.rs:213`**, `refresh_model`, which is the *only*
  production site and the only one where the new field's value matters. `Env` derives
  `Default` but every literal is exhaustive, so all 9 must be touched.
- **Two inline assertions pin the exact string the wording change rewrites**:
  `dashboard.rs:1507` (`assert!(none.contains("(no active session)"))`) and `:1527`
  (`assert!(out.join("\n").contains("(no active session)"))`). Both compose with
  `attached: false` and the `NoLog` path, so both must be updated with the wording.
  `:1447` is prefix-only (`contains("(no active session")`) and `:1473` is a negative
  assertion — both survive untouched.
- **No `.snap` file contains "no active session"** — all 9 snapshots under
  `crates/app/src/snapshots/` compose with `attached: true`. The wording change is
  snapshot-neutral; `overlay-tab-binding`'s "9 `.snap` files unchanged" gate still holds.

### The tests that will move (the non-obvious cost)
Making the *watcher* strict is not free. Three watcher tests drive re-resolution with
`Wanted::default()` — an empty hint, which strict resolution answers `None` by design:
- `a_newer_log_appearing_takes_over_the_watch` (`lib.rs:903`)
- `a_missing_project_directory_still_arms_the_watch` (`lib.rs:958`)
- `attaching_mid_flight_stops_the_re_resolution` (`lib.rs:1096`)

Two of them additionally write **undated** fixture lines (no `timestamp` field), which
tier 2 can never confirm (`first_event` at `:172-185` returns `None`), so they need an
id-bearing hint (tier 1), not merely a `since_ms`. These are watcher tests, not the
`resolve_session` fallback tests — the "must pass unchanged" set (`:694-816`, `:819+`)
is untouched by the split.

### Conventions in force
- `cargo build` / `cargo test --workspace` / `cargo clippy --workspace --all-targets --
  -D warnings` / `cargo fmt --all --check`; no `unwrap()` outside tests; per-crate
  `thiserror` enums (`CLAUDE.md`).
- Dependency direction `common ← term-core ← renderer ← app`; `ai-monitor` takes a hint
  and gains no new dependency (`specs/overlay-tab-binding/loop.md:60`).
- Prior contracts this must not weaken: `specs/overlay-session-attach/loop.md` rounds
  1-2 — an empty snapshot never replaces real data, a different session never replaces
  an owned attachment.
- The sibling worktree `terminal-fix-ai-monitor-tmux` named under `HANDOFF.md:98` **no
  longer exists** (`git worktree list` shows only `main` and this branch); its specs
  (`tmux-pane-cwd`, `tabsession-tmux-reroot`) are already merged here. No overlap.

## Decisions

### D0: Ask the process which session it is running *(round 5, 2026-08-01 — supersedes the timing tiers as the primary rule)*
- **Status**: resolved
- **Rationale**: `claude` writes `~/.claude/sessions/<pid>.json`, containing `sessionId`,
  `cwd` and `startedAt` for its **own pid**. Given the tab's `claude` pid — which
  `tabsession` already finds — that is the answer, exactly, with no inference.

  This arrived after four rounds of trying to derive the same answer from clock arithmetic
  (D3 symmetric window → D3a asymmetric → D3b uniqueness → D3c rival assignment), each of
  which shipped a defect the next one had to fix. The fourth was the one that showed the
  approach was not merely buggy but **unable**: open tab A, open tab B, then type in A —
  both processes predate the write, so start times cannot say which one wrote, and D3c both
  blanked the writer and handed its dashboard to the idle tab. No refinement of a timing
  rule fixes that, because the information is not in the timings.

  Verified against every live `claude` on the machine before adopting it: 5/5 had a record,
  every `sessionId` named an existing log, every `cwd` matched, `startedAt` sat 1.3–2.0s
  after the true process start, and end-to-end resolution returned each process's own
  session — 5/5, by name.

  **Guarded, because the files outlive their processes.** The same sample held **5 files
  for 4 live processes**, so a recycled pid would find a stranger's record and — being an
  exact session id — it would be believed absolutely: this feature's original defect with
  more confidence behind it. A record is trusted only when written at or after the start of
  the process now holding that pid, within `REGISTRY_DRIFT_MS` (60s, two orders above the
  measured 1.3–2.0s).

  **Layered, not replacing.** This is an undocumented on-disk detail of another program, so
  every failure — absent file, bad JSON, missing field, failed guard — falls through to
  `--resume` argv and then the timing tiers. A future Claude Code that stops writing it
  degrades to round-4 behaviour rather than breaking. That is also why D3a/D3b/D3c stay:
  they are now the fallback, and they are still the only thing standing when the record is
  unavailable.

  **One consequence worth stating**: an authoritative id no longer falls through to timing.
  If we know the session and its log does not exist yet, the answer is "not written yet" —
  a fact — where previously control reached tier 2 and could adopt a sibling's log in
  exactly that window.


### D1: Split the resolver — `resolve_owned_session` is tiers 1-2, `resolve_session` is that plus the ranked fallback
- **Status**: resolved
- **Rationale**: Strictness becomes one *named* rule rather than a bool a caller can get
  wrong. `resolve_owned_session(dir, want) -> Option<Resolved>` returns only confirmed
  results — `owned: true` by construction, so the field cannot disagree with the
  function that produced it. `resolve_session` is then literally
  `resolve_owned_session(...).or_else(|| most_recent_session(dir).map(...owned: false))`,
  which is *definitionally* the current behaviour, so its tests pass unchanged and a
  failure there proves policy leaked into the wrong function. Rejected: a
  `strict: bool` parameter (the bug this feature fixes is exactly a caller getting an
  ownership judgement wrong — a bool invites it back) and deleting the fallback (it is
  the only thing keeping the `owned: false` half of `Resolved` and the re-attach rule
  under test).

### D2: Both production callers move to `resolve_owned_session`
- **Status**: resolved
- **Rationale**: `overlay.rs:150` (on open) and the watcher's re-resolution
  (`ai-monitor/src/lib.rs:459`) are the only two. Changing only the first leaves the
  watcher free to adopt a foreign log before attach and hand it up as an `Update` —
  which is the same defect one frame later. Rejected: filtering at
  `Attachment::admit` instead (the foreign log would still be opened, read, and folded
  into a `SessionTelemetry` before being discarded — strictness belongs where the file
  is chosen, not where it is displayed).

### D3: Tier 2's window gains an upper bound — `first_event <= since + 120_000`  *(its symmetric lower half was superseded by D3a)*
- **Status**: resolved
- **Rationale**: The current filter `first + SLACK >= since` (`lib.rs:149`) is
  one-sided: it admits *any* log whose first event is at or after `since − 120s`,
  including one that began five hours later, and `min_by_key` only prefers the nearest
  among them. Under the ranked fallback that produced one confident mistake among
  several candidates; under D1+D2+D4 it becomes the sole thing that can reach the panel
  and it sticks permanently. A session this tab started writes its first event seconds
  after the process, so a symmetric ±120s window is as generous in the direction that
  matters and closes the direction that does not. Verified against every existing
  tier-2 test — `two_tabs_on_one_project_get_their_own_sessions` and
  `a_session_started_by_this_tab_beats_a_more_active_foreign_one` both still pass, since
  their qualifying logs sit 30-60s from the process start. `claude --resume` on a
  days-old log is unaffected: tier 1 names it and never reaches tier 2. Rejected: a
  tighter `−120s/+30s` bound (a cold start's first write is not guaranteed inside 30s,
  and failing to recognise our own session is the more expensive error).

### D3a: The window is asymmetric — a log may not predate its own process *(round 2, 2026-08-01)*
- **Status**: resolved — supersedes D3's symmetry, keeps its upper bound
- **Rationale**: D3 made tier 2 two-sided at ±120s. That shipped a defect, observed in the
  first real two-tab test: app pid 4996, tab 1's `claude` (pid 5550) started 11:38:25 and
  wrote `c2abd08b` at 11:38:26; tab 2's `claude` (pid 5771) started 11:38:41 and wrote
  nothing. `|11:38:26 − 11:38:41| = 15s ≤ 120s`, so tier 2 confirmed **tab 1's live session
  as tab 2's own** — `owned: true`, non-empty, and D4 therefore attached it immediately and
  permanently. Two tabs, one dashboard: precisely the failure this feature exists to
  prevent, re-entering through the shape of its own fix.

  The error was treating the two sides of the window as one quantity. They are not.
  *Above* the process start, the number is a genuine tolerance — how long a session may
  take to write its first event — and 120s is a reasonable, measured value. *Below* it,
  there is nothing to tolerate: **a session cannot write an event before the process that
  creates it exists.** Any earlier first event belongs to a different session, however
  close in time. So the lower side becomes `FIRST_EVENT_LEAD_MS = 5_000` — a clock-skew
  allowance sized for measurement error alone, not a matching window.

  Measured before deciding, per Lesson L2: across every live `claude` on this machine that
  owned a log, `first_event − process_start` was **+1s, +2s, +1s, +2s, +1s (n=5)** and never
  negative; the single negative delta in the sample was the defect itself, at −14s.

  Also corrected: the test that guarded the old lower bound
  (`a_session_that_began_just_before_the_process_start_still_qualifies`) justified itself
  with "the process start is observed after the fact, so a first event legitimately
  predates it." That premise is false — `hint_for` reads the true start from
  `PROC_PIDTBSDINFO`, not the time of the walk — and it is what licensed the symmetry.
  It is replaced by `a_log_that_predates_the_process_is_not_that_process_s_session`.

  Rejected: widening the upper bound to ~30 minutes to also cover a tab left idle at the
  splash (it re-admits a foreign session started in that window whenever our own tab has
  no log — the original bug through a wider door); and dropping the upper bound entirely
  (same failure, unbounded). The human chose to keep 120s on 2026-08-01, accepting that an
  idle tab shows its empty state until the first write, at which point the watcher — still
  unattached, still re-resolving — fills it in.

### D3b: Uniqueness discriminates where proximity cannot *(round 2, 2026-08-01)*
- **Status**: resolved — relaxes D3a's upper bound for the sole-candidate case
- **Rationale**: D3a's upper bound is a proximity rule, and proximity is the wrong tool for
  a tab left idle at the splash: `claude` is running, the user has not typed, and the first
  event — whenever it finally lands — can be arbitrarily far from the process start. Since
  `since_ms` *is* the process start it never moves, so this is not a delay that resolves
  itself; the panel would stay empty for the life of that `claude`. (That fact was
  discovered only after the human had chosen the bound on a claim of mine that the panel
  "fills in on the first write" — it does not, and the decision was re-taken with the
  correction. See Lesson L6.)

  The rule: among logs whose first event is at or after the process start, **if there is
  exactly one, it is ours regardless of distance**. If there are two or more, uniqueness
  proves nothing and D3a's proximity rule takes over — nearest inside the slack, or nothing.

  Why this is safe against the defect that started the feature: uniqueness selects only
  from the *candidate set*, and that set is already filtered by D3a's hard invariant. A
  foreign session that was already running — `HANDOFF.md`'s `up 18h55m` case — has a first
  event older than the tab's process and never enters the set, sole or not. The residual is
  a *newly started* foreign session in a project where our own tab writes nothing; recorded
  in Known Gaps.

  Rejected: widening the upper bound to ~30 minutes (a constant with no principle behind it,
  and it admits a foreign session in that whole window rather than only when unique); and
  keeping 120s (correct only if one accepts a permanently empty panel for every idle tab —
  which is what the human was, in effect, unknowingly agreeing to).

### D3c: Tier 2 becomes an assignment, not a distance test *(round 3, 2026-08-01)*
- **Status**: resolved — the fix D3a and D3b could not be
- **Rationale**: D3a fixed the reported case (tab 2 fresh, adopting tab 1's log). Testing
  found the **mirror**: tab A idle having written nothing, tab B starting 16s later and
  writing — A adopts B's log. Measured under both rules: broken at ≤120s under D3a, broken
  at *any* distance under D3b. Same defect, tabs swapped.

  Neither bound could fix it, because both ask the wrong question. "How far is this log's
  first event from my process start?" has no good answer when two processes started
  seconds apart — both are plausible owners. The right question is **which process wrote
  it**: a log is written by exactly one process, that process existed when it was written,
  and among the processes that qualify the *latest to start* is the writer.

  So `resolve_owned_session` gains a `rivals: &[u64]` parameter — the start times of the
  other live `claude` processes in the same project — and refuses a candidate when any
  rival started after us and at or before that log's first event. `SessionHint` carries
  them from `ProcTable::rival_starts`, which costs one `cwd_of_pid` + `start_ms` per
  `claude` on the machine (single digits) at panel-open, never per frame.

  Deliberately **not** limited to this app's tabs: the question is "did another process
  write this", and Warp's `claude` answers it exactly as well as a sibling tab's. The whole
  process table is already read for `find_claude`, so this is a filter over data in hand.

  Two properties worth stating. It **degrades in one direction only**, and the direction
  matters: an unreadable cwd or process table contributes no rival, because inventing one
  would refuse a log that is genuinely ours. That is safe for *construction* — we never
  wrongly blank a panel — and **dangerous for assignment**: with no rivals known, tier 2 is
  back to distance alone and an idle tab can adopt a sibling's log, exactly as it did before
  D3c. Measured, not assumed: `resolve_owned_session(dir, want, &[])` with a sibling's log
  present returns that log. Recorded in Known Gaps rather than described as merely "safe". And it **makes uniqueness
  safe** — D3b's sole-candidate rule was dangerous only because a sibling's log could be
  that sole candidate; the rival test removes exactly those, so an idle tab can still adopt
  its own late-written session.

  Delivered as a parameter rather than a field on `Wanted` so the five protected
  `resolve_session` fallback tests keep byte-identical bodies — verified.

### D4: A confirmed resolution attaches as soon as its log has content — `owned && !snap.is_empty()` short-circuits `proves_ownership`
- **Status**: resolved
- **Rationale**: `proves_ownership` (`overlay.rs:400`, "has this log written since the
  window opened?") exists to keep a *foreign* session off the panel. Under D1+D2 a
  foreign session can no longer be resolved at all, so applying the freshness gate to a
  confirmed session only hides the user's own data: a tab running `claude --resume <id>`
  that went idle before the window opened would render `(no active session — newest log
  idle 12m)` about *its own* session. `admit` therefore attaches when `owned` is true,
  and falls back to `proves_ownership` otherwise.

  **The emptiness condition is not incidental — it is what preserves self-correction.**
  `proves_ownership` was doing a second job nobody named: it is the gate that *fails
  open*. A tier-2 near-miss is recoverable today because the wrong log fails freshness,
  `attached_flag` stays false, and the watcher keeps re-resolving (`lib.rs:451`) until
  the tab's real session appears. A bare `owned` short-circuit destroys that: `note()`
  runs at `overlay.rs:159`, *before* `LogWatcher::spawn` at `:165`, so the watcher would
  be born frozen and any tier-2 mistake would be permanent from the first callback.
  Worse, attaching on an owned-but-**empty** backfill latches
  `self.session_id = snap.session_id` = `None` (`overlay.rs:353`), after which the
  identity guard at `:380-383` hits its `_ => true` arm forever — satisfying "a different
  session never replaces an owned attachment" *vacuously*, which is a weakening of the
  rounds 1-2 contract dressed as compliance. Requiring `*snap != TelemetrySnapshot::default()`
  keeps an empty confirmed log unattached and the watcher free. The residual — a
  *non-empty* log wrongly confirmed by tier 2 — is bounded by D3/D3a's window and
  recorded in Known Gaps.

  **Non-emptiness alone does not restore the identity guard**, so D4b does the rest:
  `TelemetrySnapshot::session_id` is filled only from a `sessionId`/`session_id` field on
  a folded line (`telemetry.rs:263-266`), while non-emptiness can come from any field at
  all (`total_output`, a tool count). A confirmed log whose backfilled content names no
  session would still latch `None`.

  Rejected: leaving `proves_ownership` as the sole gate (smaller diff, but leaves the
  panel lying about a session it has positively identified); and the bare `owned`
  short-circuit, for the two reasons above.

### D4b: `admit` adopts an identity it did not have, the first time a snapshot names one
- **Status**: resolved
- **Rationale**: Closes the residual D4 narrows but cannot reach. When the attachment is
  held with `session_id: None` and a snapshot arrives carrying `Some(id)`, `admit` records
  it. From that point the identity check at `overlay.rs:380-383` compares two `Some`s and
  means something; before it, the `_ => true` arm is the documented existing behaviour
  rather than a new hole. It adopts an identity and never *changes* one: a snapshot whose
  id differs from an already-recorded `Some` is still refused.

  **The safety argument holds on one attach path, not both, and the difference is
  stated rather than glossed.**
  - *Attached at open* — `note()` runs at `overlay.rs:159`, before `LogWatcher::spawn` at
    `:165`, so the watcher is born frozen. Any later snapshot provably came from the
    attached log, and adoption is exact.
  - *Attached from a later `Update`* — the path D4's emptiness condition deliberately
    creates: an owned-but-empty backfill leaves the watcher **free**. Here `admit` runs at
    `:193` and `attached_flag` is stored at `:196-197`, while the watch thread reads that
    flag at the top of its own callback (`lib.rs:451`). A callback already in flight can
    therefore switch target and emit a differently-named snapshot inside the
    `session_id: None` window, which `_ => true` admits and D4b would adopt. This is the
    very check-then-act window `overlay.rs:374-379` documents as unclosable by memory
    ordering. It is **accepted, not closed**: under D2 both logs in that race are
    *confirmed* this tab's, so the worst outcome is adopting the id of one own-session
    over another, never a foreign one — a strictly smaller error than the one this
    feature exists to fix, and it cannot recur once an id is held.

  Rejected: gating attachment on `snap.session_id.is_some()` instead (a confirmed log that
  never writes a `sessionId` field would then never attach at all — failing closed on the
  user's own session, which is the error this whole feature is about).

  **No existing test reaches `attached && session_id.is_none()`**, so D4b needs one of its
  own — see Done-when.

### D4a: `proves_ownership` and the displacement branch become production-unreachable, and say so
- **Status**: resolved
- **Rationale**: The same bookkeeping D8 does for `Detached` and D9 for
  `resolve_session`, applied to the attach rule itself — the reviewer's catch. After D2
  every `Resolved` reaching `admit` has `owned: true`, so (a) the `proves_ownership`
  branch is entered only with `owned: false`, which production can no longer produce, and
  (b) the displacement branch `owned && !self.owned` (`overlay.rs:362`) can never fire,
  since there is no unconfirmed attachment left to displace. Both are retained — they are
  the contract for a caller using `resolve_session`, and they are what
  `a_confirmed_session_displaces_an_unconfirmed_one` (`overlay.rs:697`) and
  `a_confirmed_attachment_yields_to_nothing` (`:716`) test. Each gets a doc line naming
  its reachability honestly. Deleting them would remove the `owned: false` half of the
  rule from the suite in the same move that makes the code depend on nothing else.

### D5: The watcher is still spawned when the hint is empty
- **Status**: resolved
- **Rationale**: With no `claude` process, `Wanted` is empty and strict resolution will
  answer `None` on every re-resolution — so the watch can never attach and is, strictly,
  waste. It is spawned anyway because *not* spawning it puts the strictness rule in a
  second place (`open()`) that must agree with the resolver, and the last time this
  rule was implemented twice the two copies disagreed and pinned the watcher to a log
  the display would never show (`lib.rs:433-439`). One fs watch on one directory is the
  cheaper of the two costs. Rejected: re-deriving the hint inside the watcher on each
  re-resolve (most correct — it would notice `claude` starting mid-panel — but it pushes
  process-tree walking into `ai-monitor`, which `specs/overlay-tab-binding/loop.md:60`
  forbids: that crate takes a hint and gains no new dependency).

### D6: `dashboard::Env` gains `tab_has_claude: bool`
- **Status**: resolved
- **Rationale**: Under strict resolution the foreign-log path collapses into
  `Detached::NoLog`, whose wording `(no active session)` then implies no session exists
  anywhere — misleading in exactly the case that produced the bug report. The overlay
  already computes the distinction (`shell_pid.and_then(tabsession::hint_for)`,
  `overlay.rs:129`) and throws it away with `unwrap_or_default()`; `Env` is the existing
  channel for "everything the composer needs that the snapshot does not carry"
  (`dashboard.rs:94-95`), so the field goes there and `compose` stays pure. Costs an
  exhaustive-literal update at 9 sites — 8 in `dashboard.rs`, plus the one production
  site `overlay.rs:213` (see Blast radius). Rejected: a new `Detached`
  variant carrying the flag (the distinction is about the *tab*, not about a log that
  was declined — `Detached::of` reads a snapshot and has nothing to read it from).

### D7: The two empty-state wordings are keyed on `tab_has_claude`, not on hint emptiness
- **Status**: resolved
- **Rationale**: Exact strings — `tab_has_claude == false` →
  `(no Claude session in this tab)`; `tab_has_claude == true` →
  `(this tab's session has not written a log yet)`. The awkward case is a live `claude`
  whose hint is unusable (started without `--resume`, start time unreadable): keying on
  `hint.is_empty()` would tell the user there is no session in a tab where they can watch
  `claude` running, which reads as a bug. Keying on process presence is slightly
  optimistic when the real cause was a permission refusal, and that is the cheaper error.
  Rejected: a third wording such as `(could not identify this tab's session)` — a third
  string and a third branch for a case the user cannot act on differently.

### D8: `Detached::StaleSince` and `Detached::Undated` are retained and documented as fallback-only
- **Status**: resolved
- **Rationale**: D4 makes "not attached" mean "nothing resolved", so both variants become
  unreachable in production — reachable only through `resolve_session`, which after D2
  has no production caller. They stay, still constructed by `Detached::of` and still
  unit-tested, with a doc line stating precisely that. Nothing is deleted on the strength
  of one release's call graph. Rejected: collapsing `Detached` into the two-way state
  (smallest surface, but discards wording and tests for a path the fallback resolver can
  still produce).

### D9: `resolve_session` stays public with zero production callers, and says so
- **Status**: resolved
- **Rationale**: Direct consequence of D1+D2, called out rather than left for a reader to
  discover. Its doc comment gains a line naming `resolve_owned_session` as what
  production uses and itself as the ranked-fallback contract kept under test. Removing it
  would take `most_recent_session`'s ranking, the `owned: false` half of `Resolved`, and
  the `Attachment` displacement branch out of the test suite in one move.

### D12: `tab_has_claude` is stored on the overlay at `open()` and read by `refresh_model`
- **Status**: resolved
- **Rationale**: D6 adds the field and D7 words it; this says where the value lives, which
  is otherwise not derivable. `open()` already computes
  `shell_pid.and_then(tabsession::hint_for)` (`overlay.rs:129`) and discards the
  `Option`'s shape via `unwrap_or_default()`. It instead records
  `self.tab_has_claude = <that Option>.is_some()` on `AiMonitorOverlay`, alongside
  `context_window` — same lifecycle: set on every `open()`, read by `refresh_model`
  (`overlay.rs:213`), never recomputed per frame. Three specifics that would otherwise be
  guessed at L3:
  - **Reset**: `open()` assigns unconditionally, so a re-point to a tab with no `claude`
    clears it. It is not reset by `close()` — the panel is not composed while hidden.
  - **The `project_dir → None` branch** (`overlay.rs:137`, no cwd or no
    `~/.claude/projects`): the assignment happens *above* that branch, so the dashboard
    still renders the correct one of the two wordings rather than defaulting to "no
    Claude session in this tab" whenever the projects root is missing.
  - **Default**: `false` in `AiMonitorOverlay::new` and in the derived `Env::default()`.
    No existing literal *omits* the field — all 9 are exhaustive and must be edited — so
    the default only governs a fresh overlay before its first `open()`, and the `Env`
    built by a future caller that uses `..Default::default()`.

### D13: Code comments that assert the reversed rules are corrected in place
- **Status**: resolved
- **Rationale**: D10 scopes the *spec* edits; this scopes the in-code prose, which three
  sites state as fact and this work falsifies. `crates/app/src/tabsession.rs:109-117` and
  its test comment at `:551-552` justify the cwd gate by "the one-sided `since_ms`
  filter", which D3/D3a make asymmetric — the gate is still right, its stated reason is not.
  `crates/app/src/overlay.rs:698-701` describes an attach path (a panel attaching to
  another application's live session) that D2 makes unreachable. Corrections only: no
  behaviour change, no test renamed. Left uncorrected they are the next reader's evidence
  for a rule the code no longer has — the exact failure this spec exists to stop
  repeating.

### D10: Only `specs/overlay-tab-binding/loop.md` is edited
- **Status**: resolved
- **Rationale**: Line 24 currently reads that a session belonging to no tab-process "can
  still be shown when nothing better exists" — the rule this work reverses. It is
  rewritten in place, together with the Gate-2 rows that depend on it (`owned beats
  unowned`, `re-attach`), so the contract does not certify a behaviour the code no longer
  has. `specs/ai-monitor/spec.md`, `specs/overlay-session-attach/loop.md` and
  `docs/domain.md` are left alone — none of them state the reversed rule.

### D11: The three watcher tests are re-fixtured with id-bearing hints
- **Status**: resolved
- **Rationale**: `a_newer_log_appearing_takes_over_the_watch` (`lib.rs:903`),
  `a_missing_project_directory_still_arms_the_watch` (`:958`) and
  `attaching_mid_flight_stops_the_re_resolution` (`:1096`) drive re-resolution with
  `Wanted::default()`, which D2 makes answer `None` by design. Two of them also write
  **undated** fixture lines, which tier 2 can never confirm (`first_event` returns `None`
  on a line with no `timestamp`), so a `since_ms` hint is not enough — they need a
  `session_id` hint naming the file, i.e. tier 1. This is a fixture change, not a
  behaviour change: each test's assertion (adoption happens / the watch arms on an
  ancestor / attaching freezes the target) is preserved verbatim.

## Constraints
- **Dependency direction holds.** `ai-monitor` takes a `Wanted` and gains no new
  dependency; process-tree walking stays in `app` (`specs/overlay-tab-binding/loop.md:60`,
  `.claude/rules/dependency-direction.md`).
- **Read-only observation.** No write to any session log, to anything under `~/.claude`,
  or to any process (`specs/overlay-tab-binding/loop.md:72`).
- **Rounds 1-2 preservation is not weakened, and not vacuously satisfied.** An empty
  snapshot never erases real data; a different session never replaces an owned
  attachment (`specs/overlay-session-attach/loop.md`). D4 *adds* an attach path and
  removes none of the refusals in `admit`. Its emptiness condition, plus D4b's identity
  adoption, are what keep the second guarantee from being satisfied *vacuously*: an
  attachment that held `session_id: None` would pass the identity check against
  everything.
- **The watcher is never born frozen.** `note()` runs before `LogWatcher::spawn` in
  `open()` (`overlay.rs:159` vs `:165`), so any attach rule that fires on the backfill
  snapshot decides the watcher's freedom for its whole life. Attaching at open is allowed
  only on a confirmed *and non-empty* log (D4); otherwise re-resolution stays live.
- **The ownership rule has exactly one author.** `overlay.rs` decides; the watcher reads
  the shared flag; `dashboard.rs` is told the answer and never re-derives it
  (`lib.rs:433-439`, `dashboard.rs:100-104`).
- **`resolve_session`'s existing tests pass byte-unchanged** — the falsifiable check that
  D1 was a split and not a rewrite.
- **The 9 `.snap` files stay unchanged** (`crates/app/src/snapshots/`) — verified
  achievable: none contains the string `no active session`.
- **No `unwrap()` outside tests; no new crate dependency; gates are** `cargo fmt --all
  --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test
  --workspace` (`CLAUDE.md`).

## Known Gaps
- **`claude` started mid-panel is not noticed.** D5 spawns the watcher with the hint
  captured at open; a tab that starts `claude` while the panel is up stays empty until
  the panel is toggled or the tab re-activated. Fixing it needs the hint re-derived on
  re-resolve, which D5 rejected on dependency-direction grounds.
- **Rivals are frozen at panel-open.** `Watch.rivals` is captured once, alongside `want`,
  and every re-resolution uses that snapshot — so a `claude` started *after* the panel
  opened is invisible to the assignment, exactly as `since_ms` is. Same root cause as the
  first Known Gap (D5 declines to re-derive the hint on the watch thread), same recovery:
  toggling the panel or switching tabs re-derives both.
- **With no rivals known, assignment falls back to distance alone.** An unreadable process
  table or cwd yields an empty rival list, and tier 2 then behaves as it did before D3c —
  an idle tab can adopt a sibling's log. Verified by construction:
  `resolve_owned_session(dir, want, &[])` with a sibling's log present returns that log.
  This is the honest reading of "degrades safely": safe against wrongly blanking a panel,
  unsafe against wrongly filling one.
- **A tab whose own session never writes can adopt a log written by a process we could not
  see.** What survives D3c. The rival test only knows the `claude` processes live *at the
  moment the panel opens*: a session written by a process that has since exited leaves a log
  with no rival to account for it, and an idle tab may take it. Narrow — it needs our tab to
  have written nothing, the rival to be gone, and no other candidate — and strictly smaller
  than what D3a/D3b left open, both of which mis-assigned between two *live* tabs.

  *(Two superseded entries, kept because decisions were taken on them. (1) An earlier draft
  said a tab idle at the splash over 120s recovers when the user types. It did not —
  `since_ms` is the process start and never moves. (2) A later one said uniqueness costs
  only a *newly started foreign* session; testing found it also handed one live tab's log to
  another, which is the reported defect mirrored. D3c fixes both; the drafts are recorded
  because each was, briefly, the stated basis of a choice.)*
- **A clock step larger than `FIRST_EVENT_LEAD_MS` makes a tab's own session
  unconfirmable.** D3a sizes the lower bound (5s) for measurement error only. A sleep/wake
  cycle or an NTP correction that moves the wall clock backwards by more than that, between
  the process starting and its first event being stamped, makes the tab's own log look like
  it predates its own process — so tier 2 refuses it and the panel shows the empty state.
  New with D3a: the old symmetric window absorbed steps up to 120s. Judged the right trade
  because the alternative is admitting genuinely foreign logs, which is the defect this
  feature exists to remove; a clock step of that size is rare, and `--resume` (tier 1)
  bypasses the bound entirely.
- **A confirmed, non-empty log with no parseable timestamp shows an empty-looking
  dashboard, not a reason.** Tier 1 can confirm an undated log by name; if it has content
  D4 attaches to it, so `compose` renders its rows with no session-row explanation rather
  than `Detached::Undated`. Correct — it *is* our session — but a state with no
  explanatory row.
- **A non-empty log wrongly confirmed by tier 2 is permanent.** D4 freezes the watcher on
  it and `Attachment` never releases. D3a's asymmetric window bounds this to a session that
  began within 120s of the tab's own `claude` process in the same project; below that
  bound nothing in the process tree can separate them. This is the residual of the
  steelman against D4, deliberately accepted rather than dissolved.
- **A session resumed without `--resume <id>` is confirmable on neither tier.**
  `claude --continue` / `-c`, a fork, or a compacted transcript writes a log whose *first*
  event is inherited from the original conversation — days old, so tier 2 misses by
  orders of magnitude — while putting no id in argv for tier 1 to match
  (`ProcTable::resume_id` parses `--resume`, `-r`, `--resume=` only). Strict resolution
  therefore answers `None` for as long as the tab lives, and the panel reads
  `(this tab's session has not written a log yet)` about a session the user is actively
  typing into — a **false statement**, not merely an unhelpful one.

  This is a genuine regression, stated plainly: before this work the tier-3 ranked
  fallback resolved that case *correctly*, because `--continue` resumes the most recently
  active session and `most_recent_session` ranks exactly that. Accuracy by luck, but
  accuracy. Accepted by the human on 2026-08-01 as the cost of never showing a stranger's
  dashboard, which is the more damaging of the two errors.

  Two fixes were tested and ruled out before accepting. **File birth time** is a *worse*
  discriminator than the first event, not a better one — measured against live processes,
  own logs were born +39129s and +829s after their `claude` started while their first
  events sat at +1s and +2s. **Parsing `--continue`** cannot help, because the flag
  carries no id. The one signal that would work is `CLAUDE_CODE_SESSION_ID`, which is
  present in the process environment and names the session exactly in every case — but it
  is *inherited* (observed alongside `CLAUDE_CODE_CHILD_SESSION=1`), so reading it
  unguarded would name a parent's session and reproduce this feature's entire bug class.
  A guarded tier — trust it only when the log it names exists in this tab's own project
  directory — is the shape of the follow-up, and needs its own L2 round.

## Requirements

### R0: A tab's panel shows its own confirmed session or an explained empty state

#### R0.1: A foreign session never reaches the panel
- **Given**: `~/.claude/projects/<cwd>/` holds only `foreign.jsonl`, written by another
  application, and this tab's process tree yields no hint that matches it
- **When**: the user opens the ai-monitor panel in that tab
- **Then**: the panel renders the empty dashboard frame with an explanatory session row —
  never `foreign.jsonl`'s session id, model, spend, context or uptime

#### R0.2: The empty state states which of the two causes applies
- **Given**: no session can be confirmed for this tab
- **When**: the dashboard is composed
- **Then**: the session row reads `(no Claude session in this tab)` when the tab runs no
  `claude` process, and `(this tab's session has not written a log yet)` when it does

---

### R1: `resolve_owned_session` — confirmation-only resolution *(D1)*

#### R1.1: A session named by `--resume` is confirmed
- **Given**: `want.session_id = Some("54e576c2-…")` and `54e576c2-….jsonl` exists in `dir`,
  alongside a far more recently active `other.jsonl`
- **When**: `resolve_owned_session(dir, want)` is called
- **Then**: it returns `Some(Resolved { path: 54e576c2-….jsonl, owned: true, .. })`

#### R1.2: A session dated by the caller's process start is confirmed
- **Given**: `want.since_ms = 1_785_271_140_000` (20:39:00Z), `mine.jsonl` first event
  20:40:00Z, `foreign.jsonl` first event five days earlier but still being written
- **When**: `resolve_owned_session(dir, want)` is called
- **Then**: it returns `mine.jsonl` with `owned: true` — the more active `foreign.jsonl`
  is not considered

#### R1.3: Nothing confirmable resolves to `None`, not to a ranked guess
- **Given**: `dir` contains only `foreign.jsonl`, and `want` matches it on neither tier
- **When**: `resolve_owned_session(dir, want)` is called
- **Then**: it returns `None` — this is the defect under repair, and the assertion is
  against `None`, not against a different path

#### R1.4: An empty hint can confirm nothing
- **Given**: `want == Wanted::default()` and `dir` contains any number of logs
- **When**: `resolve_owned_session(dir, want)` is called
- **Then**: it returns `None` regardless of directory contents

#### R1.5: Every result it returns is owned by construction
- **Given**: any `dir` and any `want` for which the function returns `Some`
- **When**: the returned `Resolved` is inspected
- **Then**: `owned == true` — the function has no code path that constructs
  `owned: false`

---

### R2: Tier 2's window is bounded above and below *(D3, D3a)*

#### R2.1: A log that began long after the process is rejected *once something else can account for it* — revised, rounds 2-3
- **Given**: `want.since_ms` = 10:04:00Z and a candidate whose first event is 15:30:00Z
  (5h26m later)
- **When**: `resolve_owned_session` evaluates tier 2 **with a rival process that started in
  between, or with a second, promptly-written candidate**
- **Then**: the distant candidate is rejected. *(Revised: the original R2.1 required
  rejection unconditionally. A sole, unrivalled candidate is now adopted at any distance —
  see R2.5 and D3b — because the alternative left a tab idle at the splash with an empty
  panel for the life of its `claude`.)*

#### R2.5: A log belongs to the latest process that could have written it *(D3c)*
- **Given**: our process started at `T`, a log's first event is at `T + 16s`, and a rival
  `claude` in the same project started at `T + 15s`
- **When**: `resolve_owned_session(dir, want, rivals)` evaluates tier 2
- **Then**: the log is refused — the rival started after us and before the write, so it is
  the writer

#### R2.6: A rival that did not exist at the write never outranks the writer
- **Given**: our process started at `T` and wrote its first event at `T + 1s`; a rival
  started at `T + 3s`
- **When**: tier 2 evaluates our claim
- **Then**: the log is still ours — a process that began after the log was written cannot
  be its author, whatever the skew allowance says

#### R2.7: Rival discovery degrades to claiming nothing
- **Given**: the process table or a rival's cwd cannot be read
- **When**: `rival_starts` runs
- **Then**: it returns no rival for that process — inventing one would refuse a log that is
  genuinely ours. The cost is stated in Known Gaps: with no rivals known, assignment falls
  back to distance alone and an idle tab can adopt a sibling's log

#### R2.2: The upper bound is 120s *(the lower is not — see D3a; they measure different things)*
- **Given**: `want.since_ms = S`, one candidate with first event `S + 119_000`, another
  with `S + 121_000`
- **When**: tier 2 evaluates them
- **Then**: the first qualifies, the second does not

#### R2.3: A log that predates the process is refused *(revised, round 2 — see D3a)*
- **Given**: `want.since_ms = S` and a candidate whose first event is `S - 60_000`
- **When**: tier 2 evaluates it
- **Then**: it is **refused** — a session cannot write an event before its own process
  exists, so this belongs to some other session. *(This reverses the original R2.3, which
  required such a candidate to qualify; that requirement rested on the false premise that
  `since_ms` is an observation time rather than the true process start, and it is what
  admitted the two-tabs-one-dashboard defect.)*

#### R2.3b: Clock skew alone is tolerated
- **Given**: `want.since_ms = S` and a candidate whose first event is `S - 2_000`, inside
  `FIRST_EVENT_LEAD_MS`
- **When**: tier 2 evaluates it
- **Then**: it qualifies — only measurement error should produce a negative delta, and the
  rule must not punish it

#### R2.3c: The reported two-tab timeline
- **Given**: one log whose first event is `T + 1s`, a tab-1 hint of `since_ms = T`, and a
  tab-2 hint of `since_ms = T + 16s` whose own session has written nothing
- **When**: both resolve
- **Then**: tab 1 gets the log; tab 2 gets `None` — not tab 1's session

#### R2.4: Nearest-match among qualifiers is unchanged
- **Given**: two logs whose first events are both within ±120s of `since`
- **When**: tier 2 evaluates them
- **Then**: the one nearest to `since` by absolute difference is returned — the existing
  `min_by_key` rule, untouched

---

### R3: `resolve_session` keeps the ranked fallback intact *(D1, D9)*

#### R3.1: An unconfirmable resolution still degrades to ranking
- **Given**: `dir` contains `a.jsonl` and `want == Wanted::default()`
- **When**: `resolve_session(dir, want)` is called
- **Then**: it returns `a.jsonl` with `owned: false` — unchanged behaviour

#### R3.2: The pre-existing fallback tests pass byte-unchanged
- **Given**: `a_named_session_wins_outright`, `a_session_started_by_this_tab_beats_a_more_active_foreign_one`,
  `two_tabs_on_one_project_get_their_own_sessions`,
  `an_empty_hint_degrades_to_ranking_and_claims_nothing`,
  `resolution_ranks_by_last_event_not_by_mtime` (`lib.rs:694-900`)
- **When**: `cargo test -p ai-monitor` runs after the split
- **Then**: all pass with **no edit to their bodies** — an edit needed here means policy
  leaked into the wrong function

#### R3.3: Its status is stated where a caller would look
- **Given**: a reader opening `resolve_session`'s doc comment
- **When**: they read it
- **Then**: it names `resolve_owned_session` as what production calls, and itself as the
  ranked-fallback contract retained under test with no production caller

---

### R4: Both production call sites resolve strictly *(D2)* — `app` ↔ `ai-monitor` boundary

#### R4.1: The overlay resolves strictly on open *(app side)*
- **Given**: a tab whose hint confirms nothing, opened on a `cwd` whose project directory
  holds a live foreign log
- **When**: `AiMonitorOverlay::open` resolves
- **Then**: `log` is `None` and `owned` is `false`; no `backfill_telemetry` is performed
  on the foreign file

#### R4.2: The watcher re-resolves strictly *(ai-monitor side)*
- **Given**: an unattached `LogWatcher` whose `want` confirms nothing, and a foreign log
  in the watched directory that receives an append
- **When**: the notify callback runs its re-resolution
- **Then**: the target is not switched to the foreign log

#### R4.3: The watcher emits no `Update` from an unowned log
- **Given**: the conditions of R4.2
- **When**: the callback completes
- **Then**: no `Update` carrying that log's telemetry is sent on the channel — the
  overlay is never offered foreign data to reject

---

### R5: Attachment is decided by confirmation, not freshness *(D4, D4a, D4b)*

#### R5.1: A confirmed, non-empty session attaches however stale it is
- **Given**: `started_ms` = window open time, a confirmed log whose `last_event_ms` is
  12 minutes *earlier*, and a non-default snapshot folded from it
- **When**: `Attachment::admit(&snap, started_ms, owned: true)` runs
- **Then**: it returns `true` and `attached` becomes `true` — `proves_ownership` is not
  consulted, and the dashboard renders the session's real accumulated data

#### R5.2: A confirmed but empty session does not attach, and leaves the watcher free
- **Given**: a confirmed log with no parseable content, so the backfill snapshot equals
  `TelemetrySnapshot::default()`
- **When**: `admit(&snap, started_ms, owned: true)` runs during `open()`
- **Then**: `attached` stays `false`, `attached_flag` is not set, and `LogWatcher::spawn`
  is therefore handed a flag that permits re-resolution

#### R5.3: An attachment holding no identity adopts the first one offered
- **Given**: an attachment with `attached: true` and `session_id: None`
- **When**: a snapshot carrying `session_id: Some("abc")` is admitted
- **Then**: the attachment records `Some("abc")`, so subsequent identity checks compare
  two `Some` values rather than falling through `_ => true`

#### R5.4: An identity already held is never replaced
- **Given**: an attachment holding `session_id: Some("mine")`
- **When**: a snapshot carrying `session_id: Some("other")` arrives
- **Then**: `admit` returns `false` — adoption fills a gap, it never overwrites

#### R5.5: The existing refusals are unchanged
- **Given**: an attached panel showing real data
- **When**: an empty snapshot, or one from a different named session, arrives
- **Then**: both are still refused — `a_different_session_never_replaces_an_attached_panel`,
  `attachment_is_monotonic_within_an_open` and `a_confirmed_attachment_yields_to_nothing`
  pass unchanged

#### R5.6: The now-unreachable branches say so
- **Given**: a reader at `proves_ownership` (`overlay.rs:400`) or the displacement branch
  (`:362`)
- **When**: they read the doc comment
- **Then**: it states that production can no longer reach it after R4, and that it is the
  contract for a caller using `resolve_session`

---

### R6: The watch is armed regardless of what the hint knows *(D5)*

#### R6.1: An empty hint still spawns the watcher
- **Given**: a tab with no `claude` process, opened on a cwd with an existing project
  directory
- **When**: `open()` runs
- **Then**: `self.watcher` is `Some` — the strictness rule is not restated in `open()`

#### R6.2: A missing projects root still declines to arm
- **Given**: `~/.claude/projects` itself does not exist
- **When**: `open()` runs
- **Then**: `LogWatcher::spawn` returns `None` and no recursive `$HOME` watch is armed —
  `no_projects_root_means_no_watch_rather_than_a_home_wide_one` passes unchanged

---

### R7: The empty state names its cause *(D6, D7, D12)* — overlay ↔ dashboard boundary

#### R7.1: The overlay records whether the tab runs `claude` *(producer)*
- **Given**: `open()` is called with a `shell_pid` whose subtree contains a `claude`
  process
- **When**: the hint is computed
- **Then**: `self.tab_has_claude` is set to `true` from the `Option`'s `is_some()`, before
  the `Option` is flattened with `unwrap_or_default()`

#### R7.2: The record is taken above the `project_dir` branch
- **Given**: a tab running `claude` on a cwd for which `project_dir` returns `None`
- **When**: `open()` runs and `refresh_model` composes
- **Then**: the panel still reads `(this tab's session has not written a log yet)` —
  a missing projects root does not silently downgrade the wording

#### R7.3: Every `open()` re-decides it
- **Given**: the panel was opened in a tab running `claude`, then re-pointed to a tab with
  none
- **When**: `open()` runs for the second tab
- **Then**: `self.tab_has_claude` is `false` — the assignment is unconditional, not
  set-once

#### R7.4: `refresh_model` carries it to the composer *(producer → consumer)*
- **Given**: `self.tab_has_claude` holds either value
- **When**: `refresh_model` builds `dashboard::Env` (`overlay.rs:213`)
- **Then**: the field is populated from it, not defaulted

#### R7.5: No `claude` in the tab reads as such *(consumer)*
- **Given**: `Env { attached: false, tab_has_claude: false, .. }` and `snap: None`
- **When**: `compose` runs
- **Then**: the session row reads `(no Claude session in this tab)`

#### R7.6: `claude` present but unconfirmed reads as such *(consumer)*
- **Given**: `Env { attached: false, tab_has_claude: true, .. }` and `snap: None`
- **When**: `compose` runs
- **Then**: the session row reads `(this tab's session has not written a log yet)`

#### R7.7: An attached panel shows no empty state at all
- **Given**: `Env { attached: true, .. }` with either value of `tab_has_claude`
- **When**: `compose` runs
- **Then**: the session row shows id · model · context% exactly as today, and the 9 files
  under `crates/app/src/snapshots/` are byte-identical

---

### R8: The reversed decisions are recorded, not silently contradicted *(D8, D9, D10, D13)*

#### R8.1: The spec line that stated the old rule is rewritten
- **Given**: `specs/overlay-tab-binding/loop.md:24`, which reads that a session belonging
  to no tab-process "can still be shown when nothing better exists"
- **When**: this work lands
- **Then**: the line states the strict rule instead, and names this spec as what reversed
  it and why

#### R8.2: The gate rows that depended on it are rewritten with it
- **Given**: the Gate-2 rows `owned beats unowned` and `re-attach`
  (`specs/overlay-tab-binding/loop.md:52-53`), which certify behaviour R4 removes from
  production
- **When**: this work lands
- **Then**: they are restated against the retained-under-test fallback, so the contract
  does not certify behaviour the code no longer has

#### R8.3: Code comments asserting the reversed rules are corrected
- **Given**: `tabsession.rs:109-117` and `:551-552` (which justify the cwd gate by "the
  one-sided `since_ms` filter") and `overlay.rs:698-701` (which describes an attach path
  R4 makes unreachable)
- **When**: this work lands
- **Then**: each states the current rule; no test is renamed and no behaviour changes

#### R8.4: `Detached::StaleSince` / `Undated` are documented as fallback-only
- **Given**: a reader at the `Detached` enum
- **When**: they ask why two carefully-worded variants never appear on screen
- **Then**: the doc says they are reachable only via the `resolve_session` path, which has
  no production caller

---

### R9: The migration touches every site that pins the old behaviour *(D11)*

#### R9.1: The three re-resolving watcher tests carry id hints
- **Given**: `a_newer_log_appearing_takes_over_the_watch` (`lib.rs:903`),
  `a_missing_project_directory_still_arms_the_watch` (`:958`) and
  `attaching_mid_flight_stops_the_re_resolution` (`:1096`), which drive re-resolution with
  `Wanted::default()` and (two of them) write undated fixture lines
- **When**: they are migrated
- **Then**: each passes a `Wanted { session_id: Some(<the file's stem>), .. }` hint so
  tier 1 confirms the adopted file, and **every assertion body is preserved verbatim**

#### R9.2: The other five watcher tests are untouched
- **Given**: the `spawn` sites at `lib.rs:651, 1008, 1039, 1193, 1256`
- **When**: `cargo test -p ai-monitor` runs
- **Then**: all pass unedited — they either never fire a re-resolution, start `attached`,
  or watch a single-log directory where `None` and "same target" are indistinguishable

#### R9.3: The inline wording assertions are updated
- **Given**: `dashboard.rs:1507` and `:1527`, which assert the exact string
  `(no active session)` on the `NoLog` path
- **When**: D7's wording lands
- **Then**: both assert the new `tab_has_claude`-keyed string; `:1447` (prefix-only) and
  `:1473` (negative) are left untouched

#### R9.4: All nine `Env` literals compile
- **Given**: the 8 exhaustive literals in `dashboard.rs` (`:708, 1440, 1466, 1487, 1520,
  1536, 1568, 1587`) and the production one at `overlay.rs:213`
- **When**: the field is added
- **Then**: `cargo build --workspace` succeeds with every literal updated — the test ones
  to `false` unless the case under test needs otherwise

## Tasks

Slices are cut along **file ownership**, because that is what actually serialises this
change: `ai-monitor/src/lib.rs` and `app/src/overlay.rs` are each touched by several
requirements, and two workers in one file is a merge conflict, not parallelism.

### T1: Split the resolver and bound tier 2's window `[infra]`
- **Fulfills**: R1, R2, R3
- **Depends on**: (none)
- **Owns**: `crates/ai-monitor/src/lib.rs` — the resolver region (`:124-206`) and the
  resolver tests (`:693-900`)
- Add `resolve_owned_session(dir, want) -> Option<Resolved>` carrying tiers 1-2 verbatim;
  redefine `resolve_session` as it `.or_else(ranked fallback)`; change tier 2's filter to
  bounded form (shipped as `abs_diff <= SLACK`; corrected by D3a to `since - LEAD <= first <= since + SLACK`); update the doc comments
  per R3.3 and the `FIRST_EVENT_SLACK_MS` doc, which currently describes a one-sided
  bound. New tests for R1.3, R1.4, R2.1, R2.2. **The existing five fallback tests must not
  be edited** — that is R3.2, and it is the falsifiable check that this was a split.

### T2: Make the watcher's re-resolution strict `[vertical]`
- **Fulfills**: R4.2, R4.3, R9.1, R9.2
- **Depends on**: T1 *(same file, and needs `resolve_owned_session` to exist)*
- **Owns**: `crates/ai-monitor/src/lib.rs` — the watcher (`:408-500`) and its tests
  (`:900-1300`)
- Point the callback's re-resolution at `resolve_owned_session`. Re-fixture the three
  tests at `:903, :958, :1096` with `session_id` hints naming the file each expects to be
  adopted, preserving every assertion body. Add the R4.3 test: an unowned log appended
  pre-attach produces no `Update`.

### T3: Confirmation-based attachment in the overlay `[vertical]`
- **Fulfills**: R4.1, R5, R6, R8.3 *(the `overlay.rs:698-701` comment only)*
- **Depends on**: T1 — **parallel with T2** (disjoint files, and T2's output is not T3's
  input: both consume `resolve_owned_session`, neither consumes the other)
- **Owns**: `crates/app/src/overlay.rs`
- Switch `open()`'s resolution (`:150`); change `admit`'s attach condition to
  `owned && *snap != TelemetrySnapshot::default()` falling back to `proves_ownership`;
  add D4b's identity adoption; add the reachability doc lines of R5.6. Keep the watcher
  spawn unconditional (R6.1). Tests for R5.1-R5.4; R5.5's three existing tests must pass
  unedited.

### T4: The explained empty state, end to end `[vertical]`
- **Fulfills**: R0.1, R0.2, R7, R8.4, R9.3, R9.4
- **Depends on**: T2, T3 — T3 for the shared `overlay.rs`, T2 because R0.1's guarantee is
  only true once the watcher can no longer feed foreign data in after open
- **Owns**: `crates/app/src/dashboard.rs`, plus the three `overlay.rs` lines T3 released
  (the struct field, the `open()` assignment, the `Env` literal at `:213`)
- Producer and consumer ship together, per the boundary rule: add `tab_has_claude` to
  `Env` *and* the overlay state that fills it *and* the two wordings that read it, in one
  task. Update all 9 `Env` literals and the two inline assertions at `dashboard.rs:1507`
  and `:1527`. Verify the 9 `.snap` files are byte-identical (R7.7) —
  `git status --porcelain crates/app/src/snapshots/` must be empty.

### T5: Record the reversal in the spec and the stale comments `[docs]`
- **Fulfills**: R8.1, R8.2, R8.3 *(the `tabsession.rs` sites)*
- **Depends on**: (none) — **fully parallel**: `specs/overlay-tab-binding/loop.md` and
  `crates/app/src/tabsession.rs` are touched by no other task
- Rewrite `loop.md:24` to the strict rule, naming this spec as what reversed it and why;
  restate the Gate-2 `owned beats unowned` and `re-attach` rows against the
  retained-under-test fallback. Correct `tabsession.rs:109-117` and `:551-552`, whose
  justification cites the one-sided `since_ms` filter that T1 removes. Comment and prose
  only — no code behaviour, no test renamed.

**DAG**: `T1 → {T2 ∥ T3} → T4`; `T5` independent. No cycles.

## External Dependencies

### Pre-work
- (none) — no new crate dependency, no config, no service. `libc` and `notify` are
  already present and this work adds no FFI.

### Post-work
- Delete `HANDOFF.md` from the repo root once the work lands — it says so itself
  ("Untracked scratch note. Delete it when the work lands.").
- Eyeball the two empty states in a real window via `/run-term`: one tab running `claude`,
  one not, both in a project directory that holds another application's live log. Every
  gate here is a unit test; the defect was originally found by looking at the screen.
- **Two sessions started within 120s of each other in one project, neither named by
  `--resume`.** ~~D3 narrows the window but cannot separate them; tier 2 picks the nearest
  by first-event and may pick the other tab's. Pre-existing, unchanged by this work.~~

  **Corrected 2026-08-01 after this gap fired in the first real two-tab test — see D3a.**
  Two things in the struck-through text were wrong. It was *not* "unchanged by this work":
  D4 made the mis-attachment immediate and permanent, and D3's symmetric window made a
  **physically impossible** match admissible. And it was not an edge case — it fired at a
  16-second gap, which is simply how a second tab gets opened.

  What remains after D3a: two sessions whose first events land within `FIRST_EVENT_LEAD_MS`
  (5s) of each other, *both after* their respective process starts. Tier 2 then picks the
  nearest and may pick wrong. That residual is real but small, and unlike the original it
  requires the two sessions to be near-simultaneous rather than merely near.
