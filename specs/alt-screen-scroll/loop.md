# loop.md — Loop Contract: alt-screen-scroll

> Supervisor contract for "make the mouse wheel scroll inside full-screen apps".
> Reported symptom: 스크롤이 안되는데 — the wheel does nothing while Claude Code
> (or any alt-screen app) is running.

- mode: interactive
- related spec: specs/vt-tui-completeness/spec.md (not a regression from it — see Diagnosis)

## Diagnosis (established before this contract, from measurement)

The wheel is dead on the alt screen for a pre-existing reason, **not** a regression
from the vt-tui-completeness work:

- `Grid` freezes history while `alt_screen` is set (scrollback D5, shipped long
  before this session). Measured: alt screen + 40 lines of app output →
  `history_len() == 0`, `scroll_up(5)` leaves `view_offset` at 0.
- `TerminalApp::wheel_grid` only redraws when `view_offset` actually changes
  (`terminal_app.rs:857`), so the gesture is silently swallowed.
- Baseline control: normal shell output scrolls correctly (`history=36`,
  offset 0→5). Plain zsh never emits `CSI r`, so the scroll-region path is not
  involved.

What other terminals do instead, and we do not:
1. **Mouse reporting.** An app that enables mouse tracking wants wheel events.
   Claude Code enables `?1000`, `?1002`, `?1003` and `?1006` (measured from its
   real PTY output) — so it expects SGR-encoded wheel events and scrolls itself.
   We parse none of these, so it never learns the wheel moved.
2. ~~**Alternate scroll.**~~ **REMOVED after Gate 3.** Translating the wheel to
   arrow keys for an untracked alt-screen app was already weighed and **rejected**
   by `terminal-scroll` D5 — it writes to the PTY, which `scrollback` D4 forbids
   outright, and it breaks `terminal-scroll` R5.1 (under `?1049h` the wheel must
   still reveal the frozen primary history). This contract proposed it without
   having read that sibling spec. Only the additive mouse-reporting half shipped.

Sending arrows to an app that asked for mouse events is actively wrong: Claude
Code would read `ESC[A` as "up arrow" and move its input cursor rather than scroll.
So mouse reporting is the part that fixes the reported symptom.

## Goal
Make the mouse wheel scroll the running application **when that application has
asked for mouse events**, without changing any existing wheel behavior. Apps that
did not ask keep the scrollback viewport, alt screen included (`terminal-scroll`
R5.1).

## Scope & non-goals
- In: `crates/app/` (wheel routing + event encoding) and `crates/term-core/`
  (mouse-mode state parsed from `?1000/?1002/?1003/?1006`, exposed like
  `bracketed_paste`). Tests for both.
- Out: `GridSnapshot`'s shape (vt-tui-completeness C-2 — mouse mode is input state,
  the renderer never reads it); click/drag/motion *reporting* beyond what is needed
  for the wheel, unless the chosen scope includes it; any new dependency;
  re-litigating the alt-screen history freeze itself (scrollback D5 stays).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| format | `cargo fmt --check` |
| build | `cargo build --workspace` |
| typecheck | N/A — Rust; performed by `cargo build` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| no-unwrap outside tests | `! awk '/#\[cfg\(test\)\]/{exit} /unwrap\(\)\|expect\(/{f=1} END{exit !f}' crates/term-core/src/grid.rs` (and `parser.rs`) |
| dependency direction | `! grep -qE 'wgpu\|winit\|raw-window' crates/term-core/Cargo.toml` |
| no dependency drift | `git diff --quiet -- Cargo.lock crates/app/Cargo.toml crates/term-core/Cargo.toml` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| workspace test failures | `= 0` | `cargo test --workspace` |
| clippy warnings | `= 0` | clippy under `-D warnings` |
| primary-screen scrollback unchanged | `= 100%` | `tests/scrollback.rs` + `terminal_app.rs` scroll tests all green, unmodified |
| new behavior has a mapped, green test | `>= 1 per behavior` | wheel-on-alt-screen emits the right bytes; wheel-on-primary still moves `view_offset` and writes **nothing** to the PTY |
| vacuous tests | `= 0` | mutation spot-check: neuter each new behavior, confirm ≥1 test fails |
| insta snapshots moved | `= 0` | `git status --porcelain crates/term-core/tests/snapshots` empty |

## Gate 3 — Qualitative  (score + grounds + action)

- **Fixes the reported symptom** — does the wheel actually scroll *Claude Code*,
  judged against its measured mode set (`?1000/?1002/?1003/?1006`), not against a
  generic TUI?
- **Correct encoding** — SGR (`?1006`) wheel events are `ESC [ < 64 ; col ; row M`
  (up) and `65` (down), 1-based coords; the legacy X10 form is used when `?1006` is
  off. Verified against the protocol, not assumed.
- **No primary-screen regression** — on the primary screen the wheel still moves the
  viewport and writes nothing to the PTY. This is the highest-risk direction.
- **Architecture fit** — mouse mode lives on `Grid` beside `bracketed_paste` /
  `app_cursor_keys` and stays out of `GridSnapshot`; the wheel keeps its single
  ordered hit-test (terminal-scroll D1) and the drag-suppression rule (D6).
- **Surface containment** — no new deps; `term-core` stays GPU-free.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the wheel routing, mouse-mode parsing and
encoding, reusing the existing `encode_seq`/`wheel_lines` helpers, tests, docs,
lint/format.

**🛑 STOP — call a human (escalate):**
- Any change to `GridSnapshot`'s shape.
- Any new dependency.
- Changing the alt-screen history freeze (scrollback D5) or primary-screen wheel
  semantics — the fix must be additive.
- Expanding into full click/drag/motion reporting beyond the approved scope.
- Weakening or deleting an existing test to make a gate pass.

## Exit criteria
Exit only when every gate passes and the Evidence Report is emitted. A 🛑 boundary
ends the loop in escalation, not completion.

---

## Lessons

### L1 — Read the sibling specs before proposing a scope
- cause: escalation
- source: human-approved
- form: gate3-item
- status: active
- check: "Does any option in this contract's scope re-decide something an existing `specs/*/spec.md` already resolved? Name the spec and decision id checked, or state that none touch this surface."
- note: this contract proposed alternate scroll as a recommended option; `terminal-scroll` D5 had already weighed and rejected it, and R5.1/R5.2 + `scrollback` D4 forbade it outright. The human approved a scope that silently reversed a shipped decision, because the proposal never checked. Grep the specs for the surface being touched BEFORE offering options.

### L2 — A mutation that only probes helpers is not a vacuity check
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "For each new behavior, was the mutation applied at the CALL SITE (the wiring), not only inside the pure helper it calls? If the wiring is untestable, say so explicitly instead of reporting the gate green."
- note: 9 mutation probes were reported as full coverage, but all 9 hit pure encoder/routing functions; deleting the call site itself broke nothing and Gate 2 was reported green when it was red.
