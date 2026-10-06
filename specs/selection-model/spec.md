# Spec: selection-model

## Meta
- **Created**: 2026-07-23
- **Type**: dev
- **Status**: planned (batch-approved; not yet executed)
- **Set**: clipboard-copy-paste (see `specs/clipboard-copy-paste/partition-manifest.md`)
- **Generated**: via decompose → specify `mode: batch`
- **Approved by**: decompose partition gate (batch — human owns the bar at the ONE partition gate)
- **Approved at**: 2026-07-23
- **L2-reviewer**: PASS (after fixing the D8 CJK margin-wrap false-negative + R3.3/R3.4 join ambiguity)

## Goal
Give `term-core` the GPU-free core state the clipboard copy & paste set consumes:
a **selection** concept (anchor/focus cell range) on the `Grid` and `GridSnapshot`,
**cell→String extraction** of the selected range (trailing whitespace trimmed,
soft-wrapped rows joined, double-width continuation cells skipped), and tracking of
**bracketed-paste mode** (DECSET/DECRST 2004) behind a `Terminal` accessor.

## Non-goals
- No mouse/pixel handling, no highlight rendering, no clipboard I/O — those are the
  sibling features `selection-input`, `selection-highlight`, `text-paste`,
  `copy-clipboard`. This feature is the headless model layer only.
- No block/rectangular selection (linear/stream selection only).
- No scrollback-anchored selection (selection lives in the live viewport; a selection
  is not shown while scrolled into history — mirrors the image-placement baseline).
- No word/line double/triple-click semantics (that is a selection-input concern).

## Confirmed Goal
Add, entirely within `term-core` and with no wgpu/winit dependency, (1) a `Selection`
value type + `Grid`/`GridSnapshot` state carrying it, (2) `GridSnapshot::selection_text`
extraction, and (3) DECSET/DECRST 2004 bracketed-paste mode on the `Grid` exposed via
`Terminal::bracketed_paste()`. All behavior is unit/snapshot testable headlessly.

## Declared Surface
- feature-id: selection-model
- depends_on: []
- declared-surface:
  - crates/term-core/src/selection.rs
  - crates/term-core/src/grid.rs
  - crates/term-core/src/snapshot.rs
  - crates/term-core/src/parser.rs
  - crates/term-core/src/lib.rs

