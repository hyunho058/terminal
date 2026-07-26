# Spec: terminal-scroll

## Meta
- **Created**: 2026-07-25
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-25

## Goal
Fix: the mouse wheel does nothing over the terminal grid — scrollback isn't wired to input. `term-core` already has the viewport (`Terminal::scroll_up/scroll_down/scroll_to_bottom`, `Grid::view_offset`, and `GridSnapshot` composes history+live from `view_offset` — see `crates/term-core/tests/scrollback.rs`), but `crates/app` never calls it. In `terminal_app.rs` the `MouseWheel` arm (~line 1047) routes only to the sidebar via `wheel_scroll`, and `input.rs:119` returns the scroll unchanged when the cursor is over the grid. Wire it: when `hit_test` is `None` (cursor over grid), translate the wheel delta to a line count and call `terminal.scroll_up/down(n)`, then `redraw()`; jump back to live (`scroll_to_bottom()`) on keystroke. Keep it app-only — don't touch the dependency direction. Add tests.

## Non-goals
- Any change to `term-core` or `renderer` — the viewport API (`scroll_up`/`scroll_down`/`scroll_to_bottom`/`view_offset`) and `GridSnapshot`'s history+live composition already exist and stay untouched (SD2 dependency direction).
- **Unconditional snap-to-live on new PTY output.** Rejected at the L0 gate and re-affirmed after the L2 steelman: `scrollback` D2 deliberately *pins* the viewport when output arrives while scrolled back, and named auto-follow-on-output "the tmux pain point". A deliberate scroll is never interrupted by the shell printing. (A bounded exception for near-bottom offsets **is** in scope — see D12.)
- Keyboard scroll chords (Shift+PageUp/PageDown, Cmd+Home/End) — a separate binding feature.
- Scroll-position indicator, scrollbar, or any new rendered chrome (the renderer already receives `view_offset`; drawing an affordance is out of scope).
- Search-in-scrollback, reflow-on-resize of history, alternate-screen semantics beyond what `scrollback` D5 already shipped.
- Forwarding wheel events to the shell as mouse-report or arrow-key sequences (scroll stays display-only per `scrollback` D4).

## Confirmed Goal
A wheel event whose cursor is over the terminal grid (sidebar `hit_test` → `None`) scrolls that tab's retained scrollback: the delta is translated to a whole-line count — for both winit `LineDelta` and trackpad `PixelDelta`, with fractional remainder accumulated across events so slow trackpad scrolling is never silently dropped — and drives `Terminal::scroll_up(n)`/`scroll_down(n)` on the **active tab**, followed by a redraw. The view returns to live (`scroll_to_bottom()`) whenever user input reaches that tab's shell — by any route: an encoded keystroke, an IME commit, or a paste. New output snaps the view only when it was already within one notch of the bottom (a stray-scroll guard, D12); a deliberate scroll stays pinned. A wheel over the sidebar keeps scrolling only the sidebar, exactly as today. All changes live in `crates/app`; `term-core` and `renderer` are unmodified. Verified by unit tests on the pure delta→lines translation and headless tests on the resulting `view_offset` transitions, with `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` green.

## Research

**The gap — everything below the app already works**
- `Terminal` exposes the full viewport API and nothing in `crates/app` calls any of it: `scroll_up`/`scroll_down`/`scroll_to_bottom`/`view_offset`/`at_bottom`/`at_top`/`history_len` (`crates/term-core/src/lib.rs:79-114`), all thin delegates to `Grid` (`crates/term-core/src/grid.rs:399-424`). A repo-wide grep for these names hits only `term-core` sources and `term-core` tests — zero hits under `crates/app`.
- `GridSnapshot::from_grid` already composes the visible window from history + live rows for the current offset and carries `view_offset`/`at_bottom`/`at_top` (`crates/term-core/src/snapshot.rs:38-81`). So a changed `view_offset` renders correctly with **no renderer change** — the only thing missing is the call.
- Behavior is already pinned headlessly in `crates/term-core/tests/scrollback.rs` (clamping at 43-60, `scroll_down`/`scroll_to_bottom` at 67-84, auto-increment pinning at 91, `usize::MAX/2` overflow safety at 121).
- The predecessor spec `specs/scrollback/spec.md:23` names this work explicitly as out of its own scope ("the app binds scroll keys") and again at :190 under **Post-work** — this feature is that post-work.

**The two call sites named in the goal**
- `crates/app/src/terminal_app.rs:1047-1061` — the `MouseWheel` arm converts `MouseScrollDelta::{LineDelta, PixelDelta}` into the app's `WheelDelta`, calls `wheel_scroll`, and on change updates `inner.scroll` + `publish_scroll()` + `refresh_hover()` + `window.request_redraw()` (a bare request — "sidebar-only change (D7)", deliberately *not* `redraw()`).
- `crates/app/src/input.rs:111-127` — `wheel_scroll` early-returns the scroll unchanged when `sidebar_layout::hit_test(...)` is `None` (line 119, "over the grid — do not move an invisible/other list"). That `None` branch is the exact hook the grid path attaches to.

**Hit-test semantics that decide the gate**
- `sidebar_layout::hit_test` (`crates/renderer/src/sidebar_layout.rs:408-446`) returns `None` in exactly two cases: **collapsed** and outside the small restore-button rect (so the whole window is grid), or **expanded** and `x >= sidebar_w` (or outside the viewport). Every in-sidebar point resolves to `Card`/`WaitingRow`/`Chrome`/`Panel` — dead space is swallowed as `Panel`, never leaked as `None`.
- `hit_test → None` is a *wider* region than `cursor_cell → Some` — `pixel_to_cell` (`crates/app/src/input.rs:276-294`) also rejects the top band above `top_inset`. Gating on `hit_test` (as the goal says) means a wheel in the top band still scrolls the grid; gating on `cursor_cell` would make that band inert.

