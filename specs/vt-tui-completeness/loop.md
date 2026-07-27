# loop.md — Loop Contract: vt-tui-completeness

> Supervisor contract for the "vt-tui-completeness" feature. Invoked as
> agent-orchestrate's **Phase 3.5 verification gate** over an implementation that
> is already complete — so iteration 1's WORK phase is a no-op and VERIFY runs
> first. Seeded from `specs/vt-tui-completeness/spec.md`.

- spec: specs/vt-tui-completeness/spec.md
- mode: interactive (no `pre-approved-unattended`)

## Goal
Verify that the shipped VT parser extension satisfies all 11 requirements /
63 sub-requirements in `specs/vt-tui-completeness/spec.md` — DECSTBM scroll region,
region-aware scrolling (SU/SD/IL/DL/IND/RI), in-line char ops (ICH/DCH/ECH),
parameterized EL, and DECAWM pending-wrap — **without** regressing the shipped
scrollback, wide-char, or prompt-redraw behavior, and without violating the spec's
8 constraints.

## Scope & non-goals
- In: `crates/term-core/` only — `src/grid.rs`, `src/parser.rs`, `src/lib.rs`,
  `tests/scrollback.rs`, `tests/vt_tui.rs`. Fixes to close a gate finding, plus
  added tests for behavior already in scope.
- Out: `renderer`/`app`/`common` crates; `GridSnapshot`'s shape (spec C-2); any new
  dependency (C-3); `Cargo.lock` (hook-blocked); the spec's declared non-goals
  (DECTCEM `?25`, DECOM — but NOT DECSC/DECRC, brought in scope by D17); the
  spec's Post-work items (`/run-term`
  eyeball, `docs/*.md` updates) — those are the human's, not this gate's.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| format | `cargo fmt --check` |
| build | `cargo build --workspace` |
| typecheck | N/A — Rust; type-checking is performed by `cargo build` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| no-unwrap (spec C-4, R8.2) | `! awk '/#\[cfg\(test\)\]/{exit} /unwrap\(\)\|expect\(/{found=1} END{exit !found}' crates/term-core/src/grid.rs` — same for `parser.rs`, `lib.rs` |
| dependency direction (C-1, R9.3) | `! grep -qE 'wgpu\|winit\|raw-window' crates/term-core/Cargo.toml` |
| no dependency drift (C-3) | `git diff --quiet -- crates/term-core/Cargo.toml Cargo.lock` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| workspace test failures | `= 0` | `cargo test --workspace` |
| clippy warnings | `= 0` | clippy exits non-zero under `-D warnings` |
| sub-requirements with a mapped, green test | `= 63 / 63` | trace each R0.1-R9.3 to a named test; any unmapped one is a gate failure |
| pre-existing term-core tests still green | `= 100%` (spec C-5) | `scrollback.rs`, `grid_snapshot.rs`, `selection.rs`, `cursor_key_mode.rs`, `pty_smoke.rs` + inline `grid.rs`/`image.rs` modules |
| insta snapshots moved | `= 0` | `git status --porcelain crates/term-core/tests/snapshots` empty; no `.snap.new` files |
| vacuous (non-load-bearing) tests found | `= 0` | mutation spot-check: neuter each new production behavior, confirm ≥1 mapped test fails |
| test coverage % | N/A — no coverage tooling configured in this repo | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Spec alignment** — do the 17 decisions D1-D17 hold in the code as written?
  Explicitly adjudicate the **two known deviations** (below); a deviation that is
  right must be justified against the spec's own reasoning, and one that is wrong
  is a gate failure, not a footnote.
- **Architecture fit** — new state follows the `alt_screen`/`app_cursor_keys`
  precedent (mode lives on `Grid`, absent from `GridSnapshot`); the history-push
  block lives in exactly one place (D10).
- **Regression safety** — does the R3.4 full-screen-equivalence guard genuinely
  protect the shipped scrollback feature, or does it only appear to?
- **Test integrity** — are any of the 63 sub-requirements only *vacuously*
  satisfied (assertion that cannot fail, or that passes without the feature)?
- **Surface containment** — `term-core` only; GPU-free; no new deps.

### Deviations this gate must adjudicate (raised by the maker, not discovered later)
1. **IND fill color.** Spec D6 lists IND among the BCE (pen-background) ops. The
   implementation routes `ESC D` through `line_feed`, so IND and LF both fill with
   the **default** background. Maker's rationale: IND and LF are the same
   operation, and R3.4 pins LF to the default fill. → Right or wrong?
2. **IL/DL history push.** Spec D1 lists IL/DL among ops that push to history when
   the region is full-screen. The implementation makes IL/DL **never** push (they
   scroll `[cursor_row..=region_bottom]` via `scroll_rows_*`, bypassing the history
   block); SU and LF/IND do push when full-screen. Maker's rationale: line
   insert/delete is an in-screen edit, not terminal scrolling. → Right or wrong?

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/format/type errors; adding or
strengthening tests for the 63 sub-requirements already in scope; doc/comment
corrections; local renames; behavior-preserving refactors that keep Gate 1 green;
fixing a genuine bug the gate surfaces inside `crates/term-core/`.

**🛑 STOP — call a human (escalate):**
- Any change to `GridSnapshot`'s shape (spec C-2) — including "just one field" for
  cursor visibility.
- Any new dependency, or any edit to `Cargo.toml` / `Cargo.lock` (C-3, hook-blocked).
- Any change outside `crates/term-core/`.
- Implementing a still-declared non-goal (DECTCEM `?25`, DECOM). DECSC/DECRC are
  no longer a non-goal — D17 brought them in scope after evidence showed the
  deferral was unsafe once `CSI r` landed.
- **Resolving either adjudicated deviation by changing the spec** rather than the
  code, or vice versa — the deviation verdict is the human's call, since D1/D6 were
  human-approved decisions.
- Weakening, deleting, or `#[ignore]`-ing an existing test to make a gate pass.
- Anything conflicting with this contract's Goal or the spec.

## Exit criteria
Exit only when **every** gate passes and the Evidence Report is emitted. Hitting a
🛑 boundary ends the loop in escalation, not completion.

---

## Lessons

### L1 — Derive counts, never state them
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `test "$(grep -c '^#### R[0-9]*\.[0-9]*:' specs/vt-tui-completeness/spec.md)" = "$(grep -oE '= ([0-9]+) / \1' specs/vt-tui-completeness/loop.md | head -1 | grep -oE '[0-9]+' | head -1)"`
- note: the sub-requirement count was hand-stated as 55 when the spec had 57, and that wrong number became a Gate-2 threshold — a gate that could never have detected its own staleness. Counts that gate anything must be computed from the source of truth.
