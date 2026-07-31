# loop.md — Loop Contract

> Supervisor document for **tab-resize** (`specs/tab-resize/spec.md`).
> This is a **post-implementation verification** contract: pre-work and all nine tasks
> (T1–T9) are already built. The loop starts by verifying, not by re-doing the work.

## Goal
Prove the shipped drag-to-resize implementation satisfies the spec's `## Requirements`
(R0–R10, 52 GWT sub-requirements) without violating the spec's `## Constraints` or the
project rules (`dependency-direction`, `no-gui-crates`, `macos-only`, `zsh-local-only`) —
and report honestly which sub-requirements have automated evidence and which do not.

## Scope & non-goals
- **In:** the five files the tasks touched (`crates/renderer/src/sidebar_layout.rs`,
  `crates/renderer/src/lib.rs`, `crates/common/src/config.rs`, `crates/common/src/error.rs`,
  `crates/app/src/input.rs`, `crates/app/src/terminal_app.rs`); added tests for behaviour
  already inside R0–R10; comment/doc corrections; clippy/rustfmt fixes.
- **Out:** new features beyond R0–R10; changing the approved decisions D1–D17a; anything
  that would require re-approving the spec.
- **Explicitly out of automated reach** — these must **NOT** be reported as passed:
  - **R0.1** end-to-end drag (window + pointer required — `/run-term`, a human at the trackpad).
  - **R2.1/R2.2/R2.4** the *application* of the cursor icon to the live window. The
    region→icon mapping is unit-tested; that macOS actually shows a ⟷ is not.
  - **R5.2** "all three tabs resized" — needs real PTYs.
  - **R7.4/R7.5/R7.6** the `CursorMoved` / `CursorLeft` / `Focused(false)` arms themselves.
  - **R10.1/R10.2/R10.3** persistence across an actual relaunch.
  The decision logic behind each of these IS covered (see Gate 2's coverage metric); what
  is unverified is the winit event wiring, which this codebase cannot test headlessly —
  `crates/app` and `crates/renderer` have no `tests/` directory by design.

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
| **term-core stays GPU/window-free** | `test "$(grep -cE '^(wgpu\|winit)' crates/term-core/Cargo.toml)" = "0"` |
| **refit guard is at the call site, not inside `refit`** (R5.3) | `test "$(grep -c 'needs_refit' crates/app/src/settings.rs)" = "0"` |
| **hover resolves the region exactly once** (R2.3) | `test "$(grep -c 'sidebar_region(' crates/app/src/terminal_app.rs)" = "1"` |
| **`refit` is the only grid re-fit site** (R5.4) | `test "$(grep -rl 'resize_all' crates/app/src/ \| grep -v 'tabs.rs' \| tr -d ' ')" = "crates/app/src/settings.rs"` — only `settings.rs` *calls* `Tabs::resize_all`; `tabs.rs` defines it. Added at iteration 3: R5.4 had been listed as "structurally checked" with no grep actually behind it |
| **the event loop computes no sidebar geometry** (R1.7, SD2) | `test "$(grep -cE 'Rect *\{\|_rect\(' crates/app/src/terminal_app.rs)" = "0"` — added at iteration 5. This is R1.7's enforceable half; its "no *threshold* arithmetic" clause is **not** checked because the spec contradicts itself there (T5 explicitly mandates `collapse_threshold` live in `input.rs`) |
| **no GUI/widget crate added** | `! git diff HEAD -- '*/Cargo.toml' \| grep -qE 'egui\|iced\|druid\|slint\|gtk\|gpui\|dioxus\|^\+tao'` — **HEAD, not `main`**: `main` is *ahead* of this branch (it carries the hyperlink feature), so `git diff main` shows that feature as deleted and is meaningless here |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| tests passing | `≥ 849`, `0 failed` | `cargo test --workspace 2>&1 \| grep -E '^test result' \| awk -F'[ ;]' '{p+=$4; f+=$7} END {print p, f}'` — sum ONLY `test result:` lines, never `running N tests` (double-counts) |
| clippy warnings | `= 0` | exit code of the `-D warnings` command |
| rustfmt drift | `= 0 files` | `cargo fmt --all -- --check` |
| sub-requirements with **direct** automated evidence | `≥ 32 of 52` | manual trace of each GWT to a named test or Gate-1 structural check |
| sub-requirements with **no** automated evidence at all | `≤ 9 of 52`, each named | the wiring/human list in Scope above |
| new dependencies added | `= 0` | `git diff HEAD -- '*/Cargo.toml'` |

> **Threshold amended by the human at iteration 2** (`≥36` → `≥32`). The original figure
> rested on a self-graded coverage claim; the Gate-3 checker sampled it and proved five
> sub-requirements (R5.1, R7.2, R7.3, R7.7, R9.3) were counted as "directly tested" when
> only their *predicate* was tested, not the call site. R9.3 was then genuinely closed with
> a new renderer test; the other four are honestly re-classified as partial. **Closing the
> remaining gap requires a headless winit event-loop harness** — new infrastructure this
> spec never scoped — so the bar was lowered by the contract owner, deliberately and on the
> record, rather than by the loop quietly re-grading itself.

**Coverage classification** (the trace behind the two coverage metrics — 52 total):

*(Corrected at iteration 2 after the Gate-3 checker's audit — the pre-audit split was
33/3/7/9 and over-claimed by five.)*

- **28 directly tested**: R1.1–R1.6, R3.1–R3.4, R4.1–R4.4, R6.1, R6.2, R8.2–R8.5, R8.7,
  R9.1–R9.6, R10.5.
- **4 structurally checked** by the Gate-1 greps above: R2.3, R5.3, R5.4, R1.7.
- **11 partially covered**: R0.2, R2.1, R2.2, R5.1, R7.1, R7.2, R7.3, R7.7, R8.1, R8.6,
  R10.4.

**The operative direct/partial rule** (spelled out at iteration 5 — the metric lands at
exactly 32 against a bar of 32, so the boundary carries no margin and must not be vague):

> **direct** = *both* the decision function *and* its observable effect are unit-tested;
> only the winit dispatch between them is unverified.
> **partial** = only one end is tested.

By that rule R4.1/R4.2 are direct — `next_collapsed`'s return is pinned in `input.rs` *and*
the effect `set_sidebar_collapsed(true) → left_inset == 0.0` is pinned in
`renderer/src/lib.rs` — while R5.1 is partial, because its "`refit` was not called" half has
no tested endpoint at either end.

**Arithmetic**: 28 + 4 + 11 + 9 = **52** ✓, evidence total **32**. *(Corrected at iteration
5: the iteration-3 table read 28 + 4 + 12 + 9 = 53 because the "4 structural" list carried
three requirement IDs plus a duplicate grep — a self-inflicted counting error the Gate-3
checker caught. The real figure at that point was 31, one under the bar; R1.7's rect clause
was then genuinely hardened into a Gate-1 check to close it, not re-graded.)*
- **R9.1 was reclassified to partial at iteration 3, then genuinely closed at iteration 4**
  by extracting `input::suppress_delivery` — the predicate the event loop actually calls —
  and testing it directly (`the_production_suppression_predicate_drops_only_true_no_ops`).
  It counts in the 28 above. The gap was closed by making the code testable, not by
  re-grading it.
- **9 uncovered** — pure winit/PTY wiring or human-only: R0.1, R2.4, R5.2, R7.4, R7.5,
  R7.6, R10.1, R10.2, R10.3.

Two clauses inside otherwise-covered sub-requirements are also untested and are named here
rather than buried: **R10.5**'s "no byte sequence `encode_key` forwards to zsh" (asserted by
inspection of the chord table, not by a test that calls `encode_key`), and **R8.2**'s
"byte-identical" promise, which now holds for comments and CRLF but has no exhaustive
property test.