**Existing app idioms to copy**
- Pure translation lives in `input.rs` and is unit-tested without a window: `wheel_scroll` + its test at `input.rs:812-827`, `sidebar_press`/`sidebar_hover` (`:74-105`), `pixel_to_cell` + tests (`:276-335`). The `WheelDelta` enum (`:63-69`) already abstracts winit's two unit systems for exactly this reason.
- Event-loop *decisions* get extracted as free functions and tested in `terminal_app.rs`'s own `mod tests` — `next_composition_state`/`commit_target` at `terminal_app.rs:1227-1376`. `crates/app` has **no `tests/` directory**; every app test is an inline `#[cfg(test)] mod tests`.
- `Running::redraw()` (`terminal_app.rs:329-336`) is the "real content change" entry point — it sets `snapshot_dirty` so `RedrawRequested` re-clones the grid. A grid-scroll changes content, so it needs `redraw()`, **not** the bare `window.request_redraw()` the sidebar path uses.
- Geometry inputs are available on the renderer: `renderer.cell_size() -> (f32, f32)` in physical px, plus `left_inset()`/`top_inset()` (used at `terminal_app.rs:463-474`, `770-780`).

**Keystroke snap point**
- `Running::on_key` (`terminal_app.rs:784-839`) dispatches in four steps: overlays → registered handlers → tab chords → default `encode_key` + PTY write. Only step 4 is "a key that goes to the shell"; steps 1-3 (Cmd+C copy, ai-monitor toggle, Cmd+T/W/[/]) return early and must not snap.
- PTY output arrives on `UserEvent::PtyOutput` (`terminal_app.rs:861-903`), which drains and calls `redraw()` when the tab is active. Per the L0 gate this path stays untouched — `scrollback` D2's auto-increment keeps the viewport pinned.

**Constraints found in the codebase**
- The app seam (`crates/app/src/seam.rs:1-18, 108-112`) is **keyboard-only** — `InputHandler::on_key`/`Overlay` take a `KeyInput`; there is no pointer/wheel registration. So a wheel feature cannot route through SD4's seam and must extend the `MouseWheel` arm directly, the same sanctioned exception the sidebar and drag-selection paths already take (`terminal_app.rs:148-155` calls drag-selection "the sanctioned event-loop exception").
- No scroll-related config key exists (`crates/common/src/config.rs:19-73`: font/theme/grid/sidebar only). Adding one would touch `common`, which is outside this feature's app-only scope.
- Selection and images are populated only at `view_offset == 0` (`snapshot.rs:63-71`), so scrolling into history hides an existing selection without any app-side bookkeeping.
- Per-tab state comes free: each tab owns its own `Terminal` (`inner.tabs.active_mut().terminal`, e.g. `terminal_app.rs:1042`), so `view_offset` is already per-tab.
- Build/test/lint: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` (`CLAUDE.md`); a `rustfmt` PostToolUse hook formats edited `*.rs`.

## Decisions

### D1: Route the wheel by the sibling's `hit_test`, not by `cursor_cell`
- **Status**: assumed (auto-resolved from L1 + the stated goal)
- **Rationale**: The grid path attaches to the exact `None` branch `wheel_scroll` already early-returns on (`input.rs:119`) — one ordered hit-test owns the routing and the app performs no geometry of its own (the sidebar feature's SD2/D1 rule). Rejected: gating on `x >= sidebar_w` (duplicates layout knowledge the renderer owns, and is wrong when collapsed, where `hit_test` returns `None` for everything but the restore button); gating on `cursor_cell().is_some()` (`pixel_to_cell` additionally rejects the band above `top_inset`, so a wheel in the top band would be silently inert — a dead zone with no visible cause).

### D2: One notch = 3 lines; trackpad pixels = px ÷ cell height; sign follows winit
- **Status**: resolved
- **Rationale**: `MouseScrollDelta::LineDelta(_, y)` → `y × 3` lines, matching Terminal.app/iTerm2's default feel so the terminal doesn't read as sluggish next to its neighbors. `PixelDelta(p)` → `p.y ÷ cell_h` (from `renderer.cell_size()`, physical px — the same units `last_cursor` and the insets already use), so a trackpad drag moves the text exactly as far as the finger moved. Positive y scrolls **into history** (`scroll_up`), following winit's convention that positive means "content moves down, revealing content above" — the same sign the sidebar's `wheel_scroll` follows (`input.rs:122-126`); macOS applies its own natural-scrolling inversion upstream, so no `#[cfg]` or preference read is needed. Rejected: 1 line/notch (precise but sluggish vs. every other macOS terminal), 5 (coarse), and any extra trackpad acceleration multiplier (unpredictable, and macOS already shapes the pixel stream).

### D3: One signed `f32` line-accumulator on `Running`, reset on tab switch only
- **Status**: resolved
- **Rationale**: A slow trackpad drag delivers a few px per event; truncating each event independently would round every one of them to 0 lines and the terminal would appear not to scroll at all. So the app keeps `wheel_accum: f32`, adds the translated delta, emits `trunc()` whole lines, and retains the fraction. **Both `WheelDelta` variants feed this one accumulator, and its unit is lines** — D2's translation (`×3` / `÷cell_h`) happens *before* accumulation, so a mouse and a trackpad can even alternate mid-gesture without a discontinuity; there is deliberately not one accumulator per variant. Direction reversal needs no special handling — it is self-correcting (an unspent `+0.6` cancelled by a later `-0.6` nets zero, which is the physically right answer). **A tab switch is the only thing that resets it** — snaps (D4/D12) deliberately do not — and it is **not** fed at all while the wheel is ignored (D6). The tab-switch reset is load-bearing, not hygiene: without it an unspent `+0.9` from tab A makes the very first small wheel event in tab B jump a full line, and since the accumulator exists precisely because sub-line residue is perceptible, "the residue is under a line so it can't matter" would be self-refuting. That same argument is why **snaps must not reset it**, which the first cut got wrong: the accumulator belongs to an in-flight wheel gesture over the *active* grid, while a snap can be triggered by an invisible tab (D12's output snap, D4's tab-routed IME commit) — so "reset on snap" let a background tab's async output silently eat 0.7 lines out of a gesture the user was making somewhere else, non-deterministically. One rule with no active/background branching: a tab switch ends a gesture; nothing else does. The cost is a sub-line fraction surviving a snap, which is bounded under one line by construction and completes the very next wheel event on the same grid. Rejected: rounding each event independently (kills trackpad scrolling outright), an integer accumulator (same truncation one layer down), per-tab accumulators (state for something one reset handles), resetting on snap only when the snapped tab is active (a second rule to keep in sync for no perceptible gain).

