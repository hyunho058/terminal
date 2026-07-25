# Spec: copy-clipboard

## Meta
- **Created**: 2026-07-23
- **Type**: dev
- **Status**: planned (batch-approved; not yet executed)
- **Set**: clipboard-copy-paste (see `specs/clipboard-copy-paste/partition-manifest.md`)
- **Generated**: via decompose → specify `mode: batch`
- **Approved by**: decompose partition gate (batch — human owns the bar at the ONE partition gate)
- **Approved at**: 2026-07-23

## Goal
Close the copy loop: a Cmd+C `InputHandler` that writes the active grid's selection
text (from `AppCtx.selection_text`) to the system clipboard via objc2 NSPasteboard, and
the `main.rs` wiring that registers both this handler and the `selection-highlight`
`DrawLayer` (through the SD4 `Overlay` seam) so a live selection is visible — without
disturbing the existing image-paste / ai-monitor / paste registrations.

## Non-goals
- No selection *model*, *mouse handling*, or *highlight layer* (owned by
  `selection-model` / `selection-input` / `selection-highlight`); this feature only
  consumes them.
- No Cmd+V handling (that is `text-paste`).
- No SIGINT handling — Ctrl+C (a `Control` chord) is untouched; only Cmd+C (a `Super`
  chord) is intercepted.
- No copy-on-select (explicit Cmd+C is the copy trigger; see Known Gaps).

