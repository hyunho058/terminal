# Spec: tmux-pane-cwd

## Meta
- **Created**: 2026-07-29
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-29
- **Complexity**: Moderate (3 signals — a cross-boundary dependency on two sibling features
  that are specified but unbuilt; a renegotiation of the sibling partition's declared
  surface; and a counted per-keystroke performance budget as a first-class constraint. Two
  further signals present in the first draft — blocking subprocess I/O on the winit event
  loop, and a multi-variable debounce/retry state machine — were **removed** by D1's
  revision rather than mitigated.)
- **L2-reviewer**: NEEDS_FIX ×2 (round 1: 12 findings — D1 re-placed off the UI thread;
  round 2: 7 findings — the toggle gate, the cold-start window and the publication-predicate
  change). All findings verified against both codebases before acceptance; both retries spent.

## Declared Surface
- feature-id: tmux-pane-cwd
- depends_on: [tmux-root-resolution, tabmeta-tmux-reroot]
- declared-surface:
  - crates/app/src/session.rs
  - crates/app/src/tabs.rs
  - crates/app/src/terminal_app.rs
  - crates/app/src/overlay.rs          # cold-start fresh lookup — see D12
  - crates/app/src/tabmeta/mod.rs      # partition renegotiation — see D8
  - crates/app/src/tabmeta/worker.rs   # partition renegotiation — see D8
- declared-deviation:
  - crates/app/src/tabsession.rs       # ONE keyword: `fn resolved_root` -> `pub(crate) fn`.
    # C3 assigns this file to the sibling set, and the alternative was a second copy of its
    # 250 ms `recv_timeout` guard around a call that can hang the UI. Duplicating that
    # safety logic is worse than widening one visibility, so the guard is shared instead.
    # Zero behavior change; the diff is the keyword plus its rationale comment.

## Goal
When using tmux, the ai-monitor overlay shows no AI status data — `session (no active
session)` and empty Tasks / Skills / File Activity / Agents rows. Part of that is the
process-tree walk (owned by the sibling `tmux-pane-resolution` set); this spec owns the
other part: the tab reports the **tmux client's** working directory as its own, so the
overlay resolves the wrong `~/.claude/projects/<encoded-cwd>/` directory and the tab's
cwd-derived title names the wrong project.

## Non-goals
- Re-rooting the **process-tree** walks — `tabsession::hint_for` (session id / start-time
  hint) and `tabmeta`'s agent walk (agent badge, task label). The sibling
  `specs/tmux-pane-resolution/` set in the `terminal-fix-tmux-pane-resolution` worktree
  owns those, and this spec depends on two of its features.
- Building the tty-devno resolver itself — `tmux-root-resolution` owns it (D9).
- Multiplexers other than tmux (screen, zellij) and tmux control mode (`-CC`).
- Treating tmux panes as first-class tabs, or any tmux integration beyond cwd resolution.

## Confirmed Goal
Make a tmux tab's effective working directory that of its **active pane**, so
`Session::cwd()`'s three consumers follow the pane: the ai-monitor overlay opens on the
pane's project, the sidebar row's cwd-basename title names the pane's directory, and the
rename prefill matches what is on the card. The cwd must **follow the pane live** —
switching tmux windows re-points an open overlay and relabels the tab, exactly as `cd`
does today. With no tmux present, behavior and per-keystroke cost are unchanged or better.

## Research

### The measured root cause
- A tmux tab's shell has only the **client** under it; the pane's shell is a child of the
  tmux **server**, whose ppid is 1. Measured on this machine while the bug was live:
  `8060 zsh → 8073 tmux attach -t terminal-fix-ai-monitor-tmux`, while the pane is
  `90662 tmux new-session (ppid 1) → 7503 -zsh`, and `tmux list-panes -a` reports that
  pane's `cwd=/Users/hyunhokim/IdeaProjects/terminal-fix-ai-monitor-tmux`. No descendant
  walk or `proc_pidinfo` rooted at the tab's shell can cross that gap.
- **The wrong directory is load-bearing, not cosmetic.** On the tab from the bug report:
  the tab's shell (`8060 zsh`) and its client (`8073 tmux attach`) both have
  `cwd = /Users/hyunhokim`, while the pane (`7503 -zsh`) is in
  `.../terminal-fix-ai-monitor-tmux`. So `project_dir` resolves
  `~/.claude/projects/-Users-hyunhokim/`, **which does not exist on this machine**, while
  the pane's correct directory holds a live log. `resolve_session` therefore finds nothing,
  `open()` seeds an empty `SessionTelemetry` (`overlay.rs:126-130`), and the panel renders
  `session (no active session)` with every row empty — exactly the reported screenshot.
  This spec alone is sufficient to fix that symptom.
- `Session::cwd()` is exactly such a call — `cwd_of_pid(self.pty.child_pid()?)`
  (`crates/app/src/session.rs:170-172`, syscall at `:232-257`). It returns the **client's**
  parent-shell cwd, which only coincides with the pane's by luck of how the attach was
  launched, and never follows a `cd` performed inside the pane.

### The three consumers of `Session::cwd()` (this spec's blast radius)
- **ai-monitor log directory** — `overlay.rs:317` passes `ctx.active_cwd` into `open()`,
  which resolves `cwd.as_deref().and_then(project_dir)` (`overlay.rs:108`).
  `project_dir` is `claude_projects_dir()?.join(encode_cwd(cwd))`
  (`crates/ai-monitor/src/lib.rs:65-66`), so a wrong cwd means a wrong — or absent —
  `~/.claude/projects/<encoded-cwd>/` directory, and every dashboard row is empty
  regardless of what the session hint does.
- **Sidebar row title** — the cwd basename, the bottom tier of the label chain, in
  `Tabs::titles()` (`crates/app/src/tabs.rs:173`, cwd read at `:196`, fallback `Tab {n}`
  at `:200`).
- **Rename prefill** — `displayed_label` (`crates/app/src/terminal_app.rs:739-745`).
- `active_cwd()` / `active_shell_pid()` are the accessors (`tabs.rs:219-227`).

### Cost hazard — the binding constraint on any design
- `self.tabs.active_cwd()` is evaluated on **every key press**, twice per dispatch path
  (`terminal_app.rs:1419` and `:1462`). Today that is one `proc_pidinfo`; a
  `tmux list-clients` there would be a **subprocess spawn per keystroke**.
- `Tabs::titles()` calls `s.cwd()` **once per session per model resync** (`tabs.rs:191-196`),
  so the cost also multiplies by tab count.
- **`drain()` runs for background tabs too** — the `PtyOutput` handler resolves the session
  `by_id`, and its own comment says "(possibly background) session's PTY"
  (`terminal_app.rs:1545-1567`). Only the *redraw* is active-gated. So anything placed in
  `drain` is paid by every tab producing output, not just the visible one.
- The codebase already treats this cost as unacceptable: `displayed_label` is gated behind
  a bare modifier test *specifically* because "a `proc_pidinfo` (for the cwd) plus a
  metadata sync … is NOT paid per keystroke" (`terminal_app.rs:1425-1433`).

### The tabmeta worker — where the derivation already lives
- The worker thread derives every rostered tab's `TabMeta` on a tick: 500 ms for the active
  tab, every 4th tick for background tabs, striped by slot (`tabmeta/worker.rs:27-37`).
