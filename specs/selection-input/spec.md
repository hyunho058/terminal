# Spec: selection-input

## Meta
- **Created**: 2026-07-23
- **Type**: dev
- **Status**: planned (batch-approved; not yet executed)
- **Set**: clipboard-copy-paste (see `specs/clipboard-copy-paste/partition-manifest.md`)
- **Generated**: via decompose → specify `mode: batch`
- **Approved by**: decompose partition gate (batch — human owns the bar at the ONE partition gate)
- **Approved at**: 2026-07-23

## Goal
Turn mouse drags into a live grid selection and expose the seam data the clipboard
handlers consume: a pixel→cell mapping, a left-drag lifecycle that sets/clears the
active grid's `Selection` (redrawn live), and two new `AppCtx` fields —
`bracketed_paste` and `selection_text` — that `text-paste` and `copy-clipboard` read.
This is the one sanctioned `terminal_app.rs` event-loop exception (SD4).

## Non-goals
- No selection *model* or extraction logic (owned by `selection-model`); no highlight
  rendering (owned by `selection-highlight`); no clipboard read/write (owned by
  `text-paste`/`copy-clipboard`).
- No Cmd+C / Cmd+V handling here (those are `InputHandler`s in sibling features).
- No copy-on-select, no word/line double/triple-click, no drag auto-scroll past the
  viewport edges (baseline: drag within the visible grid; see Known Gaps).

## Confirmed Goal
Edit only `crates/app/src/{terminal_app.rs, seam.rs, input.rs}` to add: a pure
`pixel_to_cell` helper (input.rs); a left-press/drag/release selection lifecycle on the
event loop that calls `Terminal::set_selection`/`clear_selection` and redraws live
(terminal_app.rs); and two consumer-facing `AppCtx` fields populated per keypress
(seam.rs + terminal_app.rs). Existing sidebar press/hover/wheel and IME are unaffected.

## Declared Surface
- feature-id: selection-input
- depends_on: [selection-model]
- declared-surface:
  - crates/app/src/terminal_app.rs
  - crates/app/src/seam.rs
  - crates/app/src/input.rs

## Research
- The event loop today: `WindowEvent::MouseInput { Pressed, Left }` routes to `sidebar_press(last_cursor, …)` and acts only on a `Some(action)`; there is **no Released handler and no drag path** — `CursorMoved` only updates `last_cursor` + `refresh_hover` (`crates/app/src/terminal_app.rs:910-956`). The `None` (press not over a sidebar region → over the grid) branch is currently a no-op — the natural insertion point for grid selection.
- Sidebar hit-testing is the ONE geometry authority: `sidebar_press` → `sidebar_layout::hit_test`, which returns `None` over the terminal (the wheel path is likewise "gated by hit_test, not `x < sidebar_w`") (`crates/app/src/input.rs:74-92, 111+`). Reusing that ordering keeps sidebar clicks unaffected (SD4).
- The cursor→pixel mapping to invert: the cursor rect is `x = renderer.left_inset() + col*cw`, `y = renderer.top_inset() + row*ch`, with `(cw,ch) = renderer.cell_size()` (`terminal_app.rs:451-460`). `pixel_to_cell` inverts this: `col = (px-left_inset)/cw`, `row = (py-top_inset)/ch`.
- Renderer geometry is reachable from the app: `renderer.cell_size()`, `renderer.left_inset()`, `renderer.top_inset()` (`crates/renderer/src/lib.rs:493, 530, 593`).
- Key dispatch: `on_key` builds `AppCtx::new(&mut self.tabs.active_mut().writer, count, active, cwd)` (twice — overlays then router) and falls back to `encode_key(…, app_cursor)`; `app_cursor = self.tabs.active().terminal.app_cursor_keys()` (`terminal_app.rs:757-793`). Bracketed mode / selection text are read the same way, from the active tab, and must be computed into locals **before** the `active_mut().writer` borrow.
- `AppCtx` is deliberately narrow (writer, tab_count, active, active_cwd) with a `request_redraw`/`redraw_requested` pair (`crates/app/src/seam.rs:44-83`). All 7 `AppCtx::new` callers are in `seam.rs` (5 tests) + `terminal_app.rs` (2); `clipboard.rs` only *receives* `&mut AppCtx`, never constructs it — so extending `AppCtx` stays inside this feature's surface. Defaulted builder methods keep the 5 test callsites unchanged.
- `Terminal` is reached via `self.tabs.active()/active_mut().terminal`; it gains `set_selection`/`clear_selection`/`selection`/`bracketed_paste` + `snapshot()` from `selection-model`. `snapshot().selection_text()` yields the copy text.
- `last_cursor` is physical-px `(f64,f64)` updated on `CursorMoved` (`terminal_app.rs:144-146, 910-913`). winit `CursorMoved` carries no button state → a `dragging` flag tracked on `MouseInput` Pressed/Released is needed to know the button is held.
- Constraints: no-gui-crates, macos-only, `dependency-direction` (app may use renderer + term-core), no `unwrap()` outside tests, `cargo clippy … -D warnings`, `rustfmt` on save (PostToolUse hook).

