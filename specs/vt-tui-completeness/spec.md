# Spec: vt-tui-completeness

## Meta
- **Created**: 2026-07-25
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-25

## Goal
Extend the VT parser (`crates/term-core/src/parser.rs`) so full-screen TUIs like
Claude Code render correctly — today they show overlapping/ghost text and their
internal scrolling does not work. Root cause: the CSI handler
(`parser.rs:375-432`) implements only cursor moves, `J`, `K`, `m`, and
alt-screen; every other final byte falls through `_ => {}` and is silently
dropped.

Implement the redraw sequences TUIs rely on: `CSI r` (DECSTBM scroll region) with
`feed_scroll` honoring the region, `S`/`T` (SU/SD), `L`/`M` (IL/DL), `@`/`P`
(ICH/DCH), `X` (ECH); fix `K` to honor its parameter (`0K`/`1K`/`2K` —
`parser.rs:428` ignores it and always clears cursor→EOL); and handle DECAWM
`?7h`/`?7l` (`grid.rs:322` always wraps), ideally with proper pending-wrap
(last-column deferred wrap). Add term-core tests following
`tests/grid_snapshot.rs` / `tests/scrollback.rs`. Keep the dependency direction —
all of this is term-core, GPU-free. Verify with `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`.

## Non-goals
- No renderer, app, or GPU changes. This feature lives entirely in `term-core`
  (`parser.rs` + `grid.rs`); `GridSnapshot` gains no new field.
- No new external dependencies.
- **DECTCEM (`?25h`/`?25l`, cursor visibility)** — deliberately deferred; it would
  require extending the `GridSnapshot` renderer contract, which is out of scope
  here. Filed as follow-up.
- ~~**DECSC/DECRC (`ESC 7`/`ESC 8`) and `CSI s`/`CSI u`** — deferred~~ →
  **brought into scope** after empirical evidence showed the deferral was unsafe
  once `CSI r` landed. See D17 and R10.
- **DECOM (origin mode, `?6h`/`?6l`)** — not implemented; cursor addressing stays
  absolute-to-screen.
- No cross-platform branches, no GUI crates, no shell other than local zsh
  (project rules `macos-only`, `no-gui-crates`, `zsh-local-only`,
  `dependency-direction`).
- Not a full VT100/xterm conformance pass — only the sequences a full-screen TUI
  needs to redraw and scroll correctly.

## Confirmed Goal
Make `term-core` render full-screen TUIs (Claude Code, vim, less) without ghost
or overlapping text, and make their internal scrolling work, by implementing the
dropped CSI redraw sequences plus the ESC-level index pair, with correct
interaction between the scroll region and scrollback history.

**In scope (confirmed with the user):**
1. `CSI r` — DECSTBM set/reset scroll region, with `feed_scroll` honoring it.
2. `CSI S` / `CSI T` — SU / SD (scroll up / down within the region).
3. `CSI L` / `CSI M` — IL / DL (insert / delete line, region-aware).
4. `CSI @` / `CSI P` — ICH / DCH (insert / delete character in the line).
5. `CSI X` — ECH (erase character, no shift).
6. `CSI K` — EL honoring its parameter: `0` cursor→EOL, `1` BOL→cursor, `2` whole
   line.
7. DECAWM `?7h` / `?7l` — autowrap mode, with pending-wrap (deferred last-column
   wrap) as the wrap model.
8. **`ESC M` (RI, reverse index) and `ESC D` (IND, index)** — added to scope at
   L0: the ESC-level partners of SU/SD. Without RI, region scrolling only works
   downward, so the DECSTBM work would be half-wired.

