# Spec: tabsession-tmux-reroot

## Meta
- **Created**: 2026-07-29
- **Type**: dev
- **Status**: planned
- **Approved by**: partition gate (`pre-approved-batch: yes`) — per-layer prompts bypassed
- **Approved at**: 2026-07-29
- **Set**: tmux-pane-resolution (`specs/tmux-pane-resolution/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single gate)
- **L2-reviewer**: cleared thresholds after 2 fix rounds (composite 0.86, all dimensions >= 0.70)
- **Complexity**: Medium (2 signals)
  - **External resource loading**: the `tmux list-clients` subprocess (via the sibling's
    resolver) added alongside the existing `/bin/ps` spawn — two sequential subprocesses on one
    call path.
  - **Multi-actor interaction**: the resolution's correctness depends on three parties whose
    views can disagree — the tab shell, the tmux pane, and the ai-monitor projects directory
    chosen by a *different* caller (`overlay.rs`). D4 and D7 exist entirely because of that
    disagreement.

## Declared Surface
- feature-id: tabsession-tmux-reroot
- depends_on: [tmux-root-resolution]
- declared-surface:
  - crates/app/src/tabsession.rs

## Goal
Make the ai-monitor overlay's session hint see a tmux tab's agent: in `hint_for`, resolve the
walk's root through the sibling's `effective_root_uncached` before searching for `claude`, so a
tab running tmux stops returning an empty hint and falling back to ranking the projects
directory — **without** ever returning a hint that would be confidently applied to the wrong
directory.

## Non-goals
- The resolution mechanism itself — it belongs to `tmux-root-resolution` and is consumed here.
- The tab-card derivation (`tabmeta/worker.rs`) — that is `tabmeta-tmux-reroot`.
- **Re-rooting the projects *directory*.** `overlay.rs:108` picks it from a separately-passed
  `cwd` originating at the tab shell. `overlay.rs` is in no feature's declared surface here, so
  this feature cannot change it — which bounds what it can deliver. See D4 and Known Gaps.
- Changing `SessionHint`'s shape, `resume_id`, `start_ms`, `looks_like_session_id`, or the
  `ProcTable` parsing contract.
- Widening `MAX_DEPTH` / `MAX_VISITED` (D3).

## Confirmed Goal
A pure, injectable `choose_claude(root, shell_pid, find, cwd_of) -> Option<u32>` extracted from
`hint_for`, which becomes a thin wrapper supplying `ProcTable::read()`, the bounded-wait root, and
the two field reads. Done when: a tmux tab whose pane `claude` has the tab shell's **identical
cwd** yields a confirmed hint; a pane `claude` working in a *different* directory is never
admitted as that confirmed answer — control falls through to the tab-subtree walk today's code
already performs, so the outcome is a ranked guess at worst, never a confidently wrong
attachment; a non-tmux tab behaves exactly as today; and a wedged tmux server costs the feature
rather than freezing the UI.

## Research
- **`hint_for` is three lines of orchestration** (`tabsession.rs:53-60`): read the process table,
  `find_claude(shell_pid)`, then read `resume_id` and `start_ms` off the found pid. The root pid
  enters at exactly one place — `table.find_claude(shell_pid)` at `:55`.
- **Today a tmux tab yields `None`, provably.** `find_claude` BFSs *descendants* of the tab shell
  (`:113-136`) over a `kids` index built from `ps` ppid (`:104`). The tmux **client** is the
  shell's child, but panes are children of the tmux **server** (measured: pane `74877`.ppid =
  `90662`), which is not in the tab's subtree; the client itself has no `claude` under it.
- **The pane's `claude` is a direct child of `pane_pid`** (measured `74877` → `74922`;
  `38200` → `38245`), so after re-rooting it is found at depth 1 — far inside `MAX_DEPTH = 6` /
  `MAX_VISITED = 512` (`:43-45`).
- **`hint_for` runs on the winit UI thread.** Its only caller is `Overlay::open`
  (`overlay.rs:100`), reached from `on_key` (`:317`) and `on_active_changed` (`:348`) — the
  latter re-opens on **every tab switch while the overlay is visible**, so this is not strictly
  once-per-deliberate-action.
- **The fallback is *event-time* ranking, not mtime.** `most_recent_session`
  (`ai-monitor/src/lib.rs:78-92`, `:207-222`) ranks by "the log whose newest parseable event is
  the most recent" and its doc says explicitly **"Not the newest mtime."** mtime only shortlists
  `RESOLVE_CANDIDATES = 16` files and breaks ties. The manifest's accept-seed for this feature
  says "mtime-ranking"; that wording is wrong (Known Gaps).
- **A wrong-directory `session_id` does NOT fall through to ranking — it falls into the
  `since_ms` branch.** `resolve_session` (`lib.rs:130-168`): the id path returns early only when
  `dir.join("{id}.jsonl").is_file()`; otherwise control reaches `if let Some(since) = want.since_ms`
  (`:144`). That filter is **one-sided** — `(first + FIRST_EVENT_SLACK_MS >= since)` (`:149`)
  admits every log whose first event is later than `since` by *any* amount — and `min_by_key`
  then returns the nearest with **`owned: true`** (`:156-161`). This is the mechanism behind D7.
- **`owned: true` is latched by the caller.** `overlay.rs:122-130` records the attachment and
  seeds `attached_flag`, so the watcher stops re-resolving — a wrong confirmed answer is sticky.
- **The scope limit.** `Overlay::open(cwd, shell_pid)` (`overlay.rs:88`) uses `shell_pid` for the
  hint but `cwd` for the directory (`:108`), and that `cwd` is `tabs.rs:220 active_cwd()` →
  `session.rs:171 cwd_of_pid(self.pty.child_pid()?)` — the **tab shell's** cwd.
- **`tabsession` is a crate-root module** (`main.rs:24`), not a child of `tabmeta`. So the sibling
  resolver is reached as `crate::tabmeta::tmux::…`, and it must be declared `pub(crate) mod tmux;`
  — the sibling's R8.3 now requires exactly that, since `tabmeta`'s other modules are private
  (`tabmeta/mod.rs:17-21`).
- **The module already does raw `proc_pidinfo`** for `start_ms` (`:173-199`), so adding one more
  small `libc` read in-surface matches its existing style and needs no cross-module visibility.
- **Identification is by argv, not process name** (`:66-71`, `:138-147`) — Claude Code renames
  itself to its version. Re-rooting does not disturb this.

## Decisions

The six `SD<n>` entries are inherited verbatim from the set's partition manifest and apply to
every feature in the set; this feature's local decisions follow. (The manifest text is
reproduced in `specs/tmux-root-resolution/spec.md`.)

### D1: Split into a pure, injectable `choose_claude` and a thin `hint_for` wrapper
- **Status**: resolved
- **Rationale**: SD5 applied to **this feature's own code**, not just to the pre-existing pure
  helpers. As written today every new behavior would be unobservable: `hint_for` takes a `u32`
  and immediately reaches for a subprocess and the real resolver, with no seam and no access to
  `tabmeta`'s probe counters (out of surface). Extracting a core that is **generic over its two
  impure operations** — mirroring `agent::walk`'s generic children/classifier idiom, this
  codebase's established seam (`agent.rs:149-156`) — and which **returns the chosen pid, not a
  built `SessionHint`**:
  ```rust
  fn choose_claude(
      root: u32,
      shell_pid: u32,
      find: impl Fn(u32) -> Option<u32>,        // normally |p| table.find_claude(p)
      cwd_of: impl Fn(u32) -> Option<PathBuf>,  // normally the local proc_pidinfo reader
  ) -> Option<u32>
  ```
  Returning the pid is what makes the core **genuinely free of I/O**. Building a `SessionHint`
  would force it to call `start_ms(pid)` — a raw `proc_pidinfo` syscall (`:173-199`) — which no
  closure injects, and which would make the table tests *machine-dependent*: a hand-built pid
  colliding with a live process would flip `started_ms` from `None` to `Some(..)`. With the pid
  returned instead, every R1/R4 assertion is about which pid was chosen, and the two field reads
  stay in the impure wrapper where D5 can be verified directly. It also drops `table` from the
  signature entirely, since `find` already closes over it.
  The seam makes the *non-occurrence* claims observable: a test closure counts its own
  invocations, which is the only way R1.4 ("`find` called exactly once") and R4.3 ("no cwd
  comparison") can be asserted at all, since `tabmeta`'s probe counters are out of surface.
  `hint_for` then reduces to the impure wrapper:
  ```rust
  pub fn hint_for(shell_pid: u32) -> Option<SessionHint> {
      let table = ProcTable::read()?;
      let root = resolved_root(shell_pid);   // D8's bounded wait around the sibling's resolver
      let claude = choose_claude(root, shell_pid, |p| table.find_claude(p), cwd_of_pid)?;
      Some(SessionHint {
          session_id: table.resume_id(claude),
          started_ms: start_ms(claude),
      })
  }
  ```
  (`&table` and the `find` closure are both *shared* borrows — `find_claude` takes `&self` — so
  they coexist.) The module path used inside `resolved_root` is `crate::tabmeta::tmux` (not
  `super::tmux`): `tabsession` is a crate-root module, so `super::` is the crate root. This
  requires the sibling to export `pub(crate) mod tmux;` — pinned as its R8.3 and under Pre-work.
  **Rejected: leaving `hint_for` monolithic** — R1 would be unverifiable and SD5 would be
  satisfied only by tests of code this feature did not write. **Rejected: returning
  `Option<SessionHint>`** — it drags an uninjected syscall into the core, as above.
  **Rejected: a non-generic core** — `find_claude` is an inherent method and both walks are
  idempotent, so no black-box assertion could distinguish one call from two.

### D2: Prefer the pane, fall back to the tab's own subtree — with the gate on the pane branch only
- **Status**: resolved
- **Rationale**: the fallback and D7's gate are **one interleaved control flow**, published here
  once so they cannot be specified separately and drift:
  ```rust
  // The pane branch: only entered when a pane was genuinely resolved, and its
  // result is admitted only if D7's cwd gate agrees.
  if root != shell_pid {
      if let Some(pid) = find(root) {
          if let (Some(a), Some(b)) = (cwd_of(pid), cwd_of(shell_pid)) {
              if a == b {
                  return Some(pid);
              }
          }
          // Divergent, or either cwd unreadable: fall through to the tab's own
          // subtree — which is exactly what today's code searches, so this path
          // is never worse than today.
      }
  }
  find(shell_pid)   // unchanged from today, and never gated
  ```
  Three properties this shape guarantees, each of which a separated specification got wrong:
  **(a)** `effective_root_uncached` returns `shell_pid` on any failure, so `root != shell_pid` is
  true only when a pane was genuinely resolved — the guard suppresses a pointless second
  identical walk in the common non-tmux case (R1.4).
  **(b)** The gate applies to the **pane branch only**. A `claude` found in the tab shell's own
  subtree is about the tab's project by construction, so gating it on the *pane's* cwd would
  suppress a hint today's code returns — a regression. This is why the gate lives inside the
  `if root != shell_pid` block rather than wrapping the final result.
  **(c)** Every non-admitting path **falls through** rather than returning `None`, so the
  guarantee is exact: every pid this function could find before the change, it still finds.
  **Rejected: searching only the pane** — a tmux tab whose pane is idle but whose shell has an
  agent elsewhere would regress to no hint; **rejected: tab-first** — the pane is the more
  specific answer and must win when both exist and agree.

### D3: The walk's bounds are unchanged
- **Status**: resolved
- **Rationale**: `MAX_DEPTH = 6` / `MAX_VISITED = 512` were sized for `zsh → npm → node → claude`
  chains; the pane's `claude` sits at depth **1** below the new root, so re-rooting makes the
  search strictly shallower. `MAX_VISITED` is a **local counter inside each `find_claude` call**
  (`:115`, reset per invocation), so D2's fallback does not share a budget with the first walk —
  the pair is bounded at `2 × MAX_VISITED` **because they are two independent bounded calls**,
  not because one budget was doubled. Stated precisely because the distinction is what makes the
  fallback safe. **Rejected: raising the bounds** — nothing here needs more depth, and they exist
  to stop a panel toggle traversing every process on the machine.

### D4: This feature does not re-root the projects directory — and names the one-file fix
- **Status**: resolved
- **Rationale**: the accept-seed's *primary* clause — "`hint_for` resolves the session from the
  active pane's `claude`" — **is** fully deliverable from this surface, and D1/D2 deliver it. Only
  its trailing consequence clause ("instead of falling back to ranking") is partial, and only for
  a pane whose project differs from the tab shell's, because `Overlay::open` derives the directory
  from a `cwd` this feature cannot reach (`overlay.rs:88`, `:108`). The **minimal** fix is a single
  file — `overlay.rs`, deriving `dir` from `cwd_of_pid(effective_root_uncached(shell_pid))` rather
  than the passed `cwd` — not the four-file chain (`session.rs`/`tabs.rs`/`terminal_app.rs`/`seam.rs`)
  that feeds it, since only the overlay's own directory choice needs to change. `overlay.rs` is in
  **no feature's declared surface** in this partition, so this is surfaced to the human as a
  partition-completeness question with its real cost stated. **Rejected: widening this feature's
  surface unilaterally** — the partition was approved at decompose's single human gate.

### D5: `resume_id` and `start_ms` are read off whichever pid the walk returned
- **Status**: resolved
- **Rationale**: `:56-58` already reads both off `claude`, so they follow the re-root with no
  edit. This is SD2 holding at this consumer: the pane's `claude` names its session via
  `--resume` or dates it via its own start time, exactly as the tab's own would. Pinned so an
  implementer does not "helpfully" keep `start_ms` on the shell pid.

### D6: The hop runs after `ProcTable::read()`, and the resulting snapshot race is accepted
- **Status**: resolved
- **Rationale**: placing it after preserves the existing early return on an unreadable process
  table, and lets `choose_claude` stay pure, over a table the wrapper already owns (D1). The cost is a
  **snapshot race**: the table is captured at T0 and the pane pid resolved at T1, so a pane —
  or a `claude` — that appears in that window is absent from `rows`/`kids`, `find_claude(root)`
  returns `None`, and D2's fallback silently searches the tab subtree instead. The window is
  **the resolver's full latency — a presence gate plus a `tmux list-clients` spawn, bounded by
  D8's `RESOLVE_BUDGET`** — not microseconds; being honest about the magnitude matters because
  the acceptance rests on it. It is still accepted: the consequence is one stale hint on an
  overlay open that the user can simply reopen. **Rejected: resolving the root first, then
  reading the table** — it inverts the race (root stale instead of table stale) without removing
  it, and costs the tmux spawn even when the process table cannot be read at all.
  Also stated here because D1's shape makes it easy to miss: the tmux path now pays **two
  sequential subprocess spawns** inline — `/bin/ps` plus `tmux list-clients` — along with the
  sibling's liveness `cwd_of_pid` and its N tty `stat`s. See D8 for the thread that pays them.

### D7: Admit a pane result only on a positive cwd match; otherwise fall through to the tab walk
- **Status**: resolved
- **Rationale**: **this is the decision that prevents a regression**, and it is not optional.
  Without it the change is *strictly worse than today* for a cross-project pane. Today such a tab
  yields `None` → `Wanted::default()` → `resolve_session` reaches `most_recent_session` and
  returns **`owned: false`**. After a naive re-root it yields the pane's `session_id` (naming a
  file that is not in the tab shell's directory, so the id path does not return) **and** the
  pane's `started_ms` — which drops into the `since_ms` branch (`lib.rs:144-162`) against the
  *wrong* directory. That filter is one-sided (`first + SLACK >= since`, `:149`), so it admits
  every log started after the pane and `min_by_key` returns the nearest with **`owned: true`** —
  and `overlay.rs:122-130` latches it, stopping the watcher from re-resolving. A ranked guess
  would have been replaced by a *confidently wrong, sticky* attachment, violating SD4 outright.
  The gate, whose exact position in the control flow is published in D2:
  **compare the cwd of the found `claude` process against the tab shell's cwd, and admit the
  pane result only on a positive `Some(a) == Some(b)` match.**
  Three details are load-bearing:
  **(a) The compared pid is the found `claude`, not the pane root.** `root` is the pane's `-zsh`,
  whose cwd moves with every `cd`, whereas the projects directory that matters is the one
  `claude` itself is writing into (`encode_cwd` of *its* cwd). Comparing the shell would fail
  both ways: a pane that started `claude` in the tab's project then `cd`'d away would have a
  valid hint suppressed, and one that started `claude` elsewhere then `cd`'d into the tab's
  project would have the false confirm admitted.
  **(b) An unreadable cwd on either side does not admit.** Both sides come from a fallible
  reader (`Option<PathBuf>`; `n < size → None`, as `session.rs:244` shows), so a bare `!=` would
  conflate "different project" with "could not read" — and worse, `None == None` would *admit*
  the very case the gate exists to block. Only `(Some(a), Some(b))` with `a == b` admits;
  everything else falls through to the tab-subtree walk. This is safe by construction: a tmux
  tab's tab-subtree walk yields `None` today anyway, so falling through is exactly today's
  behavior, never a regression.
  **(c) Equality is byte-identical cwd, not "same project".** A pane in a *subdirectory* of the
  tab's project is therefore suppressed — which is **correct**, because Claude writes to
  `encode_cwd(subdir)`, a genuinely different projects directory. The requirements say
  "identical cwd" rather than "same project" for this reason.
  The check runs **only on the pane branch** (D2b), so non-tmux tabs pay nothing. Both cwds are
  read with the module's existing raw `proc_pidinfo` style (`:173-199`) — note this is a
  deliberate **third** in-repo copy of `cwd_of_pid` (`session.rs:232` is private;
  `tabmeta::proc::cwd_of_pid:26` is `pub(crate)` but sits inside a *private* `mod proc;`), forced
  by the partition boundary rather than by carelessness. **Rejected: emitting `session_id` but
  suppressing `started_ms`** — an id naming no file in the directory is inert, so it is nearly
  equivalent, but it leaves a populated hint whose meaning depends on a directory the hint does
  not know; **rejected: accepting the false confirm** — it is a regression, and SD4 forbids being
  worse than not having looked.

### D8: The UI-thread hang exposure is owned here, bounded, and surfaced
- **Status**: resolved
- **Rationale**: the sibling's D8 accepted an un-timed-out `Command::output()` on the grounds
  that its blast radius is the metadata **worker thread**, and explicitly deferred this
  consumer's placement to this spec — so the obligation is discharged here rather than inherited
  silently. The facts: `hint_for` runs on the **winit event-loop thread** (`overlay.rs:317`,
  `:348`), and `on_active_changed` re-enters it on every tab switch while the overlay is open.
  `/bin/ps` — the existing precedent — is kernel-backed and always terminates; `tmux
  list-clients` talks to a **user-space server over a unix socket** and can block indefinitely
  (a SIGSTOPped server, a wedged `$TMUX_TMPDIR`). If that happens the whole UI freezes — input
  and rendering, not just the overlay. That is not acceptable, and — contrary to an earlier
  draft of this decision — the mitigation **is** in-surface. `resolved_root` wraps the sibling's
  resolver in a bounded wait, entirely within `tabsession.rs`:
  ```rust
  fn resolved_root(shell_pid: u32) -> u32 {
      let (tx, rx) = std::sync::mpsc::channel();
      std::thread::spawn(move || {
          let _ = tx.send(crate::tabmeta::tmux::effective_root_uncached(shell_pid));
      });
      rx.recv_timeout(RESOLVE_BUDGET).unwrap_or(shell_pid)
  }
  ```
  with `RESOLVE_BUDGET = 250ms` — generous for a local unix-socket query, imperceptible on a
  keypress. On expiry it falls back to `shell_pid`, which SD4 already licenses as the
  "no re-root" answer, so a hung tmux costs the *feature*, never the UI. This is the
  threads-plus-channels idiom `docs/architecture.md` D7 already mandates for exactly this reason.
  The honest cost: a hung server leaks one blocked thread per attempt, and the wrapper adds a
  failure mode to a path whose contract is otherwise "return `None`" — both accepted, since a
  leaked thread parked on a socket read is vastly cheaper than a frozen terminal.
  This does **not** dispose of the better alternative, which deserves naming rather than a
  strawman rejection: the `tabmeta` worker **already** resolves this exact pid every 500 ms for
  the active tab and publishes per-tab metadata to the UI through an `Arc`-shared cache
  (`tabmeta/mod.rs:1-9`), so the overlay could *read* a published root instead of computing one —
  zero spawn, zero thread, zero UI-thread I/O. Realizing it needs `worker.rs` + `mod.rs` (to
  publish the root) and `overlay.rs` (to read it): two sibling surfaces plus an unassigned file.
  **Surfaced to the human as a partition-completeness question**, alongside D4.

### D9: The injected core is fully automated; only the wrapper's three real calls are manual
- **Status**: resolved
- **Rationale**: after D1's split, **R1–R5 are automated** — table tests over a hand-built
  `ProcTable` with injected `find`, `cwd_of` and (for R5) `resolve` closures, requiring no tmux,
  no subprocess, no live tty and no live pid. That covers the tmux-shaped traversal premise (R3),
  the fallback order and single-walk guard (R1.2–R1.4), the field source (R1.5), all six cwd-gate
  cases including the `None` semantics (R4.1, R4.3–R4.6), and the budget behavior (R5). What
  remains manual is exactly the wrapper's three genuinely impure calls — `ProcTable::read`,
  `resolved_root`'s real resolver, and the production `cwd_of_pid` — plus R0's user-visible
  outcome and R4.2's end-to-end `owned == false` consequence in the overlay. Those go to the
  `/run-term` gate. **Rejected: a test that shells out to a real tmux** — environment-dependent,
  the opposite of this workspace's headless-testability invariant.

## Constraints
- **Ordered dependency, not a shared surface**: this feature *consumes*
  `crate::tabmeta::tmux::effective_root_uncached`; it must not modify `tmux.rs`, `proc.rs`,
  `agent.rs`, `mod.rs` or `worker.rs` — nor `overlay.rs`, `session.rs`, `tabs.rs`,
  `terminal_app.rs` or `seam.rs`, none of which this partition assigns.
- **Read-only observation** — the module's stated contract (`:16-17`). The tmux hop preserves it:
  `list-clients` is a query.
- **Every failure yields `None`** (`:50-52`) — a dead pid, a permission refusal, an unreadable
  process table. Note two of this feature's own conditions are **fall-throughs, not `None`s**:
  D8's resolve-budget expiry falls back to `shell_pid` (so the walk proceeds unrooted), and D7's
  cwd divergence falls through to the ungated tab-subtree walk — which may still return a valid
  hint. **An unreadable cwd counts as a failure to agree, not as agreement** (D7b): only
  `(Some(a), Some(b))` with `a == b` admits a pane result; `None` on either side — including both
  — falls through.
- **No new dependency**; **`term-core` learns nothing**; **macOS ARM64 only**; **no `unwrap()`
  outside tests**; **`cargo clippy --workspace --all-targets -- -D warnings` must pass**.
- **Every pre-existing `tabsession.rs` test keeps its original assertions.**

## Known Gaps
- **The manifest's accept-seed says "mtime-ranking"; the fallback is *event-time* ranking**
  (`lib.rs:78-92` — "Not the newest mtime"). The error originates in the brief and propagated
  into the manifest. Flagged for human correction in
  `specs/tmux-pane-resolution/partition-manifest.md`; deliberately **not** silently rewritten,
  matching how the sibling handled SD1's `devname` error.
- **Cross-directory panes get no *pane* hint** (D7) — the pane result is not admitted and the
  tab-subtree walk runs instead, which for a tmux tab normally yields nothing, so in practice the
  overlay ranks as it does today. Correct and safe, but the *benefit* is unavailable there. The one-file fix is `overlay.rs` (D4) — **surfaced to the human as a
  partition-completeness question**, best decided together with `tabmeta-tmux-reroot`'s
  title/branch gap and D8's published-root alternative.
- **The UI-thread subprocess is time-bounded, not eliminated** (D8). A hung tmux server now costs
  the feature (fallback to `shell_pid` after `RESOLVE_BUDGET`) rather than the UI, but it leaks
  one blocked thread per attempt. The alternative that removes the spawn entirely — reading a
  root the `tabmeta` worker already publishes — spans two sibling surfaces plus `overlay.rs` and
  is **surfaced to the human**.
- **A pane in a *subdirectory* of the tab's project gets no hint** (D7c). Correct — Claude writes
  to a different projects directory — but it narrows R0's reach more than "same project" suggests.
- **The `ProcTable` snapshot race** (D6) can cost one stale hint on an overlay open.
- **Two sequential spawns on the overlay-open path** for tmux tabs (`/bin/ps` + `list-clients`).
- **Nested tmux and multi-pane clients** resolve to the client's active pane only.

## Requirements

### R0: A tmux tab's overlay resolves its pane's session *(manual gate, D9)*

#### R0.1: A tmux tab whose pane `claude` has the tab shell's identical cwd yields a non-empty hint
- **Given**: a tab whose tmux pane runs `claude` whose cwd is **byte-identical** to the tab shell's (a subdirectory does not qualify — see D7c)
- **When**: the overlay opens and `hint_for` runs
- **Then**: it returns `Some(SessionHint)` with `is_empty() == false`, rather than today's `None`

#### R0.2: A resumed pane session is named outright
- **Given**: the pane's `claude` was started as `claude --resume <uuid>`
- **When**: `hint_for` runs
- **Then**: `session_id` is that uuid, so `resolve_session` names the log instead of ranking

#### R0.3: A fresh pane session is dated
- **Given**: the pane's `claude` was started without `--resume`, with the tab shell's identical cwd
- **When**: `hint_for` runs
- **Then**: `started_ms` is that process's start time, and `resolve_session` confirms ownership via the `since_ms` branch — which is safe here precisely because D7 has already established that the directory matches

#### R0.4: The overlay stops ranking for the same-project case
- **Given**: the tab of R0.1
- **When**: the overlay resolves its log
- **Then**: `Resolved.owned == true` — confirmed, not merely the most recently active file

### R1: The hop, the fallback, and the field source (D1, D2, D5)

#### R1.1: `hint_for` is a thin wrapper over a pure, injectable core
- **Given**: the post-change file
- **When**: `hint_for` is inspected
- **Then**: it reads the process table, resolves the root via `resolved_root` (D8's bounded wait around `crate::tabmeta::tmux::effective_root_uncached`), delegates the choice to `choose_claude(root, shell_pid, find, cwd_of)`, and builds the `SessionHint` from the returned pid with `table.resume_id` and `start_ms` — so `choose_claude` performs **no I/O of its own**, every syscall reaching it through an injected closure, including D7's cwd gate

#### R1.2: The pane wins when both subtrees have an agent
- **Given**: a hand-built `ProcTable` with `claude` under both the pane root and the tab shell
- **When**: `choose_claude(pane_root, shell_pid, find, cwd_of)` runs
- **Then**: the **pane's** `claude` pid is used

#### R1.3: The tab subtree is retried when the pane has no agent, and is never cwd-gated
- **Given**: a table whose pane root has no `claude` but whose tab shell subtree does, with an injected `cwd_of` that would report a *divergent* cwd for the pane root
- **When**: `choose_claude` runs
- **Then**: the tab subtree's `claude` is returned — the gate applies to the pane branch only (D2b), so a tab-provenance hit is never suppressed; no regression against today

#### R1.4: No retry when nothing was re-rooted
- **Given**: `root == shell_pid` (the non-tmux case), a table with no `claude` anywhere, and a `find` closure that counts its invocations
- **When**: `choose_claude` runs
- **Then**: the counter reads exactly 1 — the `root != shell_pid` guard suppressed an identical second walk

#### R1.5: The chosen pid is the pane's, and the wrapper reads both fields off it
- **Given**: a pane `claude` at pid P carrying `--resume <uuid>`, with the tab shell's identical cwd
- **When**: `choose_claude` runs, then the wrapper builds the hint
- **Then**: `choose_claude` returns **P** (assertable purely), and the wrapper reads both `resume_id` and `started_ms` from **P** rather than from the shell pid (D5) — the split is what lets the first half be a table test and the second a one-line inspection

### R2: Bounds and failure behavior are unchanged (D3)

#### R2.1: The walk's bounds are untouched
- **Given**: the post-change file
- **When**: `MAX_DEPTH` and `MAX_VISITED` are inspected
- **Then**: they are still 6 and 512

#### R2.2: Each walk is independently bounded
- **Given**: a pathological table where both the pane and the tab subtrees are huge
- **When**: `choose_claude` runs with the D2 fallback
- **Then**: each `find_claude` call terminates within its own per-call `MAX_VISITED` budget, so the pair returns after at most `2 × MAX_VISITED` visits

#### R2.3: A dead pid still yields `None` without panicking
- **Given**: `hint_for(u32::MAX)`
- **When**: it runs
- **Then**: it returns `None`, as `a_real_lookup_on_a_dead_pid_is_none` already asserts

#### R2.4: Every pre-existing test passes unmodified
- **Given**: the existing `tabsession.rs` test module
- **When**: the suite runs
- **Then**: all of it passes with original assertions — parsing, resume-id spellings, argv-not-comm identification, cycle termination, start-time sanity, session-id validation

### R3: The traversal premise is pinned by a pure test (D9)

#### R3.1: A tmux-shaped table finds nothing from the tab shell
- **Given**: a `ProcTable::parse` sample with a tab shell whose child is `tmux`, and a pane shell parented to a separate tmux **server** pid with `claude` beneath it
- **When**: `find_claude(tab_shell)` is called
- **Then**: it returns `None` — documenting the defect this feature fixes

#### R3.2: The same table finds the agent from the pane
- **Given**: the same sample
- **When**: `find_claude(pane_shell)` is called
- **Then**: it returns the pane's `claude` pid — the premise the hop relies on

### R4: No confident wrong answer for a cross-project pane (D7)

#### R4.1: A divergent `claude` cwd suppresses the pane result
- **Given**: a resolved pane whose **found `claude`** has a cwd differing from the tab shell's, and no `claude` in the tab shell's own subtree
- **When**: `choose_claude` runs
- **Then**: it returns `None` — never a `SessionHint` carrying the pane's `started_ms`

#### R4.2: The suppressed case degrades to exactly today's behavior
- **Given**: the same tab
- **When**: the overlay resolves
- **Then**: `Wanted::default()` is used, `resolve_session` reaches `most_recent_session`, and `Resolved.owned == false` — a ranked guess, never a confirmed wrong attachment

#### R4.3: The gate costs nothing for a non-tmux tab
- **Given**: `root == shell_pid` and a `cwd_of` closure that counts its invocations
- **When**: `choose_claude` runs
- **Then**: the counter reads 0 — the gate is inside the `root != shell_pid` branch

#### R4.4: An identical cwd is not suppressed
- **Given**: a resolved pane whose found `claude` has a cwd byte-identical to the tab shell's
- **When**: `choose_claude` runs
- **Then**: the full hint is returned, so R0.1–R0.4 hold

#### R4.5: An unreadable cwd on either side does not admit the pane result
- **Given**: a resolved pane whose found `claude` is real but whose `cwd_of` returns `None` — and separately, the case where **both** sides return `None`
- **When**: `choose_claude` runs
- **Then**: in both cases the pane result is **not** admitted; control falls through to the tab-subtree walk, so `None == None` never counts as agreement

#### R4.6: The gate compares the found `claude`, not the pane shell
- **Given**: a pane whose shell has cwd X but whose `claude` has cwd Y, where the tab shell's cwd is Y
- **When**: `choose_claude` runs
- **Then**: the hint is **admitted** — the comparison used the `claude`'s cwd; the mirror case (shell Y, `claude` X, tab Y) is suppressed

### R5: The resolve budget bounds the UI thread (D8)

#### R5.1: A hung resolver degrades to the tab's own pid within the budget
- **Given**: `resolved_root_with` injected with a resolver closure that sleeps well past `RESOLVE_BUDGET`
- **When**: it is called with `shell_pid`
- **Then**: it returns `shell_pid` after roughly `RESOLVE_BUDGET` — it returns at all, which is the property that keeps a wedged tmux server from freezing the UI

#### R5.2: A fast resolver's value is used
- **Given**: an injected resolver returning a pane pid promptly
- **When**: `resolved_root_with` is called
- **Then**: it returns that pane pid, not `shell_pid` — the bound does not cost the feature in the normal case

#### R5.3: A merely slow resolver degrades rather than misreports
- **Given**: a resolver slower than the budget but not hung (a loaded machine)
- **When**: `resolved_root_with` is called repeatedly
- **Then**: it returns `shell_pid` on the slow calls — so the hint is *intermittent* under load, never wrong

### Decision → requirement coverage

| Decision | Covered by |
|---|---|
| SD2 (re-root, don't extend) | R3.1, R3.2, R2.1 |
| SD4 (absent on failure) | R1.3, R2.3, R4.1, R4.2 |
| SD5 (pure core + thin wrapper) | R1.1, and every table test in R1–R4 |
| D1 (`choose_claude` split, module path) | R1.1 |
| D2 (fallback order + guard) | R1.2, R1.3, R1.4 |
| D3 (bounds unchanged, per-call budget) | R2.1, R2.2 |
| D4 (directory not re-rooted) | R0.4 (same-project only), R4.2 |
| D5 (fields from the found pid) | R1.5 |
| D6 (placement + snapshot race) | R1.1; the race itself is a Known Gap |
| D7 (cwd gate: found-`claude`, positive-match-only) | R4.1–R4.6 |
| D8 (bounded wait; published-root alternative) | R1.1; the alternative surfaced to the human |
| D9 (what is testable) | R1–R4 automated; R0 routed to the manual gate |

## Tasks

### T1: Extract the injectable `choose_claude`, with the traversal and fallback tests [vertical]
- **Fulfills**: R1, R2, R3 — and R0 structurally (manual gate per D9)
- **Depends on**: (none within this feature; the feature depends on `tmux-root-resolution`)
- Split `hint_for` into `choose_claude(root, shell_pid, find, cwd_of) -> Option<u32>` — generic over
  both impure operations per D1 — plus the thin wrapper, implementing **D2's published control
  flow** (pane branch with the gate, fall-through to the ungated tab walk). Leave `ProcTable`,
  the bounds, `resume_id`, `start_ms` and `looks_like_session_id` untouched. Lands with its
  tests: the extended tmux-shaped `SAMPLE` and both `find_claude` assertions (R3.1, R3.2), the
  fallback order (R1.2, R1.3), the counting-closure single-walk guard (R1.4), and the field
  source (R1.5).

### T2: The cwd gate and its counting tests [vertical]
- **Fulfills**: R4
- **Depends on**: T1
- Add the local raw-`proc_pidinfo` `cwd_of_pid` reader and wire it as the production `cwd_of`.
  Because T1 already placed the gate inside `choose_claude`'s pane branch, **all six R4
  sub-requirements are automated** here as table tests with injected closures — divergent (R4.1),
  identical (R4.4), either-side-`None` and both-`None` (R4.5), found-`claude`-not-pane-shell
  (R4.6), and the counting closure proving the gate is never evaluated when `root == shell_pid`
  (R4.3). Only R4.2's end-to-end consequence (`owned == false` in the overlay) needs the manual
  gate.

### T3: The bounded-wait wrapper [vertical]
- **Fulfills**: R5
- **Depends on**: T1
- Implement `resolved_root_with(resolve: impl FnOnce(u32) -> u32 + Send + 'static, shell_pid) -> u32`
  — thread + `recv_timeout(RESOLVE_BUDGET)` falling back to `shell_pid` — with `resolved_root`
  as the thin binding that passes the real
  `crate::tabmeta::tmux::effective_root_uncached`. The injected-resolver seam is what makes R5.1–R5.3
  assertable without hanging the suite. **Parallel with T2** in principle, but same file — serialize.

**DAG**: `T1 → T2 → T3` (one file, serialized).

## External Dependencies

### Pre-work
- `tmux-root-resolution` must be complete **and must export its resolver across the module
  boundary**: `pub(crate) mod tmux;` in `tabmeta/mod.rs` plus `pub(crate) fn
  effective_root_uncached` (pinned as that feature's R8.3). `tabmeta`'s other modules are
  private, and `tabsession` is a crate-root module, so without this the call does not resolve and
  this feature would be forced to edit `tabmeta/mod.rs` — a partition violation caused by a gap
  in its dependency rather than by its own scope.

### Post-work
- **Manual verification via `/run-term`**: open a tab, start tmux, run `claude` in a pane of the
  *same* project, and open the ai-monitor overlay — it should attach to the pane's session
  (`owned`) rather than ranking. Then repeat with the pane in a *different* project and confirm
  the overlay falls back to an unowned ranked guess rather than confidently attaching.
- **Human decisions required** (all in Known Gaps, none self-applied): correct the accept-seed's
  "mtime-ranking" wording; decide the projects-directory scope limit (D4 — one file,
  `overlay.rs`); and weigh D8's published-root alternative, which would remove the UI-thread
  subprocess entirely but spans two sibling surfaces plus `overlay.rs`.
