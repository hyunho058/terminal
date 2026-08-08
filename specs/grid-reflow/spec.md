# Spec: grid-reflow

## Meta
- **Created**: 2026-08-06
- **Type**: dev
- **Status**: drafting — L2 derived three times (D1-D11 -> U1-U13 -> **R1-R12**, current).
  Two independent reviews failed the first two passes; the review-history section at the end
  records both and where each finding is answered. R1-R12 awaits its own review.
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

## Open questions raised at L1 (all answered by R1-R12 below — kept for provenance)
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

> **Third derivation.** Pass 1 (D1-D11) took the split as given and failed review on nine
> counts. Pass 2 (U1-U13) adopted the unified row list and failed on its steelman: the split
> was never the problem — *laziness* was — and unifying pays a permanent hot-path cost
> (`grid.rs:478` calls `print` the hot path; four `copy_within` memmoves and two contiguous
> erase spans would become per-row loops over non-contiguous slots) to simplify an operation
> that runs ~7 times per drag. This pass keeps the split and makes **eagerness** do the work.
> The user's underlying answers are unchanged throughout.

### R1: Reflow happens eagerly, inside `Grid::resize`, against a fresh buffer
- **Status**: resolved
- **Rationale**: `resize` already allocates a fresh `cols x rows` buffer and repopulates it
  (`grid.rs:1286-1292`) — today with a per-cell copy that truncates. Reflow is a change to
  *that loop*, not to the data structure: walk `History` + the old viewport into logical lines,
  re-break them at the new width, write the tail into the fresh viewport buffer and the
  remainder back into `History`. No new container, no change to `Grid::index`, and every
  contiguity-dependent primitive (`copy_within` at `grid.rs:618/641/1167/1186`, the ED spans at
  `:1269/:1277`, the wide-pair arithmetic at `:1220-1256`) is untouched.
  Eagerness is the whole fix: pass 1's failures — a mixed-width history, an uncomputable scroll
  anchor — were consequences of deferring, not of having two containers.
  **Rejected**: the unified row list (pass 2's U1 — cleaner in the abstract, but it converts
  steady-state hot paths into per-row loops to simplify a rare one, and it introduced a panic:
  stripped and unreflowed rows shorter than `cols` reaching indexed viewport writes), and lazy
  or windowed reflow (pass 1's D7 — the mixed-width hole).

### R2: The viewport is always exactly `cols` wide — the invariant that removes a panic class
- **Status**: resolved  *(answers re-review a1)*
- **Rationale**: Viewport cells are written through indexed slots (`grid.rs:484/491/494`,
  and the EL/ICH/DCH/ECH fills at `:1146/:1170/:1189/:1202`), which are safe only because the
  buffer is exactly `cols x rows`. Pass 2 broke that by letting stripped (R9) and unreflowed
  (R11) rows become viewport rows. Under R1 it cannot break: the viewport is a freshly
  allocated `cols x rows` buffer every resize, and re-broken lines are written into it, padded
  with `Cell::blank`. Short rows exist only in `History`, whose readers already tolerate any
  length (`snapshot.rs:48-51` and `grid.rs:1014-1020` both use `.get(col).unwrap_or(blank)`).
  This is stated as an invariant rather than left implicit, because it is what makes the
  "never panics" constraint hold. **Rejected**: variable-width viewport rows (pass 2's defect).

### R3: The scroll anchor is a (logical line, cell offset) pair, re-derived after reflow
- **Status**: resolved  *(answers re-review a2; carries "follow the content I was reading")*
- **Rationale**: `view_offset` is a row count from the bottom (`grid.rs:166`), which means
  something different once the row count changes. Pass 1 rejected recomputation because D7's
  laziness made the post-reflow layout unavailable; pass 2 rejected it again by inheritance and
  substituted "row identity", which the types do not provide — `Row` is a bare `Vec<Cell>`
  (`scrollback.rs:12`) and reflow splits and merges the very rows it would anchor to. Under R1
  the layout exists at resize time, so the original mechanism is simply available: record which
  logical line and how far into it the viewport top sits, re-derive the row index after
  re-breaking. `GridSnapshot.view_offset` (`snapshot.rs:17`) keeps its meaning and its type —
  the anchor is internal to `resize`, not a contract change. **Rejected**: clamping the index
  (teleports the reader); row identity (unrepresentable).

### R4: The cursor is re-broken with everything else and remapped to its own cell
- **Status**: resolved  *(replaces D2/U13)*
- **Rationale**: D2's "never rewrap the cursor's line" was impossible — the only non-rewrap
  outcome in a flat buffer is truncation. Under R1 the cursor's line is re-broken like any
  other and the cursor is remapped to the row and column holding the cell it was on.
  On the double prompt: zle repaints by the line count it *believes* it printed, computed at
  the old width, so a re-break can leave leading rows orphaned. That is a real risk and it is
  **recorded as a Known Gap rather than claimed solved** — pass 2 asserted SIGWINCH would
  handle it, and the re-review showed the reasoning does not hold. The clean fix needs OSC 133
  prompt-start marking, which this codebase does not have (`parser.rs:353` recognises `133;C`
  but consumes no payload; there is no `133;A` handler; `shell_integration.rs:85` installs only
  `133;D`) — so it is named as future work, not a refinement.
  **Rejected**: D2 (impossible), and asserting the repaint covers it (it may not).

### R5: `wrapped` marks the row that CONTINUES the one above
- **Status**: resolved  *(unchanged from U6)*
- **Rationale**: L1's research and pass 1's D10 disagreed — departing row vs arriving row.
  Settled on the arriving row, because that is the direction reflow reads: unwrapping walks
  forward while the next row continues, so the predicate belongs to the row being asked about,
  and a row can maintain a fact about itself but not about its successor. `row_soft_wraps` uses
  the departing polarity, so R10's migration inverts both loops in `links::wrap_run`
  (`links.rs:147-155`) — a one-time index change in the same commit.

### R6: The flag is maintained by one continuation bit, with its clear sites enumerated
- **Status**: resolved  *(answers re-review finding 3; the standard this spec set itself)*
- **Rationale**: Both prior passes hand-waved this. `print` writes at `cursor.col` and cannot
  tell a column-0 write that followed a wrap from one that followed CR/LF/`move_to`, and
  `pending_wrap` is already cleared before `line_feed()` runs (`grid.rs:455`). So the mechanism
  is a single one-shot `row_started_by_wrap` bit on the cursor: **set** at the two wrap
  consumption sites (`grid.rs:454-459` and the fit check at `:466-472`, inside their `autowrap`
  branch), **consumed** by the next `print` that writes column 0 — which stamps the row's flag
  and clears the bit — and **cleared without stamping** by every other path that repositions
  the cursor or restarts a row's content: CR (`:513`), LF (`:544`), BS (`:554`), TAB (`:561`),
  `move_to` (`:576`), `reverse_index` (`:747`), DECRC restore (`:808`), `clear` (`:1094`), the
  EL trio (`:1117-1135`), ICH/DCH (`:1158-1193`), IL/DL (`:709-739`), and the scroll fills
  (`:620`, `:644`). The invariant is testable and must be tested as one: **for every row,
  `wrapped` is true iff the write that produced its column-0 content came from a wrap
  consumption site.** **Rejected**: "true by construction" with no mechanism (struck down
  twice), and inferring it at reflow time from cell content (that is the heuristic R10 retires).

### R7: A wide pair is never split; the vacated column is `pen_blank()`
- **Status**: resolved  *(U9 plus the fill the re-review asked for)*
- **Rationale**: `print` already refuses to straddle a double-width char across the right margin
  (`grid.rs:461-472`) and `break_wide_pair_at` exists because an orphaned leader painted across
  two cells (`:473-483`). Reflow applies `print`'s own rule: a pair that will not fit moves
  whole to the next row. The vacated column is filled with `pen_blank()` — not
  `Cell::blank(default…)` — because the codebase draws that distinction deliberately
  (`grid.rs:586-596`, its own Known Gap G-A) and R9 makes the pen background load-bearing.
  **Rejected**: splitting then repairing (two rules for one invariant).

### R8: Image placements are remapped; reflow never routes through the scroll path
- **Status**: resolved  *(U10, with the re-review's contract question answered)*
- **Rationale**: `ImageLayer::scroll_up_one` is destructive at the top —
  `placements.retain(|p| p.row > 0)` (`image.rs:72-77`) — so reflow must not reuse it.
  `Placement.row` is a **viewport** row (`grid.rs:1107`) and `GridSnapshot.images` is populated
  only at `view_offset == 0` (`snapshot.rs:63-67`); that stays true. A placement whose row is
  still in the viewport after re-breaking is remapped to its new row and column; one pushed
  above the viewport is dropped — the same thing that happens to it today when the screen
  scrolls, so no type changes and `GridSnapshot`'s contract is untouched.
  **Rejected**: making `Placement.row` an absolute history index (pass 2 implied it without
  saying so — a boundary-contract change for a case that already has a defined behaviour).

### R9: Unwrapping strips only default-attribute trailing blanks, BCE included
- **Status**: resolved  *(carries the user's answer; keeps the BCE correction)*
- **Rationale**: Preserving every trailing blank doubles every short line when the width halves
  (`echo hello` at 80 columns is 5 glyphs and 75 blanks); stripping every trailing space eats a
  coloured block, which is blank text and meaningful content at once. So the predicate is
  "space **and** default attributes". Scroll-exposed rows are filled with `pen_blank()`
  (`grid.rs:589-596`), so after any program that sets a background and scrolls, every row ends
  in `cols` non-default-bg blanks — a naive predicate would preserve them and reproduce the
  doubling. The test is therefore against the *pen* background in force, not a hardcoded
  default, and it cannot reuse `Cell::is_blank`, which is `ch == ' '` only (`grid.rs:114`).
  Only the last row of a wrapped run has a trailing tail.
  **Known consequence** (re-review): stripped history rows read back through
  `.get(col).unwrap_or(grid.blank_cell())` (`snapshot.rs:48-51`), and `blank_cell()` is the
  *default* background (`grid.rs:1089`), so a coloured region loses its colour to the right of
  its last glyph. Recorded in Known Gaps.

### R10: `row_soft_wraps` is retired; the flag is the one wrap rule
- **Status**: resolved  *(unchanged from D5/U8)*
- **Rationale**: One rule, not two that can disagree — the existing comment says the heuristic
  is shared by copy and link-stitching precisely so they cannot drift, and a second rule beside
  it is that drift. Retiring it also fixes both of its documented errors: the false positive (a
  hard line filling the last column, `links.rs:32-51`) and the false negative (a CJK margin
  wrap leaves the last column blank, `snapshot.rs:132-133`), so copy and URL behaviour change
  in **both** directions. Requires `GridSnapshot` — "the versioned boundary type (C4)"
  (`snapshot.rs:1-5`) — to carry per-row wrap data, migrating both consumers in one change.
  **The steelman, and its price**: a heuristic recomputes from cell content and self-heals; a
  stored flag can be wrong until its row is evicted, and because it drives reflow its errors
  splice unrelated lines rather than adding a stray newline to a paste. R6's stated invariant
  plus a **property test asserting the flag agrees with the retired heuristic on every row
  where the heuristic is unambiguous** is what buys the trade. **Rejected**: keeping both.

### R11: In alt-screen, `History` reflows and the viewport resizes exactly as it does today
- **Status**: resolved  *(disambiguates U12 — re-review finding 4)*
- **Rationale**: Alt-screen here is "a history-preserving switch rather than a second buffer"
  (`grid.rs:900-905`). The TUI owns the viewport and repaints on SIGWINCH, so re-breaking it is
  wasted work that races the app's own redraw. "Left alone" was ambiguous between *untouched*
  (which R2 shows is a panic) and *resized as today*; it means the latter — the current
  truncate-and-pad path, unchanged, producing a correct `cols x rows` buffer the TUI
  immediately overwrites. `History` above it is primary content the TUI never touched and is
  read the moment the user quits and scrolls back, so it reflows.
  **Rejected**: reflowing the alt-screen viewport (races the redraw), and skipping history too
  (leaves scrollback mis-wrapped exactly when it is next read).

### R12: The cap applies after reflow; the loss and the multi-step ratchet are recorded
- **Status**: resolved  *(D4/U3, qualified as the re-review required)*
- **Rationale**: Narrowing lengthens the list and can push `History` past
  `DEFAULT_HISTORY_LIMIT = 100_000`, evicting the oldest rows; widening cannot restore them.
  The cap is not suspended — it is the one hard memory bound in a process that runs for weeks.
  Two qualifications the re-review demanded, both now in Known Gaps: the round trip is lossless
  **up to the cap**, not "by construction"; and because one drag crosses ~7 column steps and
  `refit` resizes every tab (`settings.rs:52-59`, `tabs.rs:152-159`), a single back-and-forth
  gesture can shed more history than "one narrowing" suggests. **Rejected**: exempting reflow
  growth from the cap.

## Constraints
- `term-core` stays GPU-free and window-free — no wgpu, no winit, directly or transitively
  (`.claude/rules/dependency-direction.md`). Every behaviour here must be headless-testable.
- **Never panics.** Every arithmetic path saturates and every field read has a typed fallback
  (`telemetry.rs` states the same invariant for its sibling). Reflow runs on the UI thread; a
  panic there takes the whole terminal with it.
- `GridSnapshot` is "the versioned boundary type (C4)" (`snapshot.rs:1-5`). D5 extends it
  deliberately, once, migrating both existing consumers in the same change.
- The `History` cap (`DEFAULT_HISTORY_LIMIT = 100_000`) is not suspended, conditioned, or
  raised — it is the one hard memory bound in a process that runs for weeks (D4).
- `input -> screen <= 8 ms` is the stated goal (`docs/domain.md`); D7's split exists to keep
  reflow inside it.
- macOS ARM64 only; no GUI crates.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass;
  `rustfmt` runs on every edited `*.rs` via the PostToolUse hook; `Cargo.lock` is hook-blocked.

## Known Gaps
- **The double prompt is not solved, only located** (R4). zle repaints by the line count it
  computed at the *old* width, so a re-break can orphan leading rows. The clean fix needs OSC
  133 prompt-start marking, which this codebase lacks — `parser.rs:353` recognises `133;C` but
  consumes no payload, there is no `133;A` handler, and `shell_integration.rs:85` installs only
  `133;D`. Named as future work, deliberately not claimed handled.
- **The round trip is lossless up to the cap, not by construction** (R12). Narrowing can evict
  the oldest rows and widening cannot restore them. And the loss ratchets: one drag crosses ~7
  column steps, each narrowing step can evict while each widening step restores nothing, and
  `refit` resizes every tab — so a single back-and-forth gesture sheds more than "one
  narrowing" suggests.
- **A stripped coloured region loses its colour past its last glyph** (R9). History rows read
  back through `.get(col).unwrap_or(grid.blank_cell())` (`snapshot.rs:48-51`) and `blank_cell()`
  is the default background (`grid.rs:1089`), so the stripped tail returns as default-coloured
  blanks. The direct consequence of R9's own rule, on the post-TUI scrollback R11 protects.
- **Copy and URL-stitching change in both directions** (R10) — the heuristic's false positive
  and its false negative are both fixed. Intended, but user-visible in two features this spec
  does not otherwise touch, and their tests move with it.
- **The flag is stored state where a self-healing heuristic used to be** (R10's steelman). A
  wrong flag persists until eviction and, because it drives reflow, splices lines rather than
  adding a newline to a paste. R6's invariant and R10's property test are mitigation, not
  elimination.
- **Reflow cost is reasoned, not measured.** R1 avoids pass 2's per-row-loop penalty on the hot
  paths, but the reflow pass itself is per-cell (R7 needs char width, R9 needs an attribute
  predicate) over up to 100k rows **per tab**. `Cell` is 16 bytes, so a full history is
  ~192 MB touched per pass. **L4 must measure it against the 8 ms goal before this ships**, and
  the fallback — reflow the viewport first, the rest before the next scroll — reintroduces the
  mixed-width hole and would need its own decision.
- **Reflow fires on every window resize, not just a panel toggle.** Confirmed intended, but it
  is a visible change on a gesture that is currently quiet. Note `input::needs_refit`
  (`input.rs:341`) compares the whole `(cols, rows)` tuple, so it does **not** gate on columns —
  a cols-only gate is a requirement on the new `resize`, not something inherited.

## L2 Review History (two failed passes — kept for provenance)

Two independent reviews, both read-only, both verified against source. Neither pass shipped.

### Pass 1 — D1-D11, took the viewport/`History` split as given. NEEDS_FIX, 9 findings.

| # | Finding | Answered by |
|---|---|---|
| 1 | **D2 impossible.** "Leave the cursor's line alone" cannot mean unchanged — `Grid` is a flat `cols x rows` buffer (`grid.rs:317`), so the only non-rewrap outcome is truncation, the defect this feature removes. Also fails on background tabs, where `refit` resizes a grid whose cursor a running program parked anywhere. | **R4** |
| 2 | **Flag polarity self-contradictory** — research set it on the departing row, D10 on the arriving row. `links::wrap_run`'s migration depends on which. | **R5** |
| 3 | **Mixed widths.** Eager push + deferred body reflow leaves `History` holding both widths; `from_grid` truncates a wider row silently (`snapshot.rs:48-51`). | **R1** (eager) |
| 4 | **The scroll offset was uncomputable** — it needed a post-reflow layout the lazy pass had deferred. | **R3** |
| 5 | **Wide chars unaddressed** — a re-break splits leader/`CONTINUATION` pairs into a state `print` cannot produce (`grid.rs:461-483`). | **R7** |
| 6 | **Images unaddressed** — `scroll_up_one` is destructive at the top (`image.rs:72-77`). | **R8** |
| 7 | **"True by construction" unsupported** — nothing records that the cursor reached column 0 *by* a wrap. | **R6** |
| 8 | **Steelman unanswered** — a stored flag replaces a self-healing heuristic, and drives reflow, so its errors are structural not cosmetic. | **R10** + property test + Known Gaps |
| 9 | **The alternative never considered:** unify viewport and `History` into one row list. | tried as pass 2, then **rejected** — see below |

### Pass 2 — U1-U13, adopted the unified row list. NEEDS_FIX, and its steelman killed the premise.

The reviewer's decisive argument: **the split was never the problem — laziness was.** An eager
reflow needs no new data structure, because `Grid::resize` already allocates a fresh buffer and
repopulates it (`grid.rs:1286-1292`); reflow is a change to that loop. Unifying, by contrast,
converts four `copy_within` memmoves (`grid.rs:618/641/1167/1186`), two contiguous erase spans
(`:1269/:1277`) and the wide-pair arithmetic (`:1220-1256`) into per-row loops over
non-contiguous `VecDeque` slots — a permanent cost on paths the codebase itself calls hot
(`grid.rs:478`: "`print` is the hot path"), paid to simplify an operation that runs ~7 times
per drag. U1's affordability argument counted uses of one helper (`Grid::index`, 16) and never
mentioned contiguity.

It also introduced a **panic** the split model did not have: U11 (strip trailing blanks) and
U12 (alt-screen viewport unreflowed) both yield rows shorter than `cols`, which reach indexed
viewport writes (`grid.rs:484/491/494`) — against this spec's own "never panics" constraint.
That is now closed by **R2**, which states the viewport-width invariant explicitly.

Other pass-2 findings and where they land: U4's "row identity" was unrepresentable (`Row` is a
bare `Vec<Cell>` and reflow splits the very rows it would anchor to) → **R3**; U12's
"left alone" was ambiguous between untouched and resized → **R11**; U10 silently required
`Placement.row` to become an absolute index → **R8** keeps it a viewport row; U5's cost was
mischaracterised as "memmove-class" when R7 and R9 both need per-cell work → recorded in
Known Gaps as explicitly unmeasured, with L4 required to measure it.

### Complexity
Reclassified **Complex (4 signals)**, all four named: (1) a structural change to `term-core`
plus a `GridSnapshot` boundary-contract extension; (2) multi-actor — zsh and TUIs are a second
writer of the buffer being reflowed, which is the only reason R4 and R11 exist; (3)
cross-cutting into four sibling features whose tests move (copy/selection, hyperlinks, images,
scrollback); (4) irreversible data-loss paths (cap eviction, per tab).