## Decisions

> SD1–SD5 inherited **verbatim** from `specs/clipboard-copy-paste/partition-manifest.md`.
> D6+ are this feature's local decisions.

### SD1: Selection state rides on the versioned GridSnapshot contract; term-core stays GPU-free and window-free
- **Status**: resolved
- **Rationale**: The dependency-direction rule forbids term-core from touching wgpu/winit and forbids the renderer from seeing a live `Grid`. So the selection is a value type (anchor/focus cell coords) owned in `term-core`; the `Grid` holds the live selection; `GridSnapshot` is EXTENDED (its versioned contract) to carry the selection for the renderer; and cell→String extraction (trailing-whitespace trim, soft-wrapped rows joined) lives in `term-core` beside the existing `GridSnapshot::to_text` (which already skips double-width continuation cells). The renderer only ever reads `GridSnapshot`, never a live `Grid`. Rejected: holding anchor/focus in the app and passing raw coords to the renderer (would leak selection semantics + text extraction out of the headless core, defeating headless testability).

### SD2: The highlight is hand-rolled per-cell drawing on the existing wgpu cell-bg path — no GUI/widget crate
- **Status**: resolved
- **Rationale**: The no-gui-crates rule is the project's reason to exist. The highlight is a `renderer` `DrawLayer` that reads `GridSnapshot`'s selection field and paints the selected cell range by reusing the existing per-cell background quad path (`cells.wgsl`/the bg pipeline), registered through the existing `DrawLayer`/`Overlay` seam (the same seam `ImageLayer`/`ImagePresenter` use). Rejected: any egui/iced/overlay-widget approach, and any bespoke selection shader when the cell-bg path already draws per-cell quads.

### SD3: Clipboard access is objc2 NSPasteboard, consistent with clipboard.rs
- **Status**: resolved
- **Rationale**: The macos-only rule + the existing `clipboard.rs` already use `objc2_app_kit::NSPasteboard` on the UI thread. READ text via `stringForType(NSPasteboardTypeString)`; WRITE via `clearContents` + `setString` (or `writeObjects`). No cross-platform clipboard abstraction, no new clipboard crate. Runs on the UI thread like the existing image read. Rejected: the `arboard`/`copypasta` crates (unnecessary dependency; inconsistent with the established objc2 usage).

### SD4: Input goes through the SD4 InputHandler/Overlay seam; the ONLY event-loop edit is mouse press/drag/release + pixel→cell
- **Status**: resolved
- **Rationale**: The SD4 seam (`InputRouter`/`OverlayHost`, `AppCtx`) exists so features extend input/overlays from their own modules without editing `terminal_app.rs`. Cmd+V (paste) and Cmd+C (copy) are `InputHandler`s dispatched through the router; the highlight is an `Overlay`-provided `DrawLayer`. The necessary exception is the pointer pipeline: `WindowEvent::MouseInput` (Left Pressed/Released) + drag-while-`CursorMoved` + pixel→cell mapping MUST touch the `terminal_app.rs` event loop. Existing sidebar press/hover/wheel and image-paste stay unaffected. Rejected: routing Cmd+C/Cmd+V through new bespoke event-loop branches instead of the seam.

