# Spec: grid-reflow

## Meta
- **Created**: 2026-08-06
- **Type**: dev
- **Status**: drafting — L0/L1 seeded, **L2 not yet run**
- **Split from**: `specs/ai-monitor-right-panel/spec.md` (its D22), on the L2 reviewer's
  recommendation. That spec `depends_on` this one.

## Goal
Make `Grid::resize` non-destructive: a logical line wider than the new column count re-wraps
instead of being truncated, and re-widening restores it.

## Non-goals
- Search-in-scrollback (still deferred, `specs/scrollback/spec.md:24`).
- Any ai-monitor panel work — that is the parent spec's business.

## Confirmed Goal
Today `Grid::resize` (`crates/term-core/src/grid.rs:1283-1310`) allocates a fresh buffer and
copies only `col < cols.min(self.cols)`. Narrowing therefore discards the right-hand side of
every row permanently; widening back fills blanks. Reflow-on-resize was named and deferred
once (`specs/scrollback/spec.md:24,95` — "resized history may wrap differently"); this
un-defers it, because `ai-monitor-right-panel` makes window-narrowing a routine gesture
rather than a rare one.

## Research

### Why a per-row wrap flag is the prerequisite
The grid has **no per-row wrap provenance**. `pending_wrap` (`grid.rs:145`) is cursor state,
not a property of a row, so nothing distinguishes "this row continues the one above" from
"this row began after a newline". Rejoining without that distinction splices together lines
that were never one line.

**The set-site is not where it looks.** `grid.rs:503-505` (`self.pending_wrap =
self.autowrap;`) *arms* a deferred wrap; it does not mean a wrap happened. The wrap is
**consumed** at two independent sites — `grid.rs:454-459` and the fit-check at
`grid.rs:466-472`, both via `line_feed()` — and `pending_wrap` is cleared *without* wrapping
by `carriage_return` (`grid.rs:511`), `line_feed` (544), `backspace` (554), `tab` (561) and
every cursor move (576). Concretely: `printf 'x%.0s' {1..80}; echo` on an 80-column grid arms
the flag on the 80th character, then `\r\n` clears it and no wrap occurs. Flagging at the
arming site would mark that row `wrapped` and splice it onto the next prompt line — the exact
failure the flag exists to prevent. The flag must be set on the **departing row, inside the
two consumption sites**.

### The competing wrap rule already in the codebase
`GridSnapshot::row_soft_wraps` (`crates/term-core/src/snapshot.rs:178`) is a documented D8
heuristic: "a row is treated as soft-wrapped into the next iff its last column holds a
non-blank, non-continuation cell." It has two live consumers — `selection_text`
(`snapshot.rs:165`) and URL stitching (`links.rs:144-153`) — and `links.rs:34` documents its
known false positive (a hard line that happens to fill the last column). Its doc comment
states it is shared specifically so copy and link-stitching are "one heuristic, not two that
could drift". Introducing an authoritative flag beside it creates exactly that drift: copy
and URL stitching would join lines reflow would not, and vice versa.

### Types and call sites a flag would change
- `pub type Row = Vec<Cell>` (`crates/term-core/src/scrollback.rs:12`), consumed by
  `Grid::history_row(&self) -> Option<&[Cell]>` (`grid.rs:1084`) and
  `GridSnapshot::from_grid` (`snapshot.rs:50`).
- `GridSnapshot` is "the versioned boundary type (C4)" (`snapshot.rs:1-5`) — carrying per-row
  wrap data across it is a deliberate contract extension, not an implementation detail.
- `DEFAULT_HISTORY_LIMIT = 100_000` (`scrollback.rs:15`), and `settings::refit` resizes
  **background tabs too** (`crates/app/src/settings.rs:31`).

### Alt-screen is not a second buffer here
`grid.rs:900-905` states this codebase's alt-screen is "a history-preserving switch rather
than a second buffer" — there is one cell buffer. So "do not rewrap in alt-screen" is
ambiguous between the live viewport (a TUI owns it and repaints on SIGWINCH) and history
(which is *primary* history the TUI never touched, and which the user reads after quitting
`vim`). The rule has to be stated per target.

## Open questions for L2 (none of these are decided yet)
1. **Flag set-site**: set `wrapped` on the departing row at both consumption sites — and what
   clears it when a row is later overwritten in place? Three refinements the L2 review
   surfaced: at `grid.rs:454-459` the set belongs **inside** the `if self.autowrap` branch
   (456), because the flag is also cleared there without wrapping when autowrap is off; the
   clear-site list above omits `reverse_index` (`grid.rs:747`, which its own doc at 567-575
   flags as the one repositioning path bypassing `move_to`) and `restore_cursor` (`808`,
   which *restores* a saved `pending_wrap`); and `line_feed()` at the bottom margin scrolls,
   so the departing row goes straight into `History` — the flag must be set before that call
   and must ride the row across the `scrollback::Row` boundary.
2. **Overflow direction**: narrowing roughly doubles the row count of wrapped content while
   the viewport still holds `rows` rows. Do surplus rows push into `History`, drop off the
   top, or something else? Does widening **pull rows back out of history** — required for the
   parent spec's "re-widening restores it" — and what happens to `view_offset`, a row index
   into a history whose length just changed?
3. **Cursor**: today `resize` clamps (`grid.rs:1298-1300`). After rewrap the cursor must
   follow its logical character. Additionally, `refit` fires SIGWINCH and zle redraws its own
   prompt line; if the grid has already rewrapped that line the user sees it twice. This is
   the canonical reflow double-prompt bug and needs an explicit rule.
4. **Alt-screen**: split the rule between live viewport and history (see Research).
5. **Heuristic retirement**: retire `row_soft_wraps` in favour of the flag (requires
   extending `GridSnapshot`), or keep both and state why the drift is acceptable.
6. **Cost bound**: a 430→300px drag crosses ~7 column boundaries; each refit would unwrap +
   re-break + reallocate up to 100k history rows **per tab**, synchronously on the UI thread,
   against `docs/domain.md`'s input→screen ≤ 8ms goal. Bound it (visible viewport plus a
   capped, lazily-reflowed history window) or record the cost as a Known Gap.
7. **Trailing blanks**: whether unwrap strips them decides if re-widening is actually
   lossless.

## Decisions
_(pending L2 — run `/harness-ops:specify` against this spec)_