## Research
- `GridSnapshot` is the versioned renderer contract; the renderer never sees a live `Grid` (`crates/term-core/src/snapshot.rs:1-12`, `docs/architecture.md` D5/boundaries). It already holds `cols/rows/cursor/view_offset/at_bottom/at_top/cells/images` and derives from `Grid::from_grid` (`snapshot.rs:31-73`).
- `GridSnapshot::to_text` already implements the two hard parts of extraction: it **skips double-width continuation cells** and **`trim_end`s each row** (`snapshot.rs:89-106`) — the extraction method reuses this shape over a selected sub-range.
- `Cell` carries `ch/fg/bg/attrs`; `Cell::CONTINUATION = '\0'` marks the trailing spacer of a double-width char; `Cell::is_continuation()` / `is_blank()` exist (`grid.rs:89-120`).
- Private modes follow a fixed convention: a `bool` on `Grid` + a set-accessor, flipped from `parser.rs` `csi_dispatch` on `CSI ? n h|l`, and re-exposed on `Terminal`. Exemplar: DECCKM — `Grid::app_cursor_keys`/`set_app_cursor_keys` (`grid.rs:158,201,418-422`), set at `parser.rs:344`, re-exported at `lib.rs:119-127`.
- `parser.rs:342-343` **already names `?2004` bracketed paste** ("which zsh sends on every prompt") as a private mode that currently "stays ignored" — this feature adds the arm that stops ignoring it, right beside the DECCKM arm.
- RIS (`ESC c`) resets DECCKM to false today (`parser.rs:287`) — bracketed-paste mode joins that reset for consistency.
- `Terminal` is the `term-core` public wrapper the `app` holds (`self.tabs.active().terminal.app_cursor_keys()`, `crates/app/src/terminal_app.rs:791`); mode flags are exposed as **accessors** (not `GridSnapshot` fields) precisely because the renderer never reads them (`lib.rs:119-127` doc). The **selection is the opposite** — the renderer *does* read it to highlight — so per SD1 it rides on `GridSnapshot`.
- No per-row soft-wrap flag is stored; wrapping is implicit (cursor advances to the next row when printing past the last column). Soft-wrap join must therefore be inferred at extraction time.
- Test exemplar: `crates/term-core/tests/grid_snapshot.rs` (insta snapshots of the vte→grid pipeline, GPU-free) and `crates/term-core/tests/cursor_key_mode.rs` (the DECCKM accessor tests) — the two patterns this feature's tests mirror.
- Build/test/lint: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`; no `unwrap()` outside tests; per-crate `thiserror` errors.

## Decisions

> SD1–SD5 are the set-wide Shared Decisions, inherited **verbatim** from
> `specs/clipboard-copy-paste/partition-manifest.md`. D6+ are this feature's local decisions.

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

### D6: `Selection` is a value type of two viewport cell coords with linear-span membership
- **Status**: resolved
- **Rationale**: A new `selection.rs` defines `Selection { anchor: (u16, u16), focus: (u16, u16) }` in **live-viewport** cell coordinates (`(col, row)` within `cols × rows`), matching how mouse pixel→cell will resolve and how the renderer indexes cells. It provides `normalized() -> (start, end)` ordered by `(row, col)` and `contains(col, row, cols) -> bool` implementing **linear/stream** selection (start row: `start.col..cols`; interior rows: full width; end row: `0..=end.col`) so both the renderer (highlight) and extraction share one membership rule. Rejected: block/rectangular selection (a separate mode, non-goal) and absolute scrollback-global coordinates (deferred with the "viewport-only" baseline, mirroring scrollback's own baseline; see Known Gaps).

### D7: Extraction is `GridSnapshot::selection_text(&self) -> Option<String>`, reusing the `to_text` shape
- **Status**: resolved
- **Rationale**: The snapshot already owns the cells + (per SD1) the selection field, so extraction belongs there and stays headlessly testable; the copy feature calls `snapshot.selection_text()`. It walks the selected linear span, **skips continuation cells** and **`trim_end`s each row segment** exactly like `to_text`, and joins rows: a hard newline between rows normally, but **no separator when the upper row is soft-wrapped** into the next. Returns `None` when there is no selection. Rejected: extraction on `Grid` (the app already builds snapshots; the renderer contract carries the selection anyway) and returning `String` unconditionally (a `None` selection must be distinguishable from an empty one).

### D8: Soft-wrap join is inferred by the "filled-to-last-column" heuristic (no stored wrap flag)
- **Status**: resolved
- **Rationale**: term-core stores no per-row wrap marker (wrapping is implicit). Baseline: when extracting, treat upper row *r* as soft-wrapped into *r+1* iff row *r*'s **last column** (`cols-1`) holds a non-blank, non-continuation cell — the standard terminal-copy heuristic — and in that case join the two row segments with no newline; otherwise emit `\n`. This is contained entirely in `selection_text` (no change to the print/wrap path). Rejected for the baseline: adding a precise per-row `wrapped` flag threaded through `print`/scrollback (more correct but touches the hot print path and the scrollback row type — deferred, recorded in Known Gaps). **The heuristic errs in BOTH directions, and the second is codebase-signature:** (a) *false positive* — a hard-newline line that exactly fills the width is wrongly joined; (b) *false negative* — `Grid::print` margin-wraps a double-width char rather than splitting it (`grid.rs:289` "a double-width char … occupies two cells", the `wide_char_wraps_rather_than_splitting_the_margin` test), so a genuinely soft-wrapped CJK/한글 row can leave `cols-1` **blank**, which the heuristic misreads as a hard break and mis-injects a `\n`. Both are accepted for the baseline; the precise per-row `wrapped` flag (Known Gaps) is the single correct fix for both.

### D9: Selection is a view-coupled field — populated on the live snapshot only, and invalidated by resize
- **Status**: resolved
- **Rationale**: `from_grid` copies `grid.selection()` into the snapshot **only when `view_offset == 0`** (live view), leaving it `None` while scrolled into history — identical to how inline images are populated only at the live bottom (`snapshot.rs:56-61`). Because selection coords are dimension-bound, `Grid::resize` **clears** any selection (old `(col,row)` may exceed new bounds). The model does not otherwise auto-clear on content change; *when* to clear on new output/scroll is the app's policy (selection-input), keeping this layer a pure store. Rejected: clamping selection to new bounds on resize (a resized selection is meaningless; clearing is the honest baseline).

## Constraints
- `term-core` MUST NOT gain a wgpu or winit dependency (dependency-direction rule); no `#[cfg(target_os)]` branches (macos-only rule).
- No `unwrap()`/`expect()` outside `#[cfg(test)]`; per-crate `thiserror` error type.
- `GridSnapshot` derives `PartialEq`; the new `selection` field participates so a selection change makes two snapshots compare unequal (the renderer idle-CPU cache relies on `GridSnapshot` equality — `grid.rs:55-62`, `docs/architecture.md` G1). `Selection` must derive `PartialEq`/`Eq`/`Clone`/`Copy`/`Debug`.
- `cargo clippy --workspace --all-targets -- -D warnings` must stay clean; `rustfmt` runs on save (PostToolUse hook).
- Extraction/normalization must never panic on out-of-range or inverted (`focus` before `anchor`) coordinates — clamp to grid bounds.