## Gate 3 — Qualitative  (score + grounds + action)

Scored by a **separate checker subagent** (maker ≠ checker), given only the diff, this
contract, and the spec.

- **Spec alignment** — does the code do what R0–R10 say? Two deviations were made
  deliberately and must be judged as honest-or-divergent: (a) D17's release arm became a
  tested `release_target()` function rather than relying on match-arm position; (b) D11
  said keep `sidebar_hover`'s signature, but it became `#[cfg(test)]` once production
  stopped calling it.
- **Architecture fit** — `dependency-direction` (term-core untouched), SD2 (the app
  performs no sidebar geometry — all rects come from `sidebar_layout`), D6 (`settings::refit`
  remains the one refit site).
- **The raw-vs-clamped invariant** — is D9a actually upheld everywhere? Trace every
  consumer of `resize_raw_width` and confirm no path feeds a clamped value into a collapse
  threshold, which would make D12 dead code (the exact contradiction the L2 review caught).
- **Echo-suppression correctness** — is the latch genuinely latched rather than one-shot,
  is the predicate exact-match, and can a failed write ever let D15a clear the override
  (which would break D14's promise)?
- **Config-file fidelity** — can `edit_sidebar_toml` corrupt, duplicate, or reorder a
  hand-written file on any input; can a key land under the wrong table?
- **Naming / readability** — are `ResizeDrag`, `ReleaseTarget`, `ConfigDelivery`,
  `needs_refit`, `next_collapsed` clear at their call sites?
- **Honesty of coverage claims** — does Gate 2's classification actually match reality, or
  are wiring-only requirements being quietly counted as tested?

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** clippy/rustfmt fixes, missing tests for behaviour
already inside R0–R10, comment/doc corrections, local renames, behaviour-preserving
refactors that keep Gate 1 green.

**🛑 STOP — call a human (escalate):**
- Any change to the approved decisions D1–D17a or to R0–R10 themselves.
- Widening the 260–440 band, or opening a third write path that bypasses `normalize` /
  `set_sidebar_width`'s clamp.
- Adding a GUI/widget crate, or making `term-core` depend on wgpu/winit.
- Cross-platform `#[cfg]` branches, shell selection, or SSH/remote transport.
- Changing what `config.toml` writing does in a way that could lose a user's hand-written
  file (this feature is the first thing in the workspace that writes it at all).
- Marking any of the 9 uncovered sub-requirements as verified.

## Exit criteria
Every gate passes AND the Evidence Report is emitted, with the 9 uncovered
sub-requirements reported as **open**, never as passed.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — Coverage classification must be arithmetic-checked, not asserted
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `test "$(awk '/^- \*\*[0-9]+ direct/,/^- \*\*9 uncovered/' specs/tab-resize/loop.md | grep -oE '\*\*[0-9]+ ' | tr -d '* ' | paste -sd+ - | bc)" = "52"`
- note: the coverage table twice stated a total that did not sum to 52 — first 33/3/7/9 (over-claimed by five, caught by sampling), then 28/4/12/9 = 53 (a duplicate grep counted as a requirement). Both times the headline was wrong while the partition was fine. Sum the buckets before quoting a number.

### L2 — A "byte-identical file" claim must be attacked with hostile inputs, not sampled
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "Config-file fidelity: attack `edit_sidebar_toml` with inline tables, dotted keys, `[[array-of-tables]]`, quoted headers (`[\"sidebar\"]`), CRLF and mixed line endings, no trailing newline, trailing comments, and `#` inside quoted values. Every refused shape must leave the file byte-identical; every accepted shape must still parse."
- note: three separate corruption doors (`sidebar = {..}`, `[\"sidebar\"]`, `[[sidebar]]`) each appended a duplicate `[sidebar]` and produced an unparseable config — which silently resets the user to full defaults at next launch. Each was found only by hostile input, never by the happy-path tests.

### L3 — Suppressing an event by a *subset* of its payload eventually suppresses everything
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "Echo suppression: confirm the predicate compares the WHOLE delivered payload against the last applied one, not a subset. Construct the steady state — after N writes, does any legitimate change still get through? — and confirm a stale in-flight echo cannot be misread as a foreign edit."
- note: D15 compared only `(width_px, collapsed)`. The latch is armed on every persist and the file then matches it indefinitely, so after ONE click of Hide every later reload of every unrelated field was dropped — live settings died for the session. The predicate was implemented exactly as specified; the specification was the bug.

### L4 — A guard that only fires past a clamp boundary is inert
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "For any guard conditioned on a threshold, check whether a clamp elsewhere prevents the guarded value from ever reaching it. Trace an actual continuous gesture, not a single jump."
- note: `if !next { set_sidebar_width(clamp(raw)) }` was written to stop a drag-collapse flooring the remembered width at 260 — but `clamp` floors everything under 260 while the collapse only fires under 220, so a continuous drag writes 260 throughout [220,260) before collapsing. The fix was inert for every real gesture and its comment asserted the opposite.

### L5 — `git diff main` is meaningless on this repo's feature branches
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: `main` is routinely AHEAD of a feature branch here (it carried the hyperlink feature while this branch was based on an older commit), so `git diff main` shows unrelated shipped work as *deleted*. Use `git diff HEAD` for uncommitted feature work. A Gate-1 dependency check was written against `main` and was silently measuring nothing.
