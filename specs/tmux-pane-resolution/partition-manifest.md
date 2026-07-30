# partition-manifest.md — tmux-pane-resolution
- generated_at: 2026-07-29T07:25:23Z
- goal: When a tab's process tree contains a tmux client, re-root the tab-metadata derivation at that client's pane pid, so the tab card (branch / agent badge / task label) and the ai-monitor overlay's session hint follow the active tmux pane instead of the tab's own zsh — at zero cost when tmux is absent.

## Shared Decisions

### SD1: The join key is the tty **device number** — `proc_pidinfo(PROC_PIDTBSDINFO).e_tdev` on the tab's side, `stat(client_tty).st_rdev` on tmux's side
- **Status**: resolved
- **Rationale**: tmux publishes `#{client_tty}` as a path (`/dev/ttys002`) and the tab's shell shares that controlling tty, so the two sides join on the device. Comparing `dev_t` numerically is pure libc, needs no subprocess for the tab's side, and makes no assumption about the `/dev/ttysNNN` name format. **Rejected: `/bin/ps -o tty= -p <pid>`** — simpler and it matches the existing `/bin/ps` pattern at `tabsession.rs:75`, but it costs one subprocess *per tab per tick*, which directly contradicts SD3's cost budget (the active tab refreshes every 500 ms, `worker.rs:27`). Note: `libc::devname` is **not** exposed in the crate's Apple bindings, so `e_tdev` is never resolved back to a name — the comparison runs in the `dev_t` direction only (`stat` the client tty path, compare `st_rdev`).

### SD2: Re-root, do not extend — the only new logic is "which pid do I start from"
- **Status**: resolved
- **Rationale**: a tmux `pane_pid` is the pane's own `-zsh`, and `claude` is its *direct child*. So the existing depth-1 walk (`agent::walk`), `proc::cwd_of_pid`, the git-branch lookup, the activity lookup and the task label all work **unchanged** against the new root. Every consumer changes by exactly one substitution — `tp.pid` → `effective_root(tp.pid)` — and no downstream signature changes beyond the root pid it is handed. **Rejected: teaching the walk to descend *through* tmux** (raising `MAX_DEPTH` / special-casing the server) — the tmux server is not a descendant of the tab's shell at all, so no depth increase can reach the pane; it would only inflate the per-tick node budget.

### SD3: Zero cost without tmux; at most one `tmux list-clients` spawn per tick
- **Status**: resolved
- **Rationale**: an unconditional `tmux list-clients` per tab per tick is not acceptable at the 500 ms active-tab cadence. Two gates: (1) a **presence gate** — run `tmux` only when a descendant of the tab's shell has exe basename `tmux`, so a machine without tmux, or a tab not running it, pays exactly zero; (2) a **per-tick memo** of the client→pane map keyed on tick, in the shape `git::Memo` already establishes, so N tmux tabs cost one spawn per tick, not N. **Cache the map, never the resolved root**: `pane_pid` changes when the user switches tmux windows, and the card following the active pane is the correct behavior.

### SD4: Absent on any failure — never panic, always fall back to today's behavior
- **Status**: resolved
- **Rationale**: this is the contract every wrapper in `tabmeta/proc.rs` already honors (D14). A missing `tmux` binary, a non-zero exit, unparseable output, a dead pid, a `stat` refusal, or no matching tty all resolve to "no re-root" — the derivation proceeds from the tab's own shell pid exactly as it does today. A tmux hop that fails must never be worse than not having looked.

### SD5: Every new piece splits into a pure function plus a thin syscall/spawn wrapper
- **Status**: resolved
- **Rationale**: the whole set must be testable on a machine with no tmux running (and in CI). Mirrors the split this codebase already uses — `proc::parse_childpids` / `parse_procargs` and `agent::walk`'s generic children/classifier. Concretely: a pure parser over the `tmux list-clients` output *string*, and pure root-selection given a `(client_tty, pane_pid)` map plus a `dev_t` — with the subprocess and the syscalls confined to thin wrappers around them.

### SD6: Cost is enforced by probe counters, not by inspection
- **Status**: resolved
- **Rationale**: `tabmeta/mod.rs`'s test-only `probe` module (line 325) is how every existing data source proves its own budget. Two new counters — `TMUX_LIST` and `PROC_TTY` — are declared there and added to `reset_all()`, so SD3's "zero without tmux, one spawn per tick" is an assertable counter delta rather than a claim. Counters are declared centrally so the data-source modules never depend on each other to observe cost (the existing D13(6) rule).

## Features  (ordered — a dependency appears before its dependents)

### tmux-root-resolution
- declared-surface:
  - crates/app/src/tabmeta/tmux.rs
  - crates/app/src/tabmeta/proc.rs
  - crates/app/src/tabmeta/agent.rs
  - crates/app/src/tabmeta/mod.rs
- depends_on: []
- accept-seed: For a tab whose tree contains an attached tmux client, root resolution returns that client's pane_pid; for a tab with no tmux descendant it returns the input pid unchanged and spawns no subprocess (TMUX_LIST stays 0), and every failure mode degrades to the input pid without panicking.
- pre-approved-batch: yes

### tabmeta-tmux-reroot
- declared-surface:
  - crates/app/src/tabmeta/worker.rs
- depends_on: [tmux-root-resolution]
- accept-seed: A tab running tmux shows the branch, agent badge and task label of the active pane's project — and follows the pane when the user switches tmux windows — while a warm non-tmux tab's per-tick syscall budget is unchanged from today.
- pre-approved-batch: yes

### tabsession-tmux-reroot
- declared-surface:
  - crates/app/src/tabsession.rs
- depends_on: [tmux-root-resolution]
- accept-seed: With the ai-monitor overlay open on a tmux tab, hint_for resolves the session from the active pane's claude process instead of returning None and letting the caller fall back to mtime-ranking the projects directory.
- pre-approved-batch: yes
