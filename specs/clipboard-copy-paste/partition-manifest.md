# partition-manifest.md — clipboard-copy-paste
- generated_at: 2026-07-23T23:33:55-0700
- goal: Implement clipboard copy & paste for mac-terminal — (1) Cmd+V pastes the clipboard's TEXT into the active shell (bracketed-paste-aware; image paste unchanged), and (2) drag-select text on the grid with live highlight, Cmd+C copies the extracted text to the system clipboard, a plain click clears the selection.

## Shared Decisions
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
- **Rationale**: The SD4 seam (`InputRouter`/`OverlayHost`, `AppCtx`) exists so features extend input/overlays from their own modules without editing `terminal_app.rs`. Cmd+V (paste) and Cmd+C (copy) are `InputHandler`s dispatched through the router; the highlight is an `Overlay`-provided `DrawLayer`. The necessary exception is the pointer pipeline: `WindowEvent::MouseInput` (Left Pressed/Released) + drag-while-`CursorMoved` + pixel→cell mapping MUST touch the `terminal_app.rs` event loop (it currently handles only Left+Pressed → `sidebar_press`, with no Released/drag path). Existing sidebar press/hover/wheel and image-paste stay unaffected. Rejected: routing Cmd+C/Cmd+V through new bespoke event-loop branches instead of the seam.

### SD5: Bracketed-paste mode (DECSET/DECRST 2004) follows the existing DECCKM/app_cursor_keys convention
- **Status**: resolved
- **Rationale**: term-core already tracks a private mode this exact way: a `bool` flag on `Grid` (`grid.rs`), a `Terminal` accessor (`lib.rs`), set from `parser.rs` `csi_dispatch` on `?<n> h|l` (see `app_cursor_keys`/DECCKM; parser.rs:342 already names `?2004` as the exemplar "other private mode"). Bracketed-paste mode uses the same path: a `bracketed_paste` flag on `Grid`, a `Terminal::bracketed_paste()` accessor, set on `?2004 h|l`. The app reads it via that accessor (as `terminal_app.rs` already reads `app_cursor_keys`) and surfaces it to input handlers through a new `AppCtx` field. Rejected: tracking mode 2004 in the app layer or on the `Processor` instead of `Grid` (would diverge from the established convention and split mode state across two owners).

## Features  (ordered — a dependency appears before its dependents)
### selection-model
- declared-surface:
  - crates/term-core/src/selection.rs
  - crates/term-core/src/grid.rs
  - crates/term-core/src/snapshot.rs
  - crates/term-core/src/parser.rs
  - crates/term-core/src/lib.rs
- depends_on: []
- accept-seed: term-core carries a Selection (anchor/focus) on the Grid and on GridSnapshot, extracts the selected cell range to a String (trailing whitespace trimmed, soft-wrapped rows joined, double-width continuation cells skipped), and tracks bracketed-paste mode (DECSET/DECRST 2004) behind Terminal::bracketed_paste() — all verified headlessly.
- pre-approved-batch: yes

### selection-highlight
- declared-surface:
  - crates/renderer/src/selection_layer.rs
  - crates/renderer/src/lib.rs
- depends_on: [selection-model]
- accept-seed: Given a GridSnapshot carrying a selection, a hand-rolled per-cell DrawLayer paints the selected cell range by reusing the cell-bg path (no GUI crate); a snapshot with no selection draws nothing.
- pre-approved-batch: yes

### selection-input
- declared-surface:
  - crates/app/src/terminal_app.rs
  - crates/app/src/seam.rs
  - crates/app/src/input.rs
- depends_on: [selection-model]
- accept-seed: Left-press anchors a selection, drag (CursorMoved while pressed) updates its focus and sets the live selection on the active grid with a redraw each move, release ends the drag, and a plain click clears the selection; pixel→cell mapping honors the left/top insets; AppCtx exposes the active grid's selection text and bracketed-paste mode to input handlers. Existing sidebar press/hover/wheel are unaffected.
- pre-approved-batch: yes

### text-paste
- declared-surface:
  - crates/app/src/clipboard.rs
- depends_on: [selection-input]
- accept-seed: Cmd+V with text on the pasteboard (and no image) reads NSPasteboardTypeString and types it into the active PTY with multi-line content intact, wrapped in ESC[200~/ESC[201~ when AppCtx reports bracketed-paste mode; an image on the pasteboard still follows the existing image path.
- pre-approved-batch: yes

### copy-clipboard
- declared-surface:
  - crates/app/src/copy.rs
  - crates/app/src/main.rs
- depends_on: [selection-input, selection-highlight]
- accept-seed: A Cmd+C InputHandler copies the active grid's selection text to the system clipboard via objc2 NSPasteboard (clearContents + setString), and main.rs registers the selection-highlight Overlay so a live selection renders; existing ImagePresenter/ClipboardHandler registrations are unaffected.
- pre-approved-batch: yes