### SD5: Bracketed-paste mode (DECSET/DECRST 2004) follows the existing DECCKM/app_cursor_keys convention
- **Status**: resolved
- **Rationale**: term-core already tracks a private mode this exact way: a `bool` flag on `Grid` (`grid.rs`), a `Terminal` accessor (`lib.rs`), set from `parser.rs` `csi_dispatch` on `?<n> h|l`. Bracketed-paste mode uses the same path: a `bracketed_paste` flag on `Grid`, a `Terminal::bracketed_paste()` accessor, set on `?2004 h|l`. The app reads it via that accessor and surfaces it to input handlers. Rejected: tracking mode 2004 in the app layer or on the `Processor` instead of `Grid`.

### D6: `pixel_to_cell` is a pure helper in input.rs that inverts the cursor mapping and clamps to the grid
- **Status**: resolved
- **Rationale**: `pixel_to_cell(cursor: (f64,f64), cell: (f32,f32), left_inset: f32, top_inset: f32, cols: u16, rows: u16) -> Option<(u16,u16)>` computes `col=((px-left_inset)/cw).floor()`, `row=((py-top_inset)/ch).floor()`, returns `None` when the point is left of `left_inset` or above `top_inset` (i.e. over the sidebar band / top inset), and clamps `col`/`row` into `0..cols`/`0..rows` so a drag past the right/bottom edge selects the edge cell. It is `winit`-free and unit-tested. Rejected: doing the math inline in the event loop (untestable; the project keeps geometry in pure `input.rs` helpers like `sidebar_press`).

### D7: Grid-press routing reuses the existing sidebar-first order; only a non-sidebar press *over a cell* starts a selection
- **Status**: resolved
- **Rationale**: On `MouseInput { Pressed, Left }`, `sidebar_press` still runs first and its `Some(action)` is handled exactly as today; only its **`None`** result (the press missed every sidebar region) is a selection candidate. But `sidebar_press → None` and `pixel_to_cell → None` do **not** tile the window — there is a dead band where both are `None`: the top-inset strip above row 0, and (sidebar collapsed) the left band that is neither a cell nor the restore button. So the anchor is taken **only if `pixel_to_cell(last_cursor, …)` is `Some((col,row))`**; a press in the dead band arms nothing (no selection, no clear). This reuses `hit_test` + `pixel_to_cell` as the geometry authorities (the same gating the wheel path uses), so existing sidebar card/new-tab/collapse clicks are untouched (SD4). Rejected: an `x >= left_inset` check parallel to `hit_test` (two authorities that can disagree); arming on any non-sidebar press regardless of `pixel_to_cell` (would set a bogus anchor in the dead band).

### D8: Left-drag lifecycle — `moved` is keyed on the focus cell leaving the anchor cell, not on raw motion
- **Status**: resolved
- **Rationale**: A non-sidebar Left **press over a cell** records the `anchor` cell (D7), arms `dragging`, and sets `moved=false`. Each `CursorMoved` while `dragging` maps the (clamped) focus cell; **`moved` is set true only when the focus cell differs from the anchor cell** — NOT on the raw event. This is the crux: macOS emits several jittery `CursorMoved` events during an ordinary click/tap, all landing (after `pixel_to_cell`'s floor) in the *same* anchor cell; keying `moved` on the raw event would make every click leave a degenerate `anchor==anchor` selection and R3.3 would never fire. Only once the focus cell ≠ anchor does the drag call `Terminal::set_selection(anchor, focus)` + `request_redraw` (the changed `snapshot.selection` makes snapshots unequal → a real redraw, per `selection-model`), tracking the drag live. **Release** disarms `dragging`; if `!moved` (the focus never left the anchor cell — a plain click) it calls `Terminal::clear_selection()`, satisfying "a plain click clears the selection". Drag state (`dragging`, `anchor`, `moved`) lives on the app inner struct in `terminal_app.rs`. Rejected: `moved` on every `CursorMoved` (the jitter bug above); a raw pixel-distance threshold (cell-granularity is what the selection is expressed in, so cell-change is the honest trigger); copy-on-select (Cmd+C is `copy-clipboard`'s; this feature only makes the selection).