- It resolves a cwd at `worker.rs:69` (`proc::cwd_of_pid(tp.pid)`), shared by the git-branch
  and activity paths, and the sibling's `tabmeta-tmux-reroot` re-roots exactly that call —
  **but it is conditional**: `worker.rs:67-68` skips the resolution entirely unless
  `enabled_git || enabled_agent`. Those flags come from `Cmd::SetEnabled` (`worker.rs:216-218`),
  wired at `main.rs:51-52` to the user-facing TOML booleans `sidebar.show_git_branch` /
  `sidebar.show_agent_status` (`crates/common/src/config.rs:81-83`, both default true at
  `:135-136`). With both off the worker publishes no cwd at all. See D2.
- `TabMeta` carries `branch` / `agent` / `task` and **no cwd** (`tabmeta/mod.rs:40-49`);
  `TabMetaSnapshot::get(id) -> Option<&TabMeta>` is the app-side read (`mod.rs:230-232`).
  `TabMeta` derives `PartialEq` (`mod.rs:39`) and `publish_if_changed` is `next == *published`
  over `Vec<(TabId, TabMeta)>` (`worker.rs:238-244`) — so any field added to `TabMeta` joins
  the publication predicate.
- **A tab not due this tick keeps its previous meta**: `Deriver::pass` carries `prev` forward
  for background tabs off-phase (`worker.rs:143-148`), so a published value can be up to
  `BG_INTERVAL × TICK` = 2 s old.
- **The roster is rebuilt only in `Tabs::titles()`** (`tabs.rs:179-188`, the sole
  `set_roster` caller), reached only from `sync_tab_model()` — whose five call sites
  (`terminal_app.rs:352, :669, :802, :1227, :1581`) are startup and tab/title actions. **A
  shell exiting fires none of them**, so a dead tab keeps its roster entry.
- The worker may not exist at all: `TabMetaHandle::spawn` returns `Option` (`mod.rs:160-172`)
  and `global()` is `None` if `install` never ran (`:311-313`).
- Publication is **change-only**: `publish_if_changed` skips the wake when nothing moved —
  a deliberate fix for the G1 idle-CPU debt, and the app is woken by
  `UserEvent::OverlayPending` (`terminal_app.rs:1599-1605`, handler at `:1647`), which
  syncs the snapshot via `feed_agent_states` (`:904-909`).

### The foundation this spec depends on (sibling set)
- `tmux-root-resolution` specifies `crates/app/src/tabmeta/tmux.rs` exposing
  `effective_root(&mut Memo, pid, tick) -> u32` (memoized) and
  `effective_root_uncached(pid) -> u32`. Both return a **pid, never `Option`** (its D4/R6.1).
  `mod tmux`, both functions, `Memo` and `Memo::new` are all `pub(crate)` (its R7 §531).
- **The memo is the aggregate budget.** SD3's guarantee is "N tmux tabs cost one spawn per
  tick, **not N**", delivered by the per-tick `Memo`; `effective_root_uncached` deliberately
  has none, and its D10 licenses that only because `hint_for` is "a user-initiated event and
  **not on any cadence**". Anything on a cadence must use the memoized form.
- Every fetch attempt writes a `Memo` entry **including failures** (its D9(e)), so a failed
  lookup is negatively cached for the rest of the tick and `TMUX_LIST <= 1` per tick holds
  regardless of outcome.
- **"Zero cost" means zero *spawns*, not zero syscalls** (its D9): the presence gate costs
  `2 + k` syscalls per tab per refresh for a non-tmux tab, asserted by `PROC_EXE == 1 + k`
  and `PROC_CHILDREN == 1` (its R2.6). Its D13 adds a liveness `cwd_of_pid(pane_pid)` inside
  `effective_root`, and expects each consumer to carry that in its own ledger.
- It declares **three** probe counters, not two: `TMUX_LIST`, `PROC_TTY` and `TTY_STAT`
  (its D15 — the third covers the `stat` of each `client_tty`, and its Known Gaps records
  that this refines the manifest's literal "two").
- `tabsession-tmux-reroot`'s D8 is the direct precedent for placement: `tmux list-clients`
  "talks to a **user-space server over a unix socket** and can block indefinitely (a
  SIGSTOPped server, a wedged `$TMUX_TMPDIR`). If that happens the whole UI freezes — input
  and rendering… That is not acceptable." It mandates a thread plus
  `recv_timeout(RESOLVE_BUDGET)` with `RESOLVE_BUDGET = 250 ms`.

### Reusable patterns and constraints
- Memo shape: `git::Memo` (`tabmeta/git.rs:35`, `:57`) with TTL + `MEMO_MAX` eviction
  (`git.rs:27-31, 69-76`).
- Cost is proven by counters, not inspection: `tabmeta::probe` (`tabmeta/mod.rs:325-361`)
  already declares `PROC_CWD`, `PROC_CHILDREN`, `PROC_EXE`, `GIT_STAT`, `LOG_RESOLVE` etc.
- `Tabs::is_active(id)` exists (`tabs.rs:80-82`), and the `PtyOutput` closure already holds
  both `id` and `active_id` (`terminal_app.rs:1545`).
- `Session::take_title_dirty()` (`session.rs:142-144`, consumed at `terminal_app.rs:1564`)
  is the established seam for "this session changed something the card shows".
- Project rules: this is `app`-only; `term-core` stays untouched
  (`.claude/rules/dependency-direction.md`); macOS-only, so `proc_pidinfo`/`sysctl` are fair
  game (`.claude/rules/macos-only.md`).

## Decisions

### D1: The pane cwd is derived on the tabmeta worker thread and published in `TabMeta`
- **Status**: resolved
- **Rationale**: the worker is the only place where this derivation is free. It **already**
  resolves a cwd every tick (`worker.rs:69`), the sibling's `tabmeta-tmux-reroot` **already**
  re-roots that exact call, and it uses the **memoized** `effective_root(&mut Memo, pid,
  tick)` — so N tmux tabs cost one `tmux list-clients` per tick rather than N. Adding
  `cwd: Option<PathBuf>` to `TabMeta` therefore publishes a pane-correct cwd at **zero
  additional spawn, zero additional thread, and zero UI-thread I/O**.
  **This reverses the first draft, and the reason matters.** The draft cached the cwd on
  `Session` and refreshed it inside `drain()` gated on PTY output. The L2 reviewer showed
  that keystroke echo *is* PTY output, so that placed an unbounded blocking unix-socket
  round-trip on the winit event-loop thread at 2 Hz per tmux tab, inside the input→screen
  path that `docs/domain.md` invariant 4 budgets at ≤ 8 ms — and, because `drain` runs for
  background tabs too (`terminal_app.rs:1545-1567`) and `effective_root_uncached` has no
  memo, M tmux tabs meant M uncoordinated spawns per 500 ms, falsifying the sibling's
  headline SD3 guarantee. `tabsession-tmux-reroot` D8 had independently reached the opposite
  conclusion about this same call on this same thread. The draft's own C1 was satisfied by
  the letter and evaded in substance: the spawn merely moved from `cwd()` to `drain()`, both
  driven by the same keypress.
  **Rejected: keeping it on `drain` with a 250 ms `recv_timeout` guard** — it stays inside
  the original surface and needs no partition change, but it duplicates a helper C3 forbids
  reaching (it lives in `tabsession.rs`), still spawns per-tab with no cross-tab memo, and a
  250 ms UI stall is still 30× the input→screen goal. **Rejected: reading
  `#{pane_current_path}` straight from `list-clients`** and skipping the pid hop — tmux does
  expose it (verified live), and it would save a `proc_pidinfo`, but the sibling's parser is
  specified to yield `(client_tty, pane_pid)` rows, so this means reopening its surface to
  save one syscall per tick. Recorded so an implementer who notices it does not reopen it.