## Known Gaps
- Precise per-row soft-wrap flag (threaded through `print`/scrollback) is deferred; D8's filled-to-last-column heuristic is the baseline and errs both ways: it can (a) mis-join a hard-newline line that exactly fills the width, and (b) mis-**split** a genuinely soft-wrapped row whose `cols-1` was left blank because `Grid::print` margin-wrapped a double-width (CJK/한글) char instead of splitting it (`grid.rs:289`, test `wide_char_wraps_rather_than_splitting_the_margin`). The stored `wrapped` flag is the single fix for both.
- Selection anchored into scrollback history (drag while scrolled, or a selection that survives scrolling) is out of scope; selection is live-viewport-only (D9), matching the image baseline.
  - **Superseded by `specs/selection-scroll-anchor`**, which reverses D9 and this gap. The selection is now stored as content lines, projected into the snapshot at every `view_offset`, extracted by `Terminal::selection_text`, and cleared when its text changes.
- The policy for clearing a stale selection on new shell output / viewport scroll lives in `selection-input`, not here.

## Requirements

### R0: term-core exposes a GPU-free selection + bracketed-paste-mode core, headlessly testable
#### R0.1: No GPU/window dependency is introduced
- **Given**: the `term-core` crate after this feature
- **When**: `cargo tree -p term-core` / a build is inspected
- **Then**: neither `wgpu` nor `winit` appears in `term-core`'s dependencies, and no `#[cfg(target_os)]` branch was added
#### R0.2: The whole feature is verified without a GPU
- **Given**: the new selection + bracketed-mode behavior
- **When**: `cargo test -p term-core` runs on a headless machine
- **Then**: unit + insta snapshot tests cover membership, extraction, and mode tracking, and all pass

