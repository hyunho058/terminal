# loop.md — Loop Contract

> Supervisor document for **tmux-pane-cwd** (`specs/tmux-pane-cwd/spec.md`).
> This is a **post-implementation verification** contract: all five tasks (T1–T5) plus the
> pre-flight convergence merge are already built. The loop starts by verifying, not by
> re-doing the work.

## Goal
Prove the shipped implementation satisfies the spec's `## Requirements` (R0–R10, 37 GWT
sub-requirements) without violating the spec's `## Constraints` (C1–C9) or the project rules
(`dependency-direction`, `no-gui-crates`, `macos-only`, `zsh-local-only`).

## Scope & non-goals
- **In:** the six files in the spec's `## Declared Surface` plus the one recorded
  `declared-deviation`; test additions for behaviour already in scope; comment/doc
  corrections; lint/format fixes.
- **Out:** new features beyond R0–R10; changing the approved decisions D1–D13; re-opening the
  sibling set's own surface (`tabmeta/tmux.rs`, `proc.rs`, `agent.rs`) beyond what is already
  recorded; anything that would require re-approving the spec.
- **Explicitly out of automated reach** — these must NOT be marked passed by this loop:
  - **R0.1 (render half)** — that the dashboard *renders populated rows* needs a live tmux
    server plus a running app via `/run-term`. The cwd resolution half IS automated.
  - **R4.4 end-to-end tmux window switch** — needs a human switching windows in a live
    session. The resolver half is covered by the sibling's
    `a_new_tick_refetches_so_a_window_switch_is_followed`.
  - **R7.1 split-pane focus follow** — needs a live split window. `list-clients` resolving
    `pane_pid` in client context was verified live during L1 but is not reproducible in CI.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| release build | `cargo build --release` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| format | `cargo fmt --all -- --check` |
| typecheck | N/A — `cargo build` is the type check for Rust (no separate command) |
| **R6.4** no tmux contact reachable from the event loop | `! grep -HnE '(tmux::effective_root\|resolved_root)' crates/app/src/session.rs crates/app/src/tabs.rs \| grep -vE ':[0-9]+:[[:space:]]*(///\|//\|\*)'` — the hot cwd path must reference no tmux resolver. **`-H` is load-bearing**: without it `grep -n` omits the filename for a single file and the comment filter silently stops matching. Validated in both directions — it fires on `overlay.rs:106` (the legitimate guarded reference) and is silent on these two |
| **R9.2** declared-surface respected | `test -z "$(git diff --name-only a35679d -- '*.rs' \| grep -vE 'crates/app/src/(session\|tabs\|terminal_app\|overlay)\.rs\|crates/app/src/tabmeta/(mod\|worker)\.rs\|crates/app/src/tabsession\.rs')"` |
| **R10.3** suite passes with no tmux binary | `PATH="$HOME/.cargo/bin:/usr/bin:/bin" cargo test --workspace` — drops `/opt/homebrew/bin` (where `tmux` lives) while keeping `cargo`, so any test silently depending on a live tmux fails here. Keeping cargo on the path is load-bearing: a bare `/usr/bin:/bin` makes the command fail to run at all and the gate passes vacuously |
| **R9.1** the dependency actually landed | `test -f crates/app/src/tabmeta/tmux.rs && grep -q 'pub(crate) fn effective_root' crates/app/src/tabmeta/tmux.rs` |
| **R4.1** the D6 re-point **trigger** exists | `awk '/fn feed_agent_states/,/^    }$/' crates/app/src/terminal_app.rs \| grep -vE '^[[:space:]]*(//\|///\|\*)' \| grep -q 'notify_active_changed'` — the comment filter is load-bearing: without it a commented-out `// self.overlays.notify_active_changed(…)` still matches and the gate passes on a dead trigger (Gate-3 checker finding) — verifies the *trigger*, not just the detector. `Repoint::changed` can be perfectly correct and the feature still dead if nothing calls it; a sibling feature in this repo shipped exactly that bug. Validated: silent over `push_agent_states_for`, and fails when the call is stripped |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| tests passing | `≥ 851`, `0 failed` | `cargo test --workspace` — sum the `test result:` lines ONLY, never the `running N tests` lines (they double-count) |
| clippy warnings | `= 0` | exit code of the `-D warnings` command |
| rustfmt drift | `= 0 files` | `cargo fmt --all -- --check` |
| sub-requirements with automated evidence | **`≥ 26 of 37`** | manual trace of each GWT to a named test or Gate-1 command. Baseline measured at contract time is **22**; the gap to 26 is the D6 trigger (R4.1/R4.2/R4.3) and the cost claim (R6.1), all of which are missing tests for in-scope behaviour and therefore Auto-fix. 3 are excluded **by name**: R0.1(render), R4.4, R7.1 — see Scope. The remaining 8 (R0.2, R1.2 worker-level, R3.2, R5.1, R5.3, R8.1, R8.3, R8.4) are code-traceable and may stay so if the trace is written down |
| `term-core` GPU/window deps | `= 0` | `grep -nE '^[[:space:]]*(wgpu\|winit)' crates/term-core/Cargo.toml` empty **and** `grep -rHnE '^[[:space:]]*use (wgpu\|winit)' crates/term-core/src/` empty (`dependency-direction`). **Not** a bare `grep -rn "wgpu\|winit" crates/term-core/`: `lib.rs:4` and `:199` are doc comments *asserting the absence of* these deps, so the naive form reports 2 and always fails. Threshold unchanged — only the measurement was made capable of distinguishing a dependency from a comment about one |
| new GUI-toolkit deps | `= 0` | `git diff a35679d -- '*/Cargo.toml'` shows no egui/iced/druid/slint/gtk/gpui/dioxus/tao |
| files edited outside Declared Surface | `= 0` beyond the 1 recorded deviation | the Gate-1 R9.2 command; `tabsession.rs` is the single allowed extra |
| unguarded tmux calls on the UI thread | `= 0` | every UI-thread resolve must route through `tabsession::resolved_root`'s `recv_timeout(250ms)`, never `effective_root_uncached` directly (C1) |