## Confirmed Goal
Add `crates/app/src/copy.rs` (a `CopyHandler` `InputHandler` + a `write_clipboard_string`
NSPasteboard write + a passthrough `SelectionPresenter` `Overlay` whose `layer()` returns
the renderer's `SelectionLayer`) and register both in `crates/app/src/main.rs` alongside
the existing extensions, which stay unchanged.

## Declared Surface
- feature-id: copy-clipboard
- depends_on: [selection-input, selection-highlight]
- declared-surface:
  - crates/app/src/copy.rs
  - crates/app/src/main.rs

## Research
- `main.rs` registers feature extensions through the SD4 seam with `app.register_overlay(Box::new(...))` and `app.register_handler(Box::new(...))` — today: `AiMonitorOverlay`, `ImagePresenter`, `ClipboardHandler` (`crates/app/src/main.rs:47-52`), plus the `tabmeta::TabMetaPump` overlay (`main.rs:68`). New registrations append here.
- `ImagePresenter` is the exemplar passthrough `Overlay` (`crates/app/src/clipboard.rs:114-131`): `priority()=0`, `visible()=false` ("not an input-capturing overlay; only its draw layer matters"), `on_key → Passthrough`, `layer() → Box::new(ImageLayer::new())`. `SelectionPresenter` mirrors it, returning `Box::new(renderer::selection_layer::SelectionLayer::new())` (from `selection-highlight`).
- `ClipboardHandler` is the exemplar `InputHandler` (`clipboard.rs:41-74`): `on_key` matched on a chord predicate (`is_paste`: `mods.super_key() && Key::Character("v")`); it writes to `ctx.writer` and returns `Consumed`/`Passthrough`. `CopyHandler` mirrors it with `is_copy` (`super_key() && "c"`).
- The read source: `AppCtx.selection_text: Option<String>`, provided by `selection-input` (its D9), populated at **`AppCtx` construction** inside `terminal_app.rs`'s `App::on_key` (from the active tab, gated on an existing selection) — *not* as a side effect of another router handler, so `CopyHandler`'s registration order is irrelevant. This feature only reads `ctx.selection_text`.
- objc2 NSPasteboard **write** is the mirror of the existing **read** in `clipboard.rs` (`read_clipboard_image_png` uses `NSPasteboard::generalPasteboard()` + `dataForType`, `clipboard.rs:96-108`): `pb.clearContents()` then `pb.setString_forType(&ns, NSPasteboardTypeString)` with an `NSString` built from the Rust `String`. `objc2_foundation` is already used for `NSDictionary` (`clipboard.rs:21`); add `NSString`.
- Cmd+C vs SIGINT: in a macOS terminal, **Cmd+C** is Copy and **Ctrl+C** sends SIGINT. `is_copy` matches only the `Super` modifier, so Ctrl+C (a `Control` chord) never reaches this handler and continues to `encode_key` → `\x03` unchanged.
- Router precedence: `InputRouter::dispatch` runs handlers in registration order, first `Consumed` wins (`seam.rs:130-137`). `CopyHandler` (Cmd+C) and `ClipboardHandler` (Cmd+V) match disjoint chords, so their relative order is irrelevant.
- Pure-predicate testing is the codebase's seam (`is_paste`/`unique_name` tests); the NSPasteboard write is exercised manually (`/run-term`). Constructing an `AppCtx` in a unit test is supported (`seam.rs` tests build `AppCtx::new(...)`; `selection-input` adds the `with_selection_text` builder).
- Constraints: macos-only objc2, no-gui-crates (the highlight is `renderer`'s hand-rolled layer), no `unwrap()` outside tests, `cargo clippy … -D warnings`, `rustfmt` on save.

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

### D6: Cmd+C is a `CopyHandler` `InputHandler` that **consumes the chord unconditionally**, writing `AppCtx.selection_text` only when present
- **Status**: resolved
- **Rationale**: `CopyHandler::on_key` returns `Passthrough` for any key that is not `is_copy` (`mods.super_key() && s.eq_ignore_ascii_case("c")` — case-folded like `is_paste` at `clipboard.rs:73`, so Cmd+Shift+C / caps-lock still match). For the Cmd+C chord it **always returns `Consumed`** — Cmd+C is a macOS app chord (Copy) and must be swallowed so it can never fall through to `encode_key`. This is the crux: `encode_key`'s `super_key` swallow (`input.rs:230`) covers only *Named* cursor/nav keys; a `Key::Character("c")` hits the `_ => text.map(...)` text fallback and would **type a literal `c` at the prompt** (the same path that makes Cmd+A reach the shell as `a` — documented at `input.rs:229` and pinned by the R6.2 `encode_key(ch("a"),Some("a"),cmd)==Some(b"a")` test). So Passthrough-on-empty would be a real bug, not a no-op. On the consumed chord: if `ctx.selection_text` is `Some(text)`, invoke the write sink; if `None`, consume and do nothing. Ctrl+C (SIGINT) is a `Control` chord, never matched by `is_copy`, so interrupt handling is untouched. Rejected: `Passthrough` when `selection_text` is `None` (types a stray `c`); handling Cmd+C in the `terminal_app.rs` event loop (SD4 mandates the seam); a `visible`/stateful overlay (a stateless handler suffices).

### D7: The NSPasteboard write mirrors the existing read, behind an **injectable sink** so `on_key` is unit-testable
- **Status**: resolved
- **Rationale**: `write_clipboard_string(s: &str)` runs on the UI thread inside `unsafe`: `let pb = NSPasteboard::generalPasteboard(); pb.clearContents(); let ns = NSString::from_str(s); pb.setString_forType(&ns, NSPasteboardTypeString);` — the exact structural twin of `read_clipboard_image_png`'s `generalPasteboard()` + `dataForType` (SD3); `clearContents` before `setString` is required by AppKit (declare ownership, then write). Because that write is a **real global side effect**, folding it directly into `on_key` would make the write path (and thus the `Consumed`-with-write decision) untestable. So `CopyHandler` holds an **injectable sink** — a `Box<dyn Fn(&str)>` (or `FnMut`) field that `CopyHandler::new()` defaults to `write_clipboard_string`; a `CopyHandler::with_sink(f)` test constructor substitutes a recorder (e.g. `Rc<RefCell<Vec<String>>>`), so `on_key`'s "write iff `Some`, always `Consumed`" logic is asserted without touching the real pasteboard. Rejected: an inline un-injectable `write_clipboard_string` call (the tested-vs-effectful contradiction the reviewer flagged); `writeObjects(&[ns])` (needless array wrapper — `setString_forType` is the direct single-string call); touching other pasteboard flavors.

### D8: The highlight is wired by a passthrough `SelectionPresenter` `Overlay` returning `SelectionLayer`
- **Status**: resolved
- **Rationale**: A `SelectionPresenter` in `copy.rs` mirrors `ImagePresenter` exactly — `priority()=0`, `visible()=false`, `on_key → Passthrough`, `layer() → Box::new(renderer::selection_layer::SelectionLayer::new())` — so registering it via `app.register_overlay` inserts the `selection-highlight` `DrawLayer` into the render pipeline with no `terminal_app.rs` edit. This is the reason `copy-clipboard` `depends_on selection-highlight` (it names `SelectionLayer`). The highlight lives in this feature's wiring because the highlight only exists to visualize a selection being copied. Rejected: registering the layer directly on the `Renderer` (bypasses the established `Overlay` registration path that `ImagePresenter` set) and putting `SelectionPresenter` in `terminal_app.rs` (SD4 — no event-loop edit).

### D9: `main.rs` appends the two registrations; existing extensions are untouched and ordered before them
- **Status**: resolved
- **Rationale**: In `main.rs`, after the existing `register_overlay(AiMonitorOverlay)`, `register_overlay(ImagePresenter)`, `register_handler(ClipboardHandler)` (and the later `TabMetaPump`), add `app.register_overlay(Box::new(SelectionPresenter))` and `app.register_handler(Box::new(CopyHandler::new()))`. Existing registrations keep their identity and order (overlay priority + draw z-order, not registration index, drive behavior; and Cmd+C/Cmd+V are disjoint chords). Rejected: reordering or replacing existing registrations (needless churn / regression risk).

## Constraints
- Only `copy.rs` (new) and `main.rs` may change (declared surface). The existing `main.rs` registrations (`AiMonitorOverlay`, `ImagePresenter`, `ClipboardHandler`, `TabMetaPump`) must remain and keep working.
- macos-only objc2 NSPasteboard on the UI thread; no cross-platform clipboard crate; no `#[cfg(target_os)]`.
- Reads `AppCtx.selection_text` (from `selection-input`) and names `renderer::selection_layer::SelectionLayer` (from `selection-highlight`); adds no field to `AppCtx` and no code to those crates.
- No `unwrap()`/`expect()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` clean; `rustfmt` on save.
- Ctrl+C (SIGINT) must remain unaffected — only the `Super`+C chord is consumed.

## Known Gaps
- No copy-on-select: text is copied only on explicit Cmd+C, not automatically when a drag ends (a possible follow-up).
- Only `NSPasteboardTypeString` is written (no RTF/HTML flavor).
- The real `write_clipboard_string` NSPasteboard write (the default sink) and the `Overlay` registration are verified manually (`/run-term`); the injectable sink lets `is_copy` and `on_key`'s consume/write decision be unit-tested without the real pasteboard.

## Requirements

### R0: Cmd+C copies the selection and the highlight is wired, without regressing existing extensions
#### R0.1: Existing registrations still work
- **Given**: the app after this feature
- **When**: it launches
- **Then**: ai-monitor (Cmd+Shift+M), image-paste (Cmd+V image), text-paste (Cmd+V text), and tab-metadata all still function; the two new registrations are additive
#### R0.2: The copy predicate/branch is headlessly testable
- **Given**: `is_copy` and a `CopyHandler::with_sink(recorder)`
- **When**: `cargo test -p app` runs
- **Then**: `is_copy` recognizes Cmd+C case-insensitively (and rejects Ctrl+C / plain `c`), and `on_key`'s decision (always `Consumed` on the chord; sink invoked iff `selection_text` is `Some`) is asserted via a constructed `AppCtx` and the recorder sink — no real NSPasteboard

### R1: `CopyHandler` copies the selection text on Cmd+C
#### R1.1: Cmd+C with a selection writes it to the clipboard and consumes
- **Given**: `ctx.selection_text == Some("hello world")` and a `CopyHandler::with_sink(recorder)`
- **When**: Cmd+C is dispatched to `CopyHandler::on_key`
- **Then**: the sink is invoked with `"hello world"` and the handler returns `Consumed`
#### R1.2: Cmd+C with no selection consumes without writing or typing
- **Given**: `ctx.selection_text == None`
- **When**: Cmd+C is dispatched
- **Then**: the sink is **not** invoked and the handler returns `Consumed` — the chord is swallowed so no stray `c` reaches the PTY via `encode_key`'s text fallback
#### R1.3: A non-Cmd+C key passes through
- **Given**: any key that is not the `Super`+C chord (e.g. Ctrl+C, or plain `c` with no `Super`)
- **When**: it is dispatched to `CopyHandler::on_key`
- **Then**: `is_copy` is false and the handler returns `Passthrough` (SIGINT via Ctrl+C, and normal `c` typing, unaffected)

### R2: The clipboard write uses objc2 NSPasteboard
#### R2.1: `write_clipboard_string` clears then sets the string
- **Given**: a string `s`
- **When**: `write_clipboard_string(s)` runs
- **Then**: it calls `NSPasteboard::generalPasteboard().clearContents()` then `setString_forType(NSString::from_str(s), NSPasteboardTypeString)` (the read's structural twin)
#### R2.2: A round-trip yields the copied text
- **Given**: a selection copied via Cmd+C
- **When**: the system clipboard is read (e.g. `pbpaste` / another app)
- **Then**: it holds exactly the selection text (manual/integration verification)

### R3: The selection highlight is registered through the Overlay seam
#### R3.1: `SelectionPresenter` is a passthrough overlay providing the `SelectionLayer`
- **Given**: `SelectionPresenter`
- **When**: it is used as an `Overlay`
- **Then**: `priority()==0`, `visible()==false`, `on_key → Passthrough`, and `layer()` returns a `Box<dyn DrawLayer>` that is `SelectionLayer` (z-order `SELECTION_Z`)
#### R3.2: Registering it makes a live selection visible
- **Given**: `SelectionPresenter` registered in `main.rs` and a live drag selection
- **When**: the frame renders
- **Then**: the selected cells are highlighted (the `selection-highlight` layer draws from `snapshot.selection`) — manual verification via `/run-term`

### R4: `main.rs` wiring is additive
#### R4.1: Both registrations are appended without disturbing the others
- **Given**: the `main.rs` diff
- **When**: reviewed
- **Then**: `register_overlay(SelectionPresenter)` and `register_handler(CopyHandler::new())` are added; the existing `AiMonitorOverlay`/`ImagePresenter`/`ClipboardHandler`/`TabMetaPump` registrations are unchanged in identity and order

## Tasks

### T1: `copy.rs` — `CopyHandler`, `write_clipboard_string`, `SelectionPresenter` [core]
- **Fulfills**: R1, R2, R3, R0.2
- **Depends on**: (none within this spec)
- Create `crates/app/src/copy.rs`: `is_copy` predicate (case-folded `super_key() && "c"`); `CopyHandler` holding an injectable sink (`Box<dyn Fn(&str)>`), `new()` defaulting it to `write_clipboard_string`, plus `with_sink(f)` for tests; `on_key` returns `Passthrough` for non-chords, else **always `Consumed`**, invoking the sink only when `ctx.selection_text` is `Some`; `write_clipboard_string(&str)` (objc2 `clearContents` + `setString_forType`); and `SelectionPresenter` (`Overlay`) returning `Box::new(SelectionLayer::new())`. Unit tests for R0.2/R1.1–R1.3 via a recorder sink + constructed `AppCtx` — no real pasteboard.

### T2: Register `CopyHandler` + `SelectionPresenter` in `main.rs` [wiring]
- **Fulfills**: R0.1, R4
- **Depends on**: T1
- Add `mod copy;`, import `copy::{CopyHandler, SelectionPresenter}`, and append `app.register_overlay(Box::new(SelectionPresenter))` + `app.register_handler(Box::new(CopyHandler::new()))` after the existing registrations, leaving `AiMonitorOverlay`/`ImagePresenter`/`ClipboardHandler`/`TabMetaPump` untouched.

## External Dependencies

### Pre-work
- (none) — `objc2_app_kit`/`objc2_foundation` already dependencies; consumes `selection-input`'s `AppCtx.selection_text` + `selection-highlight`'s `SelectionLayer`.

### Post-work
- (none) — this closes the copy loop; the full set (`selection-model` → `selection-highlight` / `selection-input` → `text-paste` / `copy-clipboard`) delivers both acceptance seeds.