### R1: `Selection` value type with linear-span membership
#### R1.1: Normalization orders anchor/focus in reading order
- **Given**: `Selection { anchor: (5, 2), focus: (1, 0) }` (focus before anchor)
- **When**: `selection.normalized()` is called
- **Then**: it returns `(start, end) = ((1, 0), (5, 2))` ordered by `(row, col)`
#### R1.2: Membership follows linear/stream semantics across rows
- **Given**: a selection normalized to start `(3, 0)`, end `(2, 2)` on a 10-column grid
- **When**: `contains(col, row, 10)` is queried
- **Then**: `(3,0)`, `(9,0)`, `(0,1)`, `(9,1)`, `(0,2)`, `(2,2)` are selected; `(2,0)` and `(3,2)` are not
#### R1.3: A single-cell selection selects exactly one cell
- **Given**: `Selection { anchor: (4, 1), focus: (4, 1) }` on a 10-column grid
- **When**: `contains` is queried across the grid
- **Then**: only `(4, 1)` returns true
#### R1.4: `Selection` derives value semantics
- **Given**: the `Selection` type
- **When**: it is used in a `GridSnapshot` field
- **Then**: it derives `Clone, Copy, PartialEq, Eq, Debug`, so two snapshots differing only in selection compare unequal

### R2: `Grid` holds selection state with set/clear and resize invalidation
#### R2.1: Setting a selection makes it readable
- **Given**: a fresh `Grid`
- **When**: `grid.set_selection(Selection { anchor: (2,0), focus: (6,1) })` then `grid.selection()`
- **Then**: `Some(Selection { anchor: (2,0), focus: (6,1) })` is returned
#### R2.2: Clearing removes the selection
- **Given**: a `Grid` with a selection set
- **When**: `grid.clear_selection()` then `grid.selection()`
- **Then**: `None` is returned
#### R2.3: Resize invalidates a live selection
- **Given**: a `Grid` with a selection whose coords are valid at the current size
- **When**: `grid.resize(new_cols, new_rows)` is called
- **Then**: `grid.selection()` returns `None`
#### R2.4: A default grid has no selection
- **Given**: a newly constructed `Grid`
- **When**: `grid.selection()` is queried before any set
- **Then**: `None` is returned

### R3: `GridSnapshot` carries the selection and extracts its text
#### R3.1: The live snapshot carries the grid's selection
- **Given**: a `Grid` at the live bottom (`view_offset == 0`) with a selection set
- **When**: `GridSnapshot::from_grid(&grid)` is built
- **Then**: `snapshot.selection == Some(that selection)`
#### R3.2: A scrolled snapshot carries no selection
- **Given**: a `Grid` with a selection but scrolled into history (`view_offset > 0`)
- **When**: the snapshot is built
- **Then**: `snapshot.selection == None`
#### R3.3: Extraction trims trailing whitespace per row and skips continuation cells
- **Given**: a **single-row** selection over `한글  ` — a double-width leader + its `\0` continuation cell followed by two trailing spaces — not spanning into another row
- **When**: `snapshot.selection_text()` is called
- **Then**: it returns `Some("한글")` — the continuation `\0` dropped and the trailing spaces removed (no join is implicated)
#### R3.4: Soft-wrapped rows join without a newline; hard breaks keep it (narrow-cell rows)
- **Given**: two narrow-cell (ASCII) cases — a selection spanning row `r` whose last column (`cols-1`) holds a non-blank cell into row `r+1`, and a selection spanning a row that ends **before** the last column into the next
- **When**: `selection_text()` is called
- **Then**: the filled-to-last-column pair joins with no separator; the pair that ends early joins with `\n`. (A row terminated by a **margin-wrapped double-width char** leaves `cols-1` blank and is therefore classified as a hard break here — the accepted D8 false-negative, see Known Gaps — not asserted as a join by this requirement.)
#### R3.5: No selection yields `None`
- **Given**: a snapshot with `selection == None`
- **When**: `selection_text()` is called
- **Then**: it returns `None`
#### R3.6: Inverted / out-of-range coordinates never panic
- **Given**: a selection whose `focus` precedes its `anchor`, or coordinates at the grid edge
- **When**: `selection_text()` / membership run
- **Then**: they return normalized text / bounded results without panicking