### D2: `Session::cwd()` reads the published cwd, falling back to its own direct lookup
- **Status**: resolved
- **Rationale**: `cwd()` becomes "the worker's published cwd for this tab, else today's
  `cwd_of_pid(self.pty.child_pid()?)`".
  **(a) The hot paths get cheaper, not dearer.** The per-keystroke reads
  (`terminal_app.rs:1419`, `:1462`) and the per-tab `Tabs::titles()` read (`tabs.rs:196`)
  become a snapshot lookup — no syscall at all in the common case, where today they pay a
  `proc_pidinfo` each.
  **(b) The fallback is today's behavior, so `cwd()` is never emptier than it is now** — no
  cache to prime, and no window where a tab is titled `Tab 3` when it would not be today.
  **This is upside about emptiness, not about correctness**: during the window before the
  first publish the fallback returns the *tmux client's* directory, which is the bug this
  spec exists to remove. That window is real and is closed at the one place it is visible by
  D12; outside the overlay it costs at most a briefly-wrong tab title.
  **(c) The dead-shell case still yields `None`, but not for the reason it might appear.**
  `child_pid()` is `self.child.process_id()` (`crates/term-core/src/pty.rs:148-150`) and
  nothing in the app reaps the child, so it keeps returning `Some(dead_pid)`; and the roster
  is not rebuilt on shell exit (`tabs.rs:179-188` is reached only from `sync_tab_model`,
  whose call sites are startup and tab/title actions). The actual mechanism is that the
  worker keeps deriving the still-rostered tab, `cwd_of_pid(dead_pid)` fails, and it
  publishes `cwd: None` — after which the fallback also fails. See C7 for the bounded window
  in which this is *not* yet true.
  **Rejected: `Session` owning a cache** — that was the first draft; see D1.

### D13: The worker resolves cwd unconditionally, decoupling it from the sidebar toggles
*(numbered out of sequence deliberately — it amends D2 and belongs beside it; `D2a` would
collide with D2's own parts (a)/(b)/(c), which C7 cites as D2c.)*
- **Status**: resolved
- **Rationale**: `worker.rs:67-68` skips `cwd_of_pid` entirely unless `enabled_git ||
  enabled_agent` — flags driven by the user-facing `sidebar.show_git_branch` /
  `sidebar.show_agent_status` TOML booleans (`config.rs:81-83`). Left alone, D1 + D2 would
  mean that **turning off two sidebar display preferences silently restores the original
  bug**: no cwd is published, D2's fallback returns the tmux client's directory, and the
  overlay reads the wrong project again — with nothing on screen connecting the two. It
  would also silently violate C2, putting a `proc_pidinfo` back on every keystroke.
  The condition is therefore removed: cwd is resolved on every derivation. This is faithful
  to the toggles' actual purpose, which is to skip the *expensive* work — `.git` I/O and the
  process walk — not a single `proc_pidinfo`. `TabMeta.cwd` now has one meaning independent
  of display settings, and `branch`/`agent`/`task` stay gated exactly as they are today.
  **Rejected: leaving the gate and documenting the coupling** — a correctness fix that a
  display preference can switch off is not a fix; it is a trap for whoever files the bug
  again.

### D3: A pane cwd with no `~/.claude/projects/` entry shows the empty dashboard
- **Status**: resolved
- **Rationale**: `cd /tmp` inside a pane means the pane genuinely is not in a Claude
  project, so `project_dir` resolving nothing and the panel showing its no-session state is
  the truthful answer — and it is **exactly what a non-tmux tab does today** when it `cd`s
  out of a project, so the tmux path acquires no special case at all.
  **Rejected: falling back to the tmux client's cwd** — it reintroduces precisely the
  wrong-directory attachment this spec exists to remove, and leaves the user unable to tell
  which directory the panel is describing. **Rejected: stickily keeping the last resolved
  session** — it would show a session that is not the pane's with nothing on screen saying so.

### D4: Flap protection is the sibling's per-tick negative cache plus change-only publication
- **Status**: resolved
- **Rationale**: the hazard is real — `effective_root` returning the input pid on a failed
  lookup makes the cwd fall back to the tmux client's shell, which would visibly relabel the
  tab and could re-point an open overlay mid-read. The first draft answered it with a hold
  latch, a consecutive-failure counter and a `MAX_HELD_REFRESHES` bound; the reviewer showed
  that machine had an unnamed constant, an undeclared latch and undefined transitions at
  spawn, at shell exit, and on hold release.
  **D1 removes the need for all of it.** Two existing mechanisms already cover the case:
  the sibling's `Memo` writes an entry on **every** fetch attempt including failures (its
  D9(e)), so a failure is negatively cached for the rest of the tick and cannot cause a
  same-tick flap; and `publish_if_changed` means an unchanged derivation is not republished
  at all. A failed tick therefore yields at most one transition to the shell cwd, corrected
  on the next successful tick — the same degradation every other `TabMeta` field already
  has, on the same cadence, with no new state.
  **Accepted residual**: a single-tick flip to the shell-cwd title is possible under
  transient tmux failure. Judged acceptable because it is bounded by one tick, self-correcting,
  and strictly better than today, where the title is *permanently* wrong under tmux.
  **Rejected: a hold latch on `Session`** — see above; it bought a marginal cosmetic
  improvement for a state machine with three undefined transitions.

### D5: One cwd, one owner — the worker's value is the single source of truth
- **Status**: resolved
- **Rationale**: the first draft had the app and the worker each resolving a cwd on
  different cadences, and accepted that they would visibly disagree for up to a refresh
  window (a card showing project B's title beside project A's branch). D1 collapses that:
  there is now exactly one derivation, so the title, the git branch, the agent badge, the
  task label, the overlay's log directory and the rename prefill are all guaranteed to
  describe **the same directory at the same instant**, which is a stronger property than
  the code has today even without tmux.
  **Rejected: two independent caches with accepted divergence** — that was the first draft's
  D5; it was rejected-by-consequence once D1 moved, not argued down separately.

