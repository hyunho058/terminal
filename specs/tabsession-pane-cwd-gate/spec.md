# Spec: tabsession-pane-cwd-gate

## Meta
- **Created**: 2026-08-13
- **Type**: dev
- **Status**: approved (spec only — implementation deliberately not started)
- **Approved by**: user
- **Approved at**: 2026-08-14
- **Complexity**: Complex (5 signals) — ① three subsystems in the path (`tabsession` +
  `overlay` + `tabmeta`, reading `ai-monitor` semantics); ② concurrency (winit event loop, a
  250 ms-budgeted resolver thread, the `refresh_rivals` worker, the watch thread); ③ multi-actor
  (tab shell, tmux client/server/pane, the `claude` process, rival `claude`s from other
  applications); ④ a state machine (`Repoint` keying × `OpenedOn`/rotation × `tick_action` ×
  D10's early-out × D5's edge); ⑤ data-schema change (`SessionHint` gains a field, `TabMeta`
  gains one, the `Overlay` trait signature widens).
- **L2-reviewer**: NEEDS_FIX ×3 (round 1: 9 findings + a steelman against D1; round 2: 12
  findings, 3 of them defects in the round-1 fixes; round 3: 1 blocking + 6 non-blocking, the
  blocking one introduced by a round-2 fix). All findings verified against the code before
  acceptance; two were rejected with recorded reasoning (registry-first ordering, and the
  framing of the `worker.rs` sibling collision as merge-order). Both retries spent.
- **Origin**: reported from a live session — the panel read `NO SESSION` for two `tmux attach`
  tabs while a third, non-tmux tab resolved normally. Diagnosis and process evidence in
  § Research were measured on the reporting machine, not reconstructed.

## Declared Surface
- feature-id: tabsession-pane-cwd-gate
- depends_on: [tabsession-tmux-reroot, tmux-pane-cwd, tabmeta-session-keying]
- declared-surface:
  - crates/app/src/tabsession.rs        # D1, D2, D7, D12 — the hint, the deleted gate
  - crates/app/src/overlay.rs           # D3, D4, D5, D10 — open(), the pid compare, the early-out
  - crates/app/src/seam.rs              # D6 — the `Overlay::on_active_changed` trait seam widens
  - crates/app/src/tabmeta/mod.rs       # D6 — `TabMeta` gains the agent pid
  - crates/app/src/tabmeta/worker.rs    # D6 — publish it; lift it out of the `enabled_agent` gate
  - crates/app/src/terminal_app.rs      # D6 — the two `notify_active_changed` call sites
- declared-overlap (NOT merely merge order):
  - crates/app/src/tabmeta/worker.rs — the uncommitted sibling `card-live-cwd`
    (`terminal-feat-improving-session-card`) also **writes** this file. Its spec names
    `worker.rs:107-116` as a non-goal, so the intended edit sets differ, but two features
    writing one file is a declared-surface overlap and needs a landing order, not a note.
    The overlap is **adjacent, not distant**: D6 changes `Deriver::tab`'s signature and its
    `enabled_agent` gate (`worker.rs:95`, `:125`) — the block immediately around the region
    `card-live-cwd` names — plus the `TabMeta` field pin at `:937-947`. An earlier draft said
    "it only adds a field", which understated the edit set the rebase has to absorb.
    Recommended: land this feature first, then rebase `card-live-cwd` onto the new `tab()`.

## Goal

The ai-monitor panel shows `NO SESSION` / `(no Claude session in this tab)` for every tab
running `tmux attach`, while a tab running `claude` directly shows the session normally.
Fix it, so a tmux tab's panel reports the session running in its pane.

## Non-goals

- **The tmux re-root mechanism.** `tabmeta::tmux::effective_root_uncached` and
  `tabsession::resolved_root` resolve the pane correctly — verified live against two
  attached clients. Nothing in `crates/app/src/tabmeta/tmux.rs` is in scope.
- **The projects-directory resolution.** `tmux-pane-cwd` (#22) already derives the
  overlay's directory from the pane (`overlay.rs:170`, `:190`). Not re-opened here.
- **The sidebar tab card.** It re-roots at the pane with no cwd gate and is already
  correct — visible in the report's own screenshot, showing the right worktree and branch
  beside a panel that says there is no session.
- **Non-tmux tabs.** Their path (`find(shell_pid)`, ungated) must stay byte-identical.
- **Multiplexers other than tmux**, tmux control mode (`-CC`), and non-default tmux
  sockets (`tmux -L`) — inherited non-goals from `specs/tmux-pane-resolution/`.
- **Relaxing strict resolution.** `resolve_owned_session` stays confirmation-only; this
  spec changes which hint reaches it, never how strictly it judges one.

## Confirmed Goal

`tabsession::hint_for` returns a usable `SessionHint` for a tab whose tmux pane runs
`claude`, **regardless of what directory the tab's own shell sits in** — including when
the pane's `claude` runs in a subdirectory of it (D7c's second, independent rejection).

Delivered when all of the following hold:

1. Opening the panel on a `tmux attach` tab whose pane runs `claude` in a directory the
   tab shell was never in shows `CONNECTED`, the session id, and live dashboard rows.
2. `refresh_rivals` (`overlay.rs:386`) — a second `hint_for` caller that the same gate
   silently degrades to `RivalSet::default()` (`known: false`, uniqueness rescue
   permanently withheld, "a claude appeared" never reported) — recovers on the same fix.
3. No path can confirm another application's log with `owned: true`. This is the property
   D7's gate was built to protect, and it survives the gate's removal or narrowing by a
   stated argument, not by accident.
4. Non-tmux tabs are unaffected: the `root == shell_pid` branch pays no new syscall and
   returns what it returns today.

## Research

### The defect, measured live (2026-08-13, this machine)
- The gate: `choose_claude` compares the found `claude`'s cwd against `cwd_of(shell_pid)` —
  the **tab shell's**, which under tmux is the client's (`crates/app/src/tabsession.rs:209-213`).
- Tab zsh `58495` cwd `/Users/hyunhokim`; its child `tmux attach -t
  terminal-feat-improving-session-card` `58510`; pane zsh `58354` and pane `claude` `58395`
  both cwd `…/terminal-feat-improving-session-card`. The two sides of the gate differ, so the
  pane result is dropped.
- Fall-through runs `find_claude(58495)`, whose only descendant is the tmux client, so
  `hint_for` returns `None` (`tabsession.rs:218`).
- The re-root itself is sound: `tmux list-clients -F '#{client_tty} #{pane_pid}'` returns
  `/dev/ttys005 58354` and `/dev/ttys007 64516`, matching both tabs. Only the gate rejects.
- Second tmux tab `64897` → pane `64516` → `claude` `64532` is in the identical state; the
  non-tmux tab `57632` → `claude` `57650` takes the ungated `find(shell_pid)` path and works.
- `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal-feat-improving-session-card/`
  holds a 1.3 MB live log — so the session the panel refuses to show is being written the
  whole time.

### The display chain the `None` produces
- `self.tab_has_claude = h.is_some()` → `false` (`crates/app/src/overlay.rs:202`).
- Empty hint → `Wanted::default()` (`overlay.rs:241`) → strict `resolve_owned_session`
  answers `None` → no log opened, read, or backfilled (`overlay.rs:275-281`).
- `Detached::NoLog` with `!tab_has_claude` selects `"(no Claude session in this tab)"`
  (`crates/app/src/dashboard.rs:304-307`) — the exact string in the report, which proves the
  hint was `None` rather than merely empty.

### Both `hint_for` callers, and what each does with the hint
- `overlay.rs:194` (`open`, event-loop thread) — pairs the hint with a directory.
- `overlay.rs:407` (`refresh_rivals`, worker thread) — consumes **only** `rival_starts`,
  `rivals_known` and `hint.is_some()`; it never pairs the hint with a directory
  (`overlay.rs:386-425`). Under tmux it therefore publishes `RivalSet::default()` forever
  (`known: false`, uniqueness rescue permanently withheld) and never sets `claude_appeared`.
  It is a second victim of the gate, not a constraint on changing it.

### Why a wrong directory is only dangerous on tier 2
- **Tier 1** (`want.session_id` present) looks the log up **by exact name**:
  `dir.join(format!("{id}.jsonl"))`, returning `None` when it is absent
  (`crates/ai-monitor/src/lib.rs:351-358`). A wrong `dir` therefore *fails to find*, and can
  never confirm a stranger's log.
- **Tier 2** (`since_ms` only) is the branch D7 was written about: the asymmetric window plus
  rival assignment (`lib.rs:380+`). Against a wrong `dir` this is what can return another
  application's log with `owned: true` — and `overlay.rs` latches it.
- So the property to preserve is narrow and nameable: **tier 2 must never run against a
  directory the chosen `claude` is not writing into.**

