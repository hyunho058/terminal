# loop.md — Clipboard text paste (Cmd+V)

## Goal
Cmd+V pastes clipboard **text** into the active shell's input line. Today the
`ClipboardHandler` (crates/app/src/clipboard.rs) only handles clipboard *images*;
for text it returns `Passthrough` and the fallback `encode_key` only re-encodes the
keystroke, so clipboard text is never written to the PTY. Fix: when the pasteboard
holds text (and no image), read it and write it to the active tab's PTY, consuming
the key. Image paste stays unchanged. Scope is paste only — copy/selection is a
separate, larger feature the user explicitly deferred.

## Gate 1 — Pass/Fail (binary, 100%)
- `cargo build` succeeds
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test -p app` green
- `cargo test --workspace` green

## Gate 2 — Quantitative (numbers vs thresholds)
- Paste-normalisation logic unit tests: **≥ 3**, all passing (the pure
  `normalize_paste` core — the NSPasteboard read is a thin untestable wrapper, as
  `read_clipboard_image_png` already is).
- Files touched: **1** (crates/app/src/clipboard.rs) — stays inside the SD4 seam,
  no terminal_app.rs edit.

## Gate 3 — Qualitative (judged, with grounds + action)
- **Consume semantics** — Cmd+V of text returns `Consumed` so the literal `v` is
  never also typed into the shell; `Passthrough` only when the pasteboard has
  neither image nor text.
- **Architecture fit** — the change lives in the existing `ClipboardHandler` via
  the seam; objc2 stays in the `app` crate (dependency-direction rule); image
  paste behaviour preserved.
- **Paste correctness** — pasted newlines are handled sanely (CRLF/LF → CR so a
  line doesn't double-accept); known limitation (no bracketed-paste / DEC ?2004)
  is stated, not silently ignored.

## Autonomy note
No STOP-list items expected (no schema/auth/payment/migration). Reading the system
pasteboard is read-only; writing pasted bytes to the PTY is the intended behaviour.
