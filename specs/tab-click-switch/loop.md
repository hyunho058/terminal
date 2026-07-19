# loop.md — Loop Contract: tab-click-switch

> Approved in batch by /build-order (left-tab-sidebar). Gates run by /loop.
- pre-approved-unattended: yes
- spec: specs/tab-click-switch/spec.md

## Goal
Wire the app to the left sidebar (`set_left_inset(tab_bar::width_px(..))` on startup AND config-reload) and add click-to-switch — a left-mouse press inside the sidebar switches tabs via `input::tab_target` → `tab_bar::tab_at` → `Tabs::jump` — while keeping every keyboard chord byte-unchanged; must not violate specs/tab-click-switch/spec.md.

## Scope & non-goals
- In: `crates/app/src/terminal_app.rs`, `crates/app/src/input.rs`, `crates/app/src/settings.rs`.
- Out: `crates/renderer/**` (consume `set_left_inset`/`left_inset()` and `tab_bar::{width_px,tab_height_px,tab_at}` — don't edit them); the `tab_action` chord table and `on_key` dispatch order (must stay byte-unchanged); grid-area mouse behavior (text selection/scroll).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| `input::tab_target` unit tests pass | all pass (hit / outside-band / no-tabs) | `cargo test -p app input` |
| existing chord tests still green | 0 failures | `cargo test -p app` (cmd_chords_map_to_actions, no_super_means_no_chord, encode_named_and_text) |
| clippy warnings | `== 0` | `cargo clippy --workspace --all-targets -- -D warnings` |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — `tab_target` is a real raw-input→intent translation (casts f64 cursor, derives `tab_h` via `tab_height_px`), not a `tab_at` alias; geometry stays in `tab_bar` (SD2). app→renderer only; no live `Grid` handed to renderer.
- **All three inset seams wired** — `start()`, `Resized` (via existing `grid_size()`→`resize_all`), AND `settings::apply` config-reload all use `set_left_inset(width_px(..))`; no residual `set_top_inset` reserving a top band.
- **Chords preserved (SD3)** — Cmd+T/W/[/]/1-9 and the overlays→router→tab_action→encode_key order are byte-unchanged; the mouse path is a separate additive `WindowEvent` arm.
- **Spec alignment** — D5–D9; press-to-switch reuses the existing `jump`+`sync_tab_model`+`notify_active_changed`+`redraw` path; `left_inset()` read live at click time.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the three app-file edits above, `input::tab_target` + its tests, the mouse `WindowEvent` arms, doc comments, in-scope refactors.

**🛑 STOP — call a human (escalate):**
- Any edit needed OUTSIDE the declared surface (`terminal_app.rs`/`input.rs`/`settings.rs`) — e.g. a missing `set_left_inset`/`tab_at` means an upstream dependency is unmet → escalate, don't add it here.
- Changing the `tab_action` chord table or `on_key` dispatch order (conflicts with SD3).
- Needing a GUI crate or breaking the dependency direction.
- insta snapshots change unexpectedly → `/insta-review`, do NOT blind-accept.
- Anything conflicting with this Goal or the spec.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)
<!-- A fresh contract has none. -->
