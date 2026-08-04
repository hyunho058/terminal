# Spec: ai-monitor-rival-assignment

## Meta
- **Created**: 2026-08-02
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-03

## Goal
`resolve_owned_session`'s tier-2 rival test can hand a tab's own log to a *different*
tab and leave the tab that actually wrote it empty. Reproduced against the shipped
resolver: tab A starts first and writes the only log; idle tab B, started 28s later,
resolves to A's log with `owned: true`; A resolves to `None`.

```
tab A (started first, WROTE the log) -> None
tab B (started later, idle)          -> Some(Resolved { …, owned: true })
```

Make the assignment refuse to confirm a log for a tab that cannot have written it, and
stop vetoing a tab's own log on behalf of a rival that has not written anything.

## Non-goals
- Reinstating `resolve_session`'s ranked fallback (#24 removed it from production
  deliberately). Its 5 tests stay byte-unchanged.
- Retuning `FIRST_EVENT_SLACK_MS` (120s) or `FIRST_EVENT_LEAD_MS` (5s). The lower bound
  encodes a physical impossibility and is not in question here.
- Re-reading the registry on watcher re-resolution. **Measured and rejected**: `claude`
  writes `~/.claude/sessions/<pid>.json` ~1–2s after process start regardless of
  activity, so a registry absent at panel-open is absent because that `claude` never
  started a session — re-reading finds nothing. See Research.
- The wedged-tab case (a `claude` sitting at an un-answered startup prompt, creating no
  session at all). Not a terminal defect; the panel correctly has nothing to show.
- The `claude --continue` / fork / compacted-transcript gap.

## Confirmed Goal
Tier 2 assigns a log with the rule *"a log belongs to the latest process that started at
or before its first event"* (`crates/ai-monitor/src/lib.rs:287`). The rule is sound only
when every live `claude` in the project has written a log. A rival that started after us
and wrote **nothing** still vetoes our claim to our **own** log, producing two failures
from one line:

- **F1 — the writer is silenced.** The tab that wrote the log resolves to `None` and
  shows the empty state.
- **F2 — the idle sibling is confirmed wrong.** With the writer's log now the sole
  surviving candidate, the `after.len() == 1` uniqueness arm
  (`crates/ai-monitor/src/lib.rs:300-306`) adopts it for the *other* tab at any distance,
  with `owned: true`. `Attachment::admit` then latches it permanently
  (`crates/app/src/overlay.rs:421`) — `attached` is never cleared.

F2 is the failure class #24 exists to prevent, reached by a different route: a tab
rendering another session's id, model, spend, context and uptime with nothing marking it
as a guess.

Done when:
1. **F2 is closed unconditionally.** The idle sibling resolves to `None` rather than to its
   sibling's log, in every combination of registry availability.
2. **F1 is closed whenever every rival that would have vetoed carries a validated registry
   record** — the ordinary case, since registration was measured at 1.1–2.2s after process
   start. Those rivals are disqualified by D9, the *effective* rival set goes empty, D3's
   rescue applies, and the writer resolves to its own log **at any distance**. This is what
   fixes the reproduced timeline (rival at +28s, first event at +900s); note that it works
   only because D3 tests the effective set — against the raw list the slack filter rejects
   the same log and F1 is not fixed at all. See D3.
   *F1 is NOT closed when a vetoing rival is itself recordless.* D9 outcome 3 keeps it in
   the effective set, the start-time veto stands, and the writer resolves to `None` — the
   honest limit of the change, since a veto inferred from timing is all that remains when
   timing is all there is. Nor is it closed when the rival set is *unknown* (D12).
   Stated here rather than discovered later: two earlier drafts claimed F1 was closed
   outright, and the L2 reviewer showed each of them was wrong for a different reason.
3. No tab can reach `owned: true` for a log whose writing it cannot account for.
4. The 5 protected `resolve_session` fallback tests are byte-unchanged, the 9 dashboard
   snapshots are byte-identical, and **no existing test changes contract** — including the
   two that pin the lone-candidate rule, which D3 was narrowed to preserve. Call sites
   retyped by D7/D11 are permitted; their assertions are not.

## Research

### Where the defect lives
- The veto: `let outranked = rivals.iter().any(|&r| r > since && r <= first);`
  (`crates/ai-monitor/src/lib.rs:287`). Applied per *candidate log*, using rival **start
  times only** — the rival's own log, or absence of one, is never consulted.
- The uniqueness arm that converts the veto into a confirmed wrong answer:
  `match after.len() { 1 => after.into_iter().next(), _ => …proximity… }`
  (`crates/ai-monitor/src/lib.rs:300-306`). The `1` arm deliberately bypasses
  `FIRST_EVENT_SLACK_MS`, so an adopted log may sit any distance from the process start.
- The latch: `if (owned && *snap != TelemetrySnapshot::default()) || proves_ownership(…)`
  (`crates/app/src/overlay.rs:421`, in `Attachment::admit`,
  `crates/app/src/overlay.rs:386`). `attached` is documented as never cleared, so a wrong
  confirmed attachment survives for the life of the open.
- Rival data supplied by the app: `ProcTable::rival_starts`
  (`crates/app/src/tabsession.rs:325`) returns `Vec<u64>` — start times, nothing else.
  It already walks every `claude` on the machine and already has each pid in hand.

### The signal available but unused
- `ai_monitor::registered_session(pid)` (`crates/ai-monitor/src/lib.rs:81`) reads
  `~/.claude/sessions/<pid>.json` and returns `Registered { session_id, cwd,
  started_at_ms }` (`crates/ai-monitor/src/lib.rs:55-63`).
- `Registered.cwd` is parsed and has **no consumer anywhere in the workspace** — grep for
  `.cwd` against `tabsession.rs`/`lib.rs` returns no production read. A ready-made
  confirmation signal is being discarded.
- The app validates the record with `REGISTRY_DRIFT_MS = 60_000`
  (`crates/app/src/tabsession.rs:84`, `registered_id_with` at
  `crates/app/src/tabsession.rs:100`).

### Measurements taken for this spec (2026-08-02)
- **Registry timing.** An interactive `claude` left idle for 90s wrote
  `~/.claude/sessions/<pid>.json` at **t+2s, with no activity**; `startedAt` sat +2.2s
  from the true process start and −88.8s from first activity. So `startedAt` tracks
  *process start*, the 60s drift guard passes, and registration does **not** wait on the
  session doing work. This retires the "re-read the registry later" design.
- **Registry availability across paths.** `claude` registered in 1.1–2.2s through a plain
  pty, through `zsh`, through `zsh` under our generated `ZDOTDIR`, and through
  `term_core::Pty` itself. Neither our PTY, our shell-integration `ZDOTDIR`, nor the
  inherited `CLAUDE_*` environment suppresses it.
- **Consequence for tier 2.** Because tier 1 is reliable within ~2s, tier 2 is reached
  only by a `claude` that has no registry record — one inside its first ~2 seconds, one
  too old to write the file, or one that never started a session. The defect is therefore
  narrow but *not* unreachable, and its F2 branch is the dangerous one.
- **Live confirmation of the shape.** On this machine, tab claude pid 96733
  (`/Users/hyunhokim/IdeaProjects/terminal`, started 18:20:32) has no registry record, so
  it resolves through tier 2. It currently yields `None` only because every log in that
  directory predates it; a sibling session opening a *new* log after 18:20:32 would be
  adopted by it under F2.

### Project conventions
- Build/test/lint: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`
  (`CLAUDE.md`).
- Dependency direction `common ← term-core ← renderer ← app` is enforced
  (`.claude/rules/dependency-direction.md`). `ai-monitor` depends on `serde_json` +
  `notify` only (`crates/ai-monitor/Cargo.toml`) — **no libc**, and it must stay that way:
  it takes a pid only as a filename key, never to query the kernel. Any process
  measurement stays in `app` and crosses as plain data.
- Tests are inline `#[cfg(test)]` modules; `app` is a binary crate with no lib target, so
  its tests compile through `src/main.rs`.