### D9: `AppCtx` gains `bracketed_paste: bool` and `selection_text: Option<String>`, computed once per keypress and gated on an existing selection
- **Status**: resolved
- **Rationale**: `seam.rs` adds two fields to `AppCtx` (default `false`/`None` via builder methods so the 5 existing test callsites are untouched), exposed for reading by handlers. Because `AppCtx` holds `writer: &'a mut …` live for the whole dispatch, both values must be read into **owned locals before** the `active_mut().writer` borrow (mirrors the existing `app_cursor` read at `terminal_app.rs:791-793`). **Cost control:** `selection_text` would otherwise clone the entire visible grid on *every* keystroke (`snapshot()` is a full grid clone) just to feed data only `copy-clipboard`'s Cmd+C reads — unacceptable against the `input→screen ≤ 8ms` budget. So it is **gated on a cheap check first**: `selection_text = active().terminal.selection().is_some().then(|| active().terminal.snapshot().selection_text()).flatten()` — the grid clone happens only while a selection actually exists (rare; during/after a drag), and the common no-selection keystroke pays only the `Option` check. `on_key` builds `AppCtx` **twice** (overlays block :763, router block :776); the two locals are computed **once** above both blocks and reused (the `Option<String>` is `clone`d for the second `AppCtx` only when `Some`). `text-paste` reads `bracketed_paste`; `copy-clipboard` reads `selection_text`. Rejected: eager per-keystroke `snapshot().selection_text()` (the hot-path clone above); a lazy closure field in `AppCtx` (borrow/lifetime tangle against the live `writer`); exposing the live `Grid`/`Terminal` through `AppCtx` (violates its deliberately narrow contract — `seam.rs:44`).

