# loop.md — Loop Contract: region-capture

> **STALE as of 2026-09-23 — see `spec.md` § Revision.** This contract gates the
> *file-and-path* delivery model. That model was replaced by clipboard delivery on the
> user's instruction, and the machinery several gates below name — `take_pending_write`,
> `OverlayHost::take_pending_writes`, the `OverlayPending` drain, `Tabs::deliver`,
> `pasted_dir` — was deleted. The Gate-1 commands, the mutation thresholds and the
> constraint checks still apply; the requirement-coverage row (43 sub-requirements) and
> the R7.4/R9/R10.4/R11.2 guards do not, because the requirements they counted are
> superseded. Re-derive before reusing this as a gate.

## Goal
Verify that the shipped region-capture implementation satisfies
`specs/region-capture/spec.md` — R0–R11, D1–D13, C1–C8 — without violating it,
the workspace rules, or the honesty the spec demands about its own unmeasured
gaps (A1–A7).

**The implementation is already complete.** This contract verifies; it does not
re-do the work. Iteration 1's WORK phase is a no-op.

## Scope & non-goals
- **In (the spec's declared surface, T1–T4):** `crates/app/src/capture.rs` (new),
  `crates/app/src/seam.rs`, `crates/app/src/terminal_app.rs`,
  `crates/app/src/lib.rs`, `crates/app/tests/wiring.rs`.
- **In, but OUTSIDE the spec's declared surface** — declared here rather than
  discovered later, and a Gate-3 item judges whether each is justified:
  - `crates/app/src/clipboard.rs` — `pasted_dir()` widened `fn` → `pub(crate) fn`.
    D4 *requires* reusing it ("duplicating them would create two answers to one
    question"), and it was private. No behaviour change.
  - `crates/app/src/tabs.rs` — new `Tabs::deliver(id, bytes) -> bool`, split out
    of `AppInner::deliver_to_tab`. Without the split, R7.4 and R7.7 have **no
    reachable test**: `deliver_to_tab` needs an `AppInner`, which needs an
    `EventLoopProxy`, which macOS builds only on the main thread.
- **Out:** `crates/term-core/`, `crates/renderer/`, `crates/common/`,
  `crates/ai-monitor/`, any `Cargo.toml`, `Cargo.lock`.

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| C8 — no `unwrap()` outside tests in the new module | `awk '/^#\[cfg\(test\)\]/{exit} /\.unwrap\(\)/{print; f=1} END{exit f}' crates/app/src/capture.rs` |
| C2 — no cross-platform branches introduced | `! git diff -U0 -- crates/app \| grep -q '^+.*cfg(target_os'` |
| C3 — `term-core` / `renderer` untouched | `! git status --porcelain -- crates/term-core crates/renderer \| grep -q .` |
| C3 — `Session.writer` shape unchanged | `git diff -- crates/app/src/session.rs crates/term-core/src/pty.rs \| grep -q . && exit 1 \|\| exit 0` |
| C1 — no new dependency | `! git status --porcelain -- Cargo.toml Cargo.lock crates/*/Cargo.toml \| grep -q .` |
| D10 — the `poll_all()`-appears-once guard still passes | `cargo test -p app --lib r1_4_the_rebuild_hangs_off_the_wake_arm_not_the_redraw_seam 2>&1 \| grep -q '1 passed'` |
| R10 — registration reaches the live registries | `cargo test -p app --test wiring 2>&1 \| grep -q '7 passed; 0 failed'` |

> The last two are named individually because they are the two assertions a
> registration mistake hides behind: the whole suite stays green while the chord
> reaches nothing, or while the panel's model stops being rebuilt.

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| workspace tests passing | `>= 1666` | sum of `test result: ok` lines |
| **survivors**, `capture.rs` | `== 0` | `cargo mutants -f crates/app/src/capture.rs -j 4` |
| **timeouts**, `capture.rs` | `== 0` | same run — a timeout is a slow kill, not a clean one |
| **survivors**, branch diff (seam/tabs/terminal_app/lib) | `== 0` | `cargo mutants --in-diff <(git diff) -j 4` |
| spec sub-requirements with a named, *running* test | `>= 41 of 43` | map each `#### Rn.m` to a test name and confirm it appears in `cargo test` **run output** |
| sub-requirements verified only structurally | `<= 3`, and all named | R9.1, R9.2 — *forced*: `snap_after_input` needs an `AppInner`. R11.2 — *specified* that way: its own `When` reads "its method set is inspected". The first draft of this row said "`<= 2`, both named R9.1/R9.2" and missed R11.2; the checker caught it. |
| text-matching (`include_str!`) guards | `== 4`, all named, **all comment-stripping** | R10.1 identity (`capture.rs`), R7.4 call site, R9 snap rule, R11.2 no-`exiting` (all `seam.rs` but the first). A separate row from "structural-only" above, because these are a *weaker instrument*, not a weaker verification: R10.1 and R7.4 each also have a behavioural test, so neither is structural-only — but all four match source text and so must strip comments, or a `// TODO` satisfies them. Measured: the first draft of the R10.1 guard did not strip, and a comment naming the symbol made it pass with the overlay deleted. |
| files changed outside Scope "In" | `== 0` | `git status --porcelain` |
| `feature_handlers().len()` | `== 2` (C7) | `wiring.rs` |
| overlay priorities, registered | `[200,100,0,0,0]` (R10.1) | `wiring.rs` |

> **Survivor COUNT, never a ratio** (CLAUDE.md): a file's mutant total moves with
> every edit, so a ratio written here is stale by the next commit. Today's totals,
> for context only: 53 mutants in `capture.rs`, 13 in the branch diff.
>
> The requirement-coverage row is deliberately "appears in the **run output**",
> not "appears in the source" — a grep for a comment measures only that a phrase
> was written.

## Gate 3 — Qualitative

- **C4 — the UI thread never waits.** `on_key` spawns and returns; the deadline
  lives on the watcher thread; and no lock a background thread takes is ever
  acquired on the input path. This is the invariant D10 rejected its own second
  attempt over (an `Arc<Mutex<writer>>` would have stalled the UI thread inside
  `lock()` on a blocked PTY write) — so it is judged, not assumed.
- **D8 — `Consumed` on every failure path, never `Passthrough`.** Falling through
  types a literal `g` at the prompt. All three failure paths (spawn failure,
  unusable cache dir, occupied target) must consume.
- **D3 — the verdict reads the file, never the exit status.** Nothing in the code
  may branch on an `ExitStatus`; A3 is why.
- **D6/D10 — delivery is tab-tagged.** A `TabId`, never an index, end to end; a
  closed tab drops the bytes with no panic and no other tab receiving them.
- **A5 — the seam's blast radius is stated, not hidden.** `take_pending_write`
  makes it *possible* for any future overlay to put bytes on a PTY; the
  "user-originated bytes only" invariant at `seam.rs` must say so.
- **Out-of-surface justification.** The two files outside the spec's declared
  `Files` lists (`clipboard.rs`, `tabs.rs`) — is each the minimum the spec's own
  decisions force, or is it scope creep?
- **Honesty about the unmeasured.** A1 and A2 are NOT measured (they need a human
  to grant Screen Recording and drag a rectangle). Nothing in the code or the docs
  may imply otherwise.
- **Spec alignment.** R0–R11 hold as written, including the phrasing constraint
  gap A7 forces: a `Then` may assert bytes *reach the tab's PTY*, never that they
  *appear on screen*.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):**
- Missing tests for requirements already implemented, inside Scope "In".
- Mutation survivors — add the test that kills them, or rewrite a condition that
  says less than it means (CLAUDE.md prefers this over an `exclude_re` entry).
- Lint, formatting, comment and doc corrections.
- Local renames and no-behaviour-change refactors inside Scope "In".

**🛑 STOP — escalate, do not proceed:**
- **Measuring A1/A2.** They require a human to grant macOS Screen Recording and
  to drag a rectangle. The loop must not fake, skip silently, or claim them.
- **Editing `docs/architecture.md` to record A1's answer** — the spec's Post-work
  asks for this, and there is no measured answer to record. Writing one would be
  fabricating a measurement.
- Touching `crates/term-core/`, `crates/renderer/`, `crates/common/`, or any
  `Cargo.toml`/`Cargo.lock` (C1/C3).
- Changing the chord away from `Cmd+Shift+G`, or narrowing rename's `Cmd+A` with
  a `!shift_key()` guard — D1 rejected the latter by name as blast-radius creep.
- Adding an `ApplicationHandler::exiting` hook — D12 put it out of scope
  explicitly, and R11.2 asserts its absence.
- Adding a new `UserEvent` variant — R7.3 requires reusing `OverlayPending`.
- Any change that conflicts with `spec.md` or the workspace rules
  (`no-gui-crates`, `macos-only`, `dependency-direction`, `zsh-local-only`).

## Exit criteria
Every gate passes and the Evidence Report is emitted, with A1/A2 recorded as
**open and requiring a human** — never as passed. A 🛑 boundary ends the loop in
escalation.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Candidates from the run of 2026-09-22. Interactive mode: these are DRAFTS
     awaiting the human's keep/drop decision; nothing here is `human-approved` yet. -->

### L1 — a registration set's counts, priorities and layer count do not pin its members
- cause: gate-fail
- source: pending-human-approval
- form: gate1-command
- status: active
- check: `cargo test -p app --lib r10_1_the_registration_list_names_the_capture_overlay 2>&1 | grep -q '1 passed'`
- note: `wiring.rs` asserts length 5, priorities `[200,100,0,0,0]`, layer count 5, descending order and initial invisibility — and **all five survive** replacing `CaptureOverlay` with a second `ImagePresenter`. Measured: 7 passed / 0 failed with the chord reaching nothing. A count-based registration test cannot see an identity swap between same-priority overlays. Pin the member, not the shape.

### L2 — `cargo mutants` generates no mutant for a `const`, so a constant is unguarded by the coverage gate
- cause: gate-fail
- source: pending-human-approval
- form: gate1-command
- status: active
- check: `cargo test -p app --lib the_production_budget_and_tick_are_the_decided_values 2>&1 | grep -q '1 passed'`
- note: `cargo mutants --list -f crates/app/src/capture.rs` reports 53 mutants, **none** naming `BUDGET`, `TICK` or `Capture::new`. D7's 5-minute budget could be edited to 3 seconds, or its two `Duration` arguments swapped at the one call site (they type-check in either order), with Gate 1 and the mutation gate both fully green and every real capture dying mid-drag. Zero survivors is not zero risk — it is zero risk *among the mutants that were generated*.

### L3 — an `include_str!` guard must strip comments, or a `// TODO` satisfies it
- cause: gate-fail
- source: pending-human-approval
- form: context
- status: active
- check: —
- note: the first draft of L1's own guard matched raw source. Leaving `// TODO: restore Box::new(CaptureOverlay::new(..))` behind made it pass with the overlay deleted — measured, 2 passed / 0 failed. Every text-matching guard in this feature now pipes through `.lines().map(|l| l.split("//").next()...)`, which `overlay.rs:5974` already did. A guard a comment can satisfy is not a guard.

### L4 — a three-line join between two tested halves is the least-tested code in a feature
- cause: gate-fail
- source: pending-human-approval
- form: gate1-command
- status: active
- check: `cargo test -p app --lib r7_4_the_wake_arm_drains_pending_writes_after_poll_all 2>&1 | grep -q '1 passed'`
- note: the aggregator had tests, the routing had tests, and the `terminal_app` block wiring them together had none — deleting it left the whole workspace suite green with every completed capture stranded forever. Also assert the **order** (drain after `poll_all`) and an **exactly-one-site** count: the first draft split the arm with `split(..).nth(1)`, which runs to end-of-file, so moving the drain into another function in the same file still passed.

### L5 — a test that reads the grid is measuring echo, not PTY arrival (gap A7)
- cause: gate-fail
- source: pending-human-approval
- form: context
- status: active
- note: `delivery_..._and_no_other` asserted through `terminal.snapshot()` while its failure message said "the bytes never reached the named tab's PTY". Display depends on the foreground program echoing, which this repo cannot enforce — the spec's A7 says a `Then` may assert bytes *reach* a PTY and never that they *appear*. The write's own return value is the requirement's observable; the echo is a stronger, shell-dependent fact. Assert both, and name which is which.
