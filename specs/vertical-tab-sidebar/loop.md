# loop.md — Loop Contract: vertical-tab-sidebar

> Approved in batch by /build-order (left-tab-sidebar). Gates run by /loop.
- pre-approved-unattended: yes
- spec: specs/vertical-tab-sidebar/spec.md

## Goal
Rewrite the tab-bar `DrawLayer` into a vertical LEFT sidebar (tabs stacked top→bottom in `ctx.left_inset`) and expose pure geometry functions that are the single source of truth for both drawing and click hit-testing; must not violate specs/vertical-tab-sidebar/spec.md or the dependency-direction / no-gui-crates rules.

## Scope & non-goals
- In: `crates/renderer/src/tab_bar.rs` only.
- Out: `lib.rs`/shaders (owned by renderer-left-inset — only READ `ctx.left_inset`); any `crates/app/**` wiring. Do not change the `TabBarModel` shape.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| geometry/layout unit tests pass | all pass | `cargo test -p renderer tab_bar` |
| `width_px` leaves title room | `max_chars ≥ 8` for cw=8 | test: `floor((width_px(8.0)-2*PAD_X)/8.0) ≥ 8` |
| `slot_y`↔`tab_at` round-trip | `Some(i)` for every slot i | round-trip test (R3.4) |
| clippy warnings | `== 0` | `cargo clippy --workspace --all-targets -- -D warnings` |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — vertical layout reads `ctx.left_inset` (not a hardcoded width); renderer stays window/app-agnostic (data in only via `TabModelHandle`).
- **Single source of truth (SD2)** — `width_px`, `tab_height_px`, `slot_y`, `tab_at` are pure/unit-tested; the draw path places slots via `slot_y` and `tab_at` is its exact inverse (no independent pad/gap).
- **Naming / readability** — `BORDER_W` (thickness) never conflated with `BORDER` (color); functions named per SD2.
- **Spec alignment** — D5–D9: vertical strip/highlight/right-border geometry, per-slot centered truncated titles, `TabBarModel` unchanged; overflow gap documented, not silently mishandled.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the `tab_bar.rs` rewrite, its unit tests (rewriting the old horizontal-layout tests), doc comments, in-scope refactors.

**🛑 STOP — call a human (escalate):**
- Any edit needed OUTSIDE `crates/renderer/src/tab_bar.rs`.
- A change requiring `lib.rs`/shader edits (that surface belongs to renderer-left-inset — if `ctx.left_inset` is missing, the dependency is unmet → escalate, don't add it here).
- Needing a GUI crate, or breaking the dependency direction / one-way data flow.
- insta snapshots change unexpectedly → `/insta-review`, do NOT blind-accept.
- Anything conflicting with this Goal or the spec.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)
<!-- A fresh contract has none. -->
