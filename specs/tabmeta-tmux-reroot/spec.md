# Spec: tabmeta-tmux-reroot

## Meta
- **Created**: 2026-07-29
- **Type**: dev
- **Status**: planned
- **Approved by**: partition gate (`pre-approved-batch: yes`) — per-layer prompts bypassed
- **Approved at**: 2026-07-29
- **Set**: tmux-pane-resolution (`specs/tmux-pane-resolution/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single gate)
- **L2-reviewer**: PASS after 2 fix rounds (composite 0.906)
- **Complexity**: Moderate (3 signals — integration into an existing multi-stage flow;
  cross-boundary contract renegotiation, since D8 declares the manifest's accept-seed
  unachievable and D6 answers a decision deferred from a sibling's D9; and a counted
  performance budget as a first-class acceptance criterion)

## Declared Surface
- feature-id: tabmeta-tmux-reroot
- depends_on: [tmux-root-resolution]
- declared-surface:
  - crates/app/src/tabmeta/worker.rs

## Goal
Make the tab card follow the active tmux pane: in `Deriver::tab`, resolve the derivation
root through `tmux::effective_root` before reading cwd, and hand that root to both
`cwd_of_pid` and `agent_of_pid` — so branch, agent badge and task label all describe the
pane's project rather than the tab's own shell.

## Non-goals
- The resolution mechanism itself — gate, spawn, parse, tty join, memo type — all of which
  belong to `tmux-root-resolution` and are consumed here unchanged.
- The ai-monitor overlay's session hint (`tabsession.rs`) — that is `tabsession-tmux-reroot`.
- Any change to the cadence (`TICK`, `BG_INTERVAL`, `should_refresh`), the differ
  (`publish_if_changed`), the roster/active command handling, or `note_activity_gap`.
- Re-rooting the **roster**. `TabPid.pid` remains the tab's own shell pid; only the
  derivation's local root changes.
- Any new field on `TabMeta` or `GridSnapshot`. The published shape is byte-identical.
- **The sidebar row title.** `tabs.rs` derives it from the tab shell's cwd; that file is in no
  feature's declared surface in this partition. See Known Gaps — this is a partition-level
  question surfaced to the human, not something this feature may reach out and fix.

## Confirmed Goal
One `enabled` binding, one resolved-root statement, one gated cwd/fallback block absorbing the
old `:69` read, and one substitution at `:82` — plus one `Deriver` field for the per-tick memo
(the full shape is published once, in D1). Done
when: a tab running tmux publishes the *pane's* branch, agent and task; the **active** tab
reflects a tmux window switch within one tick and a background tab within `BG_INTERVAL`
ticks; a non-tmux tab publishes exactly what it does today; a pane that dies mid-derivation
degrades to the tab's own metadata rather than a blank card; the source toggles still stop
all tmux work; and every pre-existing `worker.rs` test passes with its original assertions.

## Research
- **Exactly two call sites consume the root pid** — `super::proc::cwd_of_pid(tp.pid)` at
  `worker.rs:69` and `super::agent::agent_of_pid(tp.pid)` at `:82`. The file's other `.pid`
  occurrences (`:382`, `:412`, `:481`) are `TabPid { pid: … }` struct-literal *writes* in
  tests, not reads. SD2's "one-line substitution" holds literally at `:82`; at `:69` it held for
  the pre-D9 design, but D9's pane-death fallback expands that site into D1's gated block, so
  the site is **absorbed** rather than argument-swapped. The *count* of root consumers is still
  two, which is the property SD2 actually depends on.
- **cwd is already resolved once and shared** (`:66-79`) by the git and activity paths, so
  re-rooting *before* that single read propagates to the branch, the activity state and the
  task label with no further change.
- **`Deriver` already owns the per-tick memos** — `git_memo`, `act_memo` (`:43-50`), both
  constructed in `Deriver::new()` (`:53-62`). `Deriver` is built exactly once per worker in
  `run()` (`:184`), so a third memo's lifetime is automatically the worker's.
- **The toggle guard is an `if` *expression bound to `cwd`*** (`:68-72`). This matters for
  scoping: a `root` computed *inside* that block would not be in scope at `:82`. See D1.
- **The guard is a disjunction**, so three of its four states run the gate: both on, git-only,
  agent-only. Notably the two existing cost tests set `enabled_agent = false` (`:379`) and
  `enabled_agent = false` (`:409`) — i.e. they exercise the **git-only** asymmetric state, not
  "both on".
- **`Cmd::SetActive` re-derives in band** (`:203-214`) via `deriver.tab(&tp, tick_index, …)` at
  the *current, unadvanced* `tick_index` (only `:225` advances it). It inherits the hop with no
  separate edit, and the staleness that unadvanced index would cause is already handled by
  `tmux-root-resolution`'s D7(d) wall-clock guard.
- **Background tabs refresh every `BG_INTERVAL = 4` ticks** (`:30`, `should_refresh` `:35-37`)
  ≈ 2 s. Nothing signals a tmux window switch to the app, so a background tmux tab lags by up
  to that interval — the "within one tick" claim holds only for the active tab.
- **No existing test constrains the new cost.** `PROC_EXE`/`PROC_CHILDREN` are asserted only in
  `empty_roster_does_no_work` (`:356-368`), which passes an **empty roster**, so `tab()` is
  never called. `steady_state_repo_derivation_does_no_git_read` (`:372-400`) asserts only
  `PROC_CWD`, `GIT_READ`, `GIT_STAT`.
- **`empty_roster_does_no_work` calls `reset_all()` at `:352`, before `Deriver::new()` at
  `:353`** — so its "no source touched" assertion silently requires `tmux::Memo::new()` to
  perform no counted I/O. Pinned as R6.1.
- **The cost tests use `std::process::id()`** (`:380-383`, `:410-413`). The gate inspects that
  pid and its *descendants*, never its ancestors — so running `cargo test` inside a tmux pane
  does **not** trip the gate. A corollary that constrains what is testable here: the test
  process can never produce a tmux client in its own subtree, so `TMUX_LIST` is structurally
  pinned at 0 in this file. See D10.
- **`cwd_of_pid` is unmemoized** (`proc.rs:26-28`) and bumps `PROC_CWD` on every call. Since
  `tmux-root-resolution`'s D13 already calls it for its liveness check, a re-rooted tab pays
  `PROC_CWD == 2` per tick. This is part of D8's ledger.
- **The `prev`-reuse branch never reaches the hop.** `pass()` (`:140-147`) either calls `tab()`
  or clones the previous `TabMeta`; the hop lives entirely inside `tab()`, so a tab skipped this
  tick cannot re-root, cannot spawn, and cannot perturb any counter. Its only observable
  consequence is the `BG_INTERVAL` lag that R0.5 covers. Recorded so this is not re-derived.
- **The differ makes a re-root free when nothing changed** (`publish_if_changed`, `:238-257`):
  no lock, no generation bump, no wake — so the hop cannot reintroduce the G1 idle-CPU debt.
- **Line 1 of a card is `user > task > program > cwd`** (`sidebar_model::resolve_line1_tier`,
  `sidebar_model.rs:93-107`), where the `cwd` tier is `SidebarRow.title`, built in
  `tabs.rs:194-200` from **`s.cwd()` — the tab shell's** cwd basename. `task` now comes from the
  pane. This is the incoherence recorded in Known Gaps.

## Decisions

The six `SD<n>` entries are inherited verbatim from the set's partition manifest and apply to
every feature in the set; this feature's local decisions follow. (The manifest text is
reproduced in `specs/tmux-root-resolution/spec.md`; it is not duplicated a third time here.)

### D1: One `enabled` binding, one `root`, both in scope for both call sites — the full prologue
- **Status**: resolved
- **Rationale**: the exact shape is pinned because the obvious phrasings do not compile. The
  guard at `:68-72` is an `if` **expression bound to `cwd`**, so anything computed inside it is
  scoped to that block and unavailable at `:82`. This is the complete replacement prologue for
  `Deriver::tab`, incorporating D9's fallback — published as **one** snippet so no two decisions
  can describe `:69` differently:
  ```rust
  let enabled = self.enabled_git || self.enabled_agent;
  // Re-root at the tmux pane when this tab runs a client (SD2). Absent tmux this is
  // `tp.pid`, so every downstream read is byte-identical to today.
  let root = if enabled {
      super::tmux::effective_root(&mut self.tmux_memo, tp.pid, tick)
  } else {
      tp.pid
  };
  // cwd feeds the git branch and Claude activity; skip it when neither is wanted.
  // The read is gated *outside* the match: a `match (enabled, cwd_of_pid(root))`
  // would evaluate the scrutinee tuple eagerly and fire the syscall even when
  // `enabled` is false, breaking R3.1 and the existing toggle test.
  let (root, cwd) = if !enabled {
      (tp.pid, None)
  } else {
      match super::proc::cwd_of_pid(root) {
          Some(c) => (root, Some(c)),
          // The pane died between the sibling's liveness check and here (D9):
          // retry from the tab's own shell so the card degrades to today's
          // metadata rather than going blank.
          None if root != tp.pid => (tp.pid, super::proc::cwd_of_pid(tp.pid)),
          None => (tp.pid, None),
      }
  };
  ```
  Line `:69`'s standalone `cwd_of_pid` call is **absorbed** into this block rather than merely
  having its argument swapped; `:82` then becomes `agent_of_pid(root)`, reading the **rebound**
  `root` — so on the fallback path the agent walk is redirected to the tab's own shell too,
  which is what keeps branch and agent describing the same process. Resolving once keeps pane
  identity consistent within a derivation: two independent resolutions could straddle a tmux
  window switch and yield a card whose branch came from one pane and whose agent came from
  another. **Rejected: re-rooting inside `cwd_of_pid`/`agent_of_pid`** — that pushes tmux
  awareness into `proc`/`agent`, a sibling feature's surface.

### D2: The roster is never re-rooted
- **Status**: resolved
- **Rationale**: `TabPid.pid` stays the tab's own shell pid. The roster records *which process a
  tab owns*, which is still the shell — the pane is an implementation detail of what runs inside
  it and changes as the user moves around tmux. Keeping it stable leaves `SetRoster`/`SetActive`,
  the `TabId` mapping and every other consumer untouched, and a tmux session ending simply stops
  producing a re-root. **Rejected: rewriting the roster pid on resolution** — it would make the
  roster's meaning tick-dependent and strand the tab if the pane exited. (This decision is also
  what makes the title/branch divergence in Known Gaps structural rather than incidental.)

### D3: The memo is a `Deriver` field, constructed in `Deriver::new()`
- **Status**: resolved
- **Rationale**: it joins `git_memo` and `act_memo` as the third per-tick memo on the same
  struct (`:43-50`, `:53-62`), so its lifetime is the worker's with no plumbing through `pass()`
  or `run()`, and the existing tests that build `Deriver::new()` directly need no new
  scaffolding. **Rejected: a memo owned by `run()` and threaded as a parameter** — it changes
  `tab()`'s and `pass()`'s signatures for no benefit; **rejected: a process-global memo** —
  shared mutable state across threads for a value whose purpose is per-tick freshness.

### D4: The hop sits behind the same toggle condition as the cwd read — including both asymmetric states
- **Status**: resolved
- **Rationale**: with both sources off the tab needs no cwd and no agent, so it needs no root
  either; running the gate would be pure waste, and R1.8's contract is that a disabled source
  stops its *I/O*, not merely clears its field. The condition is therefore the same
  `enabled_git || enabled_agent` disjunction, evaluated as in D1. Its three open states all
  re-root, which is correct and is stated explicitly because it is easy to miss: **git-only**
  still needs the pane's cwd (the branch must be the pane's repo), and **agent-only** still
  needs the pane's tree (the badge must be the pane's agent). The git-only state is not
  hypothetical — it is what both existing cost tests set (`:379`, `:409`). D4's guard is also
  **not** protected by the existing `disabling_git_clears_branch_and_stops_git_work`, which
  asserts `PROC_CWD`/`GIT_*` but never `PROC_EXE`/`PROC_CHILDREN`; R3 adds those assertions.

### D5: `SetActive`'s in-band refresh inherits the hop with no separate edit
- **Status**: resolved
- **Rationale**: it already routes through `deriver.tab(&tp, tick_index, …)` (`:203-214`), so a
  newly-activated tmux tab shows its pane's metadata immediately rather than waiting out a
  background interval (R1.7). That path derives at an *unadvanced* `tick_index`, which would
  serve a stale client map under a tick-only memo key — already handled upstream by
  `tmux-root-resolution`'s D7(d) wall-clock guard, so **this feature adds nothing**. Recorded
  explicitly so the interaction is not later rediscovered as a bug.

### D6: Do **not** fuse the presence gate into the agent walk
- **Status**: resolved
- **Rationale**: `tmux-root-resolution`'s D9 deferred this decision here, so it is answered
  rather than left open. Fusing would save one `children_of` and `k` `exe_path` calls per
  non-tmux tab per refresh. Rejected because: (a) for a **tmux** tab the two walks are over
  *different trees* — the tab's shell, then the pane's — so fusion saves nothing in the case the
  feature exists for; (b) it would replace a two-line substitution with a restructuring of
  `agent_of_pid`'s entry point, which lives in a **sibling feature's declared surface**, turning
  an ordered dependency into a cross-surface edit; (c) the saving is `1 + k` syscalls (typically
  2 when `k = 1`) against a per-tab budget already around 10, and the depth-0 `exe_path` — the
  genuinely new call — is not recoverable by fusion at all. Revisit only if profiling shows the
  process-walk path dominating a tick.

### D7: The published shape and the differ are untouched
- **Status**: resolved
- **Rationale**: `TabMeta` gains no field and `publish_if_changed` is not modified. A re-root
  changes the *values* flowing into existing fields, so change-only publication keeps working
  unmodified: a tmux tab whose pane metadata is stable publishes nothing and wakes nobody,
  exactly like a non-tmux tab. This pins the property that the hop cannot reintroduce the G1
  idle-CPU debt.

### D8: The accept-seed's "syscall budget unchanged" is refined with a complete ledger
- **Status**: resolved
- **Rationale**: the manifest's accept-seed says a warm non-tmux tab's per-tick syscall budget
  is "unchanged from today". That is **not literally achievable**: the presence gate costs
  `2 + k` syscalls per refresh (`tmux-root-resolution` D9), of which the depth-0 `exe_path(root)`
  is genuinely new work no design choice removes. The honest ledger, which R5 asserts:
  | Path | Delta vs today |
  |---|---|
  | non-tmux tab | `PROC_EXE +(1 + k)`, `PROC_CHILDREN +1`, `TMUX_LIST 0`, `PROC_CWD` unchanged at 1 |
  | tmux tab (pane alive) | the above, plus `PROC_TTY +1`, `TMUX_LIST +1` per **tick** (not per tab), `TTY_STAT +N` per tick, and **`PROC_CWD` 1 → 2** (the sibling's D13 liveness check plus this feature's read) |
  | tmux tab (pane already dead at resolution) | as above but **`PROC_CWD` 1 → 2** with **no re-root** — D13's liveness read fails inside `effective_root`, which returns `tp.pid`, so this feature reads cwd once against the tab's own shell |
  | tmux tab (pane died mid-derivation, D9) | as above but **`PROC_CWD` 1 → 3** — D13's liveness read, this feature's now-failing read, and the fallback read of `tp.pid` |
  | both toggles off | nothing — no gate, no spawn, no cwd (guaranteed by D1's read being gated outside the `match`) |
  What *is* unchanged is the budget the existing assertions actually cover: `PROC_CWD == 1`,
  `GIT_READ == 0`, `GIT_STAT <= 1` for a warm **non-tmux** repo tab. **Rejected: reading the
  accept-seed loosely and asserting "unchanged"** — the counters would contradict it;
  **rejected: dropping the depth-0 check to make it literally true** — that silently breaks the
  `exec tmux` users the sibling feature deliberately covers. Surfaced for human amendment.

### D9: A pane that dies mid-derivation falls back to the tab's own root
- **Status**: resolved
- **Rationale**: the sibling's D13 validates `pane_pid` liveness *inside* `effective_root` and
  then returns — but the pane can exit in the window between that check and `worker.rs:69`/`:82`.
  Without handling, `cwd` is `None`, branch and task are `None`, the depth-1 walk against a dead
  pid yields nothing, and the card goes **blank** — precisely the outcome D13 exists to prevent,
  with its guarantee lost at the hand-off. So when the root was re-rooted and its cwd read fails,
  the derivation retries from the tab's own pid — the `None if root != tp.pid` arm of **D1's
  single published prologue**, which is the one authoritative code shape (this decision
  deliberately does not restate it, so the two cannot drift). Two properties of that shape are
  load-bearing here: the cwd read is gated **outside** the `match`, because a
  `match (enabled, cwd_of_pid(root))` scrutinee is evaluated eagerly and would fire the syscall
  even with both toggles off; and the rebound `root` also redirects `agent_of_pid`, so branch
  and agent never describe different processes.
  Cost is one extra `cwd_of_pid` **only on the rare failure path** — bringing that path to
  `PROC_CWD == 3` (the sibling's D13 liveness read, this feature's failing read, and the
  fallback read), which D8's ledger and R2.2 both record. **Rejected: accepting the blank card
  for one tick** — user-visible, and it contradicts SD4's "never worse than not having looked";
  **rejected: having `effective_root` return its already-fetched cwd** — that would change a
  sibling feature's signature, a cross-surface edit.

### D10: Tests assert what is reachable in this surface; behavioral proof of the re-root is the manual gate
- **Status**: resolved
- **Rationale**: this feature's unit tests **cannot** exercise a real re-root. The gate inspects
  the root and its descendants, and the test process's tmux (if any) is an *ancestor*, so
  `TMUX_LIST` is structurally 0 in `worker.rs` tests — and `Deriver` deliberately has no
  injection seam for `effective_root` (D3), since adding one means editing `tmux.rs`, a sibling's
  surface. Therefore: the memo-sharing and map-parsing behaviors are proven **upstream** by
  `tmux-root-resolution`'s R5.1/R5.2 against fake trees; this feature's automated tests assert
  only what is reachable here — scoping, the toggle guard, the pane-death fallback, the cost
  ledger, and non-regression; and the user-visible outcomes (R0) are verified at the `/run-term`
  manual gate. **Rejected: writing tests that require a live tmux client** — they would be
  skipped-or-flaky depending on the developer's environment, the opposite of this workspace's
  headless-testability invariant; **rejected: adding a seam in `tmux.rs`** — a partition
  violation.

## Constraints
- **Ordered dependency, not a shared surface**: this feature *consumes* `tmux::effective_root`,
  `tmux::Memo` and `Memo::new()`; it must not modify `tmux.rs`, `proc.rs`, `agent.rs` or
  `mod.rs`, all of which belong to `tmux-root-resolution`'s declared surface — nor `tabs.rs` or
  `sidebar_model.rs`, which belong to no feature here.
- **No new dependency**; **`term-core` learns nothing** (`dependency-direction`).
- **macOS ARM64 only**; no `#[cfg(target_os = …)]` branches (`macos-only`).
- **No `unwrap()` outside tests**; `effective_root` returns `u32`, so there is nothing to unwrap.
- **`cargo clippy --workspace --all-targets -- -D warnings` must pass**; the `rustfmt`
  PostToolUse hook rewrites edited `*.rs`.
- **Every pre-existing `worker.rs` test keeps its original assertions** — none may be relaxed.

## Known Gaps
- **The card's title and its branch can disagree — and this partition cannot fix it.** Line 1 is
  `user > task > program > cwd` (`sidebar_model.rs:93-107`), whose `cwd` tier is
  `SidebarRow.title`, built in `tabs.rs:194-200` from the **tab shell's** cwd. After this feature
  `branch` and `task` come from the **pane**. When a pane has a task label the card is coherent
  (line 1 is the pane's task); when it does not — a pane running no agent — the card shows the
  tab shell's folder name beside the pane's branch, e.g. title `terminal` next to branch
  `feat/other-project`. `tabs.rs` and `sidebar_model.rs` are in **no feature's declared surface**
  in this partition, so no feature here may fix it. **Surfaced to the human as a
  partition-completeness question** (extend a surface, or add a fourth feature); deliberately not
  reached into. This is the sharpest argument against D2 and is recorded rather than answered.
- **The manifest's accept-seed overstates cost neutrality** (D8). Flagged for human amendment.
- **Background tmux tabs lag up to `BG_INTERVAL` (≈2 s)** behind a tmux window switch, since
  nothing signals the switch to the app. Accepted; the active tab is within one tick.
- **Gate/walk fusion is declined, not impossible** (D6).
- **Multi-pane tabs are not distinguished.** A tab whose client has several panes resolves to the
  client's *active* pane only; a split showing two projects reports one of them. Intended, untested.

## Requirements

### R0: A tmux tab's card describes its active pane *(verified at the manual gate, D10)*

#### R0.1: Branch comes from the pane's repo
- **Given**: a tab whose tmux client's active pane has cwd in repo B, while the tab's own shell sits in repo A
- **When**: a derivation pass runs for that tab
- **Then**: the published `TabMeta.branch` is repo **B**'s branch

#### R0.2: The agent badge comes from the pane's agent
- **Given**: the pane's shell has `claude` as a direct child and the tab's own shell has none
- **When**: a derivation pass runs
- **Then**: `TabMeta.agent` is `Some(Agent { kind: Claude, .. })`, resolved by the existing depth-1 walk against the pane root

#### R0.3: The task label comes from the pane's session
- **Given**: the pane's cwd resolves to a Claude projects log with a current task label
- **When**: a derivation pass runs
- **Then**: `TabMeta.task` is that label, not the tab shell's

#### R0.4: The active tab follows a window switch within one tick
- **Given**: the **active** tab's card shows pane P1's metadata
- **When**: the user switches to a tmux window whose pane P2 differs, and one tick elapses
- **Then**: the published `TabMeta` describes P2

#### R0.5: A background tab follows within `BG_INTERVAL` ticks
- **Given**: a **background** tmux tab showing P1
- **When**: the user switches to P2
- **Then**: the card describes P2 within `BG_INTERVAL` (4) ticks ≈ 2 s — not within one tick

#### R0.6: A non-tmux tab is unchanged
- **Given**: a tab with no tmux in its tree
- **When**: a derivation pass runs
- **Then**: the published `TabMeta` equals what the pre-change code produces, and `TMUX_LIST == 0`

### R1: The hop's placement and scope (D1, D2)

#### R1.1: `root` is in scope at both call sites
- **Given**: the post-change `Deriver::tab`
- **When**: it is compiled
- **Then**: `root` is bound by a `let` **before** the cwd guard block, and is used at both former `tp.pid` sites — the code compiles, which the block-scoped alternative does not

#### R1.2: The root is resolved once per derivation and shared
- **Given**: a single call to `Deriver::tab` for a tmux tab
- **When**: it derives branch, agent and task
- **Then**: `effective_root` was called exactly once, and both call sites read the same `root` — its resolved value on the pane-alive path, or `tp.pid` on D9's fallback — never two independent resolutions

#### R1.3: The roster keeps the tab's own shell pid
- **Given**: a tmux tab that has been re-rooted for derivation
- **When**: the roster is inspected
- **Then**: its `TabPid.pid` is still the tab's shell pid, unchanged

### R2: Pane death mid-derivation (D9)

*R2.1 and R2.2 are **not reachable by a unit test in this surface** and are verified at the
`/run-term` manual gate, per D10: `effective_root` can never re-root inside `worker.rs` tests
(the test process's tmux is an ancestor, and the gate inspects descendants), so `root` is always
`tp.pid` there and the fallback arm cannot be entered. Creating a seam would mean editing
`tmux.rs` — a partition violation. R2.3 **is** reachable and stays automated.*

#### R2.1: A dead re-rooted pane falls back to the tab's own metadata *(manual gate)*
- **Given**: a tmux tab whose pane exits between the liveness check and the cwd read, for a tab whose own shell has a valid cwd in a git repo
- **When**: the derivation runs
- **Then**: `TabMeta.branch` is the **tab shell's** branch, not `None` — the card does not go blank; and the agent badge likewise describes the tab shell, since the rebound `root` redirects `agent_of_pid` too

#### R2.2: The fallback's cost is one extra cwd read, only on failure *(manual gate)*
- **Given**: the dead-pane case above with counters reset
- **When**: the derivation runs
- **Then**: `PROC_CWD == 3` — D13's liveness read, the failing pane read, and the fallback read; whereas the pane-alive path stays at `PROC_CWD == 2`

#### R2.3: A dead pid with no fallback available yields blank without panicking
- **Given**: a tab whose own pid is also dead (pid 0)
- **When**: the derivation runs
- **Then**: it returns `TabMeta::default()` and does not panic, as `dead_pid_derives_to_blank_without_panic` already asserts

### R3: Toggles gate all tmux work, in every state (D4)

#### R3.1: Both sources off performs no tmux work at all
- **Given**: `enabled_git = false`, `enabled_agent = false`, counters reset
- **When**: `Deriver::tab` runs
- **Then**: `TMUX_LIST == 0`, `PROC_CWD == 0`, **`PROC_EXE == 0` and `PROC_CHILDREN == 0`** — the gate did not run

#### R3.2: git-only still re-roots
- **Given**: `enabled_git = true`, `enabled_agent = false`, counters reset, root `std::process::id()`
- **When**: `Deriver::tab` runs
- **Then**: the gate ran — `PROC_EXE == 1` and `PROC_CHILDREN == 1` (`k = 0` under `cargo test`) — and the branch is derived from the resolved root

#### R3.3: agent-only still re-roots
- **Given**: `enabled_git = false`, `enabled_agent = true`, counters reset, root `std::process::id()`
- **When**: `Deriver::tab` runs
- **Then**: the gate ran, and `PROC_CHILDREN == 2` — one listing for the gate and one for `agent_of_pid`'s own walk, which is what distinguishes this state from R3.2

#### R3.4: The existing toggle test passes unmodified
- **Given**: `disabling_git_clears_branch_and_stops_git_work` as currently written
- **When**: the suite runs
- **Then**: it passes with its original assertions, including `PROC_CWD == 0`

### R4: Cadence, differ and publication are unchanged (D5, D7)

#### R4.1: An activated tab re-derives in band with the hop applied
- **Given**: a tmux tab that is not active
- **When**: `Cmd::SetActive` names it
- **Then**: it is re-derived immediately through the same hop, without waiting a background interval

#### R4.2: `TabMeta`'s shape is unchanged
- **Given**: the post-change code
- **When**: `TabMeta` is inspected
- **Then**: it has exactly today's fields — `branch`, `agent`, `task`

#### R4.3: An unchanged publish still takes no lock and no wake
- **Given**: `publish_bumps_generation_only_on_change` as currently written
- **When**: the suite runs
- **Then**: it passes unmodified — the hop did not change the differ

### R5: The cost delta is bounded, counted and non-regressive (D6, D8)

#### R5.1: The pre-existing cost assertions hold unchanged
- **Given**: `empty_roster_does_no_work` and `steady_state_repo_derivation_does_no_git_read`
- **When**: the suite runs
- **Then**: both pass with their original expected deltas, unrelaxed

#### R5.2: The non-tmux delta is exactly the gate's, with concrete numbers
- **Given**: reset counters and one refresh of a non-tmux tab rooted at `std::process::id()`, with **`enabled_agent = false`** (isolating the gate from `agent_of_pid`'s own walk, as both existing cost tests do at `:379` and `:409`), whose direct-child count under `cargo test` is `k = 0`
- **When**: the refresh runs
- **Then**: `PROC_CHILDREN == 1`, `PROC_EXE == 1`, `PROC_CWD == 1`, `TMUX_LIST == 0` — i.e. the `1 + k` / `1` gate delta with `k = 0`. (With the `Deriver::new()` default of both toggles on, `PROC_CHILDREN` is 2, since `agent_of_pid` lists children again — which is why the toggle state must be stated.)

#### R5.3: Test behavior does not depend on the developer's tmux
- **Given**: the suite run from inside a tmux pane
- **When**: the cost tests execute against `std::process::id()`
- **Then**: they produce the same counters as outside tmux, because the gate inspects descendants while tmux is an ancestor

### R6: Construction is side-effect free (D3)

#### R6.1: `Memo::new()` performs no counted I/O
- **Given**: `empty_roster_does_no_work`, which calls `reset_all()` at `:352` **before** `Deriver::new()` at `:353`
- **When**: `Deriver::new()` constructs the tmux memo
- **Then**: every probe counter is still 0 — the constructor touches no source

### Decision → requirement coverage

| Decision | Covered by |
|---|---|
| SD1–SD3 (join, cost budget) | R0.1, R0.6, R5.2 |
| SD4 (absent on failure) | R2.1, R2.3 |
| SD5–SD6 (purity, counters) | R5.1–R5.3, R6.1 |
| D1 (one `root`, correct scope) | R1.1, R1.2 |
| D2 (roster untouched) | R1.3 |
| D3 (memo on `Deriver`) | R6.1 |
| D4 (toggle guard, all states) | R3.1–R3.4 |
| D5 (`SetActive` inherits) | R4.1 |
| D6 (no fusion) | R5.2 (the accepted delta) |
| D7 (shape + differ untouched) | R4.2, R4.3 |
| D8 (cost ledger) | R5.1–R5.3, R2.2 |
| D9 (pane-death fallback) | R2.1–R2.3 |
| D10 (what is testable here) | R0 routed to the manual gate; R1–R6 automated |

## Tasks

### T1: Wire the hop, with its scoping, toggle and fallback tests [vertical]
- **Fulfills**: R1, R2.3, R3, R4, R6 — and R0, R2.1, R2.2 structurally (verified at the manual
  gate per D10)
- **Depends on**: (none within this feature; the feature depends on `tmux-root-resolution`)
- Add `tmux_memo: tmux::Memo` to `Deriver` and `Memo::new()` to `Deriver::new()`. Replace the
  prologue of `Deriver::tab` with **D1's single published snippet** verbatim (the `enabled`
  binding, the `root` resolution, and the gated cwd/fallback block, which absorbs the old `:69`
  call), then make `:82` read `agent_of_pid(root)`. Lands with its own tests: the both-off gate
  silence (R3.1, incl. `PROC_EXE`/`PROC_CHILDREN == 0`), both single-toggle states with their
  concrete counters (R3.2, R3.3), the dead-root-no-fallback case (R2.3), and construction purity
  (R6.1). Touch nothing else in the file.

### T2: The cost ledger and non-regression suite [vertical]
- **Fulfills**: R5
- **Depends on**: T1
- Assert the concrete non-tmux delta with `k = 0` (R5.2) and confirm the pre-existing cost,
  toggle and differ tests pass with original assertions (R5.1, R4.3). Every counter-touching
  test holds `probe::serial()`.

**DAG**: `T1 → T2`. Linear — both edit the same file, so they cannot run concurrently.

## External Dependencies

### Pre-work
- `tmux-root-resolution` must be complete — this feature calls `tmux::effective_root`,
  `tmux::Memo` and `Memo::new()`, none of which exist until then.

### Post-work
- **Manual verification via `/run-term`** — this is the only proof of R0 (D10). Open a tab,
  start tmux, and confirm the card shows the pane's branch/agent/task and follows a window
  switch between two panes in different repos.
- **Human decisions required** (both in Known Gaps, neither self-applied): amend the manifest's
  accept-seed for this feature (D8); and decide how to handle the title/branch divergence, which
  needs a surface this partition does not assign.
