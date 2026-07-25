# loop.md — Loop Contract: selection-model

- feature-id: selection-model
- spec: specs/selection-model/spec.md
- pre-approved-unattended: yes   # approved at build-order batch gate 2026-07-24

## Goal
Implement the term-core selection core per `specs/selection-model/spec.md`: a `Selection`
value type + `Grid`/`GridSnapshot` selection state, `GridSnapshot::selection_text`
extraction (trim/soft-wrap-join/continuation-skip), and bracketed-paste mode 2004 behind
`Terminal::bracketed_paste()` — without violating the spec's Decisions (SD1–SD5, D6–D9).

## Scope & non-goals
- In: `crates/term-core/src/{selection.rs, grid.rs, snapshot.rs, parser.rs, lib.rs}` (the declared surface).
- Out: any renderer/app code; any GPU/winit dependency in term-core; mouse/clipboard/highlight (sibling features).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — `cargo build` is the typecheck in Rust (no separate step) |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| new headless tests for R1–R4 | `≥ 1 per requirement, all green` | `cargo test -p term-core` |
| test coverage % | N/A — no coverage tooling configured in this repo | — |
| fps / input-latency | N/A as a gate — 120fps / ≤8ms are project GOALS not gates (docs/domain G1) | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — term-core stays GPU-free/window-free (no wgpu/winit); `GridSnapshot` keeps deriving `PartialEq` so the selection field participates in the idle-CPU cache (spec Constraints); bracketed-mode follows the DECCKM/`app_cursor_keys` convention (SD5).
- **Naming / readability** — `Selection`/accessors read like the surrounding grid API.
- **Spec alignment** — satisfies the accept seed + R1–R4 GWTs; the D8 soft-wrap heuristic + its Known-Gap limits are honored (no silent divergence).
- **Extraction correctness** — `selection_text` skips continuation cells + `trim_end`s per row like `to_text`; inverted/edge coords never panic (R3.6).

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope unit/snapshot tests,
docs/comments, local renames, behaviour-preserving refactors — all WITHIN the declared surface.

**🛑 STOP — call a human (escalate):**
- Any change that makes `term-core` depend on `wgpu` or `winit` (dependency-direction rule).
- Adding any GUI/widget crate (no-gui-crates rule).
- Editing `Cargo.lock` (a PreToolUse hook blocks it — let cargo manage it).
- Adding a `#[cfg(target_os = ...)]` branch (macos-only rule).
- Editing any file OUTSIDE the declared surface (cross-feature contamination).
- Anything conflicting with the spec's Decisions/SDs or this Goal.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- A fresh contract has none. -->

### L1 — selection_text capacity math underflowed on out-of-range rows
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: Extraction capacity/index math must use saturating arithmetic — out-of-range selection rows can collapse onto one grid row where `first > last`. A `String::with_capacity((last - first + 1))` panicked under debug overflow-checks; fixed with `saturating_sub` and guarded by the regression test `out_of_range_rows_extract_without_panicking`.
