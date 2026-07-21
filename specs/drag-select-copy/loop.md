# loop.md — Drag-to-select text + Cmd+C copy

## Goal
Let the user drag the mouse over the terminal grid to select text (with a visible
highlight) and press Cmd+C to copy the selection to the macOS clipboard. Today
there is no selection at all: a left-press only routes to the sidebar
(`terminal_app.rs` MouseInput arm), there is no drag/release tracking, no
selection model, no highlight layer, and no Cmd+C — so dragging over the grid does
nothing. Paste (Cmd+V) already works; this closes the copy half.

Scope: stream (reading-order) selection over the **visible viewport**, plus Cmd+C
copy. Out of scope (stated, not silently dropped): rectangular/block selection,
double/triple-click word/line select, selection that survives scrollback/new
output, and drag-and-drop of text.

## Design (mirrors existing patterns; respects dependency direction)
- **term-core** (`selection.rs`, new): `Selection { anchor:(u16,u16), head:(u16,u16) }`
  in (col,row); `ordered()` (row-major min/max); `row_span(row, cols) -> Option<(u16,u16)>`.
  `GridSnapshot::selection_text(&Selection) -> String` (skips `is_continuation`
  cells, `trim_end` per row, join `\n`). GPU/window-free. Exported from lib.
- **renderer** (`selection_layer.rs`, new): `SelectionLayer` + `SelectionHandle`
  cloned from `cursor_layer.rs` — alpha-blended quad per selected row-span,
  `LoadOp::Load`, re-recorded each frame from a shared handle. `SELECTION_Z = 4`
  (below `CURSOR_Z = 5`, above glyphs). Reads `term_core::Selection`.
- **app**: pure `cell_at(cursor, cell, left_inset, top_inset, cols, rows)` px→cell
  (clamped) in `input.rs`; `write_clipboard_text(&str)` in `clipboard.rs`
  (`clearContents` + `setString_forType`, objc2 stays in app); drag state machine in
  `terminal_app.rs` (press over grid → begin; CursorMoved while selecting → extend +
  bare `request_redraw`; Left release → finalize; plain click → clear); Cmd+C in
  `on_key` copies `selection_text` and consumes; selection cleared on tab switch/resize.

## Gate 1 — Pass/Fail (binary, 100%)
- `cargo build` succeeds
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace` green

## Gate 2 — Quantitative (numbers vs thresholds)
- New unit tests ≥ **7**, all passing: term-core `ordered`/`row_span`/`selection_text`
  (≥4), renderer selection-quad geometry (≥1), app `cell_at` (≥2).
- Crates touched: **3** (term-core, renderer, app) — inherent to a
  model+render+input feature; term-core stays free of wgpu/winit.

## Gate 3 — Qualitative (judged, with grounds + action)
- **dependency-direction** — `Selection` + text extraction live in term-core with no
  GPU/window deps; renderer consumes `term_core::Selection`; objc2 clipboard write
  stays in app. (`.claude/rules/dependency-direction.md`.)
- **interaction-correctness** — drag builds a selection; a plain click (no movement)
  clears it; release finalizes; Cmd+C copies the selected text (wide-char
  continuations skipped, trailing spaces trimmed) and consumes the key so no literal
  `c` reaches the shell; selection is cleared on tab switch so no highlight bleeds
  across tabs.
- **render-fit** — `SelectionLayer` follows the `cursor_layer` template (shared
  handle, re-recorded per frame, alpha blend, z below cursor); selection-only changes
  use bare `window.request_redraw()` (no needless grid re-snapshot), matching the
  sidebar-scroll precedent.

## Autonomy note
No STOP-list items (no schema/auth/payment/migration). Clipboard write is the
intended user action. Full visual/drag confirmation needs a live run (mouse + GPU +
real pasteboard can't be unit-tested); gates cover the pure logic + build/clippy.
