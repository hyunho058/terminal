# loop.md — titlebar-tab-controls

## Goal
Implement `specs/titlebar-tab-controls/spec.md` (feature ② of sidebar-titlebar): a
full-width **top title bar** (reserved via the existing `set_top_inset`) hosting
**new-tab (+)**, **close (×)**, and the **collapse/expand toggle** — the toggle
relocated out of the sidebar (D11/D14). Shift EVERY top-relative y-source by
`top_inset` from ONE value, including the sibling `resize_handle_rect` (D6). Remove
the relocated controls (NewTab/HideSidebar/Restore) from the sidebar. Consume every
title-bar-band press before sidebar/resize/selection (D13). Tasks T1→T4.

## Gate 1 — Pass/Fail (binary, 100%)
- `cargo build` succeeds
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace` green

## Gate 2 — Quantitative (numbers vs thresholds)
- New unit tests ≥ **4**, all passing: `titlebar_hit` for `+` / `×` / toggle / empty
  band; and the `top_inset` alignment test (with `top_inset = k`, a card's drawn-row y
  == its hit-test-row y, and `resize_handle_rect` starts at `k`).
- Files match the declared surface: NEW `crates/renderer/src/titlebar.rs` +
  `crates/app/src/titlebar.rs`; edits to `sidebar_layout.rs`, `sidebar_header.rs`,
  `renderer/lib.rs`, `input.rs`, `terminal_app.rs`. **Zero** edits under
  `crates/term-core/**` (SD2).

## Gate 3 — Qualitative (judged, with grounds + action)
- **top-inset-single-source** (D6) — every top-relative y shifts by `top_inset` from the
  single `SidebarLayoutInput.top_inset`: drawn rows (`list_top`/`set_list_scroll`) AND
  hit-test rows AND `header_rect`/`restore` AND the sibling `resize_handle_rect`. No
  drawn-vs-hit-test desync; the resize band starts at `top_inset`.
- **band-ownership** (D13) — every `MouseInput` Pressed with `cursor.y < top_inset` is
  consumed (button → its action; empty → swallow) BEFORE the sidebar hit-test, the
  resize handle, and `over_grid → begin_selection`. No fall-through.
- **toggle-reflects-state** (D14) — the title-bar toggle flips collapse via
  `toggle_sidebar(!collapsed())` (reusing `set_sidebar_collapsed`, no new collapse
  logic); its glyph reflects expanded vs collapsed; the separate Restore affordance is
  retired.
- **render-fit** (D8/SD1/SD2) — the titlebar `DrawLayer` is pinned topmost (occludes any
  bleed into the band); `term-core` untouched; no GUI crates; hit-test geometry is a pure
  `titlebar` module shared by draw + hit-test (SD3).

## Autonomy note
No STOP-list items (no schema/auth/payment/migration). Mouse + GPU can't be
unit-tested; gates cover the pure geometry (titlebar_hit, top_inset alignment) +
build/clippy/tests. Final visual/interaction confirmation is a live run (`/run-term`).