### The divergence that survived #22
- `tmux-pane-cwd` (#22, `d3d3dc1`) moved the overlay's directory to the pane —
  `pane_cwd` / `pane_cwd_with` (`overlay.rs:170`, `:529`), used at `overlay.rs:190`.
- But `pane_cwd` reads the cwd of the **pane's shell** (`resolved_root` → `cwd_of_pid(root)`),
  while `choose_claude` returns the pane's **`claude`**. A pane that `cd`s after launching
  `claude` makes those two differ — which is exactly the failure D7(a) names when it rejects
  comparing the pane shell ("its cwd moves with every `cd`").
- `hint_for` **already reads the chosen `claude`'s own cwd** at `tabsession.rs:91`
  (`let mine = cwd_of_pid(claude)`), for rival matching. Carrying it outward costs no
  additional syscall. (`:92` is `start_ms`, the third input D7's table needs.)

### Cost facts that bear on the fix
- A tmux tab pays `resolved_root` **twice** per open — once inside `pane_cwd`
  (`overlay.rs:190`) and once inside `hint_for` (`tabsession.rs:89`) — i.e. two
  `tmux list-clients` spawns and two 250 ms budgets on the winit event-loop thread.
  `Repoint::changed`'s own doc already records this ("two 250 ms-budgeted `tmux list-clients`
  calls on the event-loop thread", `overlay.rs:513-514`), and `on_active_changed` fires it
  after every metadata sync.
- Dropping the gate *removes* two `cwd_of_pid` calls from the pane branch
  (`tabsession.rs:209`), so the syscall count does not rise.

### Prior art in-repo the fix must respect
- `specs/tabsession-tmux-reroot/spec.md` D7 — the gate's three load-bearing details:
  (a) compare the found `claude`, never the pane shell; (b) an unreadable cwd never admits;
  (c) equality is byte-exact, so a subdirectory is rejected.
- Same spec, § Known Gaps — "Cross-directory panes get no *pane* hint (D7) … the *benefit* is
  unavailable there", and D4, which named `overlay.rs` as the one-file fix and surfaced it to
  the human as a partition-completeness question. #22 closed D4's **directory** half; the
  **gate** half is what this spec closes.
- `ai_monitor::Registered` (`lib.rs:55-64`) carries the `claude` process's own recorded
  `cwd` alongside its `session_id` — already read on the naming path via `registered_id`
  (`tabsession.rs:119`), and measured as matching in 5/5 live processes (`lib.rs:52-54`).
- Tests that encode the current gate semantics and will need deliberate revision:
  `a_pane_working_elsewhere_is_not_admitted`, `an_unreadable_cwd_never_counts_as_agreement`,
  `the_gate_compares_the_found_claude_not_the_pane_shell`,
  `a_rejected_pane_falls_through_to_the_tabs_own_claude`,
  `nothing_re_rooted_means_exactly_one_walk_and_no_cwd_read` (`tabsession.rs:814-964`).

### Build / test / lint
- `cargo build`; `cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`
  (`CLAUDE.md` › Conventions). Toolchain pinned in `rust-toolchain.toml`.
- Dependency direction `common ← term-core ← renderer ← app` (`.claude/rules/dependency-direction.md`).
  This change lives entirely in `app`, which already owns `libc` and the process tree.

### Sibling-worktree check (no partition conflict)
- `terminal-feat-improving-session-card` holds uncommitted `specs/card-live-cwd/`
  (`sidebar_model.rs`, `tabmeta/worker.rs`).
- `terminal-feat-ai-monitor-antigravity` holds uncommitted `specs/ai-monitor-antigravity/`
  (`agy` detection across `ai-monitor` + `app`) — broad enough to touch `overlay.rs`, so it
  is flagged as a merge-order risk rather than a blocker.
- Neither declares `crates/app/src/tabsession.rs`.

## Decisions

### D1: The projects directory is derived from the chosen `claude`'s own cwd, carried on `SessionHint`
- **Status**: resolved (rationale corrected after L2 review — see the stated assumption below)
- **Rationale**: The gate exists to stop a hint about process P being resolved against a
  directory P is not writing into. Every version of that problem comes from having **two**
  candidate directories and comparing them. This removes the second candidate: `hint_for`
  already reads the chosen `claude`'s cwd at `tabsession.rs:91`, so it carries it outward on
  `SessionHint`, and `Overlay::open` derives `dir` from that value. Hint and directory then
  describe **the same process**, so the *pid* mismatch this feature's whole history is about
  stops being representable. Three consequences fall out for free:
  the cross-directory tmux case resolves; the D7(c) subdirectory case resolves, because
  `claude` writes to `encode_cwd` of exactly the directory it is in; and the pane-`cd`s-after-launch
  divergence (`pane_cwd` reads the pane **shell**, `choose_claude` returns the pane **`claude`**)
  stops being reachable. Costs no additional syscall.
  **Rejected: keep `pane_cwd` as the directory source and merely delete the gate.** Smaller
  diff, `tabsession.rs` only — but it leaves the two candidates in place, so a pane that `cd`s
  after launching `claude` still resolves tier 2 against a directory `claude` is not writing
  into. That is the exact failure D7(a) names when it refuses to compare the pane shell, and
  shipping the smaller diff would re-open it one level down.
  **Rejected: compare against the pane root's cwd instead of the tab shell's.** Same objection,
  stated by D7(a) directly: the pane's `-zsh` moves with every `cd`.
- **Stated assumption (A1), and why it is not free.** What the panel actually needs is
  `dir == the directory claude writes its log into`. What D1 delivers is
  `dir == encode_cwd(the kernel's cwd for claude's pid)`. Those are equal only if `claude`
  writes to `encode_cwd` of the directory it is running in — an assumption about a program this
  project does not control. Two honest qualifications:
  (i) **It is not new, and not D1's to justify alone.** `project_dir(cwd)` is how every path in
  `ai-monitor` already resolves a log, including today's `pane_cwd` path and the sidebar card's.
  D1 does not introduce the assumption; it applies the existing one to a better-chosen cwd.
  (ii) **The evidence is real but thin.** `ai-monitor`'s own measurement (`lib.rs:52-54`) found
  every live `claude`'s recorded `cwd` matching, 5/5 — but that is n=5, one machine, one day.
  Because D2 deletes the last comparison in the system, a violation of A1 would be
  **undetectable**: tier 1 silently finds nothing and the panel simply looks empty. That is the
  price of removing the gate, and it is why D7 adds a cross-check rather than a plain fallback:
  when a validated `Registered.cwd` is also present and disagrees with the kernel, the
  disagreement is itself the signal A1 has been violated.

### D2: The cwd gate is deleted from the pane branch, not narrowed
- **Status**: resolved
- **Rationale**: Under D1 the gate has nothing left to guard — it would compare a directory
  against itself. Keeping a weakened version would preserve the shape of a rule whose premise
  is gone, and the next reader would have to re-derive why it never fires. Deleting it also
  removes two `cwd_of_pid` calls from the pane branch (`tabsession.rs:209`), so the syscall
  count falls rather than rises. `choose_claude` keeps its other two contracts unchanged: the
  pane wins over the tab's own `claude` when both exist, and a pane with no `claude` still
  falls through to the tab's own subtree — never `None`.
  **Rejected: keep the gate behind a "same project root" (prefix) comparison.** It would admit
  the subdirectory case the user put in scope, but it re-introduces the two-candidate shape D1
  exists to remove, and prefix-matching directories is a weaker rule than identity for a value
  that is about to be `encode_cwd`'d into a filename.

### D3: An unreadable `claude` cwd yields a name-only hint — tier 1 allowed, tier 2 suppressed
- **Status**: resolved
- **Rationale**: `cwd_of_pid` is fallible, so D1 needs an answer for the case where the one
  directory cannot be read. The two resolution tiers fail very differently against a wrong
  directory: tier 1 looks the log up by **exact filename** (`lib.rs:351-358`) and a wrong
  directory merely fails to find it, while tier 2's asymmetric `since_ms` window plus rival
  assignment can return a **stranger's** log with `owned: true`, which the overlay then
  latches. So when the directory is not provably the chosen `claude`'s own, the hint is still
  returned, but tier 2 is suppressed at the resolver with no change to `ai-monitor` at all. The
  foreign-log protection D7 was written for therefore survives the gate's deletion in a
  strictly stronger form: it is no longer "compare two directories and refuse on mismatch",
  it is "tier 2 only ever runs against a directory the chosen `claude` is provably in".
  **Rejected: return no hint at all** (today's behavior) — it keeps a live, named session
  invisible for a fallible syscall, which is the class of failure this whole feature exists
  to remove.
  **Rejected: return the full hint and let `dir` fall back to `pane_cwd`** — maximum recovery,
  but it is precisely tier-2-against-an-unproven-directory, the case D7 was built to prevent.
- **Which field, exactly — this is not interchangeable.** Suppression is expressed by building
  `Wanted { since_ms: None }` **in `open()`**. It is emphatically NOT done by zeroing
  `SessionHint::started_ms`, and the two are easy to confuse because they hold the same
  quantity. Zeroing the hint field instead breaks two unrelated things at once:
  `baseline_from` yields `None` (`overlay.rs:1007-1016`), so `opened_on` is `None`, so
  `session_rotated` short-circuits and **rotation detection is dead for the life of the open**;
  and `rival_refresh_needed(attached, armed = false)` then returns `true` forever
  (`overlay.rs:990-991`), putting a `/bin/ps` fork and a 250 ms `resolved_root` on every append
  burst — the exact cost `refresh_rivals`' attached guard exists to prevent.
- **What the user actually sees in the fully-degraded case.** When the cwd is unreadable by
  kernel *and* by registry *and* no `session_id` is known, `Wanted` is empty, the resolver
  answers `None`, and the panel shows "(this tab's session has not written a log yet)" with no
  rows. An earlier draft of this rationale claimed the user "can watch it running"; that is
  false here and has been removed. The recovery D3 buys is real but narrower: it is the
  `--resume` / registry-named case, where tier 1 still finds the log by exact filename.
- **Discharges the prior spec's rejection.** `tabsession-tmux-reroot` D7 rejected this exact
  shape — "emitting `session_id` but suppressing `started_ms` … leaves a populated hint whose
  meaning depends on a directory the hint does not know". That objection was correct **then**
  and is answered **now** by D1: the hint carries its own directory, so its meaning no longer
  depends on one it cannot see. Cited rather than left for a reader to trip over.

### D4: `open()` resolves the hint first, and one named resolver function supplies the directory
- **Status**: resolved
- **Rationale**: D1 needs the hint's cwd *before* the directory is chosen, but today the order
  is the reverse (`Repoint::opened` at `overlay.rs:190`, `hint_for` at `:194`). `hint_for`
  moves above, and the closure passed to `Repoint::opened` returns `hint.cwd` when a `claude`
  yielded a directory — **either `HintCwd` variant**, since `Disputed` still carries the
  registry's path and D7 row 3 opens on it — and falling back to `Self::pane_cwd(pid)` **only**
  on `HintCwd == None`. In one line: `Some(Proven(d)) | Some(Disputed(d)) => Some(d), None =>
  pane_cwd(pid)`; only if *that* is `None` does `Repoint::opened`'s `.or(notified)`
  (`overlay.rs:505`) reach the notified cwd.
  Two earlier drafts got this wrong in opposite directions — the first fell back "when no
  `claude` was found", leaving the unprovable state with three different answers across D4, D7
  and D12; the second swept `Disputed` into the `pane_cwd` fallback, which would have opened the
  panel on the pane *shell's* directory: a **third** path, neither the kernel's nor the
  registry's, making tier 1's exact-filename lookup miss, nullifying D7's whole reason for
  preferring the registry on a disagreement, and leaving `Disputed(PathBuf)`'s payload read by
  nobody. The rule above is the single answer for all three decisions.
  **The anti-transposition guard weakens, and pretending otherwise would be the more dangerous
  error.** An earlier draft of this decision claimed the property was "preserved exactly"
  because `notified` stays the sole `Option<PathBuf>` parameter. The *signature* survives; the
  *property* does not. That guard works by removing the second candidate from the closure's
  reach — `overlay.rs:493-496` records that a resolver taking no argument "was **not**
  sufficient either, because a swap wrapped in `||` still type-checks", and that passing the
  pid in "removes the second candidate entirely". D4's whole purpose is to put a second
  candidate (`hint.cwd`) back inside that closure's capture scope, so `|_| <captured cwd>`
  becomes a legitimate spelling again and a transposition is once more a type-checking
  one-token edit. **The replacement guard must be structural, not another unit test** — a second
  L2 review round showed the obvious remedy does not work. A unit test on `opened` cannot catch
  this hazard, because the hazard lives at the **`open()` call site** and such a test types its
  own arguments; `overlay.rs:1538-1541` records that exact failure ("An earlier version of these
  tests called a free predicate with hand-typed arguments and passed against the buggy call
  site"), and `:493-496` already rules out the closure-wrapping variant. An earlier draft of this
  decision proposed that test anyway *and* claimed "no current fixture can make it", which is
  false: `repeated_identical_notifications_never_restart_the_watcher` (`overlay.rs:1546-1569`)
  already drives `opened` with `notified = CLIENT` and a resolver returning `PANE`.
  So the guard is structural instead: the resolver is a **named free function**
  `fn hint_or_pane_cwd(hint: Option<&SessionHint>, pid: Option<u32>) -> Option<PathBuf>` passed
  by name, never a closure. With no capture there is no second candidate in scope, which is the
  property `:493-496` says the guard actually depends on; `|_| cwd` becomes unwritable at the
  call site again. A source-text pin over `open()`, in the idiom already used at
  `overlay.rs:2612-2638`, backs it up.
  **This requires widening `Repoint::opened`'s resolver, and that must be done rather than
  worked around.** Today it takes `resolve: impl FnOnce(Option<u32>) -> Option<PathBuf>`
  (`overlay.rs:497-506`) — arity 1 — so a named arity-2 function cannot be passed to it. The
  resolver parameter therefore becomes `FnOnce(Option<&SessionHint>, Option<u32>)`, and the hint
  travels **through** it. This is not a concession: it *strengthens* the guard, because
  `Option<PathBuf>` and `Option<&SessionHint>` are not interchangeable types, so the
  transposition stops being expressible at all rather than merely being discouraged.
  The alternative an implementer reaches for first — `|pid| hint_or_pane_cwd(h.as_ref(), pid)` —
  compiles silently and is a closure **with a capture**, i.e. exactly the state this decision
  says defeats the guard. It is named here so it is rejected knowingly.
  (Borrow choreography: `h.as_ref()` at the `opened` call and `h.unwrap_or_default()` after it
  is fine under NLL.) A tmux tab that has a session
  now pays **one** `resolved_root` instead of two — `hint_for` already calls it internally —
  halving the `tmux list-clients` spawns and the 250 ms budgets on the winit event-loop
  thread, a cost `Repoint::changed`'s own doc records (`overlay.rs:513-514`) and which fires
  after every metadata sync.
  **Rejected: reorder but keep `pane_cwd` called unconditionally** — keeps the second spawn
  and its budget on the UI thread for no benefit, since its value is now only ever a fallback.
  **Rejected: keep the current order and re-derive the directory from the hint afterwards** —
  two places deciding the directory is the exact shape that produced the re-open storm.

### D5: A different `claude` pid in the same tab re-points the panel
- **Status**: resolved
- **Rationale**: Under D1 the directory is a function of the chosen `claude`'s pid, and that
  process's cwd is fixed at launch — so the directory only goes stale when the *pid* changes.
  The panel stops re-resolving once attached, so a user who quits `claude`, `cd`s elsewhere in
  the same pane and starts a new one would otherwise keep seeing the old session indefinitely.
  `opened_on` already carries the pid and process start for staleness checks
  (`OpenedOn`, `overlay.rs:880-892`), and `apply_rotation` already re-points on "same pid, new session id"
  (`/clear`); this extends the trigger to "different pid", which is the same question one level
  up.
- **The trigger's state and placement, since "edge-triggered" alone is not derivable.**
  The compare is `matches!((published, last_seen), (Some(a), Some(b)) if a != b)` — **guarded on
  `Some`**, not the bare `published != last_seen`. An earlier draft wrote the bare form, which
  contradicts D6(a) outright: with both as `Option<u32>`, `None != Some(x)` is `true`, so an
  absent published value would read as "a different pid" and storm — the very hazard D6(a)
  exists to prevent. A `None` publish means *no information*: it re-points nothing and it does
  **not** overwrite `last_seen_pid`, so a toggle flip or a transient walk failure cannot erase
  the trigger's memory.
  `last_seen_pid` is a field the overlay owns, updated on every evaluation — deliberately not
  `opened_on.pid`, which is level-triggered (it changes only on a re-open) and is `None` for a
  perfectly live session whose start time could not be read (`overlay.rs:1013-1015`).
  **Placement, stated against the visibility gate rather than only its neighbours.**
  `tick_action` reads `tab_changed` → `!visible ⇒ Idle` → `rotated()` (`overlay.rs:866-877`).
  The pid check goes **below the `!visible` gate** and above `rotated()`. Below `tab_changed`
  because a tab change re-opens unconditionally and re-reads everything; below `!visible`
  because a closed panel must decide nothing — placing it above that gate would let a pid change
  return `Tick::Reopen` for a **closed** panel, and `open()` sets `self.visible = true`
  (`overlay.rs:316`) without the `ctx.set_monitor_open` that only the chord path performs
  (`overlay.rs:1120`), so the panel would paint while the renderer had reserved no band for it.
  Above `rotated()` because a changed pid makes the rotation question moot — it concerns a
  process that is no longer the one being tracked. That file's doc says "the ordering is the
  part a wrong edit breaks", so all three ranks are stated rather than left to the implementer.
- **D10 is a precondition of D5, not merely a cost optimisation.** `open()` has three callers
  (`overlay.rs:1107` chord, `:1164` restore, `:1200` tick) and only the tick path evaluates D5,
  so after a chord-open `last_seen_pid` is stale or `None` and the first tick fires
  `Tick::Reopen`. That is harmless *only* because D10's early-out absorbs it. Rather than rely
  on that, `open()` seeds `last_seen_pid` from the **published** value — never from its own
  hint, since D6(b) forbids mixing the two derivations — and D10 remains the backstop.
- **Premise, labelled as an assumption (A2).** D5's scope rests on "a `claude` process's cwd is
  fixed at launch, so the directory only goes stale when the pid changes". `claude` is a node
  process and `process.chdir()` exists; if it were ever called, D1's directory would go stale
  with no trigger at all. Judged low-probability and left unhandled — but it is load-bearing for
  D5's scope, so it is named rather than assumed silently.
  **Rejected: record it as a Known Gap** — it is not a pre-existing gap but one this spec
  *creates*, because D1 is what makes the directory depend on the pid. Leaving it would ship a
  new staleness alongside the fix.

### D6: D5's trigger is the agent pid the tabmeta worker already derives, published on `TabMeta`
- **Status**: resolved
- **Rationale**: The worker already walks each tab's tree every 500 ms, already re-roots at the
  tmux pane, and already constructs `ClaudeId { pid, start_ms }` for the card's own session
  keying (`crates/app/src/tabmeta/mod.rs:94-97`, built by `worker::Deriver::tab`). The pid D5
  needs is therefore **already computed** — it is simply not published on `TabMeta`
  (`mod.rs:55-70` carries `branch`, `agent`, `cwd`, `task`). Publishing it makes the overlay's
  staleness check an integer compare on a wake it already receives, adding **no** subprocess to
  any path. `publish_if_changed` compares `TabMeta` values, so a changed pid is publishable by
  construction and the wake fires without new plumbing on the worker side.
- **Two hazards the first draft of this decision missed, both found by L2 review.**
  **(a) The value is behind a user-facing config toggle.** `ClaudeId` is constructed inside
  `if self.enabled_agent` (`worker.rs:125`, construction at `:140-143`), which is driven by
  `[sidebar].show_agent_status` (`common/src/config.rs:117`, `main.rs:80`, `Cmd::SetEnabled`).
  With that preference off the published pid is `None` **forever**, so D5 never fires and the
  user watches a dead session — the staleness D5 exists to remove. (The opposite failure, a
  storm from `None != Some(pid)`, is closed separately by D5's `Some`-guarded compare.)
  **The fix is narrower than the first draft claimed, because the precedent it cited says the
  opposite.** That draft said the pid derivation is lifted "exactly as `tmux-pane-cwd` D13
  lifted `cwd`/`root` out of the same gate". D13's own comment ends: "The toggles still skip the
  **expensive work** (`.git` I/O, **the agent walk**); they no longer skip a `proc_pidinfo`"
  (`worker.rs:99-106`). D13 lifted one cheap syscall and deliberately left the agent walk gated
  — so citing it to justify un-gating that very walk inverts it. And the walk is not cheap: a
  bounded BFS of up to `MAX_NODES = 64` nodes (`agent.rs:27`), each a `proc_pidpath` plus, for an
  interpreter, a `sysctl` argv read (`agent.rs:174-213`), per rostered tab per 500 ms tick.
  Lifting it wholesale would impose that on every tab — including tabs with no agent at all —
  for users who have the display off, which is at odds with `tabmeta-session-keying`'s Confirmed
  Goal 4 ("tabs with no Claude agent pay nothing new").
  So the walk is lifted **only for the active tab**: it runs when `self.enabled_agent` **or**
  this tab is the active one. The panel only ever asks about the active tab, so that is the
  whole requirement, and with the toggle off the added cost is one walk per tick for one tab
  rather than one per tab. Everything else stays exactly as gated as today — `branch`,
  `activity()` and `task` remain behind their existing toggles, so turning the display off still
  stops the `.git` I/O and the log reads. An absent published value reads as **"no information"**
  and never as "a different pid" (D5).
  **(b) The two derivations are different algorithms and may legitimately disagree.**
  `hint_for`/`choose_claude` walks `ps -axo args` to `MAX_DEPTH = 6`, matching `argv[0]`'s
  basename against `claude`, pane-first with a fallback to the tab's own subtree **whenever the
  pane yields no `claude`**, over an uncached `resolved_root` (`tabsession.rs:301-335`). The
  worker walks to `MAX_DEPTH = 3`, classifying by `proc_pidpath` plus interpreter-argv, over a
  500 ms-memoised `effective_root` (`tabmeta/agent.rs`), and falls back to the tab's own pid on a
  **different** condition — an unreadable pane cwd, not an absent agent (`worker.rs:108-116`).
  (An earlier draft said the worker had "no tab fallback"; it has one, with a different trigger,
  which is if anything a sharper source of divergence.) Two reachable divergences: a
  `node …/claude-code/cli.js` install classifies as `Claude` to the worker but not to
  `is_claude`; and a tmux tab whose pane has no agent while its tab shell does yields `None`
  from the worker and a real pid from `hint_for`. Under a `published != opened_on` compare
  either case is permanent inequality — a per-wake storm. This is the second reason the compare
  is against `last_seen_pid` (D5): it fires on the published value **changing**, so a stable
  disagreement costs nothing.
  Verified costs, stated honestly — three of them, none a fork:
  (i) `on_active_changed(cwd, shell_pid)` is an `Overlay` **trait** method (`seam.rs:252`,
  `:378`) with a compile-time pin test (`overlay.rs:2192-2197`), so carrying a third value
  widens that seam, both `terminal_app.rs` call sites, and `OverlayHost::notify_active_changed`.
  `AiMonitorOverlay` is the only non-default impl, so "touches every impl" overstated it. That
  pin exists to hold R10.4 (no route from a tab switch to the panel's width); a bare
  `Option<u32>` preserves the property, and the pin is **re-typed** to the new signature rather
  than deleted.
  (ii) `TabMeta` is compared whole by `publish_if_changed`, and `TabMetaPump::poll`
  (`tabmeta/mod.rs:316-321`) turns a publish into a wake and a redraw. So a pid change alone now
  costs a redraw for a value **no card draws**. Accepted: it is bounded by the 500 ms tick and by
  how rarely an agent pid changes, and the alternative is a fork.
  (iii) The active-tab walk described above, when the display toggle is off.
- **Mechanics the lift actually needs, stated so L3 can derive them.** `Deriver::tab(&mut self,
  tp, tick, now)` does **not** know which tab is active — `pass()` does (`worker.rs:224`) and the
  `Cmd::SetActive` arm implicitly does (`:289`). So `tab()` takes an `is_active` parameter, runs
  the walk on `enabled_agent || is_active`, and keeps `agent`/`task`/`branch` behind their
  existing toggles. The published field is `TabMeta::agent_pid: Option<u32>`. D5's timing
  survives the switch path: `titles()` pushes roster + active on every sidebar sync and
  `SetActive` re-derives in band, while D5's `Some`-guarded compare makes the first
  `None → Some(pid)` publish a no-op rather than a re-point.
  One named pin will fail by construction and must be updated, not deleted:
  `tabmeta_still_has_exactly_its_four_published_fields` (`worker.rs:937-947`) destructures
  `TabMeta` exhaustively precisely so that adding a field stops the build. That is the pin doing
  its job; it becomes a five-field assertion.
  **Rejected: narrow `refresh_rivals`' attached guard** — it would put a `/bin/ps` fork and a
  250 ms-budgeted `resolved_root` back on a wake that fires per append burst, which is exactly
  what that guard was added to stop (`overlay.rs:380-385`).
  **Rejected: check only on tab switch / toggle** — zero cost, but a user who stays on one tab
  watches a dead session indefinitely, which is the same class of staleness D5 exists to remove.
  Note this decision reverses, for a *different* question, the reasoning at `overlay.rs:161`
  that refused the tabmeta snapshot for `pane_cwd`. That refusal was about **cold start** — the
  snapshot has no entry until a roster round-trip completes — and it does not apply here,
  because D5's check runs only while the panel is already open on a resolved session.

### D7: The directory is kernel-first with a validated `Registered.cwd` cross-check
- **Status**: resolved (revised after L2 review — the steelman against D1 landed here)
- **Rationale**: Five states, one rule:

  | kernel `proc_pidinfo` | validated registry record | directory used | tier 2 | `SessionHint::cwd` (D12) |
  |---|---|---|---|---|
  | present | absent | kernel | allowed | `Proven` |
  | present | agrees | kernel | allowed | `Proven` |
  | present | **disagrees** | **registry** | **suppressed** | `Disputed` |
  | absent | present | registry | allowed, but `rivals_known` forced `false` | `Proven` |
  | absent | absent | `pane_cwd` fallback | **suppressed** (D3) | `None` |

  A "validated registry record" requires a readable **process start** as well as the file:
  validation is `ai_monitor::validated_session_id`, which takes `process_start_ms`
  (`lib.rs:141-144`), and `hint_for` gets that from the fallible `start_ms(claude)`
  (`tabsession.rs:92`). So the last row needs *three* failures, not two. Harmless in effect — a
  `None` start already empties `Wanted::since_ms` and `baseline_from` — but the table is a
  derivation source for L3, so the third input is named.

  The registry entry is the process's **own statement** about where it is running, already read
  and validated against the process start on the naming path (`registered_id`,
  `tabsession.rs:119`; the rule is `ai_monitor::validated_session_id`). A record passing that
  check describes *this* process, not a recycled pid's ghost, so the directory it names is
  provably the chosen `claude`'s and tier 2 stays allowed. The disagreement row is the part that
  earns its keep: D2 deletes the last comparison in the system, so without it a violation of A1
  (D1's stated assumption) would be undetectable. Here the two independent sources *are* the
  check, and `claude`'s own record wins the tie because the question — "which directory does
  this process write its log into" — is one only `claude` can answer directly.
- **A disagreement SUPPRESSES tier 2; it does not merely re-point the directory.** The first
  draft of this row kept tier 2 allowed, and L2 review showed that inverts the row's purpose in
  two compounding ways.
  **(a) It would make the rival set falsely *known*-empty.** `rival_starts_checked` reads every
  rival's cwd through the kernel (`tabsession.rs:398-404`) and, crucially, a rival whose cwd
  merely *differs* takes a bare `continue` that does **not** set `known = false` — only an
  unreadable cwd or start time does (`:398-401`, `:405-408`). On a disagreement row we would be
  using the registry value `R` for ourselves precisely because the kernel says `K`; the same
  skew must be presumed for other `claude` processes, so no rival's kernel cwd matches `R`,
  every rival is silently dropped, and the result is `(vec![], known: true)`. Per
  `SessionHint::rivals_known`'s own doc that flag "is the switch that re-enables adopting a sole
  candidate at any distance", and granting it "would confirm a foreign log with `owned: true`
  and latch it". The row added to catch an A1 violation would then hand tier 2 a directory
  chosen on a guess, no veto, *and* the unbounded uniqueness rescue.
  **(b) Mixing bases is the deeper error.** Enumerating rivals on the registry basis instead was
  rejected: it would mean reading every rival's `~/.claude/sessions/<pid>.json`, and
  `ai_monitor::Rival`'s doc explains why the type deliberately carries "process facts only".
  Suppressing tier 2 is both simpler and strictly safer, and it makes the detector *actionable*
  rather than merely observed: a disputed directory still resolves by exact filename (tier 1),
  which is the tier that cannot confirm a stranger.
- **Row 4 is mixed-basis too, so it forfeits the rescue rather than pretending otherwise.**
  When the kernel read fails and the registry supplies `R`, `mine_cwd` is `R` while every
  *rival's* cwd is still read from the kernel (`tabsession.rs:398`). Unlike row 3 there is no
  evidence of a systematic skew — our own read simply failed — so the veto is worth having and
  `R` is passed as `mine_cwd`. But this **widens** an existing protection if left alone: today
  an unreadable own cwd hits guard (1) and returns `(vec![], known: false)`
  (`tabsession.rs:387-389`), withholding the uniqueness rescue; enumerating against `R` could
  now return `(vec![], known: true)` and **grant** it — the exact privilege `rivals_known`'s doc
  says "would confirm a foreign log with `owned: true` and latch it". So row 4 keeps the veto
  and forces `rivals_known = false`, preserving today's withholding by construction. Confirmed
  Goal 3 asks for this property "by a stated argument, not by accident"; this is the argument.
- **Where the disagreement is recorded, and how often.** "Recorded" was left as prose in the
  first draft, with no sink and no carrier, which meant A1's only detector detected nothing. It
  is now both: the state travels as `HintCwd::Disputed` (D12), which is what drives the
  suppression, and it is logged. A1 is a stated assumption about a program we do not control, so
  its violation must be visible in a log, not only in behaviour.
  **Rate discipline is part of the decision, not an implementation detail.** `hint_for` runs
  from `refresh_rivals` on a worker thread per append burst while unarmed
  (`overlay.rs:376-385`), so an unguarded `warn!` would emit continuously for a *stable*
  violation. It follows the in-repo precedent (`worker.rs:241-258`): `warn!` once per
  **transition** into the disputed state for a given pid, `debug!` thereafter — "routine
  failures stay at `debug!`, never `warn!`".
- **The resolved cwd must feed the rival enumeration, not just the directory.** This is the
  defect L2 review found in the first draft. `hint_for` computes
  `rival_starts_checked(claude, mine.as_deref(), …)` where `mine = cwd_of_pid(claude)`
  (`tabsession.rs:91`, `:97-98`), and that function's **first guard** returns `(vec![], false)`
  when `mine` is `None` (`tabsession.rs:387-389`). So in exactly the state D7 exists for — the
  kernel read failed, the registry supplied a directory, tier 2 stayed allowed — tier 2 would
  run in the right directory with an **empty rival list**, meaning no rival can veto
  (`lib.rs:430-452` never executes a claim) and the only remaining protection is the ±120s
  distance test. That is precisely the "two tabs in one project, `claude` seconds apart" defect
  `FIRST_EVENT_LEAD_MS`'s doc records as having already shipped once (`lib.rs:296-301`);
  `rivals_known: false` correctly withholds the uniqueness *rescue* but does nothing about the
  *veto*. Therefore `hint_for` resolves the cwd through D7's chain **first**, and passes that
  resolved value into `rival_starts_checked` — the ordering is load-bearing, not incidental.
- **Implementation note**: `registered_id` currently projects the record down to its id, so it
  must return (or its caller retain) the whole `Registered` to reach `.cwd`.
  **Rejected: kernel syscall only.** `registered_session_in`'s doc says the format "must never
  be load-bearing beyond 'use it when it is there'" (`lib.rs:85-86` — note this caveat sits on
  the *reader*, not on `Registered` itself). The answer is that this use *is* "when it is
  there": it is validated by the existing rule before use, and its absence degrades to the
  kernel or to D3's suppression rather than to a guess.
  **Rejected: registry-first, kernel-second.** This is the reviewer's proposed inversion and it
  is a real contender — the registry is the more direct evidence, and it would answer the
  steelman head-on. Rejected on measurement, not on principle: the same sample that found 5/5
  recorded cwds matching also found **1 of 5** live processes with no record at all after 1h17m,
  "for reasons never identified" (`lib.rs:366-372`). Leading with a source that is absent
  roughly a fifth of the time puts an undocumented third-party file on the primary path to buy
  a cross-check that the disagreement row already provides from the fallback position.

### D8: The five gate tests are re-targeted to the invariant that replaces the gate
- **Status**: resolved
- **Rationale**: Each of those tests names a property, not merely a branch, and the properties
  mostly survive D1 in a stronger form — so they are rewritten to pin the replacement, with a
  recorded reason for any that is genuinely deleted. `a_pane_working_elsewhere_is_not_admitted`
  becomes "a pane working elsewhere resolves against **its own** project";
  `an_unreadable_cwd_never_counts_as_agreement` becomes D3/D7's fallback chain;
  `the_gate_compares_the_found_claude_not_the_pane_shell` becomes "the directory comes from the
  found `claude`, not the pane shell", which is D1 restated and still the load-bearing rule.
  `a_rejected_pane_falls_through_to_the_tabs_own_claude` keeps its meaning unchanged — there is
  no rejection any more, but "a pane with no `claude` still falls through, never `None`" is the
  same guarantee and stays.
  **One test must be checked rather than re-targeted**:
  `nothing_re_rooted_means_exactly_one_walk_and_no_cwd_read` asserts `cwds == 0` on the
  non-tmux path. After D2, `choose_claude` performs **no** cwd read on *any* path, so that
  assertion passes for every possible input and becomes vacuous — apparent coverage that can
  never fail. It must be re-pointed at where the cost now lives (`hint_for`) or deleted with a
  stated reason; it must not be left green and meaningless. This repo has an explicit precedent
  for exactly that failure (`tmux.rs:648-651`, a test deleted rather than kept as apparent
  coverage after it was found to assert against a function containing no `stat`).
  **Rejected: delete all five and write fresh** — it loses the record of which historical defect
  each was protecting against, which is the most valuable thing they carry.
- **The re-targeted tests need a seam that does not exist yet.** After D2 the five-row chain of
  D7 lives in `hint_for`, which calls `cwd_of_pid`, `registered_id` and `start_ms` **directly**
  (`tabsession.rs:88-104`) with no injection point — unlike `choose_claude`, `registered_id_with`,
  `resolved_root_with` and `rival_starts_checked`, every one of which this module deliberately
  split for exactly this purpose. Without a `hint_for_with` split taking those three reads as
  closures, the most safety-critical logic in this spec would ship with **no headless test**.
  Adding that split is therefore part of D8, not an implementation detail discovered later.
- **Scope note**: eight `choose_claude` call sites pass the `cwd_of` closure D12's signature
  change removes (`tabsession.rs:820`, `:842`, `:854`, `:875`, `:894`, `:911`, `:933`, `:947`).
  They are loud compile errors rather than a risk, but "five tests" understates the edit.

### D9: The published pid is scoped to the agent kind the panel tracks
- **Status**: resolved (Tier-3 pair-tension finding, D6 × D5)
- **Rationale**: `AgentKind` has five variants — `Claude`, `Antigravity`, `Codex`, `Gemini`,
  `Aider` (`crates/app/src/tabmeta/mod.rs:128-134`) — and tabmeta's walk finds *an agent*, not
  specifically a `claude`. If a pane runs `agy`, an unscoped published pid would never equal
  `opened_on.pid` (a `claude` pid), so D5's compare would fire on **every** tick: a re-point
  storm, each iteration re-forking `/bin/ps`, re-spawning the watcher and re-running the whole
  backfill. So the compare must be **kind-aware** — only a `Claude`-kind pid participates in
  D5's check — and it must be **edge-triggered** on a change, never level-triggered. This is a
  correctness constraint, not a preference: the alternative is a storm, so no option was
  weighed. It also keeps the sibling `ai-monitor-antigravity` work from turning into a defect
  here the moment `agy` panes become real.
- **Status correction from L2 review: this is insurance, not a change.** `ClaudeId` is already
  built as `(kind == AgentKind::Claude).then(…)` (`worker.rs:140`), so the value D6 publishes is
  Claude-only today. D9 therefore *pins* an existing property rather than introducing one — it
  earns its place as a test and a stated invariant, so that adding `agy` support (the sibling
  spec) cannot silently widen the published pid and turn D5's compare into a storm. The
  **absent**-value hazard is a different one and belongs to D6(a).

### D10: A pane `cd` that does not move `claude` costs no re-point — enforced in `open()`, not by re-keying `Repoint`
- **Status**: resolved (Inversion Probe finding)
- **Rationale**: Under D1 the panel's directory is `claude`'s cwd, but `Repoint::changed` keys
  on the **notified** cwd, which `tmux-pane-cwd` makes the pane *shell's* — republished by the
  tabmeta worker on every `cd`. So a `cd` inside the pane fires a full re-point (fork, watcher
  respawn, complete backfill) that resolves the same `claude`, the same directory and the same
  session. Correct, and entirely wasted, on the path D4 exists to make cheaper.
  **The intent is that such a `cd` is inert. The mechanism is deliberately *not* "key `Repoint`
  on the resolved directory".** That spelling reintroduces the exact defect the storm fix
  removed: `changed()` is handed the *notified* cwd by `on_active_changed`, so a key holding the
  *resolved* cwd is compared against a different quantity — on a tmux tab after a `cd` the two
  never match, `changed()` returns `true` on every notification, and the panel re-opens forever.
  `Repoint::opened`'s doc states this directly ("The cwd this panel was last **notified** with —
  never the resolved one", `overlay.rs:475`), and its warning that a transposition once compiled
  with all tests green is precisely a warning about this comparison. Making `changed()` resolve
  its own side instead would fork `/bin/ps` and pay a 250 ms budget per metadata sync, which is
  worse than the waste it fixes.
  So `Repoint` keeps its contract byte-unchanged — `key_cwd` remains the notified value, and
  `changed()` remains a cheap pre-filter — and the idempotence check moves **into `open()`**,
  where the hint and the resolved directory have already been computed at no extra cost.
- **The early-out condition, corrected — the first draft was wrong twice.** It skips the
  teardown-and-rebuild **only** when *all four* hold:
  1. the resolved directory equals the one currently open on;
  2. the chosen `claude` pid equals it too;
  3. **the resolved `session_id` equals `opened_on.session_id`**; and
  4. **`self.visible` and `self.watcher.is_some()`** — i.e. there is actually a live watcher to keep.

  Conditions 3 and 4 are not belt-and-braces; without each of them the early-out breaks a
  shipped feature.
  **Without (3) it swallows `/clear` re-pointing.** `tick_action` returns `Tick::Reopen` for a
  *rotation* as well as a tab change (`overlay.rs:867-877`), and `Tick::Reopen` routes to
  `self.open(cwd, shell_pid)` (`overlay.rs:1200`). A `/clear` rotation is by definition **same
  directory, same pid, new session id** — precisely what conditions 1 and 2 compare equal — so a
  two-value early-out would discard exactly the re-point `ai-monitor-session-rotation` shipped
  to add, and would contradict D5's own rationale, which leans on that path.
  **Without (4) the toggle chord stops working.** `close()` sets `visible = false` and
  `watcher = None` but leaves `opened_on`, `repoint` and `session` intact
  (`overlay.rs:428-432`), while `open()` sets `visible = true` only at its end
  (`overlay.rs:316`). Open → Cmd+Shift+M → Cmd+Shift+M on an unchanged tab would match
  conditions 1–3, return early, and leave the panel invisible with no watcher until the user
  switched tabs. "Keep the existing watcher" presumes a watcher that `close()` has already
  dropped.
  A tab switch still re-opens (different `shell_pid`), a new `claude` still re-opens (D5/D6),
  and a rotation still re-opens (condition 3).
- **Honest scope of the saving.** The early-out sits *after* `hint_for`, so a pane `cd` still
  costs a `/bin/ps` fork and a 250 ms-budgeted `tmux list-clients` on the winit event-loop
  thread; what it saves is the watcher teardown, the respawn and the full backfill. An earlier
  draft called such a `cd` "inert", which overstates it on the very path D4 exists to make
  cheaper. The re-*point* becomes inert; the hint lookup still runs.
- **The early-out must return *after* `Repoint::opened` has re-keyed, and three further
  consequences are named rather than discovered.** D10's motivating case is a pane `cd`, which
  is by definition a **changed notified cwd** — that is why `Repoint::changed` fired at all. If
  the early-out returned before `opened` updated `key_cwd` (`overlay.rs:503-505`), `changed()`
  would stay `true` on every subsequent wake and the panel would re-enter `open()` forever: the
  storm D10 opens by warning about. It is probably safe by construction — condition 1 needs the
  resolved directory, which is `opened`'s return value — but this file has shipped that exact
  bug once, so the ordering is stated, not inferred.
  Two smaller consequences: the early-out also skips the per-open `Config::load()`
  (`overlay.rs:183-185`), so an edited `ai_monitor.context_window` will not take effect until a
  real re-point — acceptable, since that value is already documented as applying "the next time
  the panel is opened". And the early-out is **unavailable for the whole `opened_on: None`
  class** (a live session whose start time could not be read), because condition 2 then has
  nothing to compare; those tabs keep today's re-point cost.
  **Rejected: accept the extra re-point as a Known Gap** — it is created by D1 rather than
  inherited, it lands on the hottest path in this feature, and the fix costs one comparison at a
  point where both operands are already in hand.

### D11: The card and the panel may name different directories, and that is recorded, not fixed
- **Status**: resolved
- **Rationale**: After a `cd` in the pane the sidebar card names the pane shell's directory while
  the panel watches where `claude` runs. They answer different questions — "where is this tab
  working" versus "where is this session's log" — and both answers are correct for what they
  show, so forcing agreement would require one of them to report something untrue. It is
  recorded in Known Gaps so the next reader does not file it as a defect.
  **Rejected: move the card onto the `claude`'s cwd too** — it changes the card's meaning, and
  the sibling `card-live-cwd` spec (uncommitted in `terminal-feat-improving-session-card`) is
  actively reworking the card's cwd tier, so this would collide with unmerged work.

### D12: `SessionHint` gains `Option<HintCwd>` — a three-state value carrying directory *and* provenance
- **Status**: resolved (L2 review: the Data Model dimension was carrying three decisions with no
  stated shape)
- **Rationale**: D1, D3 and D7 all write to one new field and D8 rewrites five tests around it,
  so its contract is stated here rather than left to inference. A first draft made it a bare
  `Option<PathBuf>` with two states; L2 review showed that loses exactly the bit D7's
  disagreement row is *for*, leaving A1's detector with no carrier. The field is
  `SessionHint::cwd: Option<HintCwd>`:

  ```rust
  enum HintCwd {
      /// Kernel and registry agree, or only one of them was available (D7 rows 1, 2, 4).
      Proven(PathBuf),
      /// They disagree — A1 may be violated. Usable for tier 1's exact-filename lookup,
      /// never for tier 2's timing window (D7 row 3).
      Disputed(PathBuf),
  }
  ```

  - `Some(Proven(dir))` — `open()` opens on `dir` and builds
    `Wanted { since_ms: hint.started_ms, .. }`: both tiers.
  - `Some(Disputed(dir))` — `open()` opens on `dir` (the registry's) but builds
    `Wanted { since_ms: None, .. }`: tier 1 only, plus D7's `tracing::warn!`.
  - `None` — no directory at all. `open()` falls back to `pane_cwd`, `since_ms: None`: tier 1
    only (D3, D7 row 5).

  Carrying the *permission* in the same value as the *directory* is the point: it makes
  "unproven directory with tier 2 enabled" — the state both of D7's rejected drafts would have
  created — unrepresentable, rather than forbidden by a rule someone has to remember. This
  mirrors how `SessionHint::pid` already carries a documented invariant ("It is **not** an
  identity on its own"); every field on this struct states one, and a bare `Option<PathBuf>`
  would be the odd one out. The three states map 1:1 onto D7's five rows, which is the check
  that the encoding is sufficient rather than merely convenient.
  One consequence worth naming for L4: with the gate gone (D2), `choose_claude`'s `cwd_of`
  parameter has no remaining caller-visible use, so its signature loses it. That is a
  compile-visible change, and `-D warnings` will reject the unused parameter if it is left.

## Constraints

- **`term-core` stays GPU-free and window-free**; the workspace direction
  `common ← term-core ← renderer ← app` holds. This change lives entirely in `app`, which
  already owns `libc` and the process tree (`.claude/rules/dependency-direction.md`).
- **No new dependencies**, and no GUI/widget crate (`.claude/rules/no-gui-crates.md`).
- **macOS ARM64 only**; no `#[cfg(target_os = …)]` branches (`.claude/rules/macos-only.md`).
- **No change to `crates/ai-monitor`.** D3's tier-2 suppression is expressed by leaving
  `Wanted::since_ms` as `None`; the resolver's strictness is not re-opened.
- **Nothing new on the winit event-loop thread that can block unboundedly.** `resolved_root`'s
  250 ms budget stays the only tmux wait, and D4 reduces the tmux tab's open path from two such
  waits to one. D6 adds no subprocess to any path.
- **`Repoint`'s *keying* contract is unchanged** — `key_cwd` is the notified cwd, never the
  resolved one (D10). Its resolver **parameter** does change, from
  `FnOnce(Option<u32>)` to `FnOnce(Option<&SessionHint>, Option<u32>)` (D4); that widening is
  required for the no-capture guard and is not covered by this constraint.
- **Non-tmux tabs pay no new syscall, and their `choose_claude` result is unchanged** — the
  `root == shell_pid` branch still returns what it returns today (Confirmed Goal 4). Note this
  is deliberately *narrower* than "byte-identical behavior", which an earlier draft asserted and
  D1 does not deliver: today a non-tmux tab opens on the notified cwd (`pane_cwd_with` yields
  `None` when `root == shell_pid`, `overlay.rs:540`, so `opened` returns `notified`), whereas
  under D1 it opens on the chosen `claude`'s cwd. A plain shell that `cd`s after launching
  `claude` therefore gets a *different* — and correct — directory than today. That is an
  improvement and is stated as one rather than asserted away.
- **No path may confirm another application's log with `owned: true`.** Tier 2 runs only against
  a directory the chosen `claude` is provably in (D3/D7), **and never with a falsely
  known-empty rival list**. Two halves: the cwd resolved through D7's chain feeds
  `rival_starts_checked`, so the veto is populated on the same basis as the directory wherever
  tier 2 runs; and the one state where the two bases are *known* to be skewed — D7's
  disagreement row — suppresses tier 2 outright rather than trying to reconcile them.
- **Rotation detection and the toggle chord must survive D10.** `/clear` re-pointing
  (`Tick::Reopen` from `session_rotated`) and close→reopen must both still work; D10's early-out
  compares the session id and requires a live watcher for exactly this reason.
- **The published agent pid must not be gated on a display preference.** D6 lifts it out of
  `enabled_agent`, so `[sidebar].show_agent_status` cannot silently disable D5's trigger.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` clean;
  `rustfmt` runs as a PostToolUse hook on every edited `.rs`.

## Requirements

### R0: A tmux tab's panel resolves the session running in its pane *(manual gate, R0.1)*

#### R0.1: The reported configuration resolves *(manual, on a live machine)*
- **Given**: a tab whose shell sits in `~`, running `tmux attach -t <s>`, whose pane runs
  `claude` in a worktree — the measured shape (tab zsh `58495`, pane `claude` `58395`)
- **When**: the panel is opened with Cmd+Shift+M
- **Then**: the pill reads `CONNECTED`, the session row names a session id, and the dashboard
  rows populate — where today it reads `NO SESSION` / `(no Claude session in this tab)`

#### R0.2: A pane in a subdirectory of the tab's project resolves
- **Given**: a tmux tab whose pane's `claude` runs in `<proj>/crates`, tab shell in `<proj>`
- **When**: `hint_for` runs
- **Then**: it returns `Some`, and the resolved directory is `encode_cwd(<proj>/crates)` — the
  directory `claude` actually writes into (D7c's rejection no longer applies)

#### R0.3: A non-tmux tab is unaffected
- **Given**: a tab running `claude` directly, no tmux client anywhere in its tree
- **When**: `hint_for` runs
- **Then**: `choose_claude` takes the `root == shell_pid` path and returns the same pid it
  returns today, with no additional syscall

#### R0.4: The panel confirms ownership rather than ranking
- **Given**: the tab of R0.1
- **When**: the overlay resolves its log
- **Then**: `Resolved.owned == true`

### R1: `SessionHint` carries the chosen `claude`'s directory as a three-state value *(D1, D12)*

#### R1.1: The hint carries the directory of the process it describes
- **Given**: `hint_for` has chosen `claude` pid P
- **When**: the `SessionHint` is built
- **Then**: `cwd` is derived from **P's own** cwd, not from the tab shell's and not from the
  pane shell's

#### R1.2: `HintCwd` distinguishes proven from disputed
- **Given**: the post-change `SessionHint`
- **When**: its `cwd` field is inspected
- **Then**: it is `Option<HintCwd>` with exactly `Proven(PathBuf)` and `Disputed(PathBuf)`, and
  the three states map onto D7's five rows

#### R1.3: The directory costs no additional syscall
- **Given**: `hint_for` before and after the change
- **When**: the `cwd_of_pid` calls are counted for one invocation
- **Then**: the count is unchanged — the value already read at `tabsession.rs:91` is reused

#### R1.4: A tab with no `claude` yields no hint at all
- **Given**: a tab whose tree (and pane, if any) contains no `claude`
- **When**: `hint_for` runs
- **Then**: it returns `None`, so `tab_has_claude` is `false` and the panel says
  "(no Claude session in this tab)"

### R2: The pane branch admits its `claude` with no directory comparison *(D2)*

#### R2.1: A pane working in a different directory than the tab shell is admitted
- **Given**: `choose_claude(root = pane, shell_pid = tab)` where the pane's `claude` cwd is
  `/proj/a` and the tab shell's is `/Users/me`
- **When**: it runs
- **Then**: it returns the pane's `claude` — today it returns the tab-subtree result, which for
  a tmux tab is `None`

#### R2.2: The pane still wins over the tab's own `claude`
- **Given**: a tab with a `claude` in both its own subtree and its tmux pane
- **When**: `choose_claude` runs
- **Then**: the pane's pid is returned

#### R2.3: A pane with no `claude` still falls through, never `None`
- **Given**: a re-rooted tab whose pane has no `claude` but whose own shell does
- **When**: `choose_claude` runs
- **Then**: the tab's own `claude` is returned

#### R2.4: `choose_claude` performs no cwd read on any path
- **Given**: a counting cwd reader passed to `choose_claude`
- **When**: it is driven with a re-rooted tab, a non-re-rooted tab and a dead pid
- **Then**: the reader is called zero times in every case, and the parameter is gone from the
  signature

### R3: The directory chain decides the directory *and* the tier permission *(D3, D7)*

#### R3.1: Kernel cwd present, no registry record → proven
- **Given**: `proc_pidinfo` yields `/proj/a` for the chosen `claude`; no `<pid>.json`
- **When**: the hint is built
- **Then**: `cwd == Some(Proven("/proj/a"))` and `Wanted::since_ms == hint.started_ms`

#### R3.2: Kernel and registry agree → proven
- **Given**: both yield `/proj/a`, the record validating against the process start
- **When**: the hint is built
- **Then**: `cwd == Some(Proven("/proj/a"))`, tier 2 permitted

#### R3.3: Kernel and registry disagree → disputed, registry wins, tier 2 suppressed
- **Given**: the kernel yields `/proj/a`, a validated record yields `/proj/b`
- **When**: the hint is built
- **Then**: `cwd == Some(Disputed("/proj/b"))`, `Wanted::since_ms == None`, and one
  `tracing::warn!` is emitted naming the pid and both directories

#### R3.4: Kernel read fails, registry present → proven, rescue withheld
- **Given**: `proc_pidinfo` fails; a validated record yields `/proj/a`
- **When**: the hint is built
- **Then**: `cwd == Some(Proven("/proj/a"))`, tier 2 permitted, and `rivals_known == false`

#### R3.5: Both sources fail → no directory, tier 2 suppressed
- **Given**: `proc_pidinfo` fails and no validated record exists
- **When**: the hint is built
- **Then**: `cwd == None`, `Wanted::since_ms == None`; `open()` falls back to `pane_cwd`

#### R3.6: Suppression is expressed on `Wanted`, never on the hint
- **Given**: any suppressed case (R3.3, R3.5)
- **When**: `SessionHint::started_ms` is inspected
- **Then**: it still holds the real process start — only `Wanted::since_ms` is `None`, so
  `baseline_from` still yields an `OpenedOn` and rotation detection stays alive

#### R3.7: An unreadable process start cannot validate a record
- **Given**: a `<pid>.json` exists but `start_ms(claude)` returns `None`
- **When**: the chain runs
- **Then**: the record is not treated as validated, and the case degrades to R3.5

#### R3.8: A wrong directory can never confirm a foreign log
- **Given**: a hint whose directory is not the one `claude` writes into
- **When**: resolution runs with tier 2 suppressed
- **Then**: tier 1's exact-filename lookup misses and `resolve_owned_session` returns `None` —
  no `owned: true` against another application's log

### R4: Rival enumeration shares the directory's basis *(D7)*

#### R4.1: The resolved directory feeds the rival enumeration
- **Given**: the chain resolved the directory from the registry (R3.4)
- **When**: `rival_starts_checked` is called
- **Then**: it is passed that resolved directory as `mine_cwd`, not the raw `cwd_of_pid` result

#### R4.2: A registry-sourced directory never grants the uniqueness rescue
- **Given**: R3.4's state, with no rival found in that directory
- **When**: the rival set is published
- **Then**: `known == false`, preserving today's withholding on a failed own-cwd read

#### R4.3: `refresh_rivals` recovers under tmux
- **Given**: the tab of R0.1, panel open and unattached
- **When**: `refresh_rivals` runs on its worker thread
- **Then**: it publishes a real `RivalSet` from a `Some` hint — where today `hint_for` returns
  `None` and it publishes `RivalSet::default()` forever

### R5: `open()` resolves the hint first, through a named resolver *(D4)*

#### R5.1: The hint is resolved before the directory
- **Given**: `open()` after the change
- **When**: its statement order is inspected
- **Then**: `hint_for` is called above `Repoint::opened`, and the hint's directory is what
  `opened`'s resolver returns

#### R5.2: The resolver is a named function, not a closure
- **Given**: the `Repoint::opened` call site
- **When**: it is inspected (source-text pin, in the idiom at `overlay.rs:2612-2638`)
- **Then**: it passes `hint_or_pane_cwd` by name, and `opened`'s parameter is
  `FnOnce(Option<&SessionHint>, Option<u32>) -> Option<PathBuf>`

#### R5.3: Both `HintCwd` variants supply the directory; only `None` falls back
- **Given**: hints with `Proven(d)`, `Disputed(d)` and `None`
- **When**: the resolver runs
- **Then**: the first two yield `d`; only the third calls `pane_cwd`

#### R5.4: `Repoint`'s key stays the notified cwd
- **Given**: a hint whose directory **differs** from the notified cwd
- **When**: `opened` is called
- **Then**: `key_cwd == notified`, and a subsequent `changed()` with that same notified cwd is
  `false` — the storm assertion

#### R5.5: A tmux tab with a session pays one tmux resolve per open
- **Given**: a tmux tab whose pane runs `claude`
- **When**: `open()` runs with `resolved_root` counted
- **Then**: it is invoked once, not twice

### R6: A changed `claude` pid re-points the panel *(D5, D6, D9)*

#### R6.1: The worker publishes the agent pid *(producer side)*
- **Given**: a tab whose pane runs `claude` at pid P
- **When**: the metadata tick derives that tab
- **Then**: `TabMeta::agent_pid == Some(P)`, and a change in P makes `publish_if_changed`
  publish

#### R6.2: The pid is published for the active tab even with the display toggle off
- **Given**: `[sidebar].show_agent_status == false`, tab T active with `claude` at P
- **When**: the tick derives T
- **Then**: `agent_pid == Some(P)`; non-active tabs derive no agent and `branch` / `task` /
  `activity` stay gated as today

#### R6.3: Only a Claude-kind agent is published
- **Given**: a pane running `agy` (Antigravity)
- **When**: the tick derives that tab
- **Then**: `agent_pid == None`

#### R6.4: The compare is `Some`-guarded *(consumer side)*
- **Given**: `last_seen_pid == Some(P)` and a publish of `None`
- **When**: the tick evaluates
- **Then**: no re-point occurs and `last_seen_pid` still holds `Some(P)`

#### R6.5: A changed pid re-points; an unchanged one does not
- **Given**: `last_seen_pid == Some(P)`
- **When**: `Some(Q)` with `Q != P` is published
- **Then**: `Tick::Reopen`; with `Some(P)` republished, `Tick::RefreshRivals`

#### R6.6: A closed panel decides nothing
- **Given**: the panel is closed and the published pid changes
- **When**: `tick_action` runs
- **Then**: it returns `Tick::Idle` — the pid check sits **below** the `!visible` gate, so the
  panel cannot paint into a band the renderer never reserved

#### R6.7: End-to-end — a new `claude` in the same pane moves the panel
- **Given**: the panel is attached to a session in pane P's `claude`
- **When**: the user quits it, `cd`s elsewhere in that pane and starts a new `claude`
- **Then**: within one metadata tick the panel re-points to the new session and directory

#### R6.8: The `TabMeta` field pin is updated, not deleted
- **Given**: `tabmeta_still_has_exactly_its_four_published_fields` (`worker.rs:937-947`)
- **When**: the field is added
- **Then**: the pin destructures five fields and still fails on any further change

### R7: An unchanged re-point is absorbed without a teardown *(D10)*

#### R7.1: A pane `cd` keeps the watcher
- **Given**: the panel is open on directory D with `claude` pid P and session S
- **When**: a `cd` in the pane republishes a new notified cwd and `open()` re-runs, resolving
  the same D, P and S
- **Then**: the existing watcher is kept — no respawn, no re-backfill

#### R7.2: A `/clear` rotation still re-points
- **Given**: same directory, same pid, **new** session id
- **When**: `Tick::Reopen` reaches `open()`
- **Then**: the early-out does **not** fire and the panel re-points

#### R7.3: The toggle chord still shows the panel
- **Given**: the panel is open, then closed with Cmd+Shift+M, tab unchanged
- **When**: the chord re-opens it
- **Then**: `visible == true` and a watcher exists — the early-out requires both

#### R7.4: The early-out returns only after `Repoint` has re-keyed
- **Given**: an early-out path
- **When**: it returns
- **Then**: `key_cwd` already holds the new notified cwd, so `changed()` is `false` on the next
  wake rather than re-entering `open()` forever

### R8: The gate's tests are re-targeted, and the new chain is testable *(D8)*

#### R8.1: Each of the five gate tests is re-targeted or deleted with a reason
- **Given**: the five tests naming the old gate
- **When**: the change lands
- **Then**: each pins a replacement property, or its removal carries a recorded reason

#### R8.2: The vacuous assertion is not left green
- **Given**: `nothing_re_rooted_means_exactly_one_walk_and_no_cwd_read`, whose `cwds == 0`
  assertion holds for every input once D2 lands
- **When**: the change lands
- **Then**: it is re-pointed at where the cost now lives, or deleted with a stated reason —
  never kept as apparent coverage

#### R8.3: The directory chain has an injectable seam
- **Given**: `hint_for`, which calls `cwd_of_pid`, `registered_id` and `start_ms` directly
- **When**: the change lands
- **Then**: a `hint_for_with` split takes those reads as closures

#### R8.4: All five rows are covered headlessly
- **Given**: the `hint_for_with` seam
- **When**: the suite runs on a machine with no tmux and no live `claude`
- **Then**: R3.1–R3.5 each have a passing test

### R9: The card and the panel may name different directories *(D11)*

#### R9.1: The divergence is recorded, not eliminated
- **Given**: a pane whose `claude` runs in `<proj>` while the pane shell has `cd`'d to
  `<proj>/crates`
- **When**: both surfaces render
- **Then**: the card names `crates` and the panel watches `<proj>`, and this is documented in
  Known Gaps rather than treated as a defect

### R10: Cost and non-regression *(Constraints)*

#### R10.1: No new unbounded blocking on the event-loop thread
- **Given**: a wedged tmux server
- **When**: the panel is opened on a tmux tab
- **Then**: `resolved_root`'s 250 ms budget still bounds it and the UI stays responsive

#### R10.2: `crates/ai-monitor` is unchanged
- **Given**: the landed diff
- **When**: it is inspected
- **Then**: no file under `crates/ai-monitor/` is modified

#### R10.3: The workspace stays clean
- **Given**: the landed change
- **When**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
  run
- **Then**: both pass

#### R10.4: The seam pin survives the widening
- **Given**: the `on_active_changed` compile-time pin (`overlay.rs:2196-2197`)
- **When**: the trait signature widens
- **Then**: the pin is re-typed and still pins R10.4's property (no route from a tab switch to
  the panel's width)

## Tasks

Sequencing is driven by **file overlap**, which here is the binding constraint: R1–R4 and R8 all
land in `tabsession.rs`, R5/R7 and the consumer half of R6 all land in `overlay.rs`. Tasks that
share a file are serialized even when their changes are logically independent. T5 is the one
genuinely parallel slice — it touches only `crates/app/src/tabmeta/`.

### T1: Delete the gate and carry the chosen `claude`'s kernel cwd on the hint [vertical]
- **Fulfills**: R1.1, R1.3, R1.4, R2, R8.1, R8.2
- **Depends on**: (none)
- **Files**: `crates/app/src/tabsession.rs`
- **Note**: this task alone makes a `tmux attach` tab resolve. `SessionHint::cwd` lands here in
  its two-state form (`Some` = proven by the kernel, tier 2 permitted; `None` = suppressed), so
  the tree is never in a state that permits tier 2 against an unproven directory. T2 upgrades it
  to three states.

### T2: The provenance chain — `HintCwd`, the registry cross-check, and the rival basis [vertical]
- **Fulfills**: R1.2, R3, R4.1, R4.2, R8.3, R8.4, R10.2
- **Depends on**: T1
- **Files**: `crates/app/src/tabsession.rs`
- **Note**: serialized behind T1 by file overlap, not by logic. Adds the `hint_for_with` seam
  first (R8.3), so the five rows are landed under test rather than tested afterwards.

### T3: `open()` resolves the hint first, through a named resolver [vertical]
- **Fulfills**: R0, R5, R4.3, R10.1
- **Depends on**: T2
- **Files**: `crates/app/src/overlay.rs`
- **Note**: the user-visible outcome (R0) lands here — this is the first point at which the
  panel actually lights up on a tmux tab. `Repoint::opened`'s resolver parameter widens to
  `FnOnce(Option<&SessionHint>, Option<u32>)` in this task (D4).

### T4: The idempotence early-out in `open()` [vertical]
- **Fulfills**: R7
- **Depends on**: T3
- **Files**: `crates/app/src/overlay.rs`
- **Note**: all four conditions land together — a two-condition version would regress `/clear`
  (R7.2) and the toggle chord (R7.3), both of which are shipped behavior.

### T5: Publish the agent pid on `TabMeta` [infra] ← parallel with T1–T4
- **Fulfills**: R6.1, R6.2, R6.3, R6.8
- **Depends on**: (none)
- **Files**: `crates/app/src/tabmeta/mod.rs`, `crates/app/src/tabmeta/worker.rs`
- **Note**: horizontal by the infra exception — it is a producer with no consumer until T6, and
  it shares no file with T1–T4, so it runs concurrently with the whole chain. Includes
  `Deriver::tab`'s `is_active` parameter and the `enabled_agent || is_active` lift, with
  `agent`/`task`/`branch` left gated as today.

### T6: Widen the seam and re-point on a changed pid [vertical]
- **Fulfills**: R6.4, R6.5, R6.6, R6.7, R9, R10.3, R10.4
- **Depends on**: T4, T5
- **Files**: `crates/app/src/seam.rs`, `crates/app/src/terminal_app.rs`,
  `crates/app/src/overlay.rs`
- **Note**: the trait signature and its only impl must change together to compile, which is why
  the seam widening sits with the consumer rather than with T5. Depends on T4 for `overlay.rs`
  file overlap and on T5 for the value it consumes. As the last task it also carries the
  workspace-clean check (R10.3).

## External Dependencies

### Pre-work
- A live tmux session with `claude` running in a pane, attached from a mac-terminal tab whose
  own shell is in a **different** directory — the fixture R0.1's manual gate needs. The
  measured setup (`tmux new-session -d -s <name> -c <worktree>`, then `tmux attach` from `~`)
  reproduces it.

### Post-work
- **[DONE 2026-08-16] Run R0.1's manual gate** on that fixture. No headless test can cover it: it
  needs a real tmux client, a real pane and a real `claude` writing a real log — the whole chain
  the hermetic tests replace with `ProcTable::parse` and injected closures.

  Fixture built for the run, measured rather than assumed:

  | | value |
  |---|---|
  | tmux session | `pane-cwd-gate-test`, rooted at this worktree |
  | pane shell | `17525` |
  | pane `claude` | `17591`, kernel cwd `…/terminal-fix-tabsession-pane-cwd-gate` |
  | registry record | `sessionId 9bb64bfb-9722-4ec1-8c4d-3134e86e1073`, `cwd` **agreeing** with the kernel |
  | chain row | D7 row 2 — `Proven`, tier 2 permitted |
  | tab shell | a different directory from the pane's `claude` — the condition the deleted gate failed on |

  Result: the panel showed **`CONNECTED` with the session id**, where before this change the same
  shape read `NO SESSION` / "(no Claude session in this tab)". That settles **R0.1** and, since
  the pill reads `CONNECTED` only for a confirmed resolution, **R0.4** (`Resolved.owned == true`)
  with it. The session's log reached 77 KB during the run, so tier 1's exact-filename lookup was
  resolving against a live, growing file rather than a stale one.

  **R6.7 was not exercised** by this run and remains outstanding: it needs the pane's `claude` to
  be quit, the pane `cd`'d, and a new `claude` started, so that the published pid changes under a
  panel that is already attached.
- **Rebase the sibling `card-live-cwd`** (uncommitted in `terminal-feat-improving-session-card`)
  onto the new `Deriver::tab` signature — see Declared Surface's overlap note.
- Re-check the ai-monitor panel on a **non**-tmux tab, since R0.3's guarantee is narrower than
  "unchanged" (a plain shell that `cd`s after launching `claude` now resolves differently — and
  correctly).

## Known Gaps

- **The card and the panel can name different directories** after a `cd` in the pane (D11).
  Correct for what each shows, recorded so it is not filed as a defect.
- **Non-default tmux sockets (`tmux -L work`) still never resolve** — inherited from
  `tmux-pane-resolution` D14; such a tab keeps today's behavior rather than getting a wrong one.
- **tmux behind a wrapper is not detected** — the presence gate stops at depth 1
  (`tabmeta/tmux.rs:69-72`), inherited and unchanged.
- **Nested tmux and multi-pane clients** resolve to the client's active pane only — inherited.
- **The `ProcTable` snapshot race** can still cost one stale hint on an overlay open — inherited
  from `tabsession-tmux-reroot` D6, untouched here.
- **L2 assumed: Error/Edge — the both-unknown wording.** When the cwd is unreadable by syscall
  *and* by registry *and* no session id is known, the tab shows "(this tab's session has not
  written a log yet)". Not separately decided: `dashboard.rs:299-303` already fixes this policy
  ("the key is whether the tab runs a `claude` process, deliberately NOT whether its hint was
  usable"), and that rule gives the better of the two wordings here.
- **L2 assumed: Implementation — the manual verification gate.** Proposed as the two live checks
  in R7; not separately negotiated with the user.
- **How `open()` reaches the published pid on two of its three callers is unspecified.** D5 has
  `open()` seed `last_seen_pid` from the published value, but the chord (`overlay.rs:1107`, via
  `AppCtx`) and the launch restore (`:1164`, via `OverlayCommand::RestoreMonitor`) carry no agent
  pid, and neither type is in the Declared Surface. The natural carrier — the overlay retaining
  the last pid `on_active_changed` handed it — is left to L4. Bounded because D10's early-out is
  the declared backstop for exactly this case.
- **A1 (D1): `claude` writes to `encode_cwd` of the directory it runs in.** Load-bearing for the
  whole design, evidenced at 5/5 on one machine on one day (`lib.rs:52-54`). D2 removes the last
  comparison that could detect a violation; D7's disagreement row is the only remaining
  detector, and it only fires when a registry record is also present.
- **A2 (D5): a `claude` process's cwd is fixed at launch.** If `process.chdir()` were ever
  called, D1's directory goes stale with no trigger, since the pid does not change. Judged
  low-probability and left unhandled.
- **Declared-surface overlap on `crates/app/src/tabmeta/worker.rs`** with the uncommitted
  sibling `card-live-cwd` — both features **write** that file. Recorded in Declared Surface with
  a recommended landing order (this feature first; it only adds a field). An earlier draft
  called this "merge-order risk, not a partition conflict", which understated it.
- **Merge-order risk with `ai-monitor-antigravity`** (uncommitted in
  `terminal-feat-ai-monitor-antigravity`), which spans `ai-monitor` + `app` broadly enough to
  touch `overlay.rs`. It declares no file this feature writes, so this is ordering only — but
  see D9, which exists so that adding `agy` support cannot silently widen the published pid.