### D6: A changed published cwd for the **active** tab fires the existing `notify_active_changed`
- **Status**: resolved
- **Rationale**: a fresh cwd alone does not move the panel — `notify_active_changed` is
  called only from the active-tab-changed path (`terminal_app.rs:1234-1237`). So when a
  snapshot sync observes that the **active** tab's published cwd differs from the one the
  overlay is holding, the app calls that same seam. The whole re-point path is reused:
  `on_active_changed` already guards on whether the notification changed and re-opens — the
  guard is `Repoint::changed`, which compares the incoming `(cwd, shell_pid)` against the pair
  the last `open` was *notified* with. (During implementation this was found to be load-bearing:
  comparing against the *resolved* pane cwd instead makes the two disagree by construction until
  the worker's first publish, so the guard fires on every wake and never converges.) `shell_pid`
  genuinely does not change across a tmux window switch, so the guard fires on the cwd alone
  exactly as intended. The wake already exists —
  the worker publishes on change and the app syncs the snapshot on `OverlayPending`
  (`terminal_app.rs:1647`, `:904-909`).
  **The concrete form, since the comparison D6 describes is not one this surface can make.**
  `AiMonitorOverlay.cwd` is private (`overlay.rs:67`), the type is held as `dyn Overlay`, and
  the trait exposes no reader (`seam.rs:214-230`) — so "differs from the one the overlay is
  holding" is not directly observable. It is implemented instead as: **after each
  `handle.sync` in `feed_agent_states` (`terminal_app.rs:904-909`), call
  `notify_active_changed(active_cwd(), active_shell_pid())` unconditionally**, and let
  `on_active_changed`'s existing guard (`overlay.rs:347`) no-op when nothing moved. Level-
  triggered, entirely within `terminal_app.rs`, and no new trait method.
  **Rejected: having `TerminalApp` remember the last cwd it notified** (edge-triggered) —
  `self.tabmeta_snapshot` is *also* synced from `displayed_label` (`terminal_app.rs:736-738`),
  so a Cmd+Shift+* keystroke can consume the generation edge before the `OverlayPending`
  handler sees it, and the re-point is silently lost.
  **This trigger is load-bearing for plain tab switching too, not only tmux.**
  `sync_tab_model()` sends `SetActive` and the worker derives asynchronously
  (`worker.rs:204-212`), so today's `notify_active_changed` at `:1235` reads the *pre-switch*
  snapshot; the corrected cwd arrives one publish later and needs this trigger to be seen.
  **Correcting the first draft's rationale**, which claimed `notify_active_changed`
  intrinsically "carries the active tab's cwd" and that a background-tab trigger would
  therefore mis-point the overlay. That is false: its signature takes whatever the caller
  passes (`seam.rs:352-354`), and `terminal_app.rs:1235` is merely the sole existing caller,
  which happens to pass `active_cwd()`. Under the level-triggered form above the question
  does not arise — the call always carries the active tab's values by construction.
  **Rejected: re-checking inside `AiMonitorOverlay::poll`** — `poll` returns early without a
  watcher (`overlay.rs:331-333`), and "no watcher because the directory was wrong" is
  precisely the case that must recover, so it would never fire. **Rejected: requiring a
  manual toggle** — predictable, but drops the live-follow half of the confirmed goal.

### D7: Scope is split panes and per-tab sessions; nested tmux and re-attach are out
- **Status**: resolved
- **Rationale**: two arrangements must work. **Split panes** — the cwd follows the *active*
  pane, which is free: `tmux list-clients -F '#{client_tty} #{pane_pid}'` resolves `pane_pid`
  in the client's own context, so it already reports the active pane of the client's current
  window (verified live during L1). **Multiple tabs on different sessions** — the actual
  daily setup; each tab resolves its own client's pane via the sibling's tty-devno join
  (SD1), never another tab's.
  **Out**: *nested tmux* degrades to the outer pane's cwd — wrong, but not worse than
  today's tab-shell cwd; *detach / re-attach* is best-effort (see Known Gaps), and D1 makes
  it strictly better than the draft would have, since there is no hold latch to get stuck in.

### D8: This spec renegotiates the sibling partition to add a cwd field to `TabMeta`
- **Status**: resolved
- **Rationale**: D1 requires `crates/app/src/tabmeta/mod.rs` (the `TabMeta` field) and
  `crates/app/src/tabmeta/worker.rs` (populating it), both of which sit inside the sibling
  set's declared surfaces — `tmux-root-resolution` claims `mod.rs`, `tabmeta-tmux-reroot`
  claims `worker.rs`. Stated as an explicit, declared renegotiation rather than an accident,
  because the sibling set itself surfaced this as an open partition question rather than
  answering it. The change is additive — one `Option<PathBuf>` field on `TabMeta`, one line
  in `Deriver::tab` storing the cwd instead of discarding it, plus D13's removal of the
  toggle gate — and removes no sibling field, so the two sets can land in either order
  **after** their own dependency (D9). The overlap must be declared so a coherence audit
  sees it rather than discovering a conflicting edit.
  **It does change one sibling behavior, and D6 depends on that.** `TabMeta` derives
  `PartialEq` and `publish_if_changed` is `next == *published` (`worker.rs:238-244`), so
  adding `cwd` puts it **into the publication predicate**: today a `cd` within one repo
  (same branch, same agent, same task) publishes nothing, and afterwards it bumps the
  generation and fires `wake()`. This is not incidental — D6's re-point trigger exists only
  because a cwd-only change now publishes. It is bounded by the tick and so is not a return
  to the unbounded wakes the G1 fix removed (`terminal_app.rs:1599-1605`), but it must be
  owned: `tmux-root-resolution` R7.4 pins "the pre-existing `worker.rs` tests still pass with
  their original expected deltas", and any wake-count assertion there needs re-checking.
  **Footprint**: all seven `TabMeta { … }` literals are exhaustive (no `..Default::default()`)
  — production at `worker.rs:103`, tests at `worker.rs:287, :309, :446, :462` and
  `mod.rs:428, :436, :482`, including `default_tabmeta_is_blank` (`mod.rs:479-487`), which
  asserts field-by-field. All are inside the Declared Surface and all are compile-checked,
  so this is mechanical, not risky — but it is seven sites and a doc comment, not one line.
  **`TabMeta`'s doc comment becomes false and must be updated** (`mod.rs:36-38`): it claims
  `default()` is "also the correct value for a not-yet-polled tab (so there is no third
  'unpolled' state)". With `cwd`, `None` means two distinguishable things — not yet polled,
  or polled with no resolvable cwd — and D2's fallback exists precisely to paper over that
  ambiguity, which is exactly what D12 closes at the overlay.
  **Rejected: keeping the derivation out of `tabmeta` to preserve the partition** — that is
  what forced the first draft onto the UI thread; a boundary is renegotiable and a UI freeze
  is not.

### D9: `tmux-root-resolution` **and** `tabmeta-tmux-reroot` are hard build-order preconditions
- **Status**: resolved
- **Rationale**: from the Inversion Probe, and widened by D1. The draft depended only on
  `tmux-root-resolution`; routing through the worker means this spec also needs
  `tabmeta-tmux-reroot`, which is what re-roots `worker.rs:69` to the pane. Both are
  **specified but unbuilt**, in the uncommitted `terminal-fix-tmux-pane-resolution` worktree.
  Perfect implementation of every requirement below still ships nothing until they exist. No
  task here may start until `crates/app/src/tabmeta/tmux.rs` exists with `effective_root`
  reachable as `pub(crate)` and `worker.rs`'s cwd resolution is re-rooted. This keeps exactly
  one implementation of the tty-devno join.
  **Rejected: a minimal private fallback resolver** plus a task to delete it later — it
  unblocks immediately but means two implementations of the same join coexisting, precisely
  the duplication the L0 scoping gate decided against. **Rejected: merging the whole sibling
  set into this branch** — a single branch and review, but a much larger change to land at
  once, and it discards the sibling's own build order.

### D10: Cwd freshness is bounded by the worker cadence, and that is accepted
- **Status**: resolved
- **Rationale**: from the Inversion Probe's implication arm, re-derived for D1's placement.
  `Session::cwd()` moves from exact-at-call-time to **at most one worker tick stale** — 500 ms
  for the active tab, up to 2 s for a background tab (`worker.rs:27-30`). This applies to
  every tab, tmux or not. Accepted: a `cd` is followed by the shell writing a prompt, and the
  user cannot physically `cd` and press Cmd+Shift+M inside 500 ms; a background tab's title
  being up to 2 s behind is invisible by construction, since it is not the tab being watched.
  It self-corrects on the next tick with no retry logic.
  **Note this is strictly better than the first draft's bound**, which was 500 ms measured
  from the last *attempt* and could stretch further on the failure path.
  **Rejected: bypassing the snapshot with a fresh lookup on overlay open** — it would remove
  the staleness exactly where it is most visible for one extra lookup per toggle, and remains
  the clean remedy if the accepted risk ever bites; recorded in Known Gaps.

