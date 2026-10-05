# loop.md — Loop Contract: selection-scroll-anchor (verification gate)

> Interactive /loop (human-approved), run as /agent-orchestrate's Phase 3.5 gate. The
> implementation is already complete (T1–T5, commits `7dcc6b1` `452f6b9` `bfbf10d`
> `870e724` `f3ca428` on `feat/selection-scroll-anchor`, base `9aea35e`). Iteration 1
> **verifies**; work happens only to close a failing gate.

## Goal
Prove that `feat/selection-scroll-anchor` satisfies `specs/selection-scroll-anchor/spec.md`
as approved: R0–R9 hold, C1–C6 are respected, and D1–D8 are implemented as written. A
drag selection either tracks its text or disappears, and it never highlights or copies
text the user did not select.

## Scope & non-goals
- In: the five commits' files: `crates/term-core/src/{grid,lib,parser,scrollback,selection,snapshot}.rs`,
  `crates/term-core/tests/{selection,selection_check}.rs`, `crates/app/src/{drag,session,terminal_app,lib}.rs`,
  `crates/app/tests/vertical_slice.rs`, `crates/renderer/tests/golden_frame.rs`, and the
  `snap_with_selection` helper in `crates/renderer/src/selection_layer.rs`. Also the spec's
  Post-work bookkeeping pointers in `specs/selection-model/spec.md` and
  `specs/selection-highlight/spec.md`, which are doc-only.
- Out:
  - Every spec Non-goal.
  - Release-after-tab-switch semantics beyond what the human approved at this gate.
    Iteration 1 escalated the hazard T3 reported (B3), and the human chose "inert
    release". Iteration 2 escalated the round trip (B5), and the human chose "left tab
    = not a click". Both are now dated amendments under the spec's D3 and are in scope.
    Any further change to release semantics is still a 🛑 boundary.
  - Lifting the drag freeze (D7).
  - Rendering `?2026` (Known Gap).
  - R5.9 and the `/run-term` eyeball. These are manual by the spec's own text and are
    carried to the report as human post-work, not as gates.

---

## Gate 1 — Pass/Fail  (100% required, binary)
Run from the worktree root with `SKIP_GPU_TESTS` and `SKIP_SHELL_TESTS` **unset**, so the
pixel and real-zsh tests run instead of skipping.

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| typecheck | N/A — covered by `cargo build` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |
| R8 tint tests ran, not skipped | `out=$(cargo test -p renderer --test golden_frame selection_tint -- --nocapture 2>&1); echo "$out" \| grep -q "test result: ok. 2 passed" && ! echo "$out" \| grep -qi skipping` |
| R0.3 term-core headless | `! cargo tree -p term-core \| grep -qE 'wgpu\|winit'` |
| R0.3 no dependency / lock change | `git diff --quiet 9aea35e -- '*Cargo.toml' Cargo.lock` |
| R2.8 renderer contract | `test "$(git diff -U0 9aea35e -- crates/renderer/src/selection_layer.rs \| grep -c '^@@')" = 1 && git diff -U0 9aea35e -- crates/renderer/src/selection_layer.rs \| grep -q '^@@ -286'` (one hunk, inside the `snap_with_selection` helper) |
| R6.6 / C3 links unchanged | `git diff --quiet 9aea35e -- crates/term-core/src/links.rs` |
| R6.7 visible-only extractor gone | `! grep -n "fn selection_text" crates/term-core/src/snapshot.rs && ! grep -rn "snapshot().selection_text" crates/app/src && grep -q "terminal.selection_text()" crates/app/src/terminal_app.rs` |
| R7.4 latch tests untouched | `git diff --quiet 9aea35e -- crates/app/src/input.rs && ! git diff -U0 9aea35e -- crates/app/src/terminal_app.rs \| awk '/^@@/{split($3,a,",");print substr(a[1],2)+0}' \| awk -v t="$(grep -n '^mod tests {' crates/app/src/terminal_app.rs \| cut -d: -f1)" '$1>=t{f=1} END{exit !f}'` (no hunk at or below the test module) |
| Spec bookkeeping (Post-work) | `grep -q selection-scroll-anchor specs/selection-model/spec.md && grep -q selection-scroll-anchor specs/selection-highlight/spec.md` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| Workspace tests | failed = 0; passed ≥ 1960; ignored = 18 (no test newly parked) | the `cargo test --workspace` totals |
| Requirement coverage | 100% of automatable R-ids, i.e. R0–R8 except R5.9 (manual) and the command-checked R0.3/R2.8/R6.7. Each maps to ≥ 1 passing test that asserts the requirement's **Then**. | grep the `R\d+\.\d+` tags in test sources, then the checker confirms the assertion matches the Then |
| Mutation survivors in new/changed code, on the **merged** diff | 0, or each one shown equivalent and rewritten (never `exclude_re`) | `cargo mutants --in-diff <(git diff 9aea35e) -f <file> -j 4` for each of: term-core `scrollback.rs selection.rs snapshot.rs grid.rs parser.rs lib.rs` (with `--cargo-test-arg -p --cargo-test-arg term-core`), and app `drag.rs session.rs` (with `-p app`) |
| `unwrap()` added outside tests | 0 | the `+` lines of `git diff 9aea35e` in `src/` that contain `.unwrap()`, each checked against whether it sits in a `#[cfg(test)]` module |
| C5 check bound | reference ≤ `rows × cols` cells; no work without a selection | the R4.9 test, plus reading `check_selection`'s early return |