**Done when:**
- Every sequence above has term-core tests asserting its grid semantics, in the
  style of `crates/term-core/tests/grid_snapshot.rs` and `tests/scrollback.rs`.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace` are all green.
- The dependency direction is unchanged: no wgpu/winit reaches `term-core`.

## Research

### The gap — what is dropped today
- The CSI match arm covers only `H`/`f`, `A`/`B`/`C`/`D`, `G`, `d`, `J`, `K`, `m`;
  every other final byte falls through the catch-all `_ => {}`
  (`crates/term-core/src/parser.rs:375-432`, catch-all at `parser.rs:431`). So
  `r`, `S`, `T`, `L`, `M`, `@`, `P`, `X` are all parsed by `vte` and then thrown
  away — the ghost/overlap symptom.
- EL is parameter-blind: `'K' => self.grid.clear_line_from_cursor()`
  (`parser.rs:428`) always erases cursor→EOL, so `1K` and `2K` erase the wrong
  span (`Grid::clear_line_from_cursor`, `grid.rs:526-532`).
- The private-mode (`CSI ? … h/l`) arm handles `1049`/`1047`/`47` (alt-screen),
  `1` (DECCKM), `2004` (bracketed paste), and drops everything else at its own
  `_ => {}` (`parser.rs:351-365`) — that is where `?7` (DECAWM) is lost.
- `esc_dispatch` implements exactly one sequence, `ESC c` (a deliberately
  minimal RIS that only clears DECCKM + bracketed paste) — `ESC M` (RI) and
  `ESC D` (IND) are silently no-ops (`parser.rs:285-290`).

### The wrap model today
- `Grid::print` wraps unconditionally and immediately: `if self.cursor.col + w >
  self.cols { self.line_feed(); self.cursor.col = 0; }`
  (`grid.rs:315-343`, wrap at `grid.rs:322-325`). There is no `autowrap` flag and
  no deferred/pending-wrap state — the cursor never rests *on* the last column
  after a print, which is exactly the state TUIs assume.
- A double-width char writes a `Cell::CONTINUATION` spacer in the next cell
  (`grid.rs:334-341`, `Cell::CONTINUATION`/`is_continuation` at
  `grid.rs:105-121`). Any character-shifting op (ICH/DCH/ECH) has to reckon with
  leader+spacer pairs being split.

### Scrolling and history — the invariant the region must respect
- `line_feed` (`grid.rs:350-356`) delegates to `feed_scroll`
  (`grid.rs:378-394`) when at the bottom row. `feed_scroll` does four things:
  pushes the departing top row into `History` (primary screen only —
  `grid.rs:380`), auto-increments `view_offset` to pin a scrolled-up viewport,
  calls `images.scroll_up_one()` (`image.rs:72-77`), then shifts cells up and
  blanks the new bottom row.
- `History::push` is a bounded FIFO ring (`scrollback.rs:33-38`, cap
  `DEFAULT_HISTORY_LIMIT = 100_000` at `scrollback.rs:15`).
- **Consequence:** once a scroll region exists, a scroll confined to a *partial*
  region must not push rows into `History` or move image placements — otherwise
  every TUI frame floods scrollback. The existing alt-screen guard at
  `grid.rs:380` is the precedent for this shape of conditional.
- `alt_screen_does_not_grow_history` (`tests/scrollback.rs:129-150`) is the
  existing test that locks the analogous invariant.

### Where new state would live
- `Grid`'s field block (`grid.rs:136-185`) already carries terminal *modes*
  alongside cells: `alt_screen`, `app_cursor_keys`, `bracketed_paste`. Their
  documented pattern — mode state lives on `Grid`, is absent from `GridSnapshot`
  when the renderer does not read it — is the template for a scroll region and an
  autowrap flag. Initialization site: `Grid::new` (`grid.rs:203-226`).
- `Grid::resize` (`grid.rs:553-571`) clamps the cursor and drops the selection
  but knows nothing about a region; `Grid::clear` (`grid.rs:506-512`) homes the
  cursor and clears images. Both are decision points for region lifetime.
- `Grid::move_to` clamps to the full grid (`grid.rs:369-372`) — it does not
  consult a region (DECOM/origin mode is an explicit non-goal).
- The `Terminal` facade (`lib.rs:41-77`) needs no new method: `advance` →
  `Processor::advance` → `Grid` is the whole path.

### Parser mechanics available
- `vte 0.15.0` (`crates/term-core/Cargo.toml:17`). `Params::iter()` yields
  `&[u16]` subparam slices; the existing code reads `params.iter().next()
  .and_then(|p| p.first().copied())` for param 1 (`parser.rs:348`) and walks a
  shared iterator for multi-param sequences (`parser.rs:156-190`,
  `resolve_ext_color` at `parser.rs:215-252`). `CSI r` needs the same two-param
  read as `CUP` (`parser.rs:377-383`).
- Private-marker CSIs (`>`/`<`/`=`) are early-returned before the main match
  (`parser.rs:372-374`) — a guard added after a real bug where `CSI > 4 ; 2 m`
  was misparsed as SGR. New arms must sit after that guard.
- `unicode-width 0.2.0` (`Cargo.toml:16`) supplies `ch.width()` used by `print`.

### Test conventions to follow
- `crates/term-core/tests/grid_snapshot.rs` — `insta::assert_snapshot!` over
  `t.snapshot().to_text()`, driven through the `Terminal` facade. Best fit for
  *visual* region/scroll results. Accepted via `/insta-review`.
- `crates/term-core/tests/scrollback.rs` — plain assertions with `feed_lines` /
  `top_row` helpers, chosen because "the invariants are numeric/positional, not
  visual" (`tests/scrollback.rs:5-9`). Best fit for history-count and
  cursor-position invariants.
- `crates/term-core/src/parser.rs:469-868` — a large inline `#[cfg(test)] mod
  tests` with `grid()`, `row_text()` (`parser.rs:600-606`), `cur()`
  (`parser.rs:803-806`) helpers, asserting sequence-by-sequence semantics
  directly on `Grid`. This is the closest existing style for per-CSI coverage.
- `crates/term-core/src/grid.rs:579-663` — inline unit tests for `Grid` behavior
  (wide chars, backspace).

### Project constraints that bind this work
- `.claude/rules/dependency-direction.md` — `term-core` must never reach wgpu or
  winit; all of this is GPU-free by construction.
- `CLAUDE.md` — verification is `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`; no `unwrap()` outside
  tests; per-crate `thiserror` enums.
- A `PostToolUse` hook runs `rustfmt` on every edited `*.rs`; a `PreToolUse` hook
  blocks edits to `Cargo.lock`.
- Sibling specs live at `specs/<feature>/spec.md` (e.g. `specs/vt-color-attrs/`),
  several with a companion `loop.md`.

## Decisions

### D1: A partial scroll region never writes to scrollback history
- **Status**: resolved
- **Decision**: an op retains the departing row into `History` (auto-incrementing
  `view_offset` and calling `images.scroll_up_one()`) **only when the band it
  scrolls is the whole screen** — i.e. the active region spans the whole screen
  (`top == 0 && bottom == rows - 1`), the primary screen is active, and the rows
  leaving really do leave the screen. A partial region scrolls cells only.
- **Predicate, per op** (made precise after the Gate-3 review; the original
  blanket list of "SU/SD/IL/DL/RI/IND" was imprecise):
  - **LF / IND / SU** — retain when the region is full-screen. They scroll the
    whole region, so the departing row leaves the screen.
  - **DL** — retains **only when the cursor is on the region top**, because only
    then is its band `[cursor_row..=bottom]` the whole region. Below the top the
    band is a strict subset and nothing leaves the screen. Real terminals agree:
    xterm deletes against `saveBuf` at `cur_row == 0`, and alacritty grows the
    scroll limit whenever the scrolled band starts at row 0. Omitting this was a
    **scrollback data-loss bug** — `ESC[1M` and `ESC[1S` produced byte-identical
    grids but different history.
  - **IL / SD / RI** — never retain. They are *down*-scrolls: no row departs the
    top, so there is nothing to retain.
- **Accepted consequence.** A TUI that repeatedly deletes lines at row 0 with no
  region set now grows scrollback one row per delete — e.g. 100 × `ESC[1M` retires
  100 rows. That is intended and matches both SU and real terminals: those rows
  really did leave the screen. The alternative (silently discarding them) is the
  data-loss bug this predicate fixes.
- **Rationale**: A streaming TUI scrolls its message pane hundreds of times per
  response; pushing each departing region row would fill the 100k-row ring
  (`scrollback.rs:15`) with frame garbage and make Cmd-scroll useless during a
  session. This matches xterm/iTerm2. **Rejected:** "every region scroll pushes"
  — one simpler code path, but it destroys the scrollback feature that shipped in
  PR-level work already on `main`. The conditional has direct precedent: the
  existing `if !self.alt_screen` guard at `grid.rs:380` wraps exactly the same
  three statements.

### D2: The scroll region resets on both alt-screen enter and leave
- **Status**: resolved
- **Decision**: `set_alt_screen(_)` resets the region to full-screen on **both**
  `?1049h` and `?1049l` (and the `?1047`/`?47` aliases).
- **Rationale**: A TUI killed mid-run (crash, SIGKILL) never sends `CSI r` to
  restore full-screen margins. Without this, zsh comes back and scrolls forever
  inside rows 3-20 — a permanent, `reset`-only-recoverable version of the exact
  ghost-text bug this feature exists to kill. **Rejected:** strict VT persistence
  ("region is screen state, survives until `CSI r` or RIS") — correct on paper,
  but this codebase's alt-screen is a history-preserving *switch*, not a second
  buffer (`grid.rs:426-431`), so there is no per-buffer margin state to preserve
  and nothing else clears the flag. **Rejected:** reset-only-on-RIS — leaves the
  crash case broken, and `ESC c` here is deliberately a minimal
  non-clearing RIS (`parser.rs:269-290`).

### D3: Full pending-wrap (deferred last-column wrap), not just a DECAWM flag
- **Status**: resolved
- **Decision**: printing into the last column leaves the cursor **on** that column
  with a `pending_wrap` flag set; the wrap to the next row happens on the next
  printable character. `CSI K` at that moment erases only the last column.
  DECAWM `?7l` disables wrapping entirely (printing at the last column overwrites
  in place and never sets the flag); `?7h` re-enables it.
- **Rationale**: `grid.rs:322-325` wraps eagerly, so the cursor can never rest on
  the last column — a full-width line therefore consumes an extra blank row and
  every box-drawing TUI is off by one row from the first frame. TUIs assume the
  DEC pending-wrap model. **Rejected:** "DECAWM flag only, keep immediate wrap" —
  a much smaller diff, but it leaves the row-eating bug in place. **Accepted cost:** `print`, `carriage_return`,
  `line_feed`, `backspace`, `tab`, and `move_to` all become pending-wrap-aware, and
  the existing wrap tests (`tests/grid_snapshot.rs:66-71`, `grid.rs:612-625`) must
  be re-verified rather than assumed intact.
- **Steelman answered (L2 review)**: the strongest case against D3 is that the
  reported bug is *dropped CSI sequences*, not the wrap model — and D3 rewrites a
  hot path already tuned by four shipped features (`wide-chars`,
  `hangul-ime-and-font-fallback`, `csi-cursor-moves`, the Korean-backspace fix),
  making it the highest regression risk in this spec, spent on a symptom the user
  did not report. That argument is accepted as real and answered by **sequencing,
  not by dropping D3**: L4 lands the region + CSI work first and keeps it
  independently green, then lands pending-wrap as a separate, separately
  verifiable step. If pending-wrap destabilizes the existing suite, the region
  work still ships and D3 falls back to the rejected "DECAWM flag only" option
  without re-planning. The claim that eager wrap is *itself* part of the reported
  ghost/overlap symptom is **not evidenced**; it is a correctness argument, and
  this spec no longer leans on it as a bug-fix justification.

### D4: Character-shifting ops blank any orphaned wide-char half
- **Status**: resolved
- **Decision**: after ICH / DCH / ECH mutates a row, that row is repaired: a
  `Cell::CONTINUATION` with no wide leader immediately to its left becomes a
  blank, and a wide leader whose spacer was shifted away becomes a blank. A
  repaired blank uses the **same fill as the op that caused it** — the pen
  background, per D6 — so a repair never punches a default-colored hole in a
  TUI's colored pane.
- **Rationale**: `print` writes wide chars as a leader + CONTINUATION spacer pair
  (`grid.rs:334-341`); a raw `memmove` splits the pair, and the renderer then
  either draws a stray spacer or paints a double-width glyph over its neighbour's
  cell. One row-length pass per op is negligible against a per-frame budget.
  **Rejected:** "shift raw cells, accept orphans" — fastest, but it reintroduces
  the wide-char corruption class that `korean_backspace_does_not_erase_the_prompt`
  (`grid.rs:627-649`) and the `wide-chars` feature already paid to fix.

### D5: RIS (`ESC c`) resets the scroll region — the primary-screen escape hatch
- **Status**: resolved
- **Decision**: the existing minimal `ESC c` handler (`parser.rs:285-290`) gains a
  region reset alongside the DECCKM and bracketed-paste clears it already
  performs. It remains a minimal RIS — it still does **not** call `Grid::clear()`.
- **Rationale**: Tier-3 pair analysis found that D2's alt-screen reset never fires
  for Claude Code, which renders on the **primary** screen and never sends
  `?1049h`. A primary-screen TUI killed with `CSI 3;20r` set would otherwise leave
  zsh scrolling inside rows 3-20 permanently. `reset` / `tput reset` sends RIS, so
  this makes the documented recovery path work — the same rationale the
  arrow-key feature used when it added DECCKM clearing to this very handler.
  **Rejected:** adding a heuristic auto-reset when output arrives with the cursor
  outside the region — self-healing without user action, but heuristics in a VT
  parser misfire on legitimate TUI output (a TUI *does* write outside its region
  to paint a status bar). **Rejected:** accepting the primary-screen gap — it
  leaves the exact reported case unrecoverable.

### D6: Background Color Erase (BCE) for every scroll-exposed cell
- **Status**: resolved (amended after the Gate-3 review — see the note below)
- **Decision**: cells blanked or shifted in by ECH, ICH, DCH, and the blank rows
  revealed by **LF, IND, SU, SD, IL, DL and RI** take the **active pen background**
  and the default foreground. The existing ED (`J`) and EL (`K`) erases keep using
  `Cell::blank(default_fg, default_bg)` and are **not** retrofitted.
- **Amendment (Gate-3).** The original decision scoped BCE to "the new ops only"
  and the first implementation excluded `line_feed`, justifying it as "R3.4 pins LF
  to the default fill." That justification was **false** — R3.4 constrains history
  growth, `view_offset` pinning, and image placements, and says nothing about fill
  colour; the only test asserting it had been written in the same change, making
  the citation circular. xterm advertises `bce` and applies it to LF and IND alike,
  and LF is by far the most common scroll path, so excluding it defeated the very
  case D6 exists for: a TUI streaming into a colored pane got black rows on every
  scroll. LF and IND are now BCE like the rest.
- **Rationale**: TUIs paint a colored pane by setting a bg then erasing; without
  BCE they get black holes where they erased. **Rejected:** default-bg everywhere
  — internally consistent with `grid.rs:526-550` and simpler, but it breaks the
  colored-pane idiom that motivates this feature. **Rejected:** BCE everywhere
  including `J`/`K` — most correct overall, but it changes already-shipped
  behavior and would move existing insta snapshots, widening the blast radius of a
  parser-completeness change. **Accepted cost:** an acknowledged inconsistency —
  `K` and `X` fill differently. Recorded in Known Gaps as a deliberate follow-up.

### D7: `resize` resets the scroll region to full screen
- **Status**: resolved
- **Decision**: `Grid::resize` drops any active region back to full-screen,
  exactly as it already drops the selection.
- **Rationale**: `resize` already treats dimension-bound state as invalid
  ("a selection's coords are dimension-bound; a resize invalidates it",
  `grid.rs:569-570`) — a region is the same kind of state. The TUI receives
  SIGWINCH and re-sends `CSI r` on its next frame, so nothing is lost.
  **Rejected:** xterm-style clamping (`top = min(top, rows-1)`, reset if
  degenerate) — preserves a region across a grow-resize, but a clamped-then-
  restored region is subtly wrong until the TUI redraws, for no benefit the
  SIGWINCH redraw does not already provide.

### D8: Pending-wrap is cleared by every explicit repositioning
- **Status**: resolved
- **Decision**: the `pending_wrap` flag is cleared by **all** cursor moves
  (CUP/HVP, CUU/CUD/CUF/CUB, CHA, VPA), `carriage_return`, `line_feed`,
  `backspace`, `tab`, **and `resize`**. It is **not** cleared by DECAWM `?7l`, nor
  by the erase ops (ED/EL/ECH). The flag is only *honored* when DECAWM is on, so
  leaving it set across a `?7l`/`?7h` pair is harmless and matches DEC retention
  semantics.
- **Rationale**: the DEC rule — the deferred wrap survives only the exact sequence
  "print into the last column, then print again". Not clearing on erase ops is
  what makes the D3 scenario hold: `CSI K` after an 80th character erases only the
  last column and the wrap is still pending. **Rejected:** clearing on ED/EL/ECH —
  it would contradict D3. `resize` was added after a follow-up probe: the deferred
  position is dimension-bound, so a resize that clamps the cursor off the last
  column would otherwise fire a stray wrap on the next character — the same
  reasoning D7 applies to the region and `grid.rs:569-570` already applies to the
  selection.

### D9: DECSTBM (`CSI r`) parameter and cursor semantics
- **Status**: assumed (auto-resolved from standard VT semantics + L1)
- **Decision**: `CSI t ; b r` sets `top = t-1`, `bottom = b-1` (1-based → 0-based),
  clamped to the grid. A bare `CSI r` — or `CSI 0 ; 0 r` — resets the region to
  full screen. If the resulting `top >= bottom`, the **whole sequence is ignored**
  and the previous region survives unchanged. On a successful set, the cursor
  homes to `(col 0, row 0)`.
- **Rationale**: the VT100/xterm contract. Home is absolute screen home because
  DECOM (origin mode) is an explicit non-goal, so there is no region-relative
  origin to home into. Params are read with the same two-param pattern CUP
  already uses (`parser.rs:377-383`). **Rejected:** clamping an inverted region
  into validity — real terminals ignore it, and inventing a region an app did not
  ask for is worse than dropping one byte sequence.
- **Cursor-homing boundary (L2 review).** Only the **`CSI r` sequence** homes the
  cursor. The internal region resets in D2 (alt-screen), D5 (RIS) and D7 (resize)
  must **not** home it: `resize` clamps the cursor deliberately
  (`grid.rs:567-568`), and `ESC c` is deliberately a minimal RIS that moves
  nothing (`parser.rs:269-290`). The implementation therefore needs a private
  `reset_scroll_region()` that only touches the region, distinct from the public
  `set_scroll_region()` that `CSI r` calls and that also homes.

### D10: One region-aware scroll primitive, all callers route through it
- **Status**: assumed (auto-resolved from the existing `feed_scroll` shape)
- **Decision**: two private `Grid` helpers — `scroll_region_up(n)` and
  `scroll_region_down(n)` — own all row movement within `[top..=bottom]`.
  `feed_scroll`, SU, SD, IL, DL, RI and IND all call them. The history push /
  `view_offset` bump / `images.scroll_up_one()` block (D1) lives in exactly one
  place inside `scroll_region_up`, guarded by "region is full-screen AND not
  alt-screen".
- **Rationale**: `feed_scroll` (`grid.rs:378-394`) is already the single place
  that knows scroll means "history + offset + images + memmove"; extending it
  rather than duplicating that block across seven call sites is what keeps D1's
  invariant enforceable by inspection. **Rejected:** per-sequence implementations
  in the parser — that would put grid semantics in `parser.rs`, against the
  file's own stated split ("`vte` owns the state machine; we own the grid
  semantics", `parser.rs:1-7`).

### D11: SU/SD/IL/DL region-relative semantics
- **Status**: assumed (auto-resolved from standard VT semantics)
- **Decision**:
  - **SU (`CSI n S`)** — scroll the region up `n`, blank rows in at the region
    bottom. Cursor unmoved.
  - **SD (`CSI n T`)** — scroll the region down `n`, blank rows in at the region
    top. Cursor unmoved.
  - **IL (`CSI n L`)** — insert `n` blank lines: scroll `[cursor_row..=bottom]`
    down `n`. **DL (`CSI n M`)** — delete `n` lines: scroll `[cursor_row..=bottom]`
    up `n`.
  - IL/DL when the cursor row is **outside** the region: the sequence is ignored.
  - **IND (`ESC D`)** — index: like a line feed, region-aware. **RI (`ESC M`)** —
    reverse index: move up one row, scrolling the region *down* when already at
    the region top.
  - With no region ever set, all of these operate on the full screen.
  - `n` defaults to 1; `n == 0` is treated as 1; `n` larger than the region blanks
    it entirely.
  - **IL/DL leave the cursor column unchanged** (row unchanged too).
- **Rationale**: standard xterm behavior. The IL/DL column point is the one
  genuinely ambiguous call: DEC STD 070 documents a move to the left margin, while
  alacritty and peers leave the column alone. Leaving it alone is the lower-risk
  choice — ncurses emits a CR before IL/DL in practice, so an app that wants
  column 0 already gets it, whereas clobbering the column would be a visible bug
  for an app that does not. Recorded in Known Gaps.

### D12: ICH / DCH / ECH are strictly line-scoped
- **Status**: assumed (auto-resolved from standard VT semantics)
- **Decision**: all three operate only on the cursor's row and never touch another
  row, leave the cursor unmoved, and clamp `n` to the remaining line width
  (`n == 0` → 1).
  - **ICH (`CSI n @`)** — insert `n` blanks at the cursor, shifting the rest of the
    line right; cells pushed past the right edge are discarded.
  - **DCH (`CSI n P`)** — delete `n` cells at the cursor, shifting the tail left;
    `n` blanks fill in at the right edge.
  - **ECH (`CSI n X`)** — overwrite `n` cells from the cursor with blanks; **no**
    shift.
  - All three fill with the pen background (D6) and repair wide-char orphans (D4).
- **Rationale**: the xterm contract, and the line-scoped guarantee is what makes
  D4's repair pass a single-row operation. Line-scoping also means these ops are
  region-independent — a simplification worth stating explicitly so the
  implementation does not thread region state where it is not needed.

### D13: EL (`CSI K`) honors its parameter
- **Status**: assumed (auto-resolved — this is the literal request)
- **Decision**: `0` or absent = cursor→EOL (today's `clear_line_from_cursor`),
  `1` = BOL→cursor **inclusive**, `2` = the whole line. Any other parameter is
  ignored. All three fill with the **default** background (D6 deliberately does
  not retrofit `K`). The cursor does not move.
- **Rationale**: mirrors the shape the `J` arm already uses, including its
  `_ => {}` for unknown params (`parser.rs:421-426`), so ED and EL stay
  symmetrical and reviewable side by side.

### D14: Full-screen-region equivalence is a tested regression guard
- **Status**: assumed (auto-resolved — Tier-2 implication of D1)
- **Decision**: a freshly constructed `Grid` has a region spanning the whole
  screen, and with no `CSI r` ever sent every existing behavior is byte-identical
  to today — including history growth, `view_offset` pinning, and image scrolling.
  This is asserted by an explicit test, not assumed. All new params saturate
  (`saturating_*` / `min`); no `unwrap()` outside tests (CLAUDE.md).
- **Rationale**: D1 makes the history push conditional on a predicate that did not
  exist before; the whole scrollback feature (`tests/scrollback.rs`) rides on that
  predicate being true by default. A guard test converts a silent regression into
  a red build.

### D15: Image placements are untouched by partial-region scrolls
- **Status**: assumed (auto-resolved — Tier-1 actor coverage, follows D1)
- **Decision**: `images.scroll_up_one()` fires on exactly the ops that retain to
  history under D1's per-op predicate — LF/IND/SU always when the region is
  full-screen, and DL when the cursor is on the region top. Partial-region scrolls
  leave placements at their absolute rows, and placements are **not** clipped to
  the region.
- **Known asymmetry (accepted).** There is no `scroll_down_one` counterpart, so
  the down-scrolling ops (SD, IL, RI) do not move placements back down. `SU` then
  `SD` — or `DL` then `IL` — restores the grid byte-for-byte but leaves an image
  one row high. This predates the feature (SU already moved images while SD never
  did); DL/IL widen the same class. Recorded in Known Gap G-C rather than fixed,
  since adding downward image motion is new behavior outside this spec's scope.
- **Rationale**: consistent with D1 — a partial region is a TUI's private viewport
  and should not perturb primary-screen state. Not clipping is an accepted
  approximation: a TUI that scrolls a region over an inline image leaves it
  visually stale until the next full-screen scroll or clear. Recorded in Known
  Gaps; inline images and full-screen TUIs are not a combination that occurs in
  practice today.

### D16: A multi-sequence "realistic TUI frame" test is required, not optional
- **Status**: resolved (from the inversion probe)
- **Decision**: beyond per-sequence tests, the suite must contain at least one
  test that replays a realistic composite frame — set region → CUP → print →
  DECAWM toggle → DCH → IL → RI → reset region — and asserts the resulting grid,
  cursor, `pending_wrap`, and `history_len` together.
- **Rationale**: the inversion probe named this as the most likely way every
  requirement passes while Claude Code still renders wrong. Each sequence can be
  unit-correct while their *interaction* desyncs — pending-wrap surviving into a
  region scroll, or DCH's row repair running against a region-relative row index.
  Per-sequence tests structurally cannot catch that class. **Rejected:** relying
  on eyeballing via `/run-term` — a manual check does not survive as a regression
  gate.

### D17: DECSC / DECRC implemented — the deferred non-goal became load-bearing
- **Status**: resolved (scope extension approved after empirical evidence; was
  non-goal G-E)
- **Decision**: implement `ESC 7` / `ESC 8` (DECSC/DECRC) and their ANSI spellings
  `CSI s` / `CSI u`. They save and restore the cursor position, the active pen
  (fg/bg/attrs), and the `pending_wrap` flag. A restore with nothing saved homes
  the cursor and resets the pen (the VT default). A restored position is clamped to
  the current dimensions, since the grid may have been resized in between. A
  **parameterized** `CSI l ; r s` is DECSLRM (left/right margins) — not
  implemented, and explicitly not treated as a save.
- **Rationale**: recording Claude Code's real PTY output showed it emits
  `ESC 7` `ESC [ r` `ESC 8` at startup. Because D9 makes `CSI r` home the cursor,
  implementing `CSI r` while dropping DECRC left the cursor stranded at the origin
  — **strictly worse than before this feature**, when the whole idiom was a
  harmless no-op. Deferring DECSC/DECRC was therefore not safe once `CSI r` landed:
  the two are coupled. **Rejected:** making `CSI r` not home the cursor — it
  sidesteps the desync but diverges from VT semantics, contradicts D9, and would
  break R1.2. **Rejected:** filing it as a follow-up — that knowingly ships a
  regression.
- **Implementation note**: the `CSI s` guard keys on the parameter *count*, not on
  `first.is_none()`. `vte` reports a bare `CSI s` as **one default parameter**
  (`Some(0)`), never as zero, so an is-none check would silently never save — a bug
  caught by the DECSLRM test rather than by inspection.

## Constraints

- **C-1 Dependency direction is absolute.** Every change lands in `term-core`
  (`parser.rs`, `grid.rs`) and stays GPU-free and window-free. No wgpu or winit
  reference may appear, directly or transitively
  (`.claude/rules/dependency-direction.md`).
- **C-2 `GridSnapshot` is unchanged.** No new field, no new variant. Scroll-region
  and wrap state are terminal *modes* and follow the `alt_screen` /
  `app_cursor_keys` / `bracketed_paste` precedent: they live on `Grid` and are
  absent from the snapshot because the renderer does not read them
  (`grid.rs:154-164`).
- **C-3 No new dependencies.** `vte 0.15.0` and `unicode-width 0.2.0` are
  sufficient. `Cargo.lock` is hook-blocked from edits.
- **C-4 No `unwrap()` outside tests**, per-crate `thiserror` errors, and all
  index/param arithmetic saturating or clamped (CLAUDE.md conventions). A
  malformed or extreme sequence must never panic.
- **C-5 The existing term-core suite must pass unchanged.** `tests/scrollback.rs`,
  `tests/grid_snapshot.rs`, `tests/selection.rs`, `tests/cursor_key_mode.rs`, and
  the inline `parser.rs` / `grid.rs` test modules are the regression floor. Any
  insta snapshot that moves is a finding to surface, not a diff to accept blindly
  (`/insta-review`).
- **C-6 Verification gate.** `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, and `cargo test --workspace` must all be green.
- **C-7 New CSI arms sit after the private-marker guard.** The `?` private-mode
  early return and the `>`/`<`/`=` guard (`parser.rs:351-374`) must keep
  intercepting first — that guard exists because of a real misparse bug.
- **C-8 macOS ARM64 / local zsh only.** No `#[cfg(target_os)]` branches, no GUI
  crates (`.claude/rules/macos-only.md`, `no-gui-crates.md`, `zsh-local-only.md`).

## Known Gaps

- **G-A Erase-color inconsistency (deliberate, from D6).** Every scroll-exposed
  cell and `X` (ECH) fill with the pen background, while `J`/`K` fill with the
  default background. Retrofitting BCE onto ED/EL is a separate, snapshot-moving
  change — filed as follow-up. (The LF/IND half of this gap was *closed* by D6's
  Gate-3 amendment; only the ED/EL half remains.)
- **G-B IL/DL cursor-column ambiguity (from D11).** DEC STD 070 documents a move to
  the left margin; alacritty and peers leave the column alone. This spec leaves it
  alone. If a TUI is later observed depending on the DEC behavior, revisit.
- **G-H Wide char at the margin with autowrap off (closed — recorded for history).**
  R7 did not specify what happens when DECAWM is off and a double-width glyph
  cannot fit the last column. The first implementation moved the cursor *backward*
  to make room, destroying the cell to its left. Resolved: the glyph is **dropped**
  and the cursor stays at the margin — no VT operation may move the cursor left as
  a side effect of printing. A glyph wider than the entire grid is dropped for the
  same reason (it could not carry its continuation spacer).
- **G-C Inline images are not region-clipped, and only move upward (from D15).** A
  partial-region scroll over an inline image leaves the placement visually stale
  until the next full-screen scroll or clear. Separately, there is no
  `scroll_down_one`: SD/IL/RI do not move placements back down, so an up-then-down
  pair that restores the grid exactly still leaves an image one row high.
- **G-D DECTCEM (`?25h`/`?25l`) still dropped.** Cursor visibility needs a
  `GridSnapshot` field, which C-2 forbids here. A TUI hiding its cursor mid-redraw
  can still show a stray block cursor. Explicit non-goal; follow-up spec.
- **G-E ~~DECSC/DECRC still dropped~~ — CLOSED (see D17).** Deferring these turned
  out not to be safe: `CSI r` homes the cursor, so implementing it while dropping
  `ESC 8` produced a desync that did not exist before this feature. Implemented
  rather than deferred.
- **G-F Unhandled sequences remain silent.** The `_ => {}` catch-all still swallows
  everything unimplemented with no observability, so the next missing sequence will
  again be diagnosed only by symptom. The inversion probe raised instrumenting it;
  the user prioritized combination-correctness instead. Filed as follow-up.
- **G-G Performance is unmeasured.** D4's per-op row repair and per-frame region
  scrolling are assumed cheap against the project's (still uncommitted) input→screen
  budget — project gap G1 in `docs/architecture.md` applies unchanged. No
  benchmark is in scope here.

## Requirements

### R0: Full-screen TUIs render and scroll without ghost text
*(Confirmed Goal; D16)*

#### R0.1: A realistic composite TUI frame lands correctly
- **Given**: a 24-row grid and a byte stream that sets a region, positions the
  cursor, prints, toggles DECAWM, deletes characters, inserts a line, reverse-
  indexes, and resets the region — in one `advance` call
- **When**: the stream is fed through `Processor::advance`
- **Then**: the resulting cell contents, cursor position, `pending_wrap` state and
  `history_len` all match the hand-computed expected frame, asserted together in
  one test

#### R0.2: Every named sequence reaches the grid instead of the catch-all
- **Given**: the sequence set `CSI r`, `S`, `T`, `L`, `M`, `@`, `P`, `X`, `K` with
  each parameter form, plus `ESC D`, `ESC M`, and `CSI ?7h`/`?7l`
- **When**: each is fed to a grid whose state would visibly change if handled
- **Then**: every one produces an observable grid or mode change — none is
  silently swallowed by the `_ => {}` arm

---

### R1: Scroll-region state and the `CSI r` (DECSTBM) sequence
*(D9, D14; C-2)*

#### R1.1: A fresh grid has a full-screen region
- **Given**: `Grid::new(cols, rows, fg, bg)` with no sequence yet fed
- **When**: the region is queried
- **Then**: it spans `top == 0` and `bottom == rows - 1`

#### R1.2: `CSI t;b r` sets the region and homes the cursor
- **Given**: a 24-row grid with the cursor at row 10, col 5
- **When**: `ESC [ 3 ; 20 r` is fed
- **Then**: the region becomes `top == 2`, `bottom == 19`, and the cursor moves to
  `(col 0, row 0)`

#### R1.3: A bare or zeroed `CSI r` resets to full screen
- **Given**: a 24-row grid with an active region of rows 2..=19
- **When**: `ESC [ r` (and separately `ESC [ 0 ; 0 r`) is fed
- **Then**: the region spans `0..=23` again and the cursor homes to `(0, 0)`

#### R1.4: An inverted region is ignored, leaving the previous one intact
- **Given**: a 24-row grid with an active region of rows 2..=19
- **When**: `ESC [ 20 ; 3 r` is fed (top >= bottom)
- **Then**: the region is still `2..=19`, the cursor has not moved, and no cell
  changed

#### R1.5: Out-of-range parameters are clamped to the grid
- **Given**: a 24-row grid
- **When**: `ESC [ 1 ; 999 r` is fed
- **Then**: the region becomes `0..=23` (bottom clamped to `rows - 1`) rather than
  panicking or storing an out-of-bounds index

#### R1.6: The region is invisible to the renderer contract
- **Given**: a grid with an active region of rows 2..=19
- **When**: `GridSnapshot::from_grid` is called
- **Then**: `GridSnapshot` has no region field and its public shape is byte-identical
  to before this feature — `renderer` compiles untouched

---

### R2: Region-aware scrolling for SU / SD / IL / DL / IND / RI
*(D10, D11, D6)*

#### R2.1: A line feed at the region bottom scrolls only the region
- **Given**: a 6-row grid with region rows 1..=3, rows 0..5 holding `a`..`f`, and
  the cursor on row 3
- **When**: a line feed (`\n`) is executed
- **Then**: rows 1..=3 shift up (row 1 becomes `c`, row 2 `d`, row 3 blank) while
  row 0 (`a`) and rows 4..5 (`e`, `f`) are untouched

#### R2.2: SU scrolls the region up, blanking in at the bottom
- **Given**: a grid with region rows 1..=3 holding `b`, `c`, `d`
- **When**: `ESC [ 2 S` is fed
- **Then**: row 1 holds `d`, rows 2 and 3 are blank, rows outside the region are
  unchanged, and the cursor has not moved

#### R2.3: SD scrolls the region down, blanking in at the top
- **Given**: the same grid and region
- **When**: `ESC [ 2 T` is fed
- **Then**: rows 1 and 2 are blank, row 3 holds `b`, outside rows are unchanged,
  and the cursor has not moved

#### R2.4: IL inserts blank lines from the cursor to the region bottom
- **Given**: a grid with region rows 1..=4 holding `b`,`c`,`d`,`e` and the cursor
  on row 2
- **When**: `ESC [ 1 L` is fed
- **Then**: row 2 is blank, row 3 holds `c`, row 4 holds `d`, `e` has been pushed
  out of the region, row 0 and rows below the region are unchanged, and the
  cursor row and column are both unchanged

#### R2.5: DL deletes lines, pulling the region tail up
- **Given**: the same grid and cursor on row 2
- **When**: `ESC [ 1 M` is fed
- **Then**: row 2 holds `d`, row 3 holds `e`, row 4 is blank, rows outside the
  region are unchanged, and the cursor is unchanged

#### R2.6: IL and DL outside the region are no-ops
- **Given**: a grid with region rows 2..=5 and the cursor on row 0
- **When**: `ESC [ 3 L` and then `ESC [ 3 M` are fed
- **Then**: no cell in the grid changed and the cursor did not move

#### R2.7: IND (`ESC D`) indexes within the region
- **Given**: a grid with region rows 1..=3 and the cursor on row 3 (region bottom)
- **When**: `ESC D` is fed
- **Then**: the region scrolls up by one and the cursor stays on row 3

#### R2.8: RI (`ESC M`) at the region top scrolls the region down
- **Given**: a grid with region rows 1..=3 holding `b`,`c`,`d` and the cursor on
  row 1 (region top)
- **When**: `ESC M` is fed
- **Then**: row 1 is blank, row 2 holds `b`, row 3 holds `c`, the cursor stays on
  row 1, and rows outside the region are unchanged

#### R2.9: RI below the region top just moves the cursor up
- **Given**: a grid with region rows 1..=3 and the cursor on row 2
- **When**: `ESC M` is fed
- **Then**: the cursor moves to row 1 and no cell changed

#### R2.10: Counts default, saturate, and never over-scroll
- **Given**: a grid with region rows 1..=3
- **When**: `ESC [ S` (no param), `ESC [ 0 S`, and `ESC [ 9999 S` are each fed to a
  fresh copy
- **Then**: the first two scroll by exactly 1; the third leaves the region entirely
  blank with rows outside it untouched, and no call panics

#### R2.11: Rows scrolled into the region use the pen background
- **Given**: a grid where `ESC [ 48 ; 5 ; 21 m` has set a blue pen background
- **When**: `ESC [ 1 S` scrolls a blank row in at the region bottom
- **Then**: every cell of that new row has `bg == ansi_256(21)` and `fg ==` the
  default foreground

---

### R3: Scrollback isolation under a scroll region
*(D1, D14, D15)*

#### R3.1: A partial-region scroll retains nothing in history
- **Given**: a 6-row primary-screen grid with region rows 1..=3 and
  `history_len() == 0`
- **When**: the region is scrolled 50 times via `ESC [ 1 S`
- **Then**: `history_len()` is still 0

#### R3.2: A partial-region scroll does not disturb a scrolled-up viewport
- **Given**: a primary-screen grid with retained history, the user scrolled up
  (`view_offset() == 2`), and a partial region set
- **When**: the region is scrolled several times
- **Then**: `view_offset()` is still 2 and the top visible row shows the same
  content as before

#### R3.3: A partial-region scroll does not move image placements
- **Given**: a grid with an inline image placed at row 4 and a region of rows 1..=3
- **When**: the region is scrolled up 3 times
- **Then**: the placement is still anchored at row 4

#### R3.4: A full-screen region preserves the pre-feature scrolling contract
- **Given**: a primary-screen grid with no `CSI r` ever fed (or after `ESC [ r`)
- **When**: enough lines are fed to scroll the grid several times
- **Then**: `history_len()` grows by one per scroll, a scrolled-up `view_offset`
  auto-increments to stay pinned, and image placements shift up — identical to the
  pre-feature behavior asserted by `tests/scrollback.rs`
- **Scope note.** These three clauses are the whole of this requirement; it does
  **not** constrain the *fill colour* of the row scrolled in. D6 (as amended)
  deliberately changes that colour to Background Color Erase. An earlier revision
  misread R3.4 as pinning the fill and used it to justify excluding LF from BCE —
  hence this note.

#### R3.5: The alt-screen guard still suppresses history
- **Given**: an alt-screen grid (`ESC [ ? 1049 h`) with a full-screen region
- **When**: many lines are fed
- **Then**: `history_len()` does not change, exactly as
  `alt_screen_does_not_grow_history` asserts today

---

### R4: Scroll-region lifetime and the cursor-homing boundary
*(D2, D5, D7, D9)*

#### R4.1: Entering the alternate screen resets the region without homing
- **Given**: a grid with region rows 2..=19 and the cursor at row 10, col 5
- **When**: `ESC [ ? 1049 h` is fed
- **Then**: the region spans the full screen again and the cursor is still at
  row 10, col 5

#### R4.2: Leaving the alternate screen resets the region
- **Given**: an alt-screen grid where the app has set region rows 2..=19
- **When**: `ESC [ ? 1049 l` is fed
- **Then**: the region spans the full screen again

#### R4.3: RIS resets the region without clearing or moving anything else
- **Given**: a primary-screen grid with region rows 2..=19, visible text, the
  cursor at row 10, and an inline image placed
- **When**: `ESC c` is fed
- **Then**: the region spans the full screen; the cursor is still at row 10; every
  cell and every image placement is unchanged; and `app_cursor_keys` and
  `bracketed_paste` are cleared as they are today

#### R4.4: Resize resets the region
- **Given**: a 24-row grid with region rows 2..=19
- **When**: `Grid::resize(cols, 10)` is called
- **Then**: the region spans `0..=9` (full screen at the new size) and no
  out-of-bounds index is retained

#### R4.5: Only the `CSI r` sequence homes the cursor
- **Given**: the four reset paths — `CSI r`, alt-screen enter/leave, RIS, resize —
  each starting with the cursor at row 10, col 5
- **When**: each path is exercised in isolation
- **Then**: only `CSI r` leaves the cursor at `(0, 0)`; the other three leave it
  where it was (subject to `resize`'s existing clamp)

---

### R5: In-line character operations — ICH / DCH / ECH
*(D12, D6, D4)*

#### R5.1: ICH inserts blanks and shifts the line right
- **Given**: a 10-column row holding `abcdefghij` with the cursor at col 3
- **When**: `ESC [ 2 @` is fed
- **Then**: the row reads `abc` + two blanks + `defgh`, the cursor is still at
  col 3, and the two characters pushed past the right edge are discarded

#### R5.2: DCH deletes and pulls the tail left
- **Given**: the same row with the cursor at col 3
- **When**: `ESC [ 2 P` is fed
- **Then**: the row reads `abcfghij` followed by two blanks, and the cursor is
  still at col 3

#### R5.3: ECH overwrites in place without shifting
- **Given**: the same row with the cursor at col 3
- **When**: `ESC [ 3 X` is fed
- **Then**: cols 3..=5 are blank, cols 6..=9 still read `ghij` (no shift), and the
  cursor is still at col 3

#### R5.4: All three are strictly line-scoped
- **Given**: a 3-row grid where every row holds identical text and the cursor is
  on row 1
- **When**: ICH, DCH and ECH are each applied
- **Then**: rows 0 and 2 are byte-identical to their starting content in every case

#### R5.5: Blanks introduced use the pen background
- **Given**: a row where `ESC [ 48 ; 5 ; 21 m` set a blue pen background
- **When**: `ESC [ 3 X` erases three cells
- **Then**: those cells have `bg == ansi_256(21)` and the default foreground

#### R5.6: A shift that orphans a wide-char spacer repairs it
- **Given**: a row holding `ab한글cd` where `한` occupies cols 2-3 as leader +
  CONTINUATION
- **When**: `ESC [ 3 P` at col 0 shifts the tail left, splitting the pair
- **Then**: no `Cell::CONTINUATION` remains without a wide leader immediately to
  its left; each orphaned half is a blank carrying the pen background

#### R5.7: A shift that orphans a wide-char leader repairs it
- **Given**: the same row
- **When**: an ICH pushes a wide leader's spacer past the right edge
- **Then**: the now-spacer-less leader is replaced by a blank rather than left to
  paint a double-width glyph over its neighbour

#### R5.8: Counts default, clamp, and never overrun the row
- **Given**: a 10-column row with the cursor at col 8
- **When**: `ESC [ @`, `ESC [ 0 P`, and `ESC [ 9999 X` are each fed to a fresh copy
- **Then**: the first two act on exactly 1 cell, the third blanks only cols 8..=9,
  and no call panics or touches another row

---

### R6: EL (`CSI K`) honors its parameter
*(D13)*

#### R6.1: `0K` erases from the cursor to end of line
- **Given**: a 10-column row holding `abcdefghij` with the cursor at col 4
- **When**: `ESC [ K` (and separately `ESC [ 0 K`) is fed
- **Then**: cols 0..=3 still read `abcd`, cols 4..=9 are blank, and the cursor has
  not moved

#### R6.2: `1K` erases from line start to the cursor inclusive
- **Given**: the same row and cursor
- **When**: `ESC [ 1 K` is fed
- **Then**: cols 0..=4 are blank (the cursor cell included), cols 5..=9 still read
  `fghij`, and the cursor has not moved

#### R6.3: `2K` erases the whole line
- **Given**: the same row and cursor
- **When**: `ESC [ 2 K` is fed
- **Then**: every column of that row is blank, adjacent rows are untouched, and the
  cursor has not moved

#### R6.4: An unknown EL parameter is ignored
- **Given**: the same row and cursor
- **When**: `ESC [ 7 K` is fed
- **Then**: no cell changed — matching the `J` arm's treatment of unknown params

#### R6.5: EL fills with the default background, not the pen
- **Given**: a row where the pen background has been set to blue
- **When**: `ESC [ 2 K` is fed
- **Then**: the erased cells carry the grid's **default** background (deliberate
  divergence from R5.5 — recorded as Known Gap G-A)

---

### R7: DECAWM and pending-wrap
*(D3, D8)*

#### R7.1: Printing into the last column defers the wrap
- **Given**: an 8-column, 3-row grid with the cursor at col 0, row 0
- **When**: exactly 8 characters are printed
- **Then**: all 8 land on row 0, the cursor rests at col 7 on row 0, and
  `pending_wrap` is set — row 1 is still blank

#### R7.2: The next printable character performs the deferred wrap
- **Given**: the state from R7.1
- **When**: one more character is printed
- **Then**: it lands at col 0 of row 1, the cursor is at col 1 of row 1, and
  `pending_wrap` is clear

#### R7.3: `CSI K` while pending erases only the last column
- **Given**: the state from R7.1
- **When**: `ESC [ K` is fed
- **Then**: only col 7 of row 0 is blanked, cols 0..=6 survive, and `pending_wrap`
  is still set (erases do not clear it)

#### R7.4: Explicit repositioning clears the pending wrap
- **Given**: the state from R7.1, restored fresh for each case
- **When**: each of `\r`, `\n`, `\x08`, `\t`, `ESC [ 1 ; 1 H`, `ESC [ A`,
  `ESC [ 5 G`, and a `Grid::resize` call is applied
- **Then**: in every case `pending_wrap` is clear afterwards, and a subsequent
  printable character does **not** wrap

#### R7.5: DECAWM off overwrites the last column in place
- **Given**: an 8-column grid after `ESC [ ? 7 l`
- **When**: 10 characters are printed from col 0
- **Then**: chars 1..=7 fill cols 0..=6, chars 8, 9 and 10 each overwrite col 7 in
  turn, the cursor stays at col 7 on row 0, row 1 is blank, and `pending_wrap` is
  never set

#### R7.6: DECAWM back on restores wrapping
- **Given**: the grid from R7.5
- **When**: `ESC [ ? 7 h` is fed and two more characters are printed
- **Then**: the first re-fills col 7 and sets `pending_wrap`, the second wraps to
  row 1

#### R7.7: A pending wrap is retained but not honored while DECAWM is off
- **Given**: the pending state from R7.1
- **When**: `ESC [ ? 7 l` is fed, then a character is printed, then `ESC [ ? 7 h`
- **Then**: the printed character overwrote col 7 without wrapping, and the flag
  was never cleared by the mode change

#### R7.8: A wide character that cannot fit the last column wraps immediately
- **Given**: an 8-column grid with the cursor at col 7 and `pending_wrap` clear
  (reached by cursor move, not by printing)
- **When**: `한` (width 2) is printed
- **Then**: it wraps to cols 0-1 of the next row as leader + CONTINUATION — the
  existing "a wide char never straddles the right margin" rule is preserved

#### R7.9: The shipped wrap behavior is re-verified, not assumed
- **Given**: the existing tests `line_wraps_at_right_edge`
  (`tests/grid_snapshot.rs:66-71`), `wide_char_wraps_rather_than_splitting_the_margin`
  and `korean_backspace_does_not_erase_the_prompt` (`grid.rs:612-649`)
- **When**: the pending-wrap change lands
- **Then**: each still passes; any insta snapshot that moves is investigated and
  explained in the change description rather than accepted blindly

---

### R8: Robustness — no panic, no silent corruption
*(C-4, E5)*

#### R8.1: Extreme and malformed parameters never panic
- **Given**: a small grid (e.g. 4x2)
- **When**: `ESC [ 65535 ; 65535 r`, `ESC [ 65535 L`, `ESC [ 65535 @`,
  `ESC [ 65535 X`, `ESC [ 0 ; 0 r` and truncated variants are fed in sequence
- **Then**: `advance` returns normally every time, the grid remains internally
  consistent (`cells.len() == cols * rows`), and the cursor stays in bounds

#### R8.2: The implementation carries no `unwrap()` outside tests
- **Given**: the new code in `parser.rs` and `grid.rs`
- **When**: the crate is reviewed and clippy runs with `-D warnings`
- **Then**: no `unwrap()`/`expect()` appears outside `#[cfg(test)]`, and index math
  uses `saturating_*`/`min` per the CLAUDE.md convention

#### R8.3: Still-unimplemented sequences remain harmless no-ops
- **Given**: sequences outside this spec's scope (`ESC [ ? 25 h`, `ESC 7`,
  `ESC [ s`, `ESC [ > 4 ; 2 m`)
- **When**: each is fed
- **Then**: no cell, cursor, or mode changes — in particular the `>`/`<`/`=`
  private-marker guard still intercepts before the new arms

---

### R9: Regression floor and verification gate
*(C-1, C-3, C-5, C-6, D14)*

#### R9.1: The existing term-core suite passes unchanged
- **Given**: `tests/scrollback.rs`, `tests/grid_snapshot.rs`, `tests/selection.rs`,
  `tests/cursor_key_mode.rs`, `tests/pty_smoke.rs`, and the inline `parser.rs` /
  `grid.rs` / `image.rs` test modules as they stand on `main`
- **When**: `cargo test --workspace` runs after the change
- **Then**: all pass with no test deleted or weakened; any changed assertion is
  called out explicitly

#### R9.2: The verification gate is green
- **Given**: the completed change
- **When**: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, and `cargo test --workspace` are run in order
- **Then**: all three exit zero

#### R9.3: The dependency direction is intact
- **Given**: `crates/term-core/Cargo.toml` after the change
- **When**: its dependency list is inspected and the workspace builds
- **Then**: no `wgpu`, no `winit`, and no new dependency of any kind was added —
  `term-core` remains headless-testable

---

### R10: DECSC / DECRC save and restore the cursor
*(D17 — added after the `/run-term` empirical check)*

#### R10.1: `ESC 7` then `ESC 8` restores the cursor position
- **Given**: the cursor at row 5, col 5
- **When**: `ESC 7` is fed, the cursor is moved home, then `ESC 8` is fed
- **Then**: the cursor is back at row 5, col 5

#### R10.2: The save also captures the active pen
- **Given**: a pen of 256-colour fg 196 on bg 21, bold, saved with `ESC 7`
- **When**: the pen is reset with `SGR 0` and then `ESC 8` is fed
- **Then**: a subsequently printed cell carries fg 196, bg 21 and BOLD again

#### R10.3: A restore with nothing saved homes the cursor
- **Given**: a fresh grid, cursor at row 5 col 5, bold pen, no prior `ESC 7`
- **When**: `ESC 8` is fed
- **Then**: the cursor homes to `(0, 0)` and the pen returns to its defaults

#### R10.4: A saved position is clamped across a resize
- **Given**: a 20x10 grid with `ESC 7` saved at col 18, row 8
- **When**: the grid is resized to 5x4 and `ESC 8` is fed
- **Then**: the cursor lands at col 4, row 3 and a subsequent print does not panic

#### R10.5: Claude Code's `ESC 7` / `CSI r` / `ESC 8` idiom preserves the cursor
- **Given**: the cursor at row 5, col 5 and a scroll region to be reset
- **When**: the literal byte sequence `ESC 7 ESC [ r ESC 8` is fed
- **Then**: the cursor is still at row 5, col 5 **and** the region really did reset
  to full screen — the regression this requirement exists to prevent

#### R10.6: `CSI s`/`CSI u` work, and parameterized `CSI s` is not a save
- **Given**: a cursor saved via `CSI s` and later restored via `CSI u`
- **When**: a two-parameter `CSI 2 ; 10 s` (DECSLRM) is fed from a different
  position, then `CSI u`
- **Then**: the restore returns to the `CSI s` position — DECSLRM did not
  overwrite the saved cursor

## Tasks

> **Serialization note.** Every task edits `crates/term-core/src/grid.rs` and
> `crates/term-core/src/parser.rs`. Under the file-overlap rule, tasks touching the
> same file must be serialized even when they are logically independent — so this
> DAG is a chain by construction, not by hidden coupling. Where a dependency exists
> *only* because of file overlap it is labelled **[serialization]**; where it is a
> real interface dependency it is labelled **[interface]**. The chain order is not
> arbitrary: it is the D3 steelman answer — the region and CSI work (T1–T3) must be
> independently green before the higher-risk pending-wrap rewrite (T4) lands.

### T1: Scroll-region state, DECSTBM, and region lifetime [infra]
- **Fulfills**: R1, R4
- **Depends on**: (none)
- **Touches**: `grid.rs` (new `scroll_top`/`scroll_bottom` fields + `Grid::new`
  init; public `set_scroll_region` that homes the cursor; private
  `reset_scroll_region` that does not; hooks in `resize` and `set_alt_screen`),
  `parser.rs` (the `'r'` CSI arm; region reset added to the `ESC c` handler).
- **Why first**: it introduces the state every later task reads, and it is the only
  task that can define the `set_` vs `reset_` homing boundary (R4.5) — getting that
  wrong later would require touching every call site.

### T2: Region-aware scroll primitive + scrollback isolation [vertical]
- **Fulfills**: R2, R3
- **Depends on**: T1 **[interface]** — needs the region state and
  `reset_scroll_region` from T1.
- **Touches**: `grid.rs` (`scroll_region_up` / `scroll_region_down`; rewire
  `feed_scroll` so the history-push + `view_offset` + `images.scroll_up_one` block
  is guarded by "full-screen region AND not alt-screen"), `parser.rs` (`S`, `T`,
  `L`, `M` CSI arms; `ESC D` and `ESC M` in `esc_dispatch`).
- **Highest-risk sub-goal**: R3.4 — the full-screen-equivalence guard test. Land it
  *with* this task, not after; it is what proves the scrollback feature survived.

### T3: In-line character ops and EL parameter handling [vertical]
- **Fulfills**: R5, R6
- **Depends on**: T2 **[serialization]** — logically independent of the region work
  (D12 makes ICH/DCH/ECH region-agnostic), but it edits the same two files.
- **Touches**: `grid.rs` (`insert_chars`, `delete_chars`, `erase_chars`, the
  wide-char row-repair helper, and a parameterized line-erase replacing
  `clear_line_from_cursor`'s single behavior), `parser.rs` (`@`, `P`, `X` arms; the
  `K` arm gains its `match first`).
- **Note**: this task owns the D6 split — new ops fill with the pen background,
  `K` keeps the default background (R5.5 vs R6.5). Implement both assertions
  together so the deliberate inconsistency is visible in one diff.

### T4: DECAWM and pending-wrap [vertical]
- **Fulfills**: R7
- **Depends on**: T3 **[serialization]**, and deliberately sequenced last per the
  D3 steelman answer.
- **Touches**: `grid.rs` (new `autowrap` + `pending_wrap` fields; rewrite the wrap
  branch of `print`; clear-flag hooks in `carriage_return`, `line_feed`,
  `backspace`, `tab`, `move_to`, `resize`), `parser.rs` (`?7h`/`?7l` in the
  private-mode arm).
- **Fallback**: this is the highest-regression-risk task in the spec. If R7.9 shows
  the existing wrap / wide-char / IME tests destabilizing and the cause is not
  quickly resolvable, fall back to D3's rejected alternative — DECAWM flag only,
  keeping eager wrap — and record it. T1–T3 ship regardless.

### T5: Composite-frame, robustness, and verification sweep [infra]
- **Fulfills**: R0, R8, R9
- **Depends on**: T4 **[interface]** — the composite frame (R0.1) asserts
  `pending_wrap`, so it can only be written once T4 exists.
- **Touches**: a new integration test file (`crates/term-core/tests/vt_tui.rs`)
  following the `tests/scrollback.rs` assertion style, plus any inline test-module
  additions; no production code beyond fixes the sweep surfaces.
- **Content**: the realistic multi-sequence frame (R0.1), the
  no-longer-dropped-sequence sweep (R0.2), the malformed/extreme parameter battery
  (R8.1), the unimplemented-sequences-stay-no-ops check (R8.3), and the final
  `fmt` / `clippy -D warnings` / `test --workspace` gate (R9.2).

**DAG**: `T1 → T2 → T3 → T4 → T5` (linear — see the serialization note).

## External Dependencies

### Pre-work
- (none) — no new crates, no accounts, no services. `vte 0.15.0` and
  `unicode-width 0.2.0` are already present and sufficient (C-3).

### Post-work
- **Eyeball against the real thing.** Run `/run-term` and drive Claude Code inside
  it. The whole spec is headless-testable, but "does the TUI actually look right"
  is not something `cargo test` can answer — this is the check that closes the
  original bug report.
- **`/insta-review` if any snapshot moved.** Per C-5, a moved snapshot is a finding
  to explain, not a diff to accept blindly.
- **Update `docs/domain.md` / `docs/architecture.md`** if the scroll region or
  pending-wrap warrants a vocabulary or invariant entry — both docs currently
  describe the parser's coverage as a "scaffold baseline", and `parser.rs:1-7`
  says the same in its module doc. That comment becomes wrong once this lands.