### D11: The cost ledger is stated per refresh and asserted by counters
- **Status**: resolved
- **Rationale**: the first draft claimed a non-tmux tab "spawns nothing at all" and that
  per-call cwd cost "must not exceed today's" — the reviewer showed both are wrong as
  written. The sibling's own D9 sharpens it: **"zero cost" means zero `tmux` spawns
  (`TMUX_LIST == 0`), not zero syscalls**; the presence gate costs `2 + k` syscalls per tab
  per refresh (`PROC_EXE == 1 + k`, `PROC_CHILDREN == 1`, its R2.6), and its D13 adds a
  liveness `cwd_of_pid(pane_pid)` each consumer is expected to carry explicitly. So the
  honest ledger for this feature is stated **per worker refresh, not per call**:
  - *Non-tmux tab*: `2 + k` gate syscalls + the `PROC_CWD` the worker already paid at
    `worker.rs:69`. Zero spawns.
  - *tmux tab*: the above + a share of one `TMUX_LIST` per tick (amortized across all tmux
    tabs by the memo) + `TTY_STAT` per client row + `PROC_TTY` for the tab's own tty + the
    second `PROC_CWD` from the sibling's D13 liveness check.
  - *Per tick, shared across all tabs*: one `TMUX_LIST` and `TTY_STAT == N` where N is the
    client-row count — resolved once at fetch and shared via the memo, which is the whole
    point of the sibling's D7(b) ("N stats per tick, **not M×N**", its R7.3). These are
    **per-tick, not per-tab** costs.
  - *Per keystroke and per sidebar resync*: **zero syscalls**, down from one `proc_pidinfo`
    each (D2). This is the amortization that makes the refresh-side cost acceptable, and it
    is the argument the first draft never made.
  - *Per overlay toggle* (D12): one additional uncached resolve — a user-initiated event on
    no cadence, which the sibling's D10 licenses explicitly.
  **The `2 + k` is marginal, not net-new — conditionally.** Sibling D9 notes the gate's
  `children_of` + `k × exe_path` duplicate work `agent_of_pid` does moments later, so the
  true added cost is ≈1 syscall. That duplication only exists while agent display is on
  (`worker.rs:81`); with it off the gate's `2 + k` is entirely net-new. D13 does not change
  this — it decouples *cwd* from the toggles, not the gate.
  All three sibling counters — `TMUX_LIST`, `PROC_TTY` **and** `TTY_STAT` (its D15, which
  refines the manifest's literal "two") — plus the existing `PROC_CWD` are what make these
  assertions testable rather than claimed.

### D12: The overlay-open path resolves the pane cwd directly, not from the snapshot
- **Status**: resolved
- **Rationale**: closes the cold-start hole the reviewer found in D2(b). `TabMetaSnapshot::get`
  returns `None` until the roster round-trip completes — `set_roster` is a channel send
  (`tabs.rs:188`), the worker must receive, derive and publish, and the app must take
  `OverlayPending` and sync (`terminal_app.rs:904-909`, `:1647`). Until then D2's fallback
  returns **the tmux client's directory**, so `tmux attach` followed quickly by Cmd+Shift+M
  opens the overlay on the wrong project — precisely the reported bug, reachable in a spec
  written to remove it. D10 addressed *staleness* (a value that exists but is old) and
  argued it away correctly; it never addressed *absence*.
  So `AiMonitorOverlay::open` resolves the pane cwd itself rather than taking
  `ctx.active_cwd`, which fixes absence and staleness together at the one place a wrong
  answer is actually visible. Three things make this cheap rather than a reintroduction of
  the UI-thread hazard D1 exists to remove: it is **user-initiated and on no cadence**, which
  is exactly the licence the sibling's D10 grants `effective_root_uncached`; it happens
  **once per toggle**, not per keystroke or per tick; and `hint_for` is **already called on
  this same path** (`overlay.rs:100`), so after `tabsession-tmux-reroot` lands this path
  already performs a `RESOLVE_BUDGET`-guarded tmux resolve — this adds a second resolve to a
  path that has one, rather than putting the first spawn on a clean path.
  Everything outside the overlay keeps reading the snapshot (D2): a briefly-wrong **tab
  title** during the same window is a cosmetic artifact that self-corrects on the next tick,
  and is not worth a spawn on the sidebar path.
  **Rejected: accepting and documenting the window** — narrow, and free, but it leaves a
  reproducible wrong-panel case in this spec's own headline scenario. **Rejected: forcing an
  out-of-band worker refresh on open** — it keeps all tmux contact on the worker thread, but
  needs a new request/response command on the worker channel and makes a user-facing action
  wait on a background thread, for the same outcome.

## Constraints
- **C1** — `Session::cwd()` must perform **no subprocess spawn and no blocking IPC**, on any
  path. Neither may `drain()` or any other function reachable from the winit event loop
  (`terminal_app.rs:1419`, `:1462`, `:1545-1567`). The **only** permitted UI-thread tmux
  contact is D12's once-per-toggle resolve in `AiMonitorOverlay::open`, on the path that
  already calls `hint_for`; everything else happens on the worker thread.
- **C2** — A tab with no tmux descendant must cost **zero `tmux` spawns** (`TMUX_LIST == 0`)
  and no more than the sibling's counted `2 + k` gate syscalls per worker refresh. Its
  per-keystroke and per-resync cwd cost must be **strictly lower** than today's (zero, vs one
  `proc_pidinfo`).
- **C3** — The only files outside this spec's own surface that may be touched are the two
  declared in D8 (`tabmeta/mod.rs`, `tabmeta/worker.rs`), and only additively.
  `tabmeta/tmux.rs`, `agent.rs`, `proc.rs` and `tabsession.rs` remain the sibling set's.
- **C4** — Every failure path degrades to today's behavior and never panics; no `unwrap()`
  outside tests (`CLAUDE.md` conventions, sibling SD4).
- **C5** — `term-core` gains no dependency and no change; this is `app`-only
  (`.claude/rules/dependency-direction.md`).
- **C6** — Read-only observation only: `proc_pidinfo`, `stat`, and `tmux list-clients`. No
  signals, no writes to any tmux session (`tabsession.rs:16-17`).
- **C7** — `Session::cwd()` must be no emptier than today's implementation at any point in a
  tab's life. It must also be **no fuller once the worker has observed the death**: a dead
  shell must resolve to `None` within one refresh of that tab (D2c). The bound is one
  background interval — `Deriver::pass` carries `prev` forward for an off-phase tab
  (`worker.rs:143-148`), so a background tab may report a live directory for a dead shell for
  up to `BG_INTERVAL × TICK` = **2 s**. That window is accepted, not denied.
- **C9** — Publishing a cwd must not make `TabMeta` derivation depend on any display setting:
  `sidebar.show_git_branch` and `sidebar.show_agent_status` must not be able to change which
  directory the overlay reads (D13).
- **C8** — The build must stay green under `cargo clippy --workspace --all-targets -D
  warnings` and remain testable on a machine with **no tmux running**, per the sibling's SD5
  split of pure logic from spawn/syscall wrappers.