## Gate 3 — Qualitative  (score + grounds + action)

Scored by a **separate checker subagent** (maker ≠ checker), given only the diff, this
contract, and the spec.

- **Spec alignment** — does the code do what R0–R10 say? In particular: is D13's toggle
  decoupling actually complete (both the *root* and the *cwd*, not just one), and is D12's
  overlay resolve genuinely independent of the snapshot?
- **Deviation honesty** — is the one C3 deviation (`tabsession::resolved_root` →
  `pub(crate)`) genuinely the smaller evil versus duplicating a 250 ms safety timeout, and is
  it recorded where a reviewer would find it?
- **Trigger correctness, not just detector correctness** — D6's re-point fires from
  `feed_agent_states`; verify the *trigger* actually reaches the overlay on a cwd-only change,
  not merely that the guard would work if called. (This failure mode has bitten a sibling
  feature in this repo before.)
- **Cost-claim honesty** — R6.1 claims zero syscalls on the keystroke path. Is that proven,
  or only argued? A claim proven by "the fallback closure was not invoked" is weaker than a
  probe-counter delta; say which this is.
- **Architecture fit** — does it honour `dependency-direction` (term-core untouched), keep all
  tmux I/O on the worker thread (C1), and match the codebase's pure-logic/thin-wrapper split
  (`published_or`, `pane_cwd_with`) rather than inventing a new idiom?
- **Naming / readability** — are `published_or`, `pane_cwd_with`, and `TabMeta.cwd` clear at
  their call sites, and does `TabMeta`'s revised doc comment tell the truth about the
  now-tri-state meaning of `cwd: None`?

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact, a comparison>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, **missing tests for behaviour already in
scope** (this is the expected work of iteration 1 — R4.1/R4.2/R4.3/R6.1), docs/comments,
local renames, behaviour-preserving refactors that keep Gate 1 green.

**🛑 STOP — call a human (escalate):**
- DB schema changes — N/A in this project, but the fence stands
- Data-loss-capable migrations — N/A
- Auth / permission / access-control policy changes — N/A
- Payment / security-sensitive changes (secrets, crypto, billing) — N/A
- **Widening the declared surface further** — editing any additional sibling-owned file
  (`tabmeta/tmux.rs`, `proc.rs`, `agent.rs`) or making a second C3 deviation
- **Changing an approved decision** (D1–D13) or a spec Constraint to make a gate pass
- **Putting an unguarded blocking tmux call on the winit event-loop thread** — this is the
  hazard D1 was rewritten to remove; re-introducing it to satisfy a gate crosses the fence
- **Committing or merging** — the working tree is uncommitted by design; the merge is the
  human's

## Exit criteria
The loop exits only when **every** gate above passes and the Evidence Report is emitted.
Hitting a 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Each lesson is a mistake this loop already made, hardened so it can't recur.
> Phase 0 loads `status: active` lessons: `gate1-command` / `gate3-item` forms are folded into
> the gates above; `context` forms are read as guidance. Lessons are scoped to THIS `loop.md`
> — they do not propagate to other contracts. The human owns this section (curation).

### L1 — a gate command must be demonstrated to FAIL before it is trusted
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: **gate-commands-validated-both-ways**: every Gate-1 command in this contract has been
  run against a synthetic violation and shown to FAIL, not merely run against the live tree and
  shown to pass. A command that cannot fail is not a gate.
- note: four commands in this contract would have passed vacuously on first writing — the
  hot-path grep matched a doc comment (and, once tightened to require `(`, stopped matching the
  real reference, which is a function *value*); the `term-core` GPU metric matched the two doc
  comments *asserting* there are no GPU deps, reporting 2 against a threshold of 0; the
  no-tmux gate stripped `PATH` so aggressively that `cargo` itself disappeared, so it produced
  no output at all, which reads as success; and the R4.1 trigger grep would have matched a
  commented-out call. All four were caught only by running them in the failing direction.