### D10: The pointer pipeline is the sole event-loop exception; everything else is untouched
- **Status**: resolved
- **Rationale**: Only `CursorMoved` (add a drag branch after the existing `last_cursor`/`refresh_hover`), `MouseInput` (add a Released arm + the grid-press branch in the existing Pressed arm's `None` path), and the `on_key` `AppCtx` population change. Sidebar hover/press/wheel, IME (`Ime`), `Resized`, `Occluded`, `Focused`, and the redraw loop are byte-unchanged. Rejected: refactoring the mouse match into a new module (would still edit `terminal_app.rs`; keeping the change minimal and in place is lower-risk).

## Constraints
- Only `terminal_app.rs`, `seam.rs`, `input.rs` may change (declared surface). `AppCtx::new`'s existing signature/callers stay compatible (builder methods for the new fields).
- `renderer` must not gain a `winit` dep via this work; `pixel_to_cell` is `winit`-free.
- Existing behavior preserved: sidebar Card/WaitingRow/NewTab/Collapse/Expand clicks, hover highlight, wheel scroll, IME composition routing, resize refit — none regress (D10).
- No `unwrap()`/`expect()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` clean; no `#[cfg(target_os)]`.
- Selection coords are viewport cells (`selection-model` D6/D9); a drag while scrolled into history is out of scope (selection not shown there).

## Known Gaps
- While a selection is active, each keystroke pays one `snapshot()` grid clone to populate `AppCtx.selection_text` (D9). This is bounded (only during/after a drag, not the common no-selection keystroke) and accepted; a lazier on-demand path is a possible future refinement.
- No drag auto-scroll: dragging past the top/bottom viewport edge clamps to the edge cell rather than scrolling scrollback (deferred).
- Stale-selection clearing on new shell output (not just resize) is not wired here; `selection-model` clears on resize, and this feature clears on a plain click — clearing on fresh output is a possible follow-up.
- The `terminal_app.rs` mouse lifecycle is validated via the pure `pixel_to_cell`/`AppCtx` unit tests + manual `/run-term`; the winit event loop itself is not unit-tested in this codebase.

## Requirements

### R0: Mouse-drag selection + seam plumbing land without regressing existing input
#### R0.1: Existing input paths are preserved
- **Given**: the app after this feature
- **When**: a sidebar card, new-tab, or collapse control is clicked; the wheel is scrolled; or an IME composition is typed
- **Then**: each behaves exactly as before (no selection side effects)
#### R0.2: The pure helpers are headlessly testable
- **Given**: `pixel_to_cell` (input.rs) and the extended `AppCtx` (seam.rs)
- **When**: `cargo test -p app` runs
- **Then**: unit tests assert `pixel_to_cell` and the new `AppCtx` fields without a window/GPU

### R1: `pixel_to_cell` maps a physical-px point to a viewport cell
#### R1.1: A point inside the grid maps to its cell
- **Given**: `cell=(8,16)`, `left_inset=40`, `top_inset=24`, `cols=100`, `rows=40`, cursor `(64.0, 40.0)`
- **When**: `pixel_to_cell` is called
- **Then**: it returns `Some((3, 1))` (`(64-40)/8=3`, `(40-24)/16=1`)
#### R1.2: A point over the sidebar/top band returns None
- **Given**: `left_inset=40`, `top_inset=24`, cursor `(20.0, 10.0)` (left of the grid and above it)
- **When**: `pixel_to_cell` is called
- **Then**: it returns `None`
#### R1.3: A point past the right/bottom edge clamps to the last cell
- **Given**: `cols=10`, `rows=5`, `cell=(8,16)`, insets `(40,24)`, cursor far to the bottom-right
- **When**: `pixel_to_cell` is called
- **Then**: it returns `Some((9, 4))` (clamped into range, never out of bounds)

### R2: A non-sidebar left press starts a selection; sidebar presses are unaffected
#### R2.1: A press over a sidebar region routes to the sidebar action
- **Given**: the pointer over a sidebar card
- **When**: Left is pressed
- **Then**: `sidebar_press` returns `Some(Switch(i))` and the tab switches; no selection is started
#### R2.2: A press over a grid cell anchors a selection
- **Given**: the pointer over the terminal grid (sidebar `hit_test` → `None`, `pixel_to_cell` → `Some((col,row))`)
- **When**: Left is pressed
- **Then**: that cell is recorded as the anchor and drag is armed (`moved=false`); no tab action occurs
#### R2.3: A press in the dead band arms nothing
- **Given**: the pointer in the region where both `sidebar_press` and `pixel_to_cell` return `None` (the top-inset strip, or the collapsed-sidebar left band that is not the restore button)
- **When**: Left is pressed
- **Then**: no anchor is set, no drag is armed, and no selection is created or cleared

### R3: Left-drag sets a live selection; release finalizes; a plain click clears
#### R3.1: A drag to a different cell sets the live selection and redraws
- **Given**: a selection armed at anchor `(2,0)`
- **When**: the cursor moves to a **different** cell `(6,1)` with the button held
- **Then**: `moved` becomes true, `Terminal::set_selection((2,0),(6,1))` is called on the active tab, and a redraw is requested (the highlight tracks the drag)
#### R3.2: Sub-cell jitter within the anchor cell does not start a selection
- **Given**: a selection armed at anchor `(2,0)`
- **When**: `CursorMoved` events arrive that still map (after `pixel_to_cell`'s floor) to cell `(2,0)`
- **Then**: `moved` stays false and no `set_selection` is issued (guards the trackpad-jitter click)
#### R3.3: Release ends the drag
- **Given**: a real drag in progress (`moved == true`)
- **When**: Left is released
- **Then**: dragging is disarmed and the current selection persists (available to Cmd+C)
#### R3.4: A plain click clears any selection
- **Given**: an existing selection, and a press over a grid cell whose focus never leaves the anchor cell before release (incl. sub-cell jitter)
- **When**: Left is pressed then released (`moved == false`)
- **Then**: `Terminal::clear_selection()` is called and no selection remains

### R4: `AppCtx` exposes `bracketed_paste` and `selection_text` to input handlers
#### R4.1: `AppCtx` carries the two new fields with safe defaults
- **Given**: `AppCtx::new(writer, count, active, cwd)` (unchanged signature)
- **When**: constructed without the builder methods
- **Then**: `bracketed_paste == false` and `selection_text == None` (existing callsites compile unchanged)
#### R4.2: `on_key` populates the fields from the active tab
- **Given**: the active tab reports bracketed-paste on and a non-empty selection
- **When**: a key is dispatched (`on_key` builds the `AppCtx`)
- **Then**: the `AppCtx` handed to overlays/router has `bracketed_paste == true` and `selection_text == Some(text)`, both computed into locals before the writer borrow
#### R4.3: `selection_text` is not computed when there is no selection
- **Given**: the active tab has no selection (`Terminal::selection()` is `None`)
- **When**: a key is dispatched
- **Then**: `selection_text` is `None` and no `snapshot()` grid-clone is performed for it (the common keystroke pays only the `Option` check)

### R5: The event-loop change is scoped to the pointer pipeline
#### R5.1: Only the mouse + on_key sites change
- **Given**: the diff to `terminal_app.rs`
- **When**: reviewed
- **Then**: only `CursorMoved` (drag branch), `MouseInput` (Released + grid-press branch), and `on_key` (`AppCtx` population) are modified; `Ime`/`Resized`/`Occluded`/`Focused`/wheel/sidebar-press handling are unchanged

## Tasks

### T1: `pixel_to_cell` pure helper + unit tests [infra]
- **Fulfills**: R1, R0.2 (helper portion)
- **Depends on**: (none)
- Add `pub fn pixel_to_cell(...) -> Option<(u16,u16)>` to `input.rs` inverting the cursor mapping, returning `None` over the sidebar/top band and clamping to `0..cols`/`0..rows`. Unit tests for R1.1–R1.3.

### T2: Extend `AppCtx` with `bracketed_paste` + `selection_text` [core]
- **Fulfills**: R4, R0.2 (AppCtx portion)
- **Depends on**: (none)
- In `seam.rs`, add the two fields (default `false`/`None`) + builder methods (`with_bracketed_paste`, `with_selection_text`) + pub read access, keeping `AppCtx::new`'s signature and the 5 test callsites unchanged. Unit test for R4.1 (defaults) and the builders.

### T3: Mouse selection lifecycle + AppCtx population in the event loop [core]
- **Fulfills**: R2, R3, R5, R0.1
- **Depends on**: T1, T2
- In `terminal_app.rs`: add drag-state fields to the inner struct; the grid-press branch in `MouseInput` Pressed's `None` path (anchor + arm); a `MouseInput` Released arm (disarm; clear on no-move); a `CursorMoved` drag branch (`set_selection` + redraw, mark moved) using T1's helper + `renderer` geometry + `Terminal::set_selection`/`clear_selection`; and populate T2's `AppCtx` fields in `on_key` before the writer borrow. Leave all other event arms unchanged.

## External Dependencies

### Pre-work
- (none) — consumes `selection-model`'s `Terminal::set_selection`/`clear_selection`/`bracketed_paste` + `GridSnapshot::selection_text`, already in the workspace.

### Post-work
- (none) — `text-paste` reads `AppCtx.bracketed_paste`; `copy-clipboard` reads `AppCtx.selection_text`.
