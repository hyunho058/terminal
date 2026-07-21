# coherence-report.md — sidebar-titlebar

- generated_at: 2026-07-21
- verdict: OK
- specs_audited: 2   (declared: 2 | undeclared: 0)
- input_ref: sidebar-resize@specs/sidebar-resize/spec.md + titlebar-tab-controls@specs/titlebar-tab-controls/spec.md

## Summary

| tier | classification | findings | severity |
|------|----------------|----------|----------|
| contradiction | incompatible (same role, conflicting choice) | 0 | **BLOCK** |
| overlap | unordered (no `depends_on` path between them) | 0 | **BLOCK** |
| redundancy | near-duplicate Requirements/goal | 0 | warn |
| undeclared | no `## Declared Surface` / empty `declared-surface:` | 0 | warn |

- pairs sharing a surface (non-empty glob intersection): 1
- **block-tier findings: 0**  → verdict is `BLOCK` iff this is > 0 (R4.1)

## Findings   (R3.3 — one block per conflicting pair / flagged spec)

(none)

## Considered, not flagged   (informative — audit completeness; these do NOT affect the verdict)

- **ordered overlaps** (a `depends_on` path makes the shared write sequential — D2/R2.3, NOT flagged):
  - sidebar-resize ⨯ titlebar-tab-controls  (path: sidebar-resize → titlebar-tab-controls, via `titlebar-tab-controls depends_on [sidebar-resize]`)  shares:
    - `(crates/renderer/src/sidebar_layout.rs, crates/renderer/src/sidebar_layout.rs)`
    - `(crates/renderer/src/lib.rs, crates/renderer/src/lib.rs)`
    - `(crates/app/src/input.rs, crates/app/src/input.rs)`
    - `(crates/app/src/terminal_app.rs, crates/app/src/terminal_app.rs)`

- **complementary shares** (different roles on one path — D7/R3.2, NOT a conflict; judged by the independent contradiction subagent):
  - sidebar-resize (creates the resize grab-band geometry, `D5`) ⨯ titlebar-tab-controls (offsets that same band + all top-relative y by `top_inset`, `D6`) under `crates/renderer/src/sidebar_layout.rs` — different role (create vs. offset), sequentially reconciled (B `depends_on` A).
  - sidebar-resize (registers the resize rule layer, `D8`) ⨯ titlebar-tab-controls (registers the titlebar layer pinned above it, `D8`) under `crates/renderer/src/lib.rs` — explicit z-ordering, not a contested choice.
  - sidebar-resize (owns the resize press/drag at `y ≥ top_inset`, `D6`/`D7`) ⨯ titlebar-tab-controls (consumes band presses at `y < top_inset` first, `D13`) under `crates/app/src/input.rs` + `crates/app/src/terminal_app.rs` — disjoint press-zones by construction.

## Verdict

**OK** — no findings (no block-tier findings, no warn-tier findings). The set is coherent for what is declared: the one pair sharing a surface is an approved ordered overlap (`titlebar-tab-controls depends_on [sidebar-resize]`), and the independent contradiction judge classified every shared-path co-edit as complementary (create-vs-offset, explicit z-order, disjoint press-zones), citing `D5`/`D6`/`D8`/`D13`. The inherited `SD1–SD4` are byte-identical common ground, not conflicts.