- Dashboard layout is pinned by 9 insta snapshots in `crates/app/src/snapshots/`.

## Decisions

### D1: A rival is disqualified as a claimant by its own registry record
- **Status**: resolved
- **Rule**: a live `claude` may veto our claim to a log only if it cannot be shown to
  belong elsewhere. A rival whose `~/.claude/sessions/<pid>.json` names a session
  **other than this log** did not write this log and stops vetoing. A rival that names
  **this** log outranks us absolutely — we must not take it.
- **Rationale**: the current veto (`crates/ai-monitor/src/lib.rs:287`) reasons from start
  times alone, so a rival that wrote *nothing* still outranks the process that actually
  wrote the log. The registry is the one signal that names a writer instead of inferring
  one, and it was measured for this spec as available 1.1–2.2s after process start across
  four spawn paths — the same evidence that makes it #24's primary tier.
  Rejected: **drop the veto entirely** (re-admits the two-tab confusion #24 closed in its
  second round — two tabs seconds apart each look like a plausible owner of the other's
  log). Rejected: **keep it as pure timing but require the rival to sit inside the 120s
  slack** — still infers a writer from clock arithmetic, which is precisely what four
  rounds of this feature failed to do; and the reproduced timeline has the rival 28s away,
  well inside any slack, so it would not fix the reported case.
- **Cost**: one small file read per live `claude` in the project. At panel-open that is
  proportionate to work `ProcTable::rival_starts` (`crates/app/src/tabsession.rs:325`)
  already does — one `cwd_of_pid` + `start_ms` per `claude` on the machine, on a
  user-initiated path. **It is not open-only**: D6 repeats these reads on every
  re-resolution, and under D2 an unconfirmable tab never attaches, so they continue for the
  life of that open. See D6 for the bound that makes this acceptable and D11 for the
  ordering that keeps it O(rivals) rather than O(candidates × rivals).

### D2: Without confirmation, no tab attaches — uniqueness may not confirm alone
- **Status**: resolved
- **Rule**: when no rival can be disqualified and nothing names the session, every tab
  resolves to `None`. The `after.len() == 1` arm
  (`crates/ai-monitor/src/lib.rs:300-306`) may no longer confirm a log at unbounded
  distance merely because it is the sole survivor of the veto.
