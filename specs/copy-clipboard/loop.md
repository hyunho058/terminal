# loop.md — Loop Contract: copy-clipboard

- feature-id: copy-clipboard
- spec: specs/copy-clipboard/spec.md
- pre-approved-unattended: yes   # approved at build-order batch gate 2026-07-24

## Goal
Implement copy per `specs/copy-clipboard/spec.md`: a `CopyHandler` that consumes Cmd+C
unconditionally and writes `AppCtx.selection_text` to the system clipboard via objc2
NSPasteboard (behind an injectable sink), plus a `SelectionPresenter` overlay + `main.rs`
registration wiring the highlight — without disturbing existing extensions.

## Scope & non-goals
- In: `crates/app/src/{copy.rs, main.rs}` (the declared surface).
- Out: term-core/renderer code; seam.rs / terminal_app.rs (AppCtx.selection_text comes from selection-input); the SelectionLayer itself (selection-highlight); Cmd+V (text-paste).

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
| `is_copy` + handler-branch tests (R0.2/R1) | `≥ 1, all green, recorder sink` | `cargo test -p app` |
| test coverage % | N/A — no coverage tooling configured | — |
| fps / latency | N/A as a gate — project GOALS not gates (docs/domain G1) | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — Cmd+C is an `InputHandler` via the seam (SD4); `CopyHandler`/`SelectionPresenter` mirror `ClipboardHandler`/`ImagePresenter`; the NSPasteboard write mirrors the existing read (SD3); `main.rs` registrations are additive.
- **Naming / readability** — `is_copy`/`write_clipboard_string`/`SelectionPresenter` match the sibling idiom.
- **Spec alignment** — Cmd+C consumed **unconditionally** (never types a stray `c` via encode_key's text fallback); writes only when `selection_text` is `Some`; `is_copy` case-folds; injectable sink makes the decision unit-testable (R0–R4).
- **No regression** — Ctrl+C/SIGINT unaffected (Control chord, not matched); existing AiMonitor/ImagePresenter/ClipboardHandler/TabMetaPump registrations unchanged (R0.1, R4.1).

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
- Returning `Passthrough` on the Cmd+C chord (would type a stray `c`) — the chord must be consumed unconditionally.
- Reordering/removing existing `main.rs` registrations, or affecting Ctrl+C/SIGINT.
- Adding a cross-platform clipboard crate; adding a `#[cfg(target_os = ...)]` branch (macos-only / SD3).
- Editing `Cargo.lock` (PreToolUse hook blocks it).
- Editing any file OUTSIDE the declared surface (e.g. seam.rs — AppCtx.selection_text is selection-input's; the SelectionLayer is selection-highlight's).
- Anything conflicting with the spec's Decisions/SDs or this Goal.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- A fresh contract has none. -->

### L1 — a nested tuple type in a signature trips clippy::type_complexity
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- note: A test helper returning `(Rc<RefCell<Vec<String>>>, Box<dyn Fn(&str)>)` fails `clippy::type_complexity` under `-D warnings`. Factor the parts into `type` aliases (e.g. `type Recorded = Rc<RefCell<Vec<String>>>; type Sink = Box<dyn Fn(&str)>;`) so the signature reads `(Recorded, Sink)`.