### D4: Snap to live at the PTY-write boundary — every route user input takes to the shell
- **Status**: resolved
- **Rationale**: The trigger is defined as *"bytes originating from user input reached this tab's shell"*, and is detected where those bytes are written, **not** by classifying keystrokes. Classifying was the first cut and it was wrong in two ways the L2 review caught: (a) `Ime::Commit` writes at `terminal_app.rs:371`/`:383` from a **different `WindowEvent` arm** that `on_key` never sees — and the composing gate at `:1101` plus winit's `had_ime_input` suppression mean there is no fallback — so every Hangul/CJK user would type into a frozen screen with the terminal looking dead; (b) Cmd+V paste writes from a **step-2** registered handler (`clipboard.rs:54`/`:76`), which a "step 4 only" rule excludes even though pasting a command is the clearest possible case of input heading for the shell. Anchoring at the write boundary makes those fall out for free and makes future input features inherit the behavior instead of having to remember it. Concretely: the active tab's `Session` writer is handed to the seam wrapped in a small `Write` probe, so `on_key` can ask afterward whether the dispatch wrote anything — **no change to `clipboard.rs` or any other handler**; `on_key` step 4 and both `Ime::Commit` arms snap the tab they wrote to (the IME arm's existing tab-identity routing decides which, so a commit that lands in a background tab snaps *that* tab, not the active one). Keys that never reach the shell still never snap: Cmd+C (copy), an ai-monitor toggle, Cmd+T/W/[/] chords, and modifier-only presses all leave a scrolled view standing — those are things a user does *while reading history*. The snap is suppressed while a drag is in flight and re-evaluated on release (D6); it does **not** touch the wheel accumulator (D3).

  **The invariant this rests on, stated because the probe cannot enforce it:** `AppCtx.writer` carries **user-originated bytes only** — bytes the user expects to see echoed. The probe's predicate is "bytes reached the PTY", which is a proxy for "the user typed something", and the two come apart the moment a handler writes for a non-echo reason (a shell-integration query, a bracketed-paste-mode probe, an OSC cwd query): that handler would inherit a snap that yanks the reader out of history with nothing at the call site to hint at it. This is a genuine asymmetry against the probe — an explicit per-handler flag fails *safe* (a forgotten flag means a missing snap, which someone reports), while the probe fails *unsafe* (an unrelated write silently destroys the reading position, and the jump is untraceable to its cause). It is chosen anyway because the failure it prevents is **certain today** — IME commit and paste are two shipped paths that a classification rule provably misses — while the failure it risks is hypothetical and has a named, cheap remedy: the first handler that needs a non-echo write adds an `AppCtx::write_silent()` accessor that bypasses the probe, rather than reaching for `writer`. The invariant is documented on `AppCtx::writer` and is what C7 makes reviewable; the escape hatch is deliberately not built now (it would be dead code in a crate whose seam already carries an `#![allow(dead_code)]`). Rejected: snapping on any `KeyboardInput` (Cmd+C while reading history would jump the view away); snapping inside `Session::write` (`session.rs:134` — misses the paste path, which writes through the raw `writer` field the seam exposes); a `wrote_to_shell` flag every handler must remember to set (fails safe, but leaves the two certain misses unfixed unless every present *and* future handler remembers).

### D5: Under the alternate screen the wheel scrolls the frozen primary history — with a known splice
- **Status**: resolved
- **Rationale**: This needs no app-side branch — `scrollback` D5 already shipped it: while `?1049h` is active no alt-screen frames enter history, and `scroll_up` reveals the primary-screen history captured before tmux/vim/less took over. Wiring the wheel to the same `scroll_up` delivers the win over Warp (where wheel-under-tmux did nothing at all) without new code. **What it does not deliver is a clean frame at every offset**, and the rationale should not pretend otherwise: `alt_screen` gates only history *retention* (`grid.rs:380`) — there is no separate alt-screen cell buffer — so at `0 < view_offset < rows` the snapshot composes `view_offset` rows of frozen history stacked on `rows - view_offset` rows of the *live* alt screen, whose bottom half keeps repainting (tmux's status bar, vim's cursor). Only `view_offset >= rows` gives an unspliced history view, and that is unreachable when history is shorter than the grid (`at_top` clamps at `history.len()`, `grid.rs:419-421`). This is inherited term-core composition behavior, not something this feature introduces or may fix (C1); it is accepted and recorded in Known Gaps. Rejected: translating the wheel to Up/Down escape sequences for the foreground program (iTerm2's `alternateScrollMode`) — it writes to the PTY, which `scrollback` D4 forbids outright, and makes the wheel's meaning depend on an invisible mode; making the wheel inert under alt-screen (discards the reachable primary history D5 deliberately preserved); suppressing the wheel below `view_offset >= rows` to dodge the splice (a dead zone with no visible cause, and it would block reaching the clean view at all).

### D6: No viewport change from **any** trigger while a drag is in flight — with a latched re-arm on release
- **Status**: resolved
- **Rationale**: While `Running::dragging` is true, both the wheel (D2) and the snaps (D4/D12) leave `view_offset` alone. The guard is stated over the *state change*, not over the wheel, because the review found a second door into the same bug: `CursorMoved` resolves drag focus with `cursor_cell()` (viewport coords, `terminal_app.rs:770-780`) while `Terminal::set_selection` stores live-grid coords, so **any** offset change mid-drag silently decouples "the row under my finger" from "the row I'm selecting" — and a snap-on-keystroke (scroll back → press-drag → type) reaches that state just as surely as a wheel does. It costs nothing perceptible during the drag: a selection isn't even rendered at `view_offset != 0` (`snapshot.rs:63-71`).
  **Suppression alone is a trap, so a drag records what it swallowed and replays the verdict on release.** `scrollback` D2's auto-increment keeps running inside the suppressed window (`grid.rs:378-385`), so a plain suppress leaves the snaps **permanently disarmed** — for D12, in exactly the compounded scenario it exists for: a stray brush parks the view at `view_offset == 1` (indistinguishable from live, which is why the user doesn't know); the user tries to select text; nothing highlights because selections don't render off-bottom; output streams; the offset climbs past `SNAP_FLOOR`; the drag ends and the terminal is silently frozen for good.
  A latch that only re-armed D12 would still lose D4, because **the two rules are not interchangeable and the gap is one-directional**: D4's snap has no floor (any user bytes reaching the shell snap, from *any* offset) while D12's is gated at `<= SNAP_FLOOR`, so replaying D12's verdict is strictly narrower and can never stand in for D4's. Concretely, scroll back 100 lines → press-drag → type a command → release would leave the view at 100 with the command already sent to zsh: D4's own "typed into a frozen screen, terminal looks dead" failure, re-entered through the drag door.
  So a drag records **which tab** deferred a snap, and the release replays it:
  - `advanced_while_dragging: Option<TabId>` — the tab whose output snap was swallowed (the per-event flags are otherwise consumed and discarded inside the window).
  - `input_while_dragging: Option<TabId>` — the tab whose D4 write-boundary snap was swallowed.

  On release: `input_while_dragging` → snap that tab (D4 first, and unconditional — it has no floor, so a floor-gated rule could never stand in for it); else `advanced_while_dragging` → snap that tab; else leave the viewport alone.

  **A deferred snap is replayed, never re-decided.** Two properties fall out of that, both learned the hard way at the verification gate:

  1. **Each field carries a `TabId`, not a bare flag.** Both deferred rules are per-tab at their source — D12 snaps "whichever tab received the output" (R9.4), D4 snaps the tab an `Ime::Commit` routed to — so a tab-blind replay fails twice at once: the active tab is yanked to live having earned nothing, *and* the tab that actually earned it drifts past `SNAP_FLOOR`, i.e. the exact disarm this latch prevents, reproduced one tab over.
  2. **There is no press-time floor bit.** D12's floor is applied **once**, at the source, against the receiving tab's own pre-drain offset, so anything reaching the latch has *already qualified*. Re-testing it on release could only ever subtract — and subtract using the *dragged* tab's position, discarding a background tab's qualified snap whenever the dragged tab happened to sit far from the bottom. The no-trigger cases need no conjunct because they never latch: a quiet drag at `view_offset == 2` raises no snap at all (R6.4), and output during a drag that began 50 lines up fails the source gate (R6.5).

  Both were real defects in earlier cuts; the second was introduced by the fix for the first. Reachable by scrolling tab B back to read a build error, switching to A, and drag-selecting in A while B keeps printing. Last swallowed wins if two tabs defer inside one drag — vanishingly rare, and either answer is defensible.

  The verdict is a pure function returning `Option<TabId>`, unit-tested directly in the idiom of `commit_target` (`terminal_app.rs:1227-1376`). Rejected: scroll-and-keep-dragging (needs a viewport↔history coordinate mapping and a selection model spanning history — out of scope), scroll-and-cancel-the-drag (destroys work on a stray trackpad brush), suppress-without-re-arm (permanently disarms both snaps), re-arming D12 only (silently drops D4), bare-bool latching (drops the tab identity), and keeping a press-time floor bit (double-applies the floor, on the wrong tab).

### D7: Apply, then compare `view_offset` — redraw only on a real change
- **Status**: assumed (mirrors the sidebar's existing `if new_scroll != inner.scroll` guard)
- **Rationale**: `scroll_up`/`scroll_down` clamp internally, so the app reads `view_offset()` before and after and calls `redraw()` only when it moved. Wheeling at the top of history or at the live bottom is therefore free — no snapshot re-clone, no frame. The call is `Running::redraw()`, **not** the bare `window.request_redraw()` the sidebar arm uses: a viewport change is a real content change and must mark `snapshot_dirty`, or the idle path re-presents the stale cached snapshot and the screen never moves (`terminal_app.rs:324-336`). The same before/after guard governs the D4 snap.

### D8: Pure `wheel_lines` in `input.rs`; the `MouseWheel` arm stays a thin dispatch
- **Status**: assumed (auto-resolved from L1 idiom)
- **Rationale**: All the arithmetic worth testing — units, sign, accumulation, guards — goes in one pure function next to `wheel_scroll`/`pixel_to_cell`, unit-tested with no window and no GPU, exactly as `input.rs:812-827` and `:301-335` already do. The `MouseWheel` arm only routes: sidebar branch (unchanged) or grid branch (call, compare, redraw). This is the only workable shape — SD4's app seam is **keyboard-only** (`InputHandler::on_key` takes a `KeyInput`; there is no pointer registration, `seam.rs:108-112`), so a wheel feature cannot register through it and must extend the arm directly, the same sanctioned exception the sidebar and drag-selection paths already take. Rejected: adding a pointer channel to the seam (a much larger change to a shared surface for one consumer — worth doing when a second pointer feature appears, not now); putting the arithmetic inline in the arm (untestable without a window).

### D9: Reject non-finite deltas and non-positive cell geometry
- **Status**: resolved
- **Rationale**: A NaN or infinite `PixelDelta.y` added into the accumulator would poison it permanently — every later `trunc()` stays NaN and scrolling dies silently for the rest of the session (Rust's `as usize` on NaN saturates to 0 rather than panicking, so the failure is invisible). Likewise `cell_h <= 0.0` (possible before the first layout) would divide to infinity. Both are dropped at the top of `wheel_lines`, leaving the accumulator untouched — the same NaN-safety posture `sidebar_layout::clamp_scroll` already documents. Huge-but-finite deltas need no special case: the line count saturates on cast and `scroll_up`/`scroll_down` clamp (`grid.rs:399-406`, pinned by `scrollback.rs:121`'s `usize::MAX/2` test).

### D10: The notch constant is a `const` in `crates/app` — no config key
- **Status**: assumed
- **Rationale**: A `scroll.lines_per_notch` setting would have to live in `common::Config` (`config.rs:19-73` has font/theme/grid/sidebar only), which is outside this feature's app-only scope and would drag in the settings-reload path for a constant nobody has asked to change. Ships as a named `const` so promoting it to config later is a one-line move. Rejected: adding the config key now (scope creep across a crate boundary), a magic number inline (unnameable in tests).

### D11: A tab switch or window resize leaves each tab's `view_offset` untouched
- **Status**: assumed
- **Rationale**: Per-tab scroll position comes free — every tab owns its own `Terminal`, so switching away and back returns to where the user was reading. The *offset* needs no code; the one thing a tab switch does touch is the shared wheel accumulator, which is reset (D3) — so "no code at all" is true of the viewport, not of the feature. Resize is safe by construction rather than by handling: `Grid::resize` deliberately doesn't touch `view_offset` (`grid.rs:553-571`), and `GridSnapshot::from_grid` reads history cells through `.get(col).unwrap_or(blank_cell)` (`snapshot.rs:48-51`), so a resize while scrolled can neither panic nor read out of bounds — history rows just aren't reflowed to the new width. Rejected: snapping to live on tab switch (loses the reading position, the exact complaint this feature fixes) and on resize (papers over the unreflowed-history cosmetic with a behavior change; the reflow gap is already recorded in `specs/scrollback/spec.md:95` and belongs there, not here).

### D12: New output snaps to live only from within one notch of the bottom (`view_offset <= 3`)
- **Status**: resolved
- **Rationale**: A bounded exception to the inherited pin, added after the L2 steelman. tmux's pin is tolerable because copy-mode is *modal, visibly marked, and exits with `q`*; ours is non-modal, has no indicator (out of scope), and its only exit is input. So the cheap failure — one stray trackpad brush, or inertial residue after a momentum scroll settles — parks the view at `view_offset == 1`, which on a 40-row grid is **visually indistinguishable from live**, and the terminal is then silently frozen: a long `cargo build` looks hung, and the instinctive "press a key" recovery injects a stray byte into the command line. Snapping when the pre-existing offset is `<= SNAP_FLOOR` (3 lines — one wheel notch, D2) makes exactly that case self-healing while leaving every deliberate scroll of 4+ lines pinned, so the tmux win survives intact. **The offset must be sampled *before* the PTY chunk is drained**: `scrollback` D2's auto-increment fires in `feed_scroll` (`grid.rs:378-385`, called from `line_feed` at `:352`), so a viewport parked at 1 that receives 10 lines already reads 11 by the time `UserEvent::PtyOutput` finishes draining — testing after the drain would silently disable this rule for every burst larger than the floor. The sample sits inside the existing `by_id_mut(id).map(|s| …)` closure (`terminal_app.rs:869-878`), read immediately before `s.drain()`.
  **The trigger is the `advanced` flag that closure already returns** — i.e. the drain actually fed bytes through the parser — not the bare arrival of a `PtyOutput` wake. Naming it matters because the two differ in reachable cases: a chunk carrying only an OSC 133 shell-integration marker or an OSC 2 title set advances nothing on screen. Those still count as `advanced` (bytes were processed), so this rule accepts one benign false positive: a near-bottom viewport snaps on a title set. That is deliberate — the user is within one notch of the bottom, which is where they almost certainly meant to be, and the alternative (diffing `history_len` across the drain to detect a real scroll) buys a distinction no user can perceive at the cost of a second sampling point to keep correct. Applies to whichever tab received the output (per-tab state, no active-tab special case); only an active-tab change redraws, as today (`terminal_app.rs:884`). It does **not** touch the wheel accumulator (D3 — output is not a gesture boundary, and the accumulator may belong to a gesture on a different tab) and is suppressed mid-drag, where D6's latch re-arms it on release. Rejected: snapping on any output at any offset (every other macOS terminal does this, but it is the L0-rejected behavior and discards `scrollback` D2's whole point); no floor at all (ships the silent-freeze footgun); a one-screen floor (a deliberate 5-line scroll to re-read something would be yanked away — the floor must be small enough to mean "you didn't mean to be here").

## Constraints
- C1: `crates/term-core` and `crates/renderer` are not modified. The viewport API and `GridSnapshot` are consumed exactly as they ship (SD2 dependency direction; `.claude/rules/dependency-direction.md`).
- C2: Scrolling never writes to the PTY and never moves the shell cursor — display-only (`scrollback` D4).
- C3: A wheel event over the sidebar must behave exactly as today; `input.rs:812-827`'s existing test must pass unmodified.
- C4: No new crate dependencies (`.claude/rules/no-gui-crates.md`), no `#[cfg(target_os)]` branches (`.claude/rules/macos-only.md`).
- C5: All new logic is unit-testable without a window or GPU — pure functions in `input.rs`, plus any extracted decision function in `terminal_app.rs`'s inline `mod tests`.
- C6: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` stay green; no `unwrap()` outside tests.
- C7: No sibling feature's module is edited to make the snap work — `clipboard.rs`, `overlay.rs` and any future handler inherit it from the write-boundary probe (D4). Changes are confined to `input.rs`, `terminal_app.rs`, and the seam's writer plumbing. The reviewable half of this: `AppCtx::writer` is documented **user-originated bytes only**, so a handler needing a non-echo write must add `write_silent()` rather than reuse `writer`.
- C8: The pre-drain `view_offset` sample (D12) must not be defeated by `scrollback` D2's auto-increment in `feed_scroll` (`grid.rs:378-385`) — the value is read before `Session::drain`, never after. Likewise D6's `near_bottom_at_press` is latched at press time, never re-read from the drifted offset.

## Known Gaps
- History rows are not reflowed when the column count changes, so scrolling back past a resize shows history at the old wrap width. Pre-existing and owned by `specs/scrollback/spec.md:95`, not introduced here.
- **Alt-screen splice (D5):** at `0 < view_offset < rows` under `?1049h`, the frame is frozen history on top of a still-repainting live alt screen. Inherited `GridSnapshot` composition behavior; unfixable from `crates/app` (C1) and out of scope for this feature. A term-core follow-up would have to give the alt screen its own buffer or blank the live portion while scrolled.
- No visible affordance tells the user they are scrolled back (no scrollbar, no "jump to bottom" indicator). D12's floor removes the *silent-freeze* failure this would otherwise guard, but a deliberate scroll of 4+ lines is still unmarked. `GridSnapshot` already carries `view_offset`/`at_bottom`/`at_top` for whoever draws one; deliberately out of scope (Non-goals) since drawing it requires a renderer change.
- Lines-per-notch (D10) and `SNAP_FLOOR` (D12) are fixed constants — if either proves wrong in daily use, promoting them to `common::Config` is a follow-up feature.
- L2 provisional: none. L2 unresolved: none.

## Requirements

### R0: The wheel scrolls the terminal's scrollback (goal)

#### R0.1: Wheel up over the grid reveals prior output
- **Given**: A tab whose grid has 200 lines of history retained and `view_offset == 0`
- **When**: The user wheels up one notch with the cursor over the grid
- **Then**: `view_offset` becomes 3 and the next snapshot's top 3 rows are history rows

#### R0.2: Wheel down returns to the live view
- **Given**: The same tab now at `view_offset == 3`
- **When**: The user wheels down one notch over the grid
- **Then**: `view_offset` returns to 0 and the snapshot equals the live view

### R1: Routing — the wheel goes to exactly one of grid or sidebar (D1)

#### R1.1: A wheel over the grid moves the grid, not the sidebar list
- **Given**: An expanded sidebar of width `w`, cursor at `x > w` over the grid, list `scroll == 40.0`
- **When**: A wheel event arrives
- **Then**: The active tab's `view_offset` changes and `Running::scroll` stays `40.0`

#### R1.2: A wheel over the sidebar moves the list, not the grid
- **Given**: Cursor inside the sidebar band (`hit_test` → `Card`/`Panel`/`WaitingRow`/`Chrome`), `view_offset == 0`
- **When**: A wheel event arrives
- **Then**: `Running::scroll` changes exactly as it does today and `view_offset` stays `0` — `input.rs:812-827`'s existing test passes unmodified (C3)

#### R1.3: With the sidebar collapsed the whole window is grid
- **Given**: `collapsed == true`, cursor anywhere outside the restore-button rect
- **When**: A wheel event arrives
- **Then**: The grid scrolls (`hit_test` → `None` there)

#### R1.4: The band above the grid still scrolls the grid
- **Given**: Cursor at `x > sidebar_w` but `y < top_inset` — `hit_test` → `None` while `pixel_to_cell` → `None`
- **When**: A wheel event arrives
- **Then**: The grid scrolls (the D1 discriminator: gating on `cursor_cell` would make this band inert)

### R2: Delta → line-count translation (D2)

#### R2.1: One mouse notch is three lines
- **Given**: `WheelDelta::Lines(1.0)` and a zeroed accumulator
- **When**: The delta is translated
- **Then**: The result is `+3` lines (`LINES_PER_NOTCH`)

#### R2.2: A trackpad drag of two cell-heights is two lines
- **Given**: `WheelDelta::Pixels(32.0)` with `cell_h == 16.0`
- **When**: The delta is translated
- **Then**: The result is `+2` lines (px ÷ cell height, 1:1 with the finger)

#### R2.3: Sign follows winit — positive scrolls into history
- **Given**: `view_offset == 5` with ≥8 rows of history
- **When**: A positive-y delta of one notch arrives, then a negative-y delta of one notch
- **Then**: `view_offset` goes `5 → 8 → 5` (positive = `scroll_up`, negative = `scroll_down`)

### R3: Sub-line accumulation (D3)

#### R3.1: Sub-line events accumulate instead of rounding away
- **Given**: `cell_h == 16.0` and a zeroed accumulator
- **When**: Three `Pixels(6.0)` events arrive (0.375 lines each)
- **Then**: The first two move 0 lines, the third moves 1 line — total 1, not 0

#### R3.2: The remainder survives between events
- **Given**: The sequence above has left `+0.125` unspent
- **When**: A `Pixels(14.0)` event arrives (0.875 lines)
- **Then**: Exactly 1 line moves (0.125 + 0.875), leaving `0.0`

#### R3.3: A reversal cancels unspent residue
- **Given**: `+0.6` lines unspent
- **When**: A `-0.6`-line delta arrives
- **Then**: 0 lines move and the accumulator is `0.0` (net-zero is the physically correct answer)

#### R3.4: A tab switch resets the accumulator
- **Given**: `+0.9` lines unspent from a gesture on tab A
- **When**: The user switches to tab B and a `Pixels` event worth 0.2 lines arrives
- **Then**: 0 lines move in tab B (no inherited `+0.9` jump)

#### R3.5: A snap does NOT reset the accumulator
- **Given**: `+0.7` lines unspent, and a snap-to-live has just fired (from D4 or D12, possibly for a background tab)
- **When**: A `Pixels` event worth 0.4 lines arrives over the active grid
- **Then**: 1 line moves — the in-flight gesture keeps its residue; an invisible tab's async output never eats it

### R4: Snap to live when user input reaches the shell (D4)

#### R4.1: An encoded keystroke snaps
- **Given**: Active tab at `view_offset == 20`
- **When**: The user presses `a`, and `encode_key` writes bytes to that tab's PTY
- **Then**: `view_offset` becomes 0 before the echo arrives

#### R4.2: An IME commit snaps the tab it was routed to
- **Given**: A composition began in tab A (`composing_tab == A`), A is at `view_offset == 12`, and the user has since switched to tab B
- **When**: `Ime::Commit("한")` arrives and is written to A via the existing tab-identity routing
- **Then**: **A**'s `view_offset` becomes 0 (B's is untouched) — the path `on_key` never sees

#### R4.3: A paste snaps
- **Given**: Active tab at `view_offset == 7`, clipboard holds `ls -la`
- **When**: Cmd+V dispatches to the step-2 `ClipboardHandler`, which writes through `AppCtx::writer`
- **Then**: `view_offset` becomes 0 — detected by the write probe, with `clipboard.rs` unmodified

#### R4.4: Keys that never reach the shell do not snap
- **Given**: Active tab at `view_offset == 15`
- **When**: The user presses Cmd+C (copy), toggles the ai-monitor overlay, or presses Cmd+] (tab chord)
- **Then**: `view_offset` stays 15 in each case — reading history is not interrupted

#### R4.5: A modifier-only press does not snap
- **Given**: Active tab at `view_offset == 15`
- **When**: The user presses and holds Shift alone (no bytes produced)
- **Then**: `view_offset` stays 15

#### R4.6: The input snap has no floor
- **Given**: Active tab at `view_offset == 400`
- **When**: The user types a character that reaches the shell
- **Then**: `view_offset` becomes 0 (unlike D12's output snap, this rule is unconditional)

### R5: Alternate screen (D5)

#### R5.1: The wheel reveals frozen primary history under tmux
- **Given**: 100 primary-screen lines were printed, then a program sent `?1049h`
- **When**: The user wheels up over the grid
- **Then**: `view_offset` increases and the snapshot's top rows show the retained pre-alt-screen output

#### R5.2: Scrolling writes nothing to the PTY
- **Given**: Any tab, alt screen active or not
- **When**: Any number of wheel events scroll the viewport
- **Then**: Zero bytes are written toward the PTY and the grid cursor position is unchanged (C2)

### R6: Drag suppression and the latched re-arm (D6)

#### R6.1: A wheel during a drag is swallowed whole
- **Given**: `dragging == true`, `view_offset == 4`, accumulator `+0.5`
- **When**: A wheel event worth 2 lines arrives
- **Then**: `view_offset` stays 4 **and** the accumulator stays `+0.5` (a swallowed event must not be banked)

#### R6.2: Input swallowed during a drag snaps on release, at any offset
- **Given**: A drag began at `view_offset == 100`; during it the user typed a command that reached the shell (`input_while_dragging` latched)
- **When**: The button is released
- **Then**: `view_offset` becomes 0 — D4's unconditional rule is replayed, ignoring `SNAP_FLOOR`

#### R6.3: Near-bottom output swallowed during a drag snaps on release
- **Given**: A tab at `view_offset == 1` receives grid-advancing output during a drag, and D2's auto-increment drifts its offset past 3 before the button comes up
- **When**: The button is released
- **Then**: `view_offset` becomes 0 — the snap qualified at the source, pre-drain, and is replayed rather than re-decided against the drifted value

#### R6.4: A quiet drag near the bottom changes nothing
- **Given**: A drag began at `view_offset == 2` with no output and no input during it
- **When**: The button is released
- **Then**: `view_offset` stays 2 — nothing was ever latched, so there is no untriggered jump

#### R6.5: Output during a drag on a viewport far from the bottom changes nothing
- **Given**: A tab at `view_offset == 50` receives output during a drag; no input reached the shell
- **When**: The button is released
- **Then**: `view_offset` stays pinned — the source gate (`snap_on_output(50)` is false) never raises a snap, so nothing reaches the latch

#### R6.6: The release snaps the tab that earned it, not whoever is active
- **Given**: Tab B (inactive) sits at `view_offset == 1` and receives grid-advancing output while the user holds a drag in tab A — **at any offset, including one far from the bottom**
- **When**: The button is released
- **Then**: **B**'s `view_offset` becomes 0 and A's is untouched. Two ways this has failed: a tab-blind replay yanks A to live having earned nothing while B drifts past `SNAP_FLOOR`; and a press-time floor bit sampled from A discards B's already-qualified snap whenever A sits more than `SNAP_FLOOR` from the bottom. The same holds for a swallowed `Ime::Commit`, which snaps the tab its composition began in.

### R7: Redraw discipline (D7)

#### R7.1: A viewport change marks the snapshot stale
- **Given**: A wheel event that moves `view_offset`
- **When**: The arm finishes
- **Then**: `Running::redraw()` ran — `snapshot_dirty == true`, so `RedrawRequested` re-clones the grid (a bare `request_redraw()` would re-present the cached snapshot and the screen would never move)

#### R7.2: A no-op wheel costs nothing
- **Given**: `view_offset == 0` with empty history (or the viewport already at the oldest row)
- **When**: A wheel event arrives over the grid
- **Then**: `view_offset` is unchanged and no redraw is requested

### R8: Degenerate input safety (D9)

#### R8.1: A non-finite delta is dropped without poisoning the accumulator
- **Given**: Accumulator `+0.5`
- **When**: A `Pixels(f32::NAN)` (or infinite) event arrives, then a normal 1-line event
- **Then**: The NaN event moves 0 lines and leaves the accumulator `+0.5`; the following event moves 1 line (scrolling is not dead for the session)

#### R8.2: Non-positive cell geometry is dropped
- **Given**: `cell_h == 0.0` (before first layout)
- **When**: A `Pixels(40.0)` event arrives
- **Then**: 0 lines move, no division by zero, accumulator unchanged

#### R8.3: A huge delta clamps instead of panicking
- **Given**: History holds 12 rows
- **When**: A `Pixels(1e9)` event arrives
- **Then**: `view_offset` clamps to 12, `at_top == true`, no panic and no overflow

### R9: Output snaps only from within one notch of the bottom (D12)

#### R9.1: Near-bottom output snaps to live
- **Given**: A tab at `view_offset == 2` (`<= SNAP_FLOOR`)
- **When**: A PTY chunk arrives and advances the grid
- **Then**: `view_offset` becomes 0 — the stray-brush case self-heals

#### R9.2: A deliberate scroll stays pinned
- **Given**: A tab at `view_offset == 40`
- **When**: The shell prints 5 new lines
- **Then**: No snap occurs; `scrollback` D2's auto-increment pins the content (`view_offset == 45`, same rows on screen)

#### R9.3: The offset is sampled before the drain
- **Given**: A tab at `view_offset == 2`
- **When**: A single chunk containing 10 line feeds is drained
- **Then**: The snap fires — sampling after the drain would read 12, silently disabling this rule for every burst larger than the floor

#### R9.4: A background tab snaps its own viewport only
- **Given**: Tab B (inactive) at `view_offset == 1`; tab A active with an in-flight gesture holding `+0.7` unspent
- **When**: Output advances B's grid
- **Then**: B's `view_offset` becomes 0, A's viewport is untouched, the accumulator still holds `+0.7`, and no redraw is requested for the inactive tab's grid change

#### R9.5: Output at the live bottom is a no-op
- **Given**: A tab at `view_offset == 0`
- **When**: Output advances the grid
- **Then**: `view_offset` stays 0 and no extra redraw is requested beyond today's

### R10: Per-tab position and resize invariance (D11)

#### R10.1: Each tab keeps its own reading position
- **Given**: Tab A at `view_offset == 30`, tab B at `view_offset == 0`
- **When**: The user switches A → B → A
- **Then**: A is still at 30 and B still at 0

#### R10.2: A resize while scrolled neither panics nor drops the position
- **Given**: A tab at `view_offset == 25`, history captured at 120 columns
- **When**: The window is resized to 80 columns
- **Then**: `view_offset` is still 25, the snapshot builds without panic or out-of-bounds read (history rows show unreflowed, per Known Gaps)

### R11: Structure, testability, and blast radius (D8, D10, C1/C5/C7)

#### R11.1: The translation is a pure function
- **Given**: The delta→lines logic in `input.rs`
- **When**: `cargo test -p app` runs on a machine with no display and no GPU
- **Then**: R2/R3/R8's cases all execute against that function directly, constructing no winit event and no window

#### R11.2: The release verdict is a pure function
- **Given**: The two latched drag fields (`advanced_while_dragging`, `input_while_dragging`)
- **When**: The release verdict is computed
- **Then**: It is a free function returning `Option<TabId>`, unit-tested over all four present/absent combinations with **two distinct tab ids** so the table pins *which* tab comes back and not merely that one does, in the idiom of `commit_target`. A separate test pins that the "shouldn't snap" cases (R6.4/R6.5) are enforced by the **source gate refusing to latch** — so a `None` row can never be misread as "something qualified and was then discarded"

#### R11.3: No sibling handler is edited
- **Given**: The finished change
- **When**: `git diff --stat` is inspected
- **Then**: `clipboard.rs` and every other registered handler are unchanged; paste snaps via the probe alone (C7)

#### R11.4: The core crates are untouched
- **Given**: The finished change
- **When**: `git diff --stat crates/term-core crates/renderer` is inspected
- **Then**: It is empty — the viewport API and `GridSnapshot` are consumed exactly as they ship (C1)

#### R11.5: Tuning values are named constants in the app crate
- **Given**: The finished change
- **When**: `LINES_PER_NOTCH` and `SNAP_FLOOR` are located
- **Then**: Both are `const` items in `crates/app`, and `crates/common/src/config.rs` has no scroll key (D10)

## Tasks

The DAG is a **chain**, and deliberately so. Four of the five tasks edit
`crates/app/src/terminal_app.rs` — the `MouseWheel` arm, `on_key`, both `Ime::Commit`
arms, the `PtyOutput` arm, the press/release arms, and the `Running` struct — so
running any two concurrently would put two workers in the same file. The parallelism
rule's file-overlap condition fails for every pair; adding a false `parallel with`
here would buy nothing but merge conflicts. Execution pattern: **Sequential Pipeline**.

### T1: Wheel translation + grid routing — the feature working end-to-end [vertical]
- **Fulfills**: R0, R1, R2, R3.1-R3.4, R7, R8, R11.1, R11.5
- **Depends on**: (none)
- **Touches**: `crates/app/src/input.rs` (new `wheel_lines` + `LINES_PER_NOTCH`, tests), `crates/app/src/terminal_app.rs` (`MouseWheel` arm grid branch, `wheel_accum` field, tab-switch reset in `do_tab_action`)
- The whole user-visible fix lands here: pure delta→lines translation with units, sign, sub-line accumulation and the D9 guards, wired to `scroll_up`/`scroll_down` on the active tab behind the `hit_test → None` gate, with the before/after `view_offset` comparison driving `redraw()`. Ships alone as a working feature — every later task is a refinement of *when the view returns to live*.

### T2: Snap to live at the PTY-write boundary [vertical]
- **Fulfills**: R4, R11.3
- **Depends on**: T1
- **Touches**: `crates/app/src/seam.rs` (`WriteProbe`, the user-originated-bytes-only doc invariant on `AppCtx::writer`), `crates/app/src/terminal_app.rs` (`on_key` step 4, both `Ime::Commit` arms)
- The probe wraps the writer handed to the seam so `on_key` can ask afterward whether the dispatch wrote; `clipboard.rs` is not edited (R11.3). Both IME commit arms snap the tab they wrote to, using the existing `commit_target` routing.

### T3: Output snap floor with pre-drain sampling [vertical]
- **Fulfills**: R9, R3.5
- **Depends on**: T2 (same file: the `PtyOutput` arm sits alongside T2's edits)
- **Touches**: `crates/app/src/terminal_app.rs` (`UserEvent::PtyOutput` arm, `SNAP_FLOOR`)
- The `view_offset` sample goes **inside** the existing `by_id_mut(id).map(|s| …)` closure, read immediately before `s.drain()` — after it, `feed_scroll`'s auto-increment has already moved the value (C8). Gated on the `advanced` flag that closure already returns. R3.5 is verified here because this is the first task where a snap can fire for a tab the user isn't looking at.

### T4: Drag suppression + the latched release verdict [vertical]
- **Fulfills**: R6, R11.2
- **Depends on**: T2, T3 (replays both snap rules; same file)
- **Touches**: `crates/app/src/terminal_app.rs` (press/release arms, `MouseWheel` guard, the three latch bits, the pure verdict function + its 8-case table test)
- The verdict is extracted as a free function so R6.2-R6.5 are table-driven over all eight bit combinations without a window, in the `commit_target` idiom.

### T5: Alt-screen, per-tab and blast-radius verification [vertical]
- **Fulfills**: R5, R10, R11.4
- **Depends on**: T4
- **Touches**: test modules in `crates/app/src/terminal_app.rs` and `crates/term-core/tests/` (read-only assertions against shipped behavior)
- Confirms the wiring didn't disturb what already worked: the wheel reveals frozen primary history under `?1049h`, scrolling writes zero PTY bytes, per-tab offsets survive switching, a resize while scrolled neither panics nor loses position, and `git diff --stat crates/term-core crates/renderer` is empty (C1).

## External Dependencies

### Pre-work
- (none) — no new crates, no config schema change, no migration.

### Post-work
- Eyeball the feel of the notch size and trackpad tracking in the real app (`/run-term`): 3 lines/notch and 1:1 pixel tracking are judgment calls that only daily use can confirm (D10 / Known Gaps).
- If `LINES_PER_NOTCH` or `SNAP_FLOOR` prove wrong in practice, promoting them to `common::Config` is a follow-up feature, not a fix to this one.
