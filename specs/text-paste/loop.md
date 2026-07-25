# loop.md — Loop Contract: text-paste

- feature-id: text-paste
- spec: specs/text-paste/spec.md
- pre-approved-unattended: yes   # approved at build-order batch gate 2026-07-24

## Goal
Implement text paste per `specs/text-paste/spec.md`: extend `ClipboardHandler` so Cmd+V
with no image reads `NSPasteboardTypeString` and types it into the active PTY (multi-line
intact), wrapping in `ESC[200~`/`ESC[201~` when `AppCtx.bracketed_paste` — with the image
path byte-unchanged.

## Scope & non-goals
- In: `crates/app/src/clipboard.rs` (the declared surface).
- Out: term-core/renderer code; seam.rs / terminal_app.rs (AppCtx.bracketed_paste comes from selection-input); selection/copy (siblings).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — `cargo build` is the typecheck in Rust |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| `wrap_paste` tests (R2/R3/R5) | `≥ 1 per case, all green` | `cargo test -p app` |
| test coverage % | N/A — no coverage tooling configured | — |
| fps / latency | N/A as a gate — project GOALS not gates (docs/domain G1) | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — extends the existing `ClipboardHandler` in place (image-first ordering preserved); objc2 NSPasteboard read consistent with the existing image read (SD3); reads `AppCtx.bracketed_paste`, adds no field.
- **Naming / readability** — `wrap_paste`/`read_clipboard_string` mirror the existing `read_clipboard_image_png`/helper idiom.
- **Spec alignment** — multi-line preserved verbatim; bracketed wrap only when mode on; embedded `ESC[201~` neutralized in the bracketed envelope; the non-bracketed paste-exec boundary is the accepted residual (R0–R5).
- **No regression** — an image on the pasteboard still follows the existing image path unchanged (R0.1).

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs/comments,
behaviour-preserving refactors — all WITHIN the declared surface.

**🛑 STOP — call a human (escalate):**
- Changing the existing image-paste behavior (must stay byte-identical).
- Stripping newlines from pasted text (breaks multi-line paste), or any general control-char sanitization beyond the specified `ESC[201~` neutralization (a deliberate scope boundary).
- Adding a cross-platform clipboard crate; adding a `#[cfg(target_os = ...)]` branch (macos-only / SD3).
- Editing `Cargo.lock` (PreToolUse hook blocks it).
- Editing any file OUTSIDE the declared surface (e.g. seam.rs — AppCtx.bracketed_paste is selection-input's).
- Anything conflicting with the spec's Decisions/SDs or this Goal.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- A fresh contract has none. -->

### L1 — single-pass `str::replace` is defeatable when neutralizing a terminator
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- note: `wrap_paste` removed an embedded `ESC[201~` with one `str::replace`, but `str::replace` is single-pass and does not re-scan — a SPLIT marker (`ESC[2` + `ESC[201~` + `01~`) reconstituted a live terminator inside the bracketed envelope (paste injection). Neutralize to a FIXED POINT (`while cleaned.contains(marker) { replace }`; terminates because the string strictly shrinks) and test with a reconstituted/nested-split adversarial vector, not just a plain embedded one.
