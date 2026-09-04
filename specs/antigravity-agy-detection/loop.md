# loop.md — antigravity-agy-detection

> Bug: the left-tab card background does not change when the Antigravity CLI runs in a
> tab (parity with the earlier claude detection fix, `specs/agent-skin-detection-fix`).
> Root cause proven against the user's live `agy` session on this machine.

- created: 2026-07-20
- type: bugfix
- branch: build/sidebar-redesign
- autonomy: auto-fix class (bug fix aligned with the mock's Antigravity skin; no schema /
  auth / data-loss / spec-conflict).

## Goal
When the Antigravity CLI runs in a tab, that tab's sidebar card renders the Antigravity
skin, because detection recognizes the launcher's real command name and survives the
auto-updater's in-place binary rename.

## Root causes (empirically confirmed against live pid 30004 = `agy` under a tab zsh)
- **The CLI runs as `agy`, not `antigravity`.** `~/.local/bin/agy` (Antigravity's short
  launcher) was absent from the detection tables (only `antigravity` was present), so a
  fresh `agy` — basename `agy` — did not classify. Fix: add `agy → Antigravity` to
  `AGENT_TABLE` and `ARGV_TABLE`.
- **Auto-update mangles the running binary's basename.** The updater renames the running
  file in place to `agy.<id>.old` (observed `proc_pidpath` = `~/.local/bin/agy.1784…​.old`),
  so exact-basename matching misses a long-running session. Fix: stage 1a — match the
  leading dot-component of the basename (`agy.<id>.old` → `agy`), syscall-free, exactness
  preserved (`agyx.7.old` → leading `agyx` → no match).

## Gate 1 — Pass/Fail (100%)
- `cargo build`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --check`
- **Real-process probe** (done): `agent_of_pid(<live zsh with the agy child, pid 29989>)`
  returned `Some(Antigravity)` — including the `.old` binary.

## Gate 2 — Quantitative
- No new per-process syscall: stage 1a is a string op on the already-fetched basename; the
  interpreter `sysctl` gate (D5/R4.4) is unchanged.

## Gate 3 — Qualitative (maker ≠ checker)
- correctness: `agy` (fresh) and `agy.<id>.old` (updated) both classify as Antigravity,
  locked by a test using the exact live `proc_pidpath` shape.
- no-false-positive: the leading-component match stays exact (`agyx.*` etc. do not match).
- scope: change confined to `tabmeta/agent.rs`; rendering/mapping untouched.

## Lessons
- context: agent CLIs identify by argv[0], but their on-disk exe basename is unreliable —
  claude runs a `versions/<ver>` binary, agy an auto-renamed `<name>.<id>.old`. Detection
  must not assume `proc_pidpath` basename == the command name.
- context (2026-08-30, `specs/agent-classifier-deleted-binary`): the same updater's **next**
  step is worse than the rename this spec fixed. It later *deletes* the `<name>.<id>.old`
  file, and `proc_pidpath` then returns 0 — so an `exe_path`-only classifier does not merely
  misread the name, it loses the process entirely and every stage below it becomes
  unreachable. A live `agy` session went undetected mid-run, taking both the ai-monitor panel
  and the sidebar skin with it. The generalisation: **do not read an agent's identity from a
  path that the agent's own updater owns.** The fix is the kernel's exec-time name
  (`PROC_PIDTBSDINFO.pbi_name`), which is fixed at `exec` and moves for neither event —
  measured across the real sequence, `proc_pidpath` went rc=150 → 174 → 0 while `pbi_name`
  read `agy` at all three steps.
