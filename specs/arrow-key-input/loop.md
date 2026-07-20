# loop.md — Loop Contract: arrow-key-input

> Verification gate for an implementation that is **already complete** (agent-orchestrate
> Phase 3.5). Iteration 1's WORK step is a deliberate no-op — VERIFY is the first real action.
- spec: specs/arrow-key-input/spec.md

## Goal
Prove the merged T1+T2+T3 implementation satisfies `specs/arrow-key-input/spec.md`
requirements R0–R8: `term-core` tracks DECCKM and exposes it via `Terminal::app_cursor_keys()`,
a new `esc_dispatch` implements a **DECCKM-only** RIS, and `encode_key` emits the D11/D13 byte
table for arrows / navigation / F-keys / modifier combinations while preserving the IME text
fallback — without violating the spec's Constraints.

## Scope & non-goals
- **In (the declared surface — the only files this loop may edit):**
  `crates/term-core/src/{grid.rs, parser.rs, lib.rs}`,
  `crates/term-core/tests/cursor_key_mode.rs`,
  `crates/app/src/{input.rs, terminal_app.rs}`,
  `specs/arrow-key-input/*`.
- **Out:** `crates/renderer/**`, `crates/common/**`, `crates/term-core/src/snapshot.rs`
  (`GridSnapshot` must not grow — D2), the `on_key` dispatch order, the `tab_action` chord
  table, `Cargo.toml`/`Cargo.lock` (no new dependencies — D8).
- **Not gated here (human observation, handed back — not failures):** R8.1 (arrow keys in a
  live zsh), R8.2 (`cat -v` byte inspection), R8.4 (`claude` trust prompt), and **R7.2**
  (two tabs, one in application mode, verifying tab 1 gets `ESC O A` while tab 2 gets `ESC[A`).
  These need the GUI app via `/run-term` and a human at the window. **R8.4 is explicitly NOT
  pass/fail** per D12 — if it fails while R8.1–R8.3 are green, that is a signal for a separate
  Kitty-keyboard-protocol spec, not a defect in this one.
  > R7.2 was added to this handback in iteration 2. `Running::on_key` needs a window + GPU +
  > PTY and is never constructed in a test, so the wiring has no unit coverage. Verified
  > empirically: replacing the read with `let app_cursor = false;` left all 123 tests, clippy,
  > and the byte-table oracle green. A static wiring assertion now guards the regression
  > (Gate 1), but only a human at the window can confirm the mode is genuinely live.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| format | `cargo fmt --all -- --check` |
| **byte-table + wiring oracle** | `python3 specs/arrow-key-input/check-terminfo-conformance.py` |

> The byte-table oracle exists because T3 wrote both the `KeySeq` table and its unit tests, so
> green tests only prove internal consistency. `infocmp xterm-256color` is an **external**
> oracle — the same terminfo zsh and every TUI read. The script also asserts the two
> *deliberate* D11 deviations (Home/End) so the table cannot silently drift back to terminfo's
> `smkx`-presuming `\EOH`/`\EOF`, which are bound to nothing in zsh's keymap.
>
> **Iteration 2 added a wiring assertion** to the same script (R7.2): the table is inert unless
> `terminal_app.rs` reads `self.tabs.active().terminal.app_cursor_keys()` and passes that
> binding — not a literal — to `encode_key`. Both mutations (`let app_cursor = false;`, and
> passing `false` at the call site) are caught; before this, both were invisible to every gate.

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| DECCKM headless tests (R1, R2) | 12 pass / 0 fail | `cargo test -p term-core --test cursor_key_mode` |
| `app` unit tests (R3–R7) | ≥ 20 pass / 0 fail | `cargo test -p app` |
| terminfo-agreed keys conformant | 20 / 20 | the byte-table oracle above |
| insta snapshot changes (R1.6) | `== 0` | `cargo test -p term-core --test grid_snapshot` + `git status --short crates/term-core/tests/snapshots/` |
| new crate dependencies (D8) | `== 0` | `git diff --stat -- '**/Cargo.toml' Cargo.lock Cargo.toml` |
| wgpu/winit refs in `term-core` source | `== 0` | `grep -rn 'wgpu\|winit' crates/term-core/src/` (doc-comment mentions excluded) |
| new `#[cfg(target_os` branches | `== 0` | `git diff -U0 -- crates/ \| grep -c 'cfg(target_os'` |
| `unwrap()` added outside `#[cfg(test)]` | `== 0` | `git diff -U0 -- crates/ \| grep '^+' \| grep -c 'unwrap()'`, then inspect any hit |

## Gate 3 — Qualitative  (score + grounds + action — scored by a SEPARATE checker subagent)

- **Architecture fit (D2/D10)** — is `app_cursor_keys` on `Grid` genuinely shaped like the
  `alt_screen` precedent, is it absent from `GridSnapshot`, and does `term-core` stay GPU-free?
- **Minimal-RIS discipline (D9)** — does `esc_dispatch` touch DECCKM and *nothing* else? No
  `Grid::clear()` (it would drop inline images), no alt-screen forcing, no pen/cursor reset;
  and is the arm guarded to `intermediates.is_empty() && byte == b'c'`?
- **Byte-table correctness (D11/D13)** — beyond the oracle: is the modifier placement right
  per family (tilde → second parameter `CSI n;m~`; cursor/SS3 → `CSI 1;m final`), is `m == 1`
  suppressed to the parameterless form, and does DECCKM correctly *not* apply to the tilde family?
- **Regression safety (D6/D15)** — is the `_ => text.map(...)` IME fallback intact and still
  last, is the `on_key` dispatch order unchanged, and is the Cmd-swallow scoped to exactly the
  `Key::Named` table (not `Key::Character`)?
- **Test non-vacuousness** — do the new tests actually pin behaviour, or would they pass
  against a wrong implementation? Flag any tautological or fixture-free assertion.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** formatting/lint/clippy fixes, missing or weak tests for
code already in the declared surface, doc-comment corrections, local renames, and
behaviour-preserving refactors that keep every Gate-1 command green.

**🛑 STOP — call a human (escalate):**
- Any edit needed **outside the declared surface** listed in Scope.
- Adding a crate dependency, a GUI crate, or anything breaking `common ← term-core ← renderer ← app`.
- Changing the `on_key` dispatch order or the `tab_action` chord table.
- Removing or reordering the `_ => text.map(...)` fallback (would regress IME/Korean input).
- Adding a field to `GridSnapshot`, or making RIS call `Grid::clear()`.
- insta snapshots changing → run `/insta-review`, never blind-accept.
- Anything conflicting with this Goal or with `specs/arrow-key-input/spec.md`.

## Exit criteria
Exits only when Gates 1–3 all pass and the Evidence Report is emitted, with R8.1/R8.2/R8.4
handed back as an explicit human-verification checklist. A 🛑 boundary ends in escalation,
not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — gate the un-unit-testable wiring line statically
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `python3 specs/arrow-key-input/check-terminfo-conformance.py`
- note: iteration 1 passed all three gates while `terminal_app.rs:288` (`let app_cursor = self.tabs.active().terminal.app_cursor_keys()`) had zero coverage — mutating it to `false` kept 123/123 tests, clippy, and the terminfo oracle green while deleting every application-mode arrow; `Running::on_key` needs a window+GPU+PTY so it is never unit-tested, so a static wiring assertion is the only automatic guard.