### R4: Bracketed-paste mode (DECSET/DECRST 2004) is tracked and exposed
#### R4.1: `CSI ? 2004 h` enables the mode
- **Given**: a `Terminal` with bracketed paste off
- **When**: the byte stream `ESC [ ? 2004 h` is advanced
- **Then**: `terminal.bracketed_paste()` returns `true`
#### R4.2: `CSI ? 2004 l` disables the mode
- **Given**: a `Terminal` with bracketed paste on
- **When**: `ESC [ ? 2004 l` is advanced
- **Then**: `terminal.bracketed_paste()` returns `false`
#### R4.3: Default is off
- **Given**: a freshly constructed `Terminal`
- **When**: `terminal.bracketed_paste()` is queried before any sequence
- **Then**: it returns `false`
#### R4.4: RIS resets the mode
- **Given**: a `Terminal` with bracketed paste on
- **When**: `ESC c` (RIS) is advanced
- **Then**: `terminal.bracketed_paste()` returns `false`
#### R4.5: The mode is not a `GridSnapshot` field
- **Given**: the mode is a renderer-irrelevant fact (per SD5, like DECCKM)
- **When**: `GridSnapshot` is inspected
- **Then**: it carries no bracketed-paste field; the mode is reachable only via `Terminal::bracketed_paste()`

## Tasks

### T1: `Selection` value type + membership [infra]
- **Fulfills**: R1
- **Depends on**: (none)
- Create `crates/term-core/src/selection.rs`: `Selection { anchor, focus }` deriving `Clone/Copy/PartialEq/Eq/Debug`, `normalized()`, `contains(col,row,cols)` (linear-span). Unit tests for R1.1–R1.3. Add `pub mod selection;` + re-export in `lib.rs`.

### T2: `Grid` selection state + resize invalidation [core]
- **Fulfills**: R2, R4 (Grid flag portion)
- **Depends on**: T1
- In `grid.rs`: add `selection: Option<Selection>` and `bracketed_paste: bool` fields (init `None`/`false`), `set_selection`/`clear_selection`/`selection` accessors and `set_bracketed_paste`/`bracketed_paste`; clear the selection in `resize`. Add `Terminal::set_selection`/`clear_selection`/`selection` delegations in `lib.rs`. Unit tests for R2.1–R2.4. (Touches `lib.rs`; sequenced before T4's `lib.rs` edit.)

### T3: `GridSnapshot` selection field + `selection_text` extraction [core]
- **Fulfills**: R3
- **Depends on**: T1, T2
- In `snapshot.rs`: add `pub selection: Option<Selection>`, populate from `grid.selection()` only when `view_offset == 0` (else `None`); implement `selection_text(&self) -> Option<String>` reusing the `to_text` continuation-skip + `trim_end`, applying D8 soft-wrap join and D6 linear span, clamping inverted/edge coords. Insta/unit tests for R3.1–R3.6 (mirror `tests/grid_snapshot.rs`).

### T4: Bracketed-paste mode 2004 in the parser + `Terminal` accessor [core]
- **Fulfills**: R4
- **Depends on**: T2
- In `parser.rs` `csi_dispatch`: add `Some(2004) => self.grid.set_bracketed_paste(action == 'h')` beside the DECCKM arm (update the "stays ignored" comment); reset `bracketed_paste` in the RIS path (`parser.rs:287`). Add `Terminal::bracketed_paste()` delegation in `lib.rs`. Tests for R4.1–R4.5 (mirror `tests/cursor_key_mode.rs`). (Touches `lib.rs` after T2.)

## External Dependencies

### Pre-work
- (none) — no new crates; `unicode-width` is already a `term-core` dependency.

### Post-work
- (none for this feature) — downstream siblings `selection-highlight` (reads `GridSnapshot.selection`), `selection-input` (calls `Terminal::set_selection`/reads `bracketed_paste`), `text-paste`, and `copy-clipboard` (`selection_text`) consume this surface.