## Gate 3 — Qualitative  (score + grounds + action; checker-scored, maker ≠ checker)
Bar: each item ≥ 8/10, with no correctness defect left open.

- **Spec alignment.** D1–D8 are implemented as written: line arithmetic, projection
  clipping, reference capture at set, the D5 state machine and its triggers, copy
  re-verify, the D3 move order, and the D7 "no dedicated clear". Any deviation is
  documented and justified.
- **Correctness of edge cases.** Underflow/overflow in line math; evicted vs. below-bottom
  lines; a backward drag; the alt screen; a repeated `?2026h`; `?2026l` with no block open;
  a narrower/wider history row; the spent-drag detection vs. the first move.
- **Architecture fit.** All policy lives in `term-core` (C1). The renderer contract keeps
  its shape (C2). The app arms only delegate. No duplicated soft-wrap rule (C3).
- **Test quality.** Each test asserts its requirement's **Then** (not a weaker proxy).
  Fixtures follow the spec. Every guard can go red (falsifiable). GPU/shell tests skip only
  when the resource is absent.
- **Naming / docs.** Comments cite the decisions, stale comments are updated
  (`selection-model` D9 references, the `GridSnapshot.selection` doc, the `DragLatch`
  rationale), and the new API is clear at its call sites.

```
<item>: <score>/10
  grounds: <specific, checkable observation — file:line, a measured fact>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):**
- lint, format, and type errors;
- adding missing or strengthening weak tests for in-scope requirements;
- killing mutation survivors with tests, or rewriting a condition into its equivalent
  form;
- docs and comments, including the Post-work spec pointers;
- local renames;
- behaviour-preserving refactors;
- a behaviour fix that brings code **into line with an explicit spec requirement or
  decision** (the spec text is the authority).

**🛑 STOP — call a human (escalate):**
- any behaviour beyond or against the spec text: for example, redesigning release after a
  tab switch, changing D5's timing or `SYNC_STALE`, adding a dedicated clear on
  RIS/alt/`2J`, following region scrolls, or lifting the drag freeze;
- a new dependency or a `Cargo.lock` change;
- a renderer contract change beyond removing `GridSnapshot::selection_text`;
- editing `links.rs` tests or the `terminal-scroll` D6 latch tests;
- anything that would rewrite or drop work in the five commits, where what is found
  contradicts the spec;
- (no DB, auth, payment, or secret surface exists in this change).

## Exit criteria
The loop exits only when **every** gate above passes and the Evidence Report is emitted.
Hitting a 🛑 boundary ends the loop in escalation, not completion. Exiting does not
merge, push, or squash: those stay with the human.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — The release-semantics guards must exist and pass
- cause: escalation
- source: human-approved
- form: gate1-command
- status: active
- check: `cargo test -p app --lib -- --exact drag::tests::a_release_on_another_tab_is_inert drag::tests::a_drag_that_left_its_tab_is_no_click_when_it_comes_back 2>&1 | grep -q "test result: ok. 2 passed"`
- note: B3 and B5 (2026-10-04) made a release cleared or opened a link on the wrong tab. This fails if either guard is deleted, renamed or skipped, not only if it goes red.

### L2 — Walk an "unchanged" endpoint across its changed input space
- cause: escalation
- source: human-approved
- form: gate3-item
- status: active
- check: When a decision keeps an endpoint "as today" while another rule changes what it receives (here R3.5 ignoring foreign-tab moves, which `drag_moved` feeds to release), enumerate the new input sequences and judge each one. For release: A→B, A→B→A with and without motion, a same-cell jitter, and off-grid motion.
- note: B3 and B5 were the same miss, found one path at a time across two iterations. Enumerating the sequences up front finds both in one pass.