- **Rationale**: that arm is what converts D1's failure into the dangerous branch — it
  hands the sole surviving candidate to the wrong tab with `owned: true`, which
  `Attachment::admit` (`crates/app/src/overlay.rs:421`) then latches for the life of the
  open. An empty panel is recoverable; a confirmed-wrong one is not, and it is the exact
  failure #24 exists to prevent. Consistent with that round's human-approved standard,
  "wrong-but-honest beats wrong-and-confident".
  Rejected: **nearest-by-time attaches** (today's behavior — the defect). Rejected:
  **attach but mark it unconfirmed on screen** — requires `dashboard::Env` and the 9
  pinned snapshots to change, which this spec's done-criteria forbid, and it re-introduces
  the `owned: false` display path #24 deleted.
- **Cost**: a briefly empty panel inside the ~2s window before a registry record exists.
  The watcher re-resolves and fills it.

### D3: The uniqueness rescue survives only for an EMPTY rival set
- **Status**: resolved (revised after L2 review — see Review Record)
- **Rule**: the `after.len() == 1` arm (`crates/ai-monitor/src/lib.rs:300-306`) applies
  **only when `rivals` is empty**. With any rival present, a sole surviving candidate must
  also fall inside `FIRST_EVENT_SLACK_MS` to be confirmed, else `None`.
- **Rationale**: uniqueness is what converts a bad veto into a *confirmed* wrong attachment
  at unbounded distance — it is F2's final step. But the rival set is exactly what tells
  the two situations apart. With no rival that could have written this log, "the only log
  that began after we did" is a genuine deduction and the rescue is sound; with such a
  rival present, uniqueness is an artefact of the veto having eliminated everything else,
  and deduces nothing. Restricting the arm rather than deleting it removes F2 while leaving
  the sound case standing.
- **The effective set is computed PER CANDIDATE**, because D9's outcomes are defined against
  a specific candidate log. This is unambiguous only because the gate sits behind
  `after.len() == 1` — where exactly one candidate survives, "the" effective set is that
  candidate's. An implementer computing one set once for the whole call gets a different
  answer whenever more than one candidate survives; the rescue must not be reached in that
  case at all.
- **"Empty" means EFFECTIVELY empty and KNOWN empty** — both qualifiers are load-bearing
  and were added after the second review pass, which showed the unqualified form failing
  in two opposite directions:
  - **Effectively empty** = empty *after* D9 has disqualified every rival that provably did
    not write this candidate. Testing the raw list instead makes D9 and D3 cancel: for the
    start-time veto to silence us at all, some rival start `r` must satisfy
    `our_start < r ≤ first`, which forces `first − our_start ≥ r − our_start`. So whenever
    a rival is far enough out to have silenced us, the log it was silencing us over is
    *also* outside the 120s slack — and the slack filter rejects it no matter what D9 does.
    **This is not hypothetical: it fails on this spec's own reproduced timeline**
    (rival at +28s, first event at +900s), where the raw-list form leaves the writer at
    `None` and fixes nothing. With the effective set, D9 disqualifies the registered idle
    rival, the set is empty, the rescue applies, and the writer gets its log.
  - **Known empty** = `app` actually enumerated the rivals. `ProcTable::rival_starts`
    returns `Vec::new()` when our own cwd is unreadable
    (`crates/app/src/tabsession.rs:332-334`) and silently drops any rival whose start time
    is unreadable (line 339) — so "empty" is also what *failure* looks like. Under the
    narrowing, empty is no longer a mere degradation, it is the switch that re-enables the
    **unbounded** rescue; a failed cwd read would therefore adopt a sole candidate at any
    distance with `owned: true` and latch it. That is F2 on a failure path. The resolver
    must distinguish "no rivals exist" from "rivals unknown" — see D12 — and the rescue
    fires only on the former. This is the same known-absent vs. unknown distinction D9
    outcome 2 vs. 3 already makes, applied one decision later.
  **Rejected: delete the arm outright** (this decision's first form). The L2 reviewer's
  steelman against it is decisive and is recorded here rather than paraphrased:
  `crates/ai-monitor/src/lib.rs:66-69` states that the registry is *"an undocumented
  on-disk detail of another program, so it must never be load-bearing beyond 'use it when
  it is there'"*. Deleting the arm makes tier 1 the only path that can ever confirm, so a
  future `claude` that stops writing `~/.claude/sessions/<pid>.json` would silently reduce
  the panel to permanently empty with no fallback and no error — the crate's own stated
  invariant, violated. The evidence offered for deletion ("tier 1 is reliable, so the
  rescue rescues nobody") also does not carry that weight: 1 of 5 live `claude` processes
  on this machine has had no registry record for hours, cause unidentified.
  Rejected: **fix only the veto** — leaves F2 reachable whenever the corrected veto still
  eliminates all but one candidate.
- **Consequence — zero contract exemptions.** Both tests that pin the lone-candidate rule
  (`the_upper_bound_stops_at_120s_after_the_process:1357`,
  `a_log_that_began_hours_after_the_process_is_not_this_tab_s:1255`) assert with
  `rivals = &[]`, so both keep passing **unchanged**. Verified by reading the assertions,
  not assumed. Done-criterion #4 stands in its original form.

### D4: A rival's registry record is validated by the same guard as our own
- **Status**: resolved
- **Rule**: a rival's record is believed only when `started_at_ms >= that rival's process
  start` and the drift is within `REGISTRY_DRIFT_MS` (60s) — byte-identical to the rule
  `registered_id_with` (`crates/app/src/tabsession.rs:100`) already applies to our own
  record. A record that fails validation is discarded and the rival degrades to
  start-time-only, exactly as today.
- **Rationale**: `~/.claude/sessions/` files outlive their processes (#24 measured 5 files
  for 4 live processes), so a recycled pid can present a stranger's record. One rule for
  our record and rivals' alike means the pid-reuse guard cannot drift out of agreement
  between two call sites — the failure mode this feature has already suffered twice with
  duplicated rules.
  Rejected: **skip validation because a stale record only removes a veto** — false: a
  stale record that happens to name *our* log makes that rival outrank us absolutely
  under D1, which silences the real writer. The safe direction is not uniformly safe.
  Rejected: **only trust records written since the panel opened** — discards the valid
  record of every long-running sibling, which is the common case.

### D5: The slack doc is sharpened — "unrivalled" is stated as "empty rival set"
- **Status**: resolved (rescoped after D3's revision — no test is reversed)
- **Rule**: no test changes contract. The `FIRST_EVENT_SLACK_MS` doc paragraph beginning
  *"**This bound does not apply to a sole, unrivalled candidate.**"* is sharpened so
  "unrivalled" reads as its now load-bearing meaning — **the rival set is empty** — and
  records that with any rival present the bound *does* apply to a sole candidate. The
  amendment quotes the prior sentence, per the treatment #24 gave
  `specs/overlay-tab-binding/loop.md:24`.
- **Rationale**: D3's first form reversed this paragraph outright and would have needed the
  full quote-the-old-rule treatment plus two test rewrites. The revised D3 does not reverse
  it — a sole candidate with no rivals still passes the bound by — so the paragraph stays
  true. What changes is that "unrivalled" moves from prose to a condition the code branches
  on, and a word carrying that weight must not be left to the reader's inference. This is
  the smaller half of the same obligation: record the rule the code now enforces.
  Rejected: **leave the paragraph alone** — it would remain literally true while the term
  it turns on is undefined, which is how the next round inherits an ambiguity.
- **Also corrected here**: `specs/ai-monitor-session-isolation/loop.md:37` names four
  protected tier-2 tests, **two of which no longer exist**
  (`the_upper_bound_is_the_same_120s_as_the_lower`, and
  `a_session_that_began_just_before_the_process_start_still_qualifies`, replaced at
  `crates/ai-monitor/src/lib.rs:1374`). This spec's protected list is stated by name in
  Constraints against the current tree rather than deferring to that stale list.

### D6: The watcher refreshes rival records on each re-resolution while unattached
- **Status**: resolved
- **Rule**: while `attached` is false, each re-resolution re-reads every rival's
  `<pid>.json` and re-applies D4's validation, rather than reusing the records read at
  panel-open. Refresh stops when the caller attaches, exactly as re-resolution already does.
- **Rationale**: a sibling in its first ~2 seconds has no record, and under D1 that is
  precisely the state in which it still vetoes us. Freezing the rival set at open would
  keep that stale veto for the entire open — the same "read once and freeze" shape that
  produced this feature's original misdiagnosis. Cost is one small file read per rival per
  fs event, bounded by the rival count and stopped by the attach flag.
  Rejected: **freeze at open** (reintroduces the stale-veto window; the user would have to
  close and reopen the panel). Rejected: **refresh pids but not records** — fixes the half
  that does not change; record availability is what moves inside the 2s window, process
  liveness is not.

### D7: Rivals cross the crate boundary as `(pid, process_start_ms)`, not as session ids
- **Status**: assumed (implied by D1 + D4 + D6)
- **Rule**: `rivals` changes from `Vec<u64>` to a list of `{ pid, started_ms }`. `app`
  supplies both — it already computes `start_ms` per rival
  (`crates/app/src/tabsession.rs:325`). `ai-monitor` reads `<pid>.json` itself via its
  existing `registered_session` and applies D4's validation as a **pure comparison**
  against the supplied `started_ms`.
- **Rationale**: D6 requires the watcher to re-read records on its own thread, so the
  session ids cannot simply be resolved once in `app` and passed down. Passing the process
  start as plain data is what keeps `ai-monitor` free of `libc`
  (`.claude/rules/dependency-direction.md`; `crates/ai-monitor/Cargo.toml` has no libc) —
  the crate continues to treat a pid as a filename key and never queries the kernel.
  Rejected: **resolve session ids in `app` and pass `Vec<(u64, Option<String>)>`** — cheaper
  per call, but the watcher could then never refresh them, contradicting D6.
- **Missing-data policy, stated explicitly** (the existing code leaves this to inference and
  the two halves currently disagree). `rival_starts` today does
  `.filter_map(|(pid, _)| start_of(*pid))` (`crates/app/src/tabsession.rs:339`) — a live
  `claude` whose start time is unreadable is silently **dropped**, i.e. absence ⇒ no veto —
  and returns an empty list entirely when `mine_cwd` is `None` (lines 332-334). D9 outcome 3
  takes the opposite axiom for records: absence ⇒ keep the veto. Both are kept, and the
  reason they differ is recorded rather than harmonised: a rival we cannot *time* cannot be
  placed in the ordering the veto is computed from, so there is no veto to express; a rival
  we cannot *identify* is still ordered, so its timing veto still applies. An unreadable
  `started_ms` therefore drops the rival, exactly as today.
- **Evaluation order**: rival records are read **once per re-resolution**, before the
  candidate loop, and reused across candidates. Naive placement inside the per-candidate
  filter would make it O(candidates × rivals) file reads per fs event, on top of the
  existing 16-candidate head reads.

### D8: `Registered.cwd` stays unused; `Attachment::admit` is unchanged
- **Status**: assumed
- **Rationale**: comparing a rival's `session_id` against the candidate log's file stem is
  sufficient to answer "did this rival write this log", so `cwd` adds a second, weaker test
  with its own failure modes (a rival that changed directory). Leaving it unread keeps this
  change to one signal. `admit` needs no edit because the fix removes the *production* path
  that could hand it `owned: true` for a foreign log — its latch is correct given a correct
  resolver, and editing it would change the display layer this spec's done-criteria freeze.

### D9: A rival that names ANY other session does not veto — existence is not required
- **Status**: resolved (revised after L2 review — see Review Record)
- **Rule**: exactly three outcomes per rival, evaluated against the candidate log L:
  1. Validated record names **L** → the rival owns L; it vetoes **absolutely**, before every
     other filter, and we never take L.
  2. Validated record names **anything else** → the rival does not veto L. This holds
     whether or not that other log exists on disk yet.
  3. **No record, or the record fails D4's validation** → nothing is known; the existing
     start-time veto applies unchanged (`r > since && r <= first`). This is what D4 means by
     "degrades to start-time-only", and it is the *only* case where a veto is inferred.
- **Rationale**: this decision's first form additionally required the rival's named log to
  *exist*, so that a `--continue`/fork rival — which appends to an older file its
  `sessionId` no longer matches — would keep vetoing. The L2 reviewer showed that condition
  defeats the spec's own goal: an idle sibling registers at ~2s naming a session whose
  `.jsonl` does not exist yet, which is **precisely the rival class in the reported
  timeline**. Under the existence requirement that sibling stays a full veto, the tab that
  actually wrote the log still resolves to `None`, and **F1 is not fixed at all** — only F2
  is. Confirmed against a real record on this machine naming a session whose log was never
  created.
  A rival that has written *nothing* cannot have written L. That is the deduction the rule
  now rests on, and it needs no file to exist.
- **Necessary but NOT sufficient on its own.** A second review pass showed that removing the
  existence requirement fixes nothing while D3 tests the *raw* rival list: the disqualified
  rival leaves the effective set, but the raw set is still non-empty, so the slack filter
  rejects the same log and the writer still resolves to `None` — on this spec's own
  reproduced timeline. D9 fixes F1 only in combination with D3's effective-set test. The two
  must be read together; neither works alone. An earlier draft of this bullet claimed the
  timeline had been "traced end to end", and that trace had stopped one filter short.
  Rejected: **require the named log to exist** (the first form — defeats F1, above).
  Rejected: **keep the requirement and restate the Goal to admit F1 is unfixed** — the empty
  panel is the reported symptom; a spec that fixes only the failure the user cannot see is
  not worth the change.
  Rejected: **add `cwd` as a second signal** (reverses D8) — `cwd` answers which directory,
  never which file.
- **The fork residual is closed by D13, not by a distance bound.** An earlier form of this
  bullet accepted the risk on the grounds that *"the exposure is bounded by D3: with any
  rival present, a candidate must still fall inside the 120s slack"*. D3's effective-set
  revision **voided that sentence**: a rival disqualified by outcome 2 is precisely *not
  "present"* for D3's purposes, so a mis-disqualified rival both stops vetoing **and**
  removes the slack guard — the two protections fail on the same input, and the residual
  becomes unbounded rather than ≤120s. The mitigation was keyed to the very condition that
  triggers the risk. Outcome 2 is therefore tightened by D13 rather than bounded.
- **Cost**: no `is_file()` per rival — outcome 2 is a string comparison against the
  candidate's file stem.

### D10: Tier 2's demoted role is stated in the code, not left to inference
- **Status**: resolved
- **Rule**: the doc comment on the timing tier records what the measurements established —
  tier 2 is a **fallback**, not a co-equal identification path — and carries the measured
  registration latency (1.1–2.2s, `claude` 2.1.220, 2026-08-02) so a future reader can
  re-check it. It must **not** enumerate the cases tier 2 covers as a closed list: the
  reviewer's counterexample stands, and is stated instead — 1 of 5 live `claude` processes
  on the measuring machine had no registry record after 1h17m, cause unidentified. The note
  says tier 2 exists *because* the registry is another program's undocumented detail
  (`crates/ai-monitor/src/lib.rs:66-69`), not merely because registration takes ~2s.
- **Rationale**: four rounds of this feature tried to derive session identity from clock
  arithmetic, and each shipped a defect the next had to fix. Tier 1 ended that, but nothing
  in the code says so, and the timing tier still reads like a peer. Writing the demotion
  down is what stops a fifth round from investing in the same dead end.
  Rejected: **leave the framing alone** (leaves the next round free to re-litigate timing).
  Rejected: **commit to deleting tier 2 in a follow-up** — registry coverage across `claude`
  versions has been measured only on 2.1.220, which does not support that commitment.

### D11: Rival MEMBERSHIP is republished by `app`, not frozen at open
- **Status**: resolved (from L2 review — the gap D6 and D7 left between them)
- **Rule**: the rival list lives behind a handle shared between `app` and the watcher —
  `Arc<Mutex<Vec<Rival>>>` in `Watch`, replacing the by-value `Vec`. `app` refreshes its
  contents on the tab-metadata sync it already runs; the watcher reads it under the lock at
  each re-resolution (D6), alongside re-reading each rival's record.
- **Rationale**: D6 refreshes rival *records* and D7 fixes the *payload*, but between them
  nothing refreshes **which processes are rivals**, and `ai-monitor` cannot compute it —
  enumerating processes needs `libc`, which the crate may not have. Frozen membership fails
  in both directions: a sibling that starts `claude` **after** the panel opened never vetoes
  at all, so its log can be adopted with `owned: true` — F2 verbatim, forbidden by
  Done-when #3 — and a sibling that **exits** keeps vetoing for the life of the open,
  silencing a tab that now legitimately owns its log. `app` already publishes a tab-metadata
  snapshot on a cadence and already walks the process table to build it, so the refresh
  costs a list rebuild it is substantially performing anyway.
  Rejected: **re-open the panel when the roster changes** — no new shared state, but
  `open()` drops the watcher and re-runs the full backfill, which is the re-open storm
  `Repoint` (`crates/app/src/overlay.rs:263`) exists to prevent.
  Rejected: **accept as a Known Gap** — leaves a reachable F2 and would force Done-when #3
  to be narrowed to something the spec cannot honestly claim.
- **Lock discipline**: the watcher holds the lock only to clone the list, never across the
  file reads that follow. `app` writes from a worker, never from the winit event-loop thread.
- **Implementation deviation, recorded (iteration 3 of the verification loop).** This decision
  says `app` refreshes "on the tab-metadata sync it already runs", reusing a process walk it is
  "substantially performing anyway". The implementation instead spawns an **ad-hoc thread** from
  `on_active_changed` and re-forks `/bin/ps` via `hint_for`. Two guards make that proportionate
  — it returns immediately once `attached` is set, and an in-flight flag (cleared on `Drop`, so
  a panicking worker cannot latch it) allows only one outstanding refresh — but the cost
  rationale above is no longer accurate for the shipped code. Reusing the tabmeta worker's
  existing walk remains the cleaner design and is deliberately left as follow-up rather than
  claimed as done.

### D12: "No rivals" and "rivals unknown" are different values, and the signature stays a slice
- **Status**: resolved (from L2 review pass 2; third path and boundary added in pass 3)
- **Rule**: the rival payload distinguishes *known-empty* from *unknown*. D3's uniqueness
  rescue fires on **known-empty** alone; on *unknown* the resolver behaves as though rivals
  were present (slack enforced, no unbounded adopt).
- **The three paths that yield "unknown"** — all three drop a rival, or all rivals, from a
  read that did not succeed. The third was missed on the first attempt and is the one that
  matters most, because it is the only one that can produce a *silently* empty list from a
  per-rival failure:
  1. our own cwd unreadable — `crates/app/src/tabsession.rs:332-334` returns `Vec::new()`;
  2. a rival whose `start_ms` could not be read — line 339 drops it;
  3. **a rival whose own cwd could not be read** — line 338,
     `cwd_of(**pid).as_deref() == Some(mine_cwd)`, where a `None` cwd fails the comparison
     and the rival vanishes. This is deliberate and documented at
     `crates/app/src/tabsession.rs:322-324` ("A rival whose cwd cannot be read is skipped
     rather than assumed to match"). Correct for the veto — inventing a rival would refuse a
     log that really is ours — but under D3 the same drop now *also* suppresses nothing and
     *enables* the unbounded rescue when that rival was the only one. Same door, opposite
     consequence.
  "Known-empty" is therefore only: the process table was read, every candidate rival's cwd
  and start time were read, and none of them was another `claude` in this project.
  Note that paths 2 and 3 bite **only when the drop empties the list** — a partially
  enumerated set is non-empty and already blocks the rescue. Stated so the flag is not
  later optimised away as redundant.
- **Where the distinction lives**: at the `hint_for` boundary
  (`crates/app/src/tabsession.rs:64-78`), **not** in `ProcTable::rival_starts`' return
  value. `rival_starts` keeps its current contract, returning an empty vector on the paths
  above; `hint_for` is what labels the result known or unknown. This is not a style
  preference — `rival_starts_names_every_other_claude_in_the_same_project`
  (`crates/app/src/tabsession.rs:496-534`) asserts `.is_empty()` at lines 530 and 531-533
  for exactly paths 1 and 3, so redefining the *function's* return value would change that
  test's contract and break Done-when #4's "no test changes contract". Placing the label one
  level up keeps those assertions true and unedited.
- **D7's rationale is incomplete as written**: it justifies dropping an untimeable rival
  with "there is no veto to express". Under D12 that drop has a second, opposite effect —
  it suppresses the rescue — which is why the drop must also raise *unknown*.
- **Signature**: `resolve_owned_session`'s third parameter stays a **slice** —
  `&[Rival]` plus the known/unknown flag, not a lock handle. D11's
  `Arc<Mutex<Vec<Rival>>>` lives in `Watch` only; the watcher clones under the lock and
  passes the slice. This is load-bearing for the protected tests: `&[]` literals at
  `crates/ai-monitor/src/lib.rs:931, 942, 1094, 1120, 1256` coerce to `&[Rival]` and stay
  byte-unchanged, but would not compile against a handle.
- **Rationale**: before the D3 narrowing, an empty rival list only ever *degraded* the
  answer, so conflating "none" with "could not tell" was harmless. The narrowing makes
  empty a privilege — it re-enables the unbounded rescue — and a privilege must never be
  granted by a failed read. Rejected: **keep one empty vector and treat failure as
  no-rivals** (the status quo) — it hands the strongest branch in the resolver to the
  weakest evidence. Rejected: **treat failure as "rivals present"** by injecting a
  sentinel rival — works for D3 but corrupts D9's per-rival reasoning with a process that
  does not exist.

### D13: Outcome 2 compares against the log's CONTENT session id, not its filename
- **Status**: resolved (from L2 review pass 3 — closes the residual D3 unbounded)
- **Rule**: D9 outcome 2 fires only when the rival's validated registered id differs from
  the candidate log's **content** session id — the `sessionId` / `session_id` carried on the
  log's own lines — and not merely from its filename stem. When the two disagree, or the
  content id cannot be read, the filename stem is the fallback and outcome 2 is **not**
  taken: an unreadable id is not evidence of difference.
- **Rationale**: the filename stem answers "which session *created* this file"; the content
  id answers "which session is *writing* it". They diverge exactly in the fork / `--continue`
  / compacted case, which is the residual D9 accepted — a rival appends to a pre-fork file
  while its record names the id it is now using, so a stem comparison calls it "a different
  session" and disqualifies the true writer. With D3's effective-set test that
  disqualification also removes the slack guard, so the exposure is unbounded (see D9). The
  content id closes it at the source instead of bounding it.
  **The crate already reads this.** `SessionTelemetry` parses `sessionId`/`session_id` off
  each line (`crates/ai-monitor/src/telemetry.rs:263-265`) and `TelemetrySnapshot.session_id`
  carries it — the same field `Attachment` uses for its identity guard
  (`crates/app/src/overlay.rs:425`). Resolution reads each candidate's head already, for
  `first_event` (`crates/ai-monitor/src/lib.rs:339-352`), so the id comes out of a read that
  is being paid for regardless.
- **This corrects D8's reasoning, not its conclusion.** D8 rejected a second signal by
  arguing `cwd` "answers which directory, never which file". True of `cwd` — but it was
  taken as licence to stop looking, and the content id *does* answer which file. `cwd` stays
  unused; the content id is adopted. Recorded because the flaw was in the generalisation,
  and a future round reading D8 alone would inherit it.
- **Cost**: none beyond the existing head read — the id is extracted from the same buffer
  `first_event` already decodes.

## Constraints
- `ai-monitor` must not gain `libc` or any kernel-querying dependency. It treats a pid as a
  filename key only; every process measurement stays in `app` and crosses as plain data
  (`.claude/rules/dependency-direction.md`).
- The 9 dashboard snapshots in `crates/app/src/snapshots/` stay byte-identical.
- The 5 `resolve_session` ranked-fallback tests stay byte-unchanged; `resolve_session`
  itself keeps its public, production-callerless status. By name, against the current tree:
  `a_named_session_wins_outright`,
  `a_session_started_by_this_tab_beats_a_more_active_foreign_one`,
  `two_tabs_on_one_project_get_their_own_sessions`,
  `an_empty_hint_degrades_to_ranking_and_claims_nothing`,
  `nothing_confirmable_resolves_to_none_rather_than_a_foreign_log`.
- **No test changes contract.** The two tests pinning the lone-candidate rule
  (`the_upper_bound_stops_at_120s_after_the_process`,
  `a_log_that_began_hours_after_the_process_is_not_this_tab_s`) both assert with an empty
  rival set and must keep passing **unedited** — this is the check that D3 was really
  narrowed and not merely described as narrowed.
- **Mechanical retyping is distinguished from contract change**, and the enumeration is
  complete — it is what bounds the diff at L4 and what "unedited" is checked against:
  - **Resolver call sites passing `&[u64]` literals** — `crates/ai-monitor/src/lib.rs:1128,
    1169, 1235, 1316` — retyped.
  - **`Watch.rivals` constructions** — `crates/ai-monitor/src/lib.rs:843, 1656, 1710, 1768,
    1812, 1857, 1928, 2018, 2101, 2165` (ten `rivals: Vec::new()` sites) — retyped by D11.
  - **Bare `&[]` call sites that must NOT need editing** — `crates/ai-monitor/src/lib.rs:931,
    942, 1094, 1120, 1256`. These coerce to `&[Rival]` and stay byte-unchanged, which is
    only true because D12 keeps the parameter a slice. Three of them sit in protected or
    lone-candidate tests, so this is a check, not a convenience.
  - **`crates/app` sites** — omitted from an earlier form of this list, which called itself
    complete while covering only `ai-monitor`:
    `crates/app/src/overlay.rs:178, 179, 209` (the production wiring —
    `hint.rival_starts.clone()`, the resolver call, the `Watch` field);
    `crates/app/src/tabsession.rs:41` (`pub rival_starts: Vec<u64>`, the field D7 retypes);
    `crates/app/src/tabsession.rs:76` and `325-341` (the producer, and the `hint_for`
    boundary where D12 attaches the known/unknown label).
  - **`crates/app/src/tabsession.rs:496-534`** —
    `rival_starts_names_every_other_claude_in_the_same_project` needs its expectations
    retyped (515-526), but its `.is_empty()` assertions at 530 and 531-533 **must keep
    passing unedited**. They pin the two failure paths D12 relabels, and D12 places the
    label at `hint_for` rather than in `rival_starts`' return value precisely so that this
    stays true. If an implementation finds itself editing those two assertions, D12 has been
    implemented at the wrong level — that is the check, not a formality.
  - Verification must diff *assertions*, not whole function bodies: a byte-diff of bodies —
    the method `specs/ai-monitor-session-isolation/loop.md` uses — would report the retyped
    sites as edited and cannot tell the two kinds of change apart.
- No writes anywhere under `~/.claude`. Observation only.
- The rival refresh (D6/D11) runs on the watch thread, never the winit event-loop thread,
  and stops once `attached` is set.
- macOS ARM64 only; no `#[cfg(target_os)]` branches (`.claude/rules/macos-only.md`).
- `cargo fmt --all --check` clean and `cargo clippy --workspace --all-targets -- -D warnings`
  clean.

## Known Gaps
- **A forked rival with an unreadable content id.** D13 closes the fork case by comparing
  against the log's content `sessionId`, but when that id cannot be read from the head the
  rule falls back to the filename stem, and the fork case reopens for that log. Narrow —
  it needs a log whose head carries no session id at all — and it fails toward *not*
  disqualifying the rival, which is the safe direction. The measurement that would let D13
  be tightened further: start `claude --continue` in a project, read
  `~/.claude/sessions/<pid>.json`, and compare its `sessionId` against both the filename and
  the content of the `.jsonl` actually being appended to. Still **unmeasured**.
  *Note the bound that used to sit here is gone*: an earlier draft accepted this residual as
  "bounded by D3's 120s slack", which D3's effective-set revision voided — see D9.
- **Our own forked/continued session is still unidentifiable.** A tab whose own `claude`
  was started with `--continue`/`-c` cannot be named by either tier and shows the empty
  state. Pre-existing from #24, unchanged here.
- **Pid reuse during an open.** A rival's `started_ms` is captured when `app` publishes the
  list (D7/D11). If that pid exits and the OS recycles it between publishes, D6's refresh
  validates the new process's record against the old process's start. Bounded by the
  publish interval and the 60s drift guard; not closed here.
- **Registry coverage measured on one version, with a live counterexample.** All timing
  evidence comes from `claude` 2.1.220 on 2026-08-02, and within that sample 1 of 5 live
  processes had no registry record after 1h17m for reasons not identified. D3 is narrowed
  and D10 worded so that neither depends on the registry being universally present.
- **The wedged tab is out of scope.** A `claude` sitting at an un-answered startup prompt
  creates no session; the panel correctly shows its empty state. Root cause unidentified —
  live pid 96733 was still in that state after 1h17m with no registry record and no log.

## Review Record
The L2 reviewer (maker ≠ checker) returned **NEEDS_FIX** on the first pass. Its two
load-bearing factual claims were re-verified against the tree before acting, and both held:
`a_log_that_began_hours_after_the_process_is_not_this_tab_s:1255` asserts `is_some()` for a
sole candidate 5h26m out, and two of the four protected tier-2 tests named in
`specs/ai-monitor-session-isolation/loop.md:37` no longer exist. Changes made in response:
- **D9 revised** — the "named log must exist" condition was defeating the spec's own F1 goal
  for the exact rival class in the reported timeline. Now three explicit outcomes.
- **D3 narrowed** — uniqueness retained for an empty rival set, answering the steelman from
  `crates/ai-monitor/src/lib.rs:66-69`. Side effect: zero contract exemptions, so
  Done-when #4 was restored to its original form and **D5 rescoped** from a test reversal to
  a doc sharpening.
- **D11 added** — rival membership churn, the gap D6 and D7 left between them.
- **D1, D7, D10 corrected** — the open-only cost claim, the missing-data policy and
  evaluation order, and D10's closed-list enumeration of tier 2's role.

**Pass 2** closed items 1–7 and found two blockers *introduced by the revision itself*.
Both were verified against the tree before acting, and the first was found to be worse than
reported:
- **D3 and D9 cancelled each other.** Testing the raw rival list meant that any rival far
  enough out to have vetoed us also put the log outside the 120s slack, so removing the
  veto changed nothing. The reviewer judged the spec's own reproduced timeline safe on a
  rival offset of +28s; checking the second condition shows its first event at **+900s**,
  so the spec as revised **failed to fix the defect it reproduces**. D3 now tests the
  *effective* set — after D9 disqualification — which fixes that timeline.
- **"Empty rival set" had become a privilege granted by failure.** `rival_starts` returns an
  empty vector on an unreadable cwd, and the narrowing made empty re-enable the *unbounded*
  rescue — F2 on a failure path. **D12 added**: known-empty and unknown are different
  values, and D12 also pins the resolver parameter as a slice so the five bare `&[]`
  protected call sites keep compiling unchanged.
- The mechanical-retype enumeration was completed (ten `Watch.rivals` sites were missing),
  and Done-when #2 restated around the effective set.

**Pass 3** confirmed the effective-set fix works — it re-ran its own cancellation algebra
and traced the reproduced timeline to `A = Some(L, owned: true)` at +900s and `B = None`.
It then found three more, all verified against the tree before acting:
- **D9's residual mitigation had been silently voided by D3's revision.** The fork risk was
  accepted on the basis that D3's 120s slack bounded it; under the effective-set test a
  disqualified rival is not "present", so the same input removes both the veto *and* the
  slack guard and the residual becomes unbounded. **D13 added**, closing it at the source by
  comparing against the log's content `sessionId` — a signal
  `crates/ai-monitor/src/telemetry.rs:263-265` already parses, out of a head read
  resolution already pays for. This also corrects D8's reasoning: `cwd` genuinely cannot
  answer "which file", but that was taken as licence to stop looking, and the content id can.
- **D12's "exactly two paths" was wrong — there are three.** A rival whose *own* cwd is
  unreadable is dropped at `crates/app/src/tabsession.rs:338`, so a single such rival yields
  an empty list that D12 would have labelled known-empty, re-enabling the unbounded rescue
  from a failed read. Added to the unknown side.
- **The "complete" retype enumeration covered only `ai-monitor`.** All `crates/app` sites
  added — and one of them, `rival_starts_names_every_other_claude_in_the_same_project`,
  asserts `.is_empty()` for two of the paths D12 relabels. Rather than declare a contract
  exemption, D12 now attaches the known/unknown label at the `hint_for` boundary so
  `rival_starts` keeps its contract and those assertions stay unedited. Zero exemptions
  preserved.

Reviewer findings not accepted: none across three passes. Two incidental characterisations
were corrected — the +28s timeline is not in fact safe, and pass 1's claim that F1 was
fixable only within a 120s rival offset holds for the raw list, not for the effective set.

**Retry budget is exhausted** (`references/L2-decisions.md` allows 2). Pass 3's findings are
addressed above but have **not been re-reviewed**; D13 and D12's boundary placement are the
newest and least-scrutinised parts of this spec, and each earlier revision introduced a
defect the next pass caught. That risk is carried into L3 explicitly rather than presented
as a clean bill.

## Requirements

### R0: No tab is ever confirmed as the owner of a log it cannot account for
*From the Confirmed Goal, D2, D8.*

#### R0.1: The reproduced timeline resolves each tab to its own answer
- **Given**: a project dir with one log `L` whose first event is `T+900s`; tab A's `claude`
  started at `T` with no registry record; tab B's started at `T+28s`, is idle, and has a
  validated record naming `S_B` with no log on disk
- **When**: `resolve_owned_session` runs for A, then for B
- **Then**: A returns `Some(L)` with `owned: true`; B returns `None`. This is the timeline in
  `## Goal` and must exist as a named regression test.

#### R0.2: A confirmed result is never produced from an unconfirmable state
- **Given**: any input where no rival can be disqualified and nothing names the session
- **When**: `resolve_owned_session` runs
- **Then**: it returns `None` — never a `Resolved` with `owned: true`

#### R0.3: `Attachment::admit` and the 9 dashboard snapshots are untouched
- **Given**: the change as specified
- **When**: `cargo test --workspace` runs and `git status --porcelain crates/app/src/snapshots/`
  is checked
- **Then**: `admit`'s body is byte-unchanged and the snapshot check prints nothing

### R1: A rival vetoes only when it cannot be shown to be writing elsewhere
*From D1, D9, D13.*

#### R1.1: A record naming this log vetoes absolutely
- **Given**: candidate log `L` and a rival whose validated record names `L`'s session
- **When**: the veto is evaluated for `L`
- **Then**: `L` is refused for us regardless of every other filter, applied before the
  lead-time and slack tests

#### R1.2: A record naming another session does not veto
- **Given**: candidate `L` and a rival whose validated record names `S_other`, which is
  neither `L`'s content session id nor `L`'s filename stem
- **When**: the veto is evaluated for `L`
- **Then**: that rival does not veto `L` — whether or not `S_other.jsonl` exists on disk

#### R1.3: A rival with no usable record keeps the start-time veto
- **Given**: candidate `L` with first event `f`, our start `since`, and a rival with start `r`
  that has no record, or one that fails R3's validation
- **When**: the veto is evaluated
- **Then**: the existing rule applies unchanged — `L` is vetoed iff `r > since && r <= f`

#### R1.4: Disqualification is judged on the log's content session id
- **Given**: candidate `L` whose filename stem is `A` but whose lines carry
  `"sessionId":"B"`, and a rival whose validated record names `B`
- **When**: the veto is evaluated for `L`
- **Then**: the rival is treated as `L`'s writer (R1.1), **not** disqualified — a stem/id
  disagreement must not read as "a different session"

#### R1.5: An unreadable content id falls back to the stem and does not disqualify
- **Given**: candidate `L` whose head carries no session id, and a rival naming `S_other`
- **When**: the veto is evaluated
- **Then**: the stem is used, and the ambiguity resolves toward keeping the veto rather than
  taking `L`

### R2: The uniqueness rescue fires only on an effectively- and known-empty rival set
*From D3, D5.*

#### R2.1: Sole candidate with no rivals is adopted at any distance
- **Given**: exactly one candidate whose first event is 5h26m after our process start, and a
  known-empty rival set
- **When**: `resolve_owned_session` runs
- **Then**: the candidate is returned — `a_log_that_began_hours_after_the_process_is_not_this_tab_s`
  and `the_upper_bound_stops_at_120s_after_the_process` both keep passing **unedited**

#### R2.2: Sole candidate with a surviving rival must pass the slack
- **Given**: one candidate whose first event is `since + 900s`, and a rival R1 did not
  disqualify
- **When**: `resolve_owned_session` runs
- **Then**: `None` — the rescue does not fire and the 120s slack is enforced

#### R2.3: A rival disqualified by R1.2 does not block the rescue
- **Given**: one candidate at `since + 900s`, and one rival that R1.2 disqualifies
- **When**: `resolve_owned_session` runs
- **Then**: the candidate is returned with `owned: true`. This is the sub-req that separates
  the effective set from the raw list — a raw-list implementation returns `None` here and
  fails, which is the defect the L2 review caught.

#### R2.4: The rescue is unreachable when more than one candidate survives
- **Given**: two candidates surviving the veto
- **When**: `resolve_owned_session` runs
- **Then**: the slack-and-nearest path selects; no unbounded adopt occurs for any rival set

### R3: One validation rule governs our record and every rival's
*From D4.*

#### R3.1: A record predating its process is refused
- **Given**: a record whose `startedAt` is earlier than that pid's process start
- **When**: it is validated
- **Then**: it is discarded and the rival degrades to R1.3

#### R3.2: A record more than 60s after its process start is refused
- **Given**: a record whose `startedAt` exceeds the process start by more than
  `REGISTRY_DRIFT_MS`
- **When**: it is validated
- **Then**: it is discarded and the rival degrades to R1.3

#### R3.3: The rival rule and the own-record rule are one function
- **Given**: the implementation
- **When**: the validation is located
- **Then**: rivals and our own record validate through one shared function, not two copies —
  a duplicated rule is the defect mode this feature has already shipped twice

### R4: "No rivals" and "rivals unknown" are distinct; only the former enables the rescue
*From D12.*

#### R4.1: A fully enumerated empty set enables the rescue
- **Given**: the process table was read, every rival's cwd and start were readable, and no
  other `claude` runs in this project
- **When**: resolution runs with a distant sole candidate
- **Then**: the set is *known-empty* and R2.1 applies

#### R4.2: Our own unreadable cwd yields unknown
- **Given**: `hint_for` cannot read our `claude`'s cwd
  (`crates/app/src/tabsession.rs:332-334`)
- **When**: the hint is built
- **Then**: the rival set is labelled **unknown** and R2.2's slack enforcement applies

#### R4.3: A dropped rival yields unknown even when the list ends up empty
- **Given**: exactly one other `claude` in the project, whose cwd (line 338) **or** start
  time (line 339) cannot be read, so it is dropped and the list is empty
- **When**: the hint is built
- **Then**: the set is labelled **unknown**, not known-empty — the failure path that would
  otherwise grant the unbounded rescue from a read that failed

#### R4.4: `ProcTable::rival_starts` keeps its current contract
- **Given**: the D12 implementation
- **When**: `rival_starts_names_every_other_claude_in_the_same_project`
  (`crates/app/src/tabsession.rs:496-534`) runs
- **Then**: its `.is_empty()` assertions at 530 and 531-533 pass **unedited** — the label
  lives at the `hint_for` boundary, one level up

### R5: Rival data crosses the crate boundary as plain process facts
*From D7.*

#### R5.1: `app` supplies pid and process start; `ai-monitor` reads the record
- **Given**: a live rival `claude`
- **When**: the hint is built and passed to the resolver
- **Then**: `app` contributes `{pid, started_ms}` only; `ai-monitor` performs the
  `<pid>.json` read and R3's comparison itself

#### R5.2: `ai-monitor` gains no kernel-querying dependency
- **Given**: the change as specified
- **When**: `crates/ai-monitor/Cargo.toml` is inspected
- **Then**: dependencies remain `serde_json` + `notify` — no `libc`, no process enumeration

#### R5.3: The resolver's rival parameter stays a slice, never a handle
- **Given**: the retyped API
- **When**: the bare `&[]` call sites at `crates/ai-monitor/src/lib.rs:931, 942, 1094, 1120,
  1256` are compiled
- **Then**: the rival argument at each is still a **slice literal** (`&[]`, coercing to
  `&[Rival]`), with only the added known/unknown flag appended — the shared handle lives in
  `Watch` only, and no call site needs a lock, a clone, or a restructured body.
- **Amended during T1, and the first wording of this amendment overstated its case.** The
  sub-req originally demanded the sites compile *byte-unchanged*. That is unsatisfiable **for
  a single entry point**: D12 requires the known/unknown flag to reach the resolver, `&[]` is
  `&[_; 0]` and Rust applies no user-defined conversion at a call site (so a wrapper struct
  with `From`/`Into` cannot rescue it either), and there are no default arguments — the arity
  must change.
  It is **not** unsatisfiable in general, and the amendment first claimed it was. A second
  entry point — a 3-arg `resolve_owned_session` meaning "rivals known" plus a 4-arg form for
  the unknown case — would have kept all five sites byte-identical, since all five pass
  `true` today. That alternative is **rejected on design grounds, not impossibility**: D12's
  whole argument is that `known: false` must be the safe default, and a 3-arg convenience
  that defaults to `known: true` hands the resolver's strongest branch to a caller who simply
  forgot the flag. One entry point that forces the question is worth five retyped call sites.
  Recorded this way because "unsatisfiable" is a claim a later reviewer would check, and it
  did not survive being checked.

#### R5.4: Rival records are read once per resolution, not per candidate
- **Given**: 16 candidates and 3 rivals
- **When**: one re-resolution runs
- **Then**: at most 3 registry reads occur, not 48

### R6: An unattached watcher tracks both rival records and rival membership
*From D6, D11.*

#### R6.1: Records are re-read while unattached
- **Given**: an open panel that has not attached, and a rival that had no record at open
- **When**: that rival registers and an fs event fires
- **Then**: the next re-resolution sees the new record and applies R1.2 to it

#### R6.2: Membership is refreshed in both directions
- **Given**: an open, unattached panel
- **When**: a sibling starts `claude` in the same project, or an existing rival exits
- **Then**: the new rival is present in the next re-resolution and the exited one is absent

#### R6.3: Refresh stops once attached
- **Given**: a panel attached to a confirmed session
- **When**: further fs events fire
- **Then**: no registry reads and no membership reads occur

#### R6.4: The refresh never runs on the event-loop thread
- **Given**: the implementation
- **When**: the call sites are located
- **Then**: reads occur on the watch thread; `app` writes the shared list from the metadata
  worker; the lock is held only to clone, never across file I/O

### R7: The rules the code now enforces are written where the next reader will look
*From D5, D10.*

#### R7.1: "Unrivalled" is defined at the constant
- **Given**: `FIRST_EVENT_SLACK_MS`'s doc paragraph
- **When**: it is read after the change
- **Then**: it states that the exemption applies to a known-empty *effective* rival set,
  quotes the prior sentence, and says what changed

#### R7.2: Tier 2's role is stated without a closed list
- **Given**: the timing tier's doc comment
- **When**: it is read
- **Then**: it records tier 2 as a fallback, carries the measured latency (1.1–2.2s, `claude`
  2.1.220, 2026-08-02), names `crates/ai-monitor/src/lib.rs:66-69` as the reason it must
  exist, and does **not** enumerate tier 2's cases as exhaustive

#### R7.3: The stale protected-test list is corrected
- **Given**: `specs/ai-monitor-session-isolation/loop.md:37` naming two tests that no longer
  exist
- **When**: the change lands
- **Then**: that staleness is recorded rather than propagated

### R8: The protected surface is provably untouched
*From the Constraints — the check that the change stayed inside its blast radius.*

#### R8.1: The five ranked-fallback tests are byte-unchanged
- **Given**: the change
- **When**: the bodies of `a_named_session_wins_outright`,
  `a_session_started_by_this_tab_beats_a_more_active_foreign_one`,
  `two_tabs_on_one_project_get_their_own_sessions`,
  `an_empty_hint_degrades_to_ranking_and_claims_nothing` and
  `nothing_confirmable_resolves_to_none_rather_than_a_foreign_log` are diffed against `HEAD`
- **Then**: zero differences

#### R8.2: `resolve_session`'s behavior is unchanged for every input
- **Given**: `resolve_session` hard-codes `&[]` (`crates/ai-monitor/src/lib.rs:328`)
- **When**: the new gate is applied
- **Then**: the rescue is unconditionally enabled there, so its answer is identical for all
  inputs — recorded as a structural argument, not a per-test check

#### R8.3: No test assertion changes anywhere
- **Given**: the retype touches `crates/ai-monitor/src/lib.rs:1128, 1169, 1235, 1316`, the ten
  `Watch.rivals` sites, and the `crates/app` sites named in Constraints
- **When**: the diff is reviewed
- **Then**: every change is a type or literal retype; no `assert*` line changes meaning

#### R8.4: The gates stay green
- **Given**: the change
- **When**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` run
- **Then**: all three pass with zero warnings

## Tasks

DAG shape is dictated by file overlap: `crates/ai-monitor/src/lib.rs` carries the resolver,
`Watch`, `LogWatcher` and their tests, so every task touching it is serialized. The one
genuine parallel pair is the resolver rule (T2, `ai-monitor`) and the producer (T3, `app`),
which share no file once T1 has fixed the type contract between them.

### T1: Introduce `Rival` + the known/unknown carrier, and retype every call site [infra]
- **Fulfills**: R5.2, R5.3, R8.3, R8.4
- **Depends on**: (none)
- **Scope**: add the `Rival { pid, started_ms }` type and the known/unknown wrapper; change
  `resolve_owned_session`'s third parameter to a slice of it; change `Watch.rivals`; retype
  the four resolver call sites (`crates/ai-monitor/src/lib.rs:1128, 1169, 1235, 1316`), the
  ten `Watch.rivals` constructions (843, 1656, 1710, 1768, 1812, 1857, 1928, 2018, 2101,
  2165) and the `crates/app` sites (`overlay.rs:178, 179, 209`; `tabsession.rs:41, 76`).
- **Behavior must not change.** This task exists to separate mechanical churn from the
  behavioral change, which is what makes R8.3 checkable later: after T1 the full suite passes
  with **no assertion edited**, and the five bare `&[]` sites (931, 942, 1094, 1120, 1256)
  must still compile byte-unchanged — the check that R5.3's slice pin was honored.

### T2: The rival disqualification rule [vertical]
- **Fulfills**: R1, R3, R5.4
- **Depends on**: T1
- **Scope**: read each rival's `<pid>.json` once per resolution (not per candidate), validate
  it with the *same* function that validates our own record, and implement D9's three
  outcomes — absolute veto on a match, no veto on any other named session, unchanged
  start-time veto when no record is usable. Comparison is against the candidate's **content**
  session id with the filename stem as fallback (D13/R1.4, R1.5).
- **Note**: `SessionTelemetry` already parses `sessionId`/`session_id`
  (`crates/ai-monitor/src/telemetry.rs:263-265`) and `first_event` already reads each
  candidate's head (`crates/ai-monitor/src/lib.rs:339-352`) — extract the id from that
  existing read rather than adding a second one.

### T3: The `app` producer — process facts plus known/unknown [vertical]
- **Fulfills**: R4, R5.1
- **Depends on**: T1 — **parallel with T2** (disjoint files: `crates/app/src/tabsession.rs`
  vs `crates/ai-monitor/src/lib.rs`; the type contract is fixed by T1, so neither is the
  other's input)
- **Scope**: `SessionHint.rival_starts` becomes the `Rival` payload; `hint_for` labels the
  set known or unknown across all three drop paths — our own unreadable cwd
  (`crates/app/src/tabsession.rs:332-334`), a rival's unreadable cwd (line 338), a rival's
  unreadable start (line 339).
- **The label goes at the `hint_for` boundary, never inside `rival_starts`.** Its
  `.is_empty()` assertions (`crates/app/src/tabsession.rs:530, 531-533`) must pass unedited;
  if this task finds itself editing them, D12 has been implemented one level too low (R4.4).

### T4: The rescue gate — effective and known empty [vertical]
- **Fulfills**: R2, R0.1, R0.2
- **Depends on**: T2, T3
- **Scope**: gate the `after.len() == 1` arm on an effective rival set (computed *per
  candidate*, after T2's disqualification) that is also *known* (T3's label). Add the
  reproduced timeline as a named regression test.
- **R2.3 is the discriminating test**: one candidate at `since + 900s` with one rival that
  R1.2 disqualifies must return `Some(.., owned: true)`. An implementation that gates on the
  raw list returns `None` here — that is the defect the L2 review caught twice, and this test
  is the only thing that distinguishes the two implementations.

### T5: Watcher refresh — records and membership [vertical]
- **Fulfills**: R6, R0.3
- **Depends on**: T4
- **Scope**: while unattached, re-read rival records on each re-resolution; move the rival
  list behind the shared handle in `Watch` and have `app` republish it from the tabmeta
  worker so membership tracks in both directions. Lock held only to clone, never across file
  I/O; reads on the watch thread only; everything stops once `attached` is set.
- `Attachment::admit` is **not** edited (R0.3) — if this task needs to touch it, the resolver
  is still producing a result it should not.

### T6: Write down the rules the code now enforces [docs]
- **Fulfills**: R7, R8.1, R8.2
- **Depends on**: T5 (shares `crates/ai-monitor/src/lib.rs` with T2/T4/T5)
- **Scope**: sharpen `FIRST_EVENT_SLACK_MS`'s "unrivalled" paragraph with the prior sentence
  quoted (R7.1); state tier 2's role, the measured latency and
  `crates/ai-monitor/src/lib.rs:66-69` as its reason, without a closed list (R7.2); record
  the staleness of `specs/ai-monitor-session-isolation/loop.md:37` (R7.3); record the
  structural argument that `resolve_session` is unchanged because it hard-codes `&[]`
  (R8.2) and confirm the five protected bodies diff clean against `HEAD` (R8.1).

## External Dependencies

### Pre-work
- (none) — every input is local; no new crates, no network, no account or service setup.

### Post-work
- **Re-review D12 and D13.** Both were added on the L2 reviewer's final allowed pass and were
  never themselves reviewed. Each earlier revision of this spec introduced a defect the next
  pass caught, so these two should get an adversarial read before or during implementation.
- **Measure the fork case** (Known Gaps): start `claude --continue` in a project, read
  `~/.claude/sessions/<pid>.json`, and compare its `sessionId` against both the filename and
  the content of the `.jsonl` being appended to. Settles whether D13's content-id comparison
  is sufficient or merely better than the stem.
- **Re-check registry coverage** if `claude` updates past 2.1.220 — every latency figure in
  Research comes from that version on 2026-08-02, and D3's narrowing is what keeps the change
  safe if coverage regresses.