## Known Gaps
- **Detach / re-attach is best-effort** (D7, out of scope). Re-attaching a session — possibly
  from a different tab — is expected to correct itself on the next worker tick, but that is
  not a requirement and is not tested.
- **Nested tmux degrades silently** (D7, out of scope). An inner server inside an outer pane
  resolves to the *outer* pane's cwd. Wrong, but strictly better than today's tab-shell cwd,
  and nothing tells the user which one they are seeing.
- **A single-tick title flip is possible under transient tmux failure** (D4, accepted). One
  tick (500 ms active / 2 s background) showing the shell-cwd title before correcting.
- **The worker cadence is inherited, not chosen** (D10). 500 ms / 2 s come from
  `worker.rs:27-30` and are unmeasured for this use, consistent with `docs/architecture.md`
  G1, where performance budgets are goals rather than gates.
- ~~**A briefly-wrong tab *title* survives outside the overlay** (D12). During the
  not-yet-published window the sidebar row and rename prefill still show the tmux client's
  directory basename, self-correcting on the next tick. Judged cosmetic and not worth a spawn
  on the sidebar path — but it is the residue of the hole D12 closes for the overlay only.~~
  **Corrected and closed by `specs/card-live-cwd/spec.md`.** Two things above were wrong, and
  the wording is what let the defect survive a year of reading:
  1. **"self-correcting on the next tick" was false for the sidebar row.** Nothing re-runs
     `Tabs::titles()` on a tick — it runs on tab actions only (open/close/switch, a user
     rename, `ResetToAuto`, or a program's OSC 0/2). So the row title froze at the directory
     the tab was opened in, for the life of the tab. Only the rename prefill self-corrected,
     because `displayed_label` syncs the snapshot itself. **Not one tick wide — permanent.**
  2. **"briefly" and "outside the overlay" understated the scope.** The gap needed no tmux at
     all: a plain `cd` in any tab reproduced it. And because `branch` *was* pulled live from
     the same `TabMeta` the title ignored, one card could show the old folder beside the new
     branch — which also breaks D5's claim that title, branch, badge, task, log directory and
     prefill "all describe the same directory at the same instant". D5's *value* was single-
     owned as claimed; its **reads** were not.

  `card-live-cwd` closes both by moving the folder name onto the per-frame `MetaSource` pull
  (D1) beside the task label, demoting `SidebarRow::title` to a documented pre-publish
  fallback (D2), and giving the waiting band a generation-gated rebuild (D5). What remains
  genuinely brief is only the not-yet-published window on a newly-opened tab.
- **If the tabmeta worker never installed, the feature silently no-ops.** `spawn` returns
  `Option` and `global()` is `None` when `install` never ran (`mod.rs:160-172`, `:311-313`);
  `Session::cwd()` then falls back for the whole session and tmux tabs behave as they do
  today. Licensed by C4's degrade-never-panic rule, but recorded because — like the toggle
  gate D13 removes — it turns a correctness fix into an invisible no-op rather than a
  visible failure. D12's overlay path is unaffected and still resolves correctly.
- **Wake-count assertions in existing `worker.rs` tests need re-checking** (D8). Adding `cwd`
  to the publication predicate means a bare `cd` now publishes where it did not; sibling
  `tmux-root-resolution` R7.4 pins those pre-existing deltas.
- **D8's partition overlap needs a coherence re-audit.** Two files move into this spec's
  surface from the sibling set's. Declared rather than silent, but the sibling set's
  coherence report predates this spec and does not know about it.
- ~~**This spec does not by itself make the session *hint* work under tmux.**~~ **Closed
  during execution**: `tabsession-tmux-reroot` was committed in the sibling worktree and came
  in with the convergence merge (`a35679d`), so `hint_for` now resolves from the pane too.
  Both halves — which directory, and which session within it — are fixed in this tree.
- **`RESOLVE_BUDGET` applies to the overlay's cwd resolve as well** (D12, execution finding).
  The open path now performs *two* guarded resolves, one for the session hint and one for the
  cwd, each bounded at 250 ms. Worst case 500 ms of open latency against a wedged tmux server,
  versus a frozen UI without the guard. Not merged into a single resolve because that would
  mean widening `SessionHint`, which is the sibling's surface.

## Requirements

### R0: A tmux tab's ai-monitor shows its own pane's session

#### R0.1: The overlay opens on the pane's project, not the client's
- **Given**: a tab whose shell is in `~` and whose tmux pane is in `~/IdeaProjects/foo`, with a live Claude log under `~/.claude/projects/-Users-<u>-IdeaProjects-foo/`
- **When**: the user presses Cmd+Shift+M
- **Then**: `project_dir` resolves that pane directory and the dashboard renders the session's rows, instead of `session (no active session)` with every row empty

#### R0.2: The tab is titled after the pane's directory
- **Given**: the same tab
- **When**: the sidebar model is resynced
- **Then**: the row's cwd-basename title reads `foo`, not the client shell's basename

#### R0.3: A non-tmux tab is unchanged
- **Given**: a tab with no tmux descendant
- **When**: the overlay is opened and the sidebar resynced
- **Then**: both resolve the same directory they resolve today, and `TMUX_LIST == 0`

---

### R1: The worker derives and publishes a pane-correct cwd (D1, D8, D13)

#### R1.1: `TabMeta` carries the resolved cwd
- **Given**: `TabMeta` with the new `cwd: Option<PathBuf>` field
- **When**: `Deriver::tab` runs for a rostered tab
- **Then**: the returned `TabMeta.cwd` holds the cwd resolved from `effective_root`'s pid, and `TabMeta::default().cwd` is `None`

#### R1.2: The derivation is rooted at the pane under tmux
- **Given**: a tab whose tree contains an attached tmux client whose pane pid is `P`
- **When**: `Deriver::tab` runs for that tab
- **Then**: the published `cwd` equals `cwd_of_pid(P)`, not `cwd_of_pid(tab_shell_pid)`

#### R1.3: The memoized resolver is used, never the uncached one
- **Given**: M tmux tabs rostered, all refreshing on the same tick
- **When**: a derivation pass runs
- **Then**: `TMUX_LIST == 1` for that tick — the pass calls `effective_root(&mut memo, pid, tick)`, never `effective_root_uncached`

#### R1.4: cwd resolution does not depend on the sidebar toggles
- **Given**: a worker with `enabled_git == false` and `enabled_agent == false`
- **When**: `Deriver::tab` runs
- **Then**: `cwd` is still resolved and published, while `branch`, `agent` and `task` are all `None`

#### R1.5: A cwd-only change is published
- **Given**: a tab whose branch, agent and task are unchanged but whose cwd changed
- **When**: `publish_if_changed` compares the new pass against the published one
- **Then**: it publishes, bumps the generation and calls `wake()`

---

### R2: `Session::cwd()` reads the published value (D2)

#### R2.1: The published cwd wins
- **Given**: a snapshot in which this tab's `TabMeta.cwd` is `Some("/a/b")`
- **When**: `Session::cwd()` is called
- **Then**: it returns `/a/b` and performs **no** syscall

#### R2.2: Absence falls back to today's lookup
- **Given**: a tab with no entry in the snapshot, or an entry whose `cwd` is `None`
- **When**: `Session::cwd()` is called
- **Then**: it returns `cwd_of_pid(self.pty.child_pid()?)` — exactly today's value, including `None` when the pid is unavailable

#### R2.3: `cwd()` is never emptier than today
- **Given**: a `Session` at any point in its life, including immediately after `spawn` and before any worker publish
- **When**: `Session::cwd()` is called
- **Then**: it returns `Some(_)` whenever today's implementation would

---

### R3: The overlay resolves its own cwd on open (D12)

#### R3.1: Open does not read the snapshot
- **Given**: a tab whose pane cwd has not yet been published (no roster round-trip has completed)
- **When**: `AiMonitorOverlay::open` runs
- **Then**: it resolves the pane cwd directly and opens on the pane's project, not on the tmux client's directory

#### R3.2: One resolve per open event
- **Given**: the overlay is opened — by a toggle, or by a D6 re-point
- **When**: the open path completes
- **Then**: exactly one additional uncached resolve occurred; N open events cost N, and no resolve occurs on any other path (poll, redraw, keystroke, or a notification that does not change the re-point key)

> **Amended 2026-07-30.** Originally "One resolve per *toggle* … no resolve occurs while the
> overlay merely stays open". That contradicted D6: a re-point *is* an open, and D6 exists
> precisely so a tmux window switch re-points **without** a toggle — so a resolve on a panel
> that "merely stays open" is the intended behaviour, not a violation. The requirement was
> written before D6's trigger was designed and never reconciled. The bar is unchanged in
> substance — one resolve per open, never per wake — and the D6-driven open is now counted
> honestly instead of being excluded by wording the implementation could not satisfy.

#### R3.3: A failed resolve degrades to the snapshot
- **Given**: the direct resolve fails (no tmux, non-zero exit, unparseable output)
- **When**: `AiMonitorOverlay::open` runs
- **Then**: it falls back to `Session::cwd()`'s value and opens on that, without panicking

---

### R4: A changed cwd re-points an open overlay (D6)

#### R4.1: The app notifies after every snapshot sync
- **Given**: the app has just run `handle.sync(&mut self.tabmeta_snapshot)` in `feed_agent_states`
- **When**: that sync returns
- **Then**: `notify_active_changed(active_cwd(), active_shell_pid())` is called unconditionally — level-triggered, not gated on an observed difference

#### R4.2: The overlay re-opens on a changed cwd
- **Given**: the overlay is open on project A and the active tab's published cwd becomes project B
- **When**: the notification arrives
- **Then**: `on_active_changed`'s guard (`Repoint::changed`) sees the notified cwd differ from the one the last open was notified with, and re-opens on B

#### R4.3: An unchanged cwd is a no-op
- **Given**: the overlay is open and the active tab's cwd is unchanged
- **When**: the notification arrives
- **Then**: `on_active_changed` returns without re-opening — no watcher restart, no redraw

#### R4.4: A tmux window switch re-points without a tab switch
- **Given**: the overlay is open on a tmux tab in window 0 (project A), with window 1 in project B
- **When**: the user switches tmux windows and a worker tick elapses
- **Then**: the overlay shows project B's session, with no mac-terminal tab switch involved

---

### R5: Degradation never leaves a wrong or crashed panel (D3, D4)

#### R5.1: A pane outside any Claude project shows the empty dashboard
- **Given**: the active pane's cwd is `/tmp`, which has no `~/.claude/projects/` entry
- **When**: the overlay is opened
- **Then**: it renders the no-session state for that directory, and does **not** fall back to the tmux client's cwd or to a previously resolved session

#### R5.2: A failed tmux lookup is negatively cached for the tick
- **Given**: `tmux list-clients` exits non-zero on this tick with M tmux tabs rostered
- **When**: the derivation pass runs
- **Then**: `TMUX_LIST == 1` — the failure is cached as an empty entry, not retried per tab

#### R5.3: A failed lookup costs at most one tick of wrong title
- **Given**: a tmux tab whose lookup fails on tick N and succeeds on tick N+1
- **When**: both ticks complete
- **Then**: the tab shows the shell-cwd title for at most that one tick, then returns to the pane's, with no latch or counter involved

#### R5.4: No failure path panics
- **Given**: any combination of dead pid, missing tmux binary, non-zero exit, unparseable output, or absent worker
- **When**: cwd derivation or `Session::cwd()` runs
- **Then**: it returns a value or `None` and never panics; no `unwrap()` on any of these paths

---

### R6: The cost budget is asserted by counters, not claimed (D11)

#### R6.1: Keystrokes and resyncs cost nothing
- **Given**: a tab with a published cwd
- **When**: 100 key presses are dispatched and the sidebar is resynced
- **Then**: `PROC_CWD` is unchanged by those paths — zero syscalls, down from one `proc_pidinfo` per `active_cwd()` read today

#### R6.2: A non-tmux tab spawns no tmux
- **Given**: a rostered tab with no tmux descendant
- **When**: a derivation pass runs
- **Then**: `TMUX_LIST == 0`, and the gate cost matches the sibling's counted `2 + k` (`PROC_EXE == 1 + k`, `PROC_CHILDREN == 1`)

#### R6.3: tty stats are per tick, not per tab
- **Given**: M tmux tabs and N tmux client rows
- **When**: a derivation pass runs
- **Then**: `TTY_STAT == N` for the tick — not `M × N`

#### R6.4: No tmux contact on the event loop
- **Given**: the winit event loop dispatching input and draining PTY output
- **When**: any of `Session::cwd()`, `Session::drain()`, or `Tabs::titles()` runs
- **Then**: `TMUX_LIST` is unchanged — the only permitted UI-thread resolve is R3.2's once-per-toggle open

---

### R7: The supported tmux arrangements resolve correctly (D7)

#### R7.1: The cwd follows the active pane in a split window
- **Given**: a tmux window split into two panes, in projects A and B
- **When**: the user moves focus from the A pane to the B pane and a tick elapses
- **Then**: the tab's cwd becomes B's — `list-clients` resolves `pane_pid` in the client's context, so the active pane is what is reported

#### R7.2: Each tab resolves its own session
- **Given**: three tabs, each with `tmux attach -t` a different session, in three different projects
- **When**: a derivation pass runs
- **Then**: each tab's published cwd is its own session's active pane, and no tab resolves another's

---

### R8: Freshness and death semantics are bounded (D5, D10, C7)

#### R8.1: The active tab is at most one tick stale
- **Given**: the active tab's pane performs `cd` to a new project
- **When**: one worker tick (500 ms) elapses
- **Then**: the published cwd is the new directory

#### R8.2: A dead shell stops reporting a live directory
- **Given**: a tab whose shell has exited but whose tab is still open and rostered
- **When**: that tab is next refreshed by the worker
- **Then**: `cwd_of_pid` fails, `cwd: None` is published, `Session::cwd()`'s fallback also fails, and `cwd()` returns `None`

#### R8.3: The dead-shell window is bounded by one background interval
- **Given**: a **background** tab whose shell dies just after its phase tick
- **When**: up to `BG_INTERVAL × TICK` = 2 s elapses
- **Then**: it may still report its last cwd for that window — `Deriver::pass` carries `prev` forward — and reports `None` on its next refresh

#### R8.4: One cwd feeds every consumer
- **Given**: any single published snapshot
- **When**: the title, the git branch, the agent badge, the task label, the rename prefill and the overlay's log directory are all derived
- **Then**: all six describe the same directory — there is exactly one cwd derivation

---

### R9: The build-order precondition is explicit (D9)

#### R9.1: The dependency is declared and checkable
- **Given**: a build attempt on a tree where `crates/app/src/tabmeta/tmux.rs` does not exist
- **When**: this feature's tasks are scheduled
- **Then**: they do not start — the spec's `depends_on` names both `tmux-root-resolution` and `tabmeta-tmux-reroot`, and no task here reimplements the tty-devno join

#### R9.2: Nothing outside the declared surface is edited
- **Given**: the completed change set
- **When**: it is diffed
- **Then**: it touches only the six files in `## Declared Surface` plus the single recorded `declared-deviation` (`tabsession.rs`, one keyword — see below); `tabmeta/tmux.rs`, `agent.rs` and `proc.rs` are unmodified

> **Amended 2026-07-30.** Originally "…and `tabsession.rs` are unmodified", which contradicted
> this spec's own `## Declared Surface`, where the `declared-deviation` block records the
> `fn resolved_root` → `pub(crate) fn` change. The two were inconsistent from the moment the
> deviation was declared. The bar is unchanged: exactly one file outside the surface may be
> touched, it is named, and the reason is recorded in three places — the alternative was a
> second copy of a 250 ms UI-freeze guard.

---

### R10: The existing test suite stays honest (D8)

#### R10.1: All `TabMeta` literals are updated
- **Given**: the seven exhaustive `TabMeta { … }` literals (`worker.rs:103, :287, :309, :446, :462`; `mod.rs:428, :436, :482`)
- **When**: the `cwd` field is added
- **Then**: all compile, and `default_tabmeta_is_blank` (`mod.rs:479-487`) asserts `cwd: None` alongside the existing fields

#### R10.2: Wake-count expectations are re-checked
- **Given**: pre-existing `worker.rs` tests asserting publish/wake deltas
- **When**: `cwd` joins the publication predicate
- **Then**: each affected expectation is re-derived and updated deliberately, not silently relaxed

#### R10.3: The suite runs without tmux
- **Given**: a machine with no tmux installed and no tmux server running
- **When**: `cargo test --workspace` runs
- **Then**: every test in this feature passes, with tmux-dependent logic exercised through injected fakes per the sibling's SD5 split

## Tasks

> **Plumbing note (applies to T2/T4).** `Session` holds no `TabMetaSnapshot`, and
> `TerminalApp` already owns one (`terminal_app.rs:235`, synced at `:904-909`). So the
> published cwd is **threaded in explicitly** rather than read from a global:
> `Session::cwd(&self, meta: Option<&TabMeta>)`, with `Tabs::titles` / `Tabs::active_cwd`
> taking `&TabMetaSnapshot` and doing the per-tab `get`. All three files are in surface, the
> data flow stays visible, and no second global read path appears.

### T1: Publish the pane cwd from the worker [infra]
- **Fulfills**: R1, R8.1, R10.1, R10.2
- **Depends on**: (none — but see Pre-work; unbuildable until both siblings land)
- Add `cwd: Option<PathBuf>` to `TabMeta` (`tabmeta/mod.rs:40`) and update its doc comment,
  which currently claims there is no "unpolled" state (`mod.rs:36-38`). Store the cwd
  `Deriver::tab` already resolves (`worker.rs:66-72`) instead of discarding it, via the
  **memoized** `effective_root(&mut memo, pid, tick)`. Remove the
  `enabled_git || enabled_agent` gate on cwd resolution only (D13) — `branch`/`agent`/`task`
  stay gated as today. Update all seven exhaustive `TabMeta { … }` literals and re-derive the
  publish/wake expectations that `cwd` joining the `PartialEq` predicate changes.

### T2: `Session::cwd()` reads the published cwd [vertical]
- **Fulfills**: R2, R8.2, R8.3, R8.4, R0.2
- **Depends on**: T1
- Thread the snapshot per the plumbing note. `cwd()` returns the published value when
  present, else today's `cwd_of_pid(self.pty.child_pid()?)`. Closes the user-visible loop for
  the **tab title** (R0.2) and the rename prefill.

### T3: The overlay resolves its own cwd on open [vertical]
- **Fulfills**: R3, R5.1, R0.1
- **Depends on**: (none) — **parallel with T1, T2 and T4**: it touches only `overlay.rs`, and
  it calls the sibling's `effective_root_uncached` directly rather than anything T1 builds.
- `AiMonitorOverlay::open` resolves the pane cwd itself instead of taking `ctx.active_cwd`
  (`overlay.rs:88, :317`), falling back to the passed value on any failure. Closes the
  user-visible loop for the **overlay** (R0.1), including the cold-start window.

### T4: Re-point an open overlay when the cwd changes [vertical]
- **Fulfills**: R4
- **Depends on**: T2 — file overlap on `terminal_app.rs`, and the trigger is meaningless
  until the published cwd is what `active_cwd()` returns.
- Call `notify_active_changed(active_cwd(), active_shell_pid())` unconditionally after each
  `handle.sync` in `feed_agent_states` (`terminal_app.rs:904-909`). Level-triggered, relying
  on `on_active_changed`'s existing guard (`overlay.rs:347`) to no-op.

### T5: Prove the cost and degradation budgets [infra]
- **Fulfills**: R5.2, R5.3, R5.4, R6, R7, R9, R10.3, R0.3
- **Depends on**: T1, T2, T3, T4
- Counter-delta tests in the `probe` idiom (`tabmeta/mod.rs:325-361`): `TMUX_LIST == 0` for a
  non-tmux tab, `== 1` per tick across M tmux tabs and on a failed lookup, `TTY_STAT == N`
  per tick not `M × N`, `PROC_EXE == 1 + k` / `PROC_CHILDREN == 1`, and zero `PROC_CWD` from
  the keystroke and resync paths. Plus the degradation matrix (dead pid, missing binary,
  non-zero exit, unparseable output, absent worker) and the R9 diff check that nothing
  outside Declared Surface was touched. All tmux-dependent logic exercised through injected
  fakes so the suite passes with no tmux installed.

**DAG**

```
T1 ──> T2 ──> T4 ──┐
                   ├──> T5
T3 ────────────────┘        (T3 parallel with T1, T2, T4)
```

## External Dependencies

### Pre-work
- **Build and land `tmux-root-resolution`** (sibling worktree
  `terminal-fix-tmux-pane-resolution`, `specs/tmux-root-resolution/spec.md`). Provides
  `crates/app/src/tabmeta/tmux.rs` with `effective_root` / `effective_root_uncached` /
  `Memo` as `pub(crate)`, plus the `TMUX_LIST` / `PROC_TTY` / `TTY_STAT` counters. **Nothing
  in this spec compiles without it** (D9).
- **Build and land `tabmeta-tmux-reroot`** (same worktree). Re-roots `worker.rs:69` to the
  pane pid, which is what makes T1's published cwd pane-correct rather than shell-correct.
- **Decide how the two worktrees converge** — cherry-pick the sibling commits onto
  `fix/ai-monitor-tmux`, or rebase this work onto that branch. Not a code action, but it
  blocks T1.
- **Re-run `coherence-audit` over the sibling set plus this spec** — this spec moves
  `tabmeta/mod.rs` and `worker.rs` into its own declared surface (D8), which the sibling
  set's existing coherence report predates.

### Post-work
- **Eyeball on a real tmux session** (`/run-term`): R0.1, R4.4 and R7.1 need a live tmux
  server and are not covered by the headless suite.
- **Verify the original screenshot case** — attach the tab whose client shell sits in `~`
  while the pane is in a worktree, and confirm the dashboard populates.
