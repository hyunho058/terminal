# loop.md — agent-skin-detection-fix

> Bug: the left-tab card background does not change when a Claude Code agent runs in
> a tab. Root cause is in agent DETECTION (not rendering). Two independent bugs, both
> proven empirically against a live `claude` process on this machine (Darwin 25.3.0).

- created: 2026-07-20
- type: bugfix
- branch: build/sidebar-redesign (continues the sidebar-redesign work)
- autonomy: auto-fix class (bug fixes aligned with the mock's intent; no schema / auth /
  data-loss / spec-conflict). Fence not crossed.

## Goal
When a Claude Code agent runs in a tab, that tab's sidebar card renders the Claude skin
(orange gradient background), because agent detection now (1) reads the descendant pid list
correctly and (2) recognizes the native versioned `claude` install.

## Root causes (empirically confirmed)
- **Bug 1 — `crates/app/src/tabmeta/proc.rs::parse_childpids`.** `proc_listchildpids`
  returns a **count of pids** (Apple libproc divides by `sizeof(int)` internally); the code
  divides that count by `sizeof(i32)` again. A zsh with 1 `claude` child returns `1`, and
  `1/4 = 0` → empty child list → the walk never reaches the agent. Fix: treat the return
  value as a count.
- **Bug 2 — `crates/app/src/tabmeta/agent.rs` classifier.** The `.local` installer runs
  `~/.local/share/claude/versions/<ver>`, whose `proc_pidpath` basename is a version string
  (`2.1.215`), not `claude`; only argv[0] says `claude`. Fix (chosen: targeted signature):
  recognize an agent-name path component immediately followed by a `versions` component, at
  zero extra syscall and with no false positive for a home dir named after an agent.

## Gate 1 — Pass/Fail (100%)
- `cargo build`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --check`
- **Real-process probe** (manual, ad-hoc): a live `zsh → claude` pid classifies as
  `AgentKind::Claude` through the real syscall path (`agent_of_pid`).

## Gate 2 — Quantitative
- No new per-process syscall in the walk: the interpreter-gate `sysctl` cost (D5/R4.4) is
  unchanged — the `sysctl_gate_closed_for_non_interpreters` test stays green (the versioned
  match is path-only, no argv read).

## Gate 3 — Qualitative (maker ≠ checker)
- correctness: both root causes fixed and locked by a regression test that fails on the old
  code — score + grounds + action.
- no-false-positive: the `versions/` signature does not misfire on `/Users/claude/…`.
- scope: change confined to `tabmeta/{proc,agent}.rs`; rendering/mapping untouched.

## Lessons
(none yet)
