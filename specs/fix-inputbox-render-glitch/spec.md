# Spec: fix-inputbox-render-glitch

## Meta
- **Created**: 2026-10-18
- **Type**: dev
- **Status**: approved
- **Mode**: unattended
- **Approved by**: unattended (request marker)
- **Approved at**: 2026-10-18

## Goal
Fix input line character corruption and cursor desync in TUI input boxes (like Antigravity / bubbletea).
When typing in an interactive prompt following wide CJK text, letters appear staggered with empty gap cells (e.g.
'fi e d' instead of consecutive letters) and stray trailing characters remain after cursor moves or line redraws.
Investigate term-core grid wide-character continuation handling (break_wide_pair_at, repair_wide_pairs,
clear_line_from_cursor) and renderer row_chars column indexing.

## Non-goals
- Full complex grapheme cluster segmentation, ZWJ sequences, or combining mark stacking beyond standard display width rules (non-goal preserved from `wide-chars`).
- Modifying GUI or introducing UI widget crates (prohibited by `.claude/rules/no-gui-crates.md`).
- Overhauling font fallback or swash glyph rasterization outside of column indexing and wide continuation positioning.

## Confirmed Goal
Eliminate character corruption, staggered empty gap cells, and stray trailing characters when typing, moving the cursor, or redrawing lines following wide CJK text in interactive prompts (such as bubbletea / Antigravity CLI). (inferred)
In `term-core`:
- Harden `break_wide_pair_at` so overwriting either half of a wide character cleanly blanks the partner and never leaves orphan continuation spacers or orphaned leaders when single-width or double-width characters are printed. (inferred)
- Ensure all line and display clear operations (`clear_line_from_cursor`, `clear_line_to_cursor`, `clear_line`, `clear_from_cursor`, `clear_to_cursor`) repair wide pairs and correctly handle cursor positions at or past wide character boundaries without corrupting adjacent cells or dropping trailing clears. (inferred)
- Verify `repair_wide_pairs` guarantees that no cell with `Cell::CONTINUATION` is ever left without an immediate double-width leader to its left. (inferred)
In `renderer`:
- Audit and harden `row_chars` column indexing in `crates/renderer/src/lib.rs` so that only valid continuation spacers (preceded by a double-width leader) are skipped, preventing stray or unaligned spacers from introducing staggered empty gap cells (`'fi e d'`) and column desync during cluster placement. (inferred)
- Validate end-to-end with unit and snapshot tests reproducing TUI input box sequences (prompt with CJK text, character typing, cursor movement, line clearing, and redraw). (inferred)

## Research
- `crates/term-core/src/grid.rs:560-596`: `Grid::print` calls `break_wide_pair_at(col, row)` and if `w == 2` also `break_wide_pair_at(col + 1, row)`. Overwriting a spacer blanks `col - 1` with `fill`, but does not blank `cells[idx]` if not immediately overwritten.
- `crates/term-core/src/grid.rs:1480-1515`: `clear_line_from_cursor` calls `blank_span(start, end)` which blanks cells and runs `repair_wide_pairs(row, blank)`. When `cursor.col` is on a spacer, `start` begins at the spacer, causing `repair_wide_pairs` to blank the leader at `cursor.col - 1`. If `cursor.col >= cols`, `blank_span` returns early without clearing.
- `crates/term-core/src/grid.rs:1604-1626`: `repair_wide_pairs(row, fill)` scans from 0 to `cols - 1`. A spacer is treated as an orphan if `i == 0 || self.cells[idx - 1].ch.width().unwrap_or(0) != 2`. A leader is treated as an orphan if `i + 1 >= cols || !self.cells[base + i + 1].is_continuation()`.
- `crates/term-core/src/grid.rs:1631-1644`: `clear_from_cursor` (`CSI 0 J`) and `clear_to_cursor` (`CSI 1 J`) directly write blank cells without calling `repair_wide_pairs`, leaving broken wide pairs on the boundary row.
- `crates/renderer/src/lib.rs:1605-1630`: `row_chars` iterates over `0..snapshot.cols`, skipping columns where `is_continuation(snapshot, col, row)` is true. `is_continuation` only checks `cell.is_continuation()` without verifying that `col > 0` and `col - 1` is a valid wide leader. Any orphan continuation cell causes `col` to be dropped from `char_col` and `chars`, producing an unrendered gap on screen.
- `crates/renderer/src/lib.rs:1759-1823`: In `build_glyph_instances`, `byte_col` maps run bytes to absolute columns from `char_col`. For each cluster, `pen` is anchored to `col * cw + offset`. If a column was skipped in `char_col` due to a continuation cell, subsequent clusters jump columns, creating staggered gaps (`'fi e d'`).
- `crates/common/src/config.rs:56`: `FontConfig::default().ligatures` defaults to `false` (via `specs/ligatures-default-off`), confirming that gap cells in standard runs are caused by column skips rather than ligature collapse.

## Decisions

### D1: Strict leader-validated continuation filtering in `renderer::row_chars`
- **Status**: assumed
- **Rationale**: `row_chars` must only skip a column as a continuation spacer if `col > 0` and `snapshot.cell(col - 1, row)` holds a valid double-width leader (`ch.width() == Some(2)` and not a continuation itself). Any stray or un-partnered `Cell::CONTINUATION` cell is treated as a blank space (`' '`) at its literal column `col`. This prevents orphan continuation cells from punching unrendered holes into `char_col`, which was the direct cause of staggered empty gap cells (`'fi e d'`) and column desync during cluster placement.
  *Steelman counterargument addressed*: Why add defense-in-depth to the renderer rather than relying exclusively on `term-core` repairs (D2/D3)?
  1. *Presentation resilience*: While `term-core` enforces invariants at mutation boundaries, frame captures occur asynchronously during PTY streaming. A renderer must be fundamentally fail-soft: it must never produce visually corrupted glyph layouts, staggered letter gaps, or out-of-sync cluster advances even when encountering unexpected or transient cell states.
  2. *Negligible overhead*: `snapshot.cell(col - 1, row)` is a direct contiguous slice index (`row * cols + col - 1`) resident in L1 cache with zero heap allocations, taking < 1 CPU cycle per column.
  3. *Reconciliation with clipboard*: Addressed in D6 below — text extraction and clipboard selection adopt the identical rule so visual rendering and copied text never desync.
  Rejected alternative: unconditionally skipping any cell with `is_continuation()` (status quo; causes ghost gaps and column desync whenever an orphan spacer is present), or panicking on orphan continuation (unacceptable in a resilient terminal).

### D2: Comprehensive wide-pair repair across all clear operations (`clear_line_from_cursor`, `clear_from_cursor`, `clear_to_cursor`)
- **Status**: assumed
- **Rationale**: TUI line redraws (bubbletea, readline, etc.) frequently issue `clear_line_from_cursor` (`CSI 0 K`) or display clears (`CSI 0 J`, `CSI 1 J`). `blank_span` must safely handle `cursor.col >= cols` without dropping the clear, and if `clear_line_from_cursor` starts on a continuation spacer, the orphaned leader at `cursor.col - 1` must also be blanked so no half-glyph is rendered. Furthermore, `clear_from_cursor` (`CSI 0 J`) and `clear_to_cursor` (`CSI 1 J`) must call `repair_wide_pairs` on the boundary cursor row, preventing bisected wide characters from surviving as stray trailing glyphs after line redraws. Rejected alternative: omitting wide pair repair from `CSI J` (leaves broken pairs whenever display clears bisect wide chars), or moving the cursor left during clear (violates standard VT erase semantics).

### D3: Symmetric pair breaking in `break_wide_pair_at` and strict row repair in `repair_wide_pairs`
- **Status**: assumed
- **Rationale**: `break_wide_pair_at` operates in O(1) in the `print` hot path. When called at `(col, row)`, if `cells[idx]` is a continuation spacer, it blanks the leader at `col - 1`. If `cells[idx]` is a double-width leader, it blanks the spacer at `col + 1`. In addition, when printing a double-width character (`w == 2`), breaking at `col + 1` must clean up any existing leader or spacer at `col + 1` and its partner. `repair_wide_pairs` guarantees row-level invariants after all bulk mutations: every spacer has a wide leader to its left, and every wide leader has a spacer to its right. Rejected alternative: running full-row `repair_wide_pairs` inside `print` (too expensive for the character output hot path).

### D4: Zero changes to `Cell` memory layout and `GridSnapshot` serialization
- **Status**: assumed
- **Rationale**: `Cell::CONTINUATION` remains `'\0'` and `Cell` size/alignment is unmodified. This preserves zero-copy cache comparisons in `idle-cpu`, snapshot instancing in `renderer`, and copy/paste extraction in `to_text()`. Rejected alternative: adding new flags or enums to `Cell` (bloats memory and breaks cache contracts).

### D5: Exact cell-span validation in renderer cluster placement
- **Status**: assumed
- **Rationale**: In `crates/renderer/src/lib.rs`, `cell_span(snapshot, col, row)` must return 2 only when `snapshot.cell(col, row)` is a valid wide leader AND `snapshot.cell(col + 1, row)` is its continuation spacer. If an orphan continuation exists at `col + 1` without a wide leader at `col`, `cell_span` returns 1, ensuring fallback fonts and center offsets do not distort single-width glyphs. Rejected alternative: relying solely on `col + 1` being a continuation cell (causes single-width characters preceding an orphan spacer to receive a 2-cell span).

### D6: Unified leader-validated continuation handling across text extraction (`to_text`, `selection_text`)
- **Status**: assumed
- **Rationale**: `GridSnapshot::to_text` and `Grid::selection_text` must mirror D1's leader validation: only skip a continuation cell if `col > 0` and the preceding cell is a double-width leader. If an un-partnered orphan continuation cell is encountered, it is treated as a space (`' '`), exactly matching what `renderer::row_chars` displays on screen. This guarantees 100% parity between on-screen glyph positions, clipboard copy, and snapshot regression tests, eliminating visual vs selection desync. Rejected alternative: leaving text extraction to unconditionally skip `Cell::CONTINUATION` (would cause clipboard copy to drop columns that the renderer visibly displays).

## Constraints
- **Zero GUI crates**: Must not introduce winit/wgpu dependencies into `term-core` (`.claude/rules/dependency-direction.md`, `.claude/rules/no-gui-crates.md`).
- **Performance**: `Grid::print` must retain O(1) pair-breaking; row-wide repair is limited to clear/shift operations (`CSI K`, `CSI J`, `CSI @`, `CSI P`, `CSI X`).
- **No regression to existing test suite**: All 172+ workspace unit tests, snapshot tests, and VT compliance tests must remain 100% green.

## Known Gaps
- (none)

## Requirements

### R0: Elimination of TUI input box glyph corruption and cursor desync — D1, D2, D3, D4, D5, D6

#### R0.1: Interactive prompt simulation with CJK text followed by typing consecutive characters
- **Given**: A `term_core::Terminal` configured with an 80×24 grid
- **When**: In `Terminal::advance`, the terminal consumes the byte sequence `\r[한국어] field\x1b[K`
- **Then**: In `GridSnapshot::to_text` (tested in `crates/term-core/tests/vt_tui.rs`) and in `renderer::row_chars` (tested in `crates/renderer/tests/tui_inputbox.rs`), the characters `'f'`, `'i'`, `'e'`, `'l'`, `'d'` occupy consecutive columns 9, 10, 11, 12, 13 without empty gap cells or staggered offsets

#### R0.2: Prompt line redraw with clear-to-end-of-line leaves no stray trailing characters
- **Given**: A grid row previously holding `[한국어] longer_previous_input`
- **When**: In `Terminal::advance`, the terminal consumes `\r[한국어] short\x1b[K`
- **Then**: All cells past column 14 up to column 79 hold blank spaces with default attributes, and no stray characters from `longer_previous_input` remain

### R1: Leader-validated continuation filtering in renderer — D1, D5

#### R1.1: Valid wide pair skips continuation column in `row_chars`
- **Given**: A `GridSnapshot` row holding `'한'` at column 0 and `Cell::CONTINUATION` at column 1
- **When**: Public function `renderer::row_chars(snapshot, row)` processes the row
- **Then**: `RowChars::chars` contains `['한']` and `RowChars::char_col` contains `[0]`, skipping column 1 from shaping input

#### R1.2: Orphan continuation cell without leader is treated as blank space in `row_chars`
- **Given**: A `GridSnapshot` row holding single-width `'a'` at column 0 and an orphan `Cell::CONTINUATION` at column 1
- **When**: Public function `renderer::row_chars(snapshot, row)` processes the row
- **Then**: `RowChars::chars` contains `['a', ' ']` and `RowChars::char_col` contains `[0, 1]`, preventing a column drop from staggering subsequent character positions

#### R1.3: `cell_span` returns 2 only for double-width leader followed by continuation spacer
- **Given**: A `GridSnapshot` row holding single-width `'x'` at column 2 and `Cell::CONTINUATION` at column 3
- **When**: Public function `renderer::cell_span(snapshot, 2, row)` is evaluated
- **Then**: It returns 1, ensuring the cluster layout for `'x'` does not receive a 2-cell span

### R2: Comprehensive wide-pair repair on clear and display operations — D2

#### R2.1: `clear_line_from_cursor` starting on a continuation spacer blanks the orphan leader
- **Given**: A grid row holding double-width `'글'` at column 4 with continuation spacer at column 5, and cursor at column 5
- **When**: Function `Grid::clear_line_from_cursor` (triggered by VT escape `CSI 0 K`) executes
- **Then**: Both column 4 (the orphaned leader) and column 5 (the spacer) are overwritten with blank cells, and columns 5..cols are cleared

#### R2.2: `clear_line_from_cursor` when `cursor.col >= cols` does not drop clear or corrupt grid
- **Given**: A grid where `cursor.col` equals or exceeds `cols - 1` with `pending_wrap` active
- **When**: Function `Grid::clear_line_from_cursor` (VT escape `CSI 0 K`) executes
- **Then**: The last column is blanked and wide pairs on the row remain intact without early exit or out-of-bounds indexing

#### R2.3: `clear_from_cursor` (`CSI 0 J`) repairs bisected wide characters on cursor row
- **Given**: A grid holding double-width `'한'` at columns 2..=3 on row 1, with cursor positioned at `(3, 1)`
- **When**: Function `Grid::clear_from_cursor` (VT escape `CSI 0 J`) executes
- **Then**: Column 2 on row 1 is repaired to blank so no orphaned half-glyph remains on screen

#### R2.4: `clear_to_cursor` (`CSI 1 J`) repairs bisected wide characters on cursor row
- **Given**: A grid holding double-width `'한'` at columns 2..=3 on row 1, with cursor positioned at `(2, 1)`
- **When**: Function `Grid::clear_to_cursor` (VT escape `CSI 1 J`) executes
- **Then**: Column 3 on row 1 is repaired to blank so no orphaned continuation spacer remains on screen

### R3: Hot-path pair breaking and bulk row repair in term-core — D3

#### R3.1: `break_wide_pair_at` called before printing narrow char over spacer blanks the leader
- **Given**: A grid cell at `(col, row)` holding `Cell::CONTINUATION` whose leader at `(col - 1, row)` is double-width
- **When**: Function `Grid::print` executes with a single-width character at `(col, row)`
- **Then**: The leader at `(col - 1, row)` is replaced with a blank cell carrying the active pen background before writing `(col, row)`

#### R3.2: `break_wide_pair_at` called before printing narrow char over leader blanks the spacer
- **Given**: A grid cell at `(col, row)` holding a double-width character with its spacer at `(col + 1, row)`
- **When**: Function `Grid::print` executes with a single-width character at `(col, row)`
- **Then**: The spacer at `(col + 1, row)` is replaced with a blank cell carrying the active pen background before writing `(col, row)`

#### R3.3: `break_wide_pair_at` called when printing double-width char cleans both target cells and their partners
- **Given**: A grid row with double-width characters at columns 0..=1 and 2..=3, and cursor at column 1
- **When**: Function `Grid::print` executes with a double-width character at column 1
- **Then**: Column 0 is blanked, column 1 receives the new wide leader, column 2 receives the new continuation spacer, and column 3 is blanked

#### R3.4: `repair_wide_pairs` eliminates orphan spacers and orphan leaders across a row
- **Given**: A row containing an orphan `Cell::CONTINUATION` at column 2 (preceded by single-width `'a'`) and an orphan wide leader at column 6 (followed by `'b'`)
- **When**: Public method `Grid::repair_wide_pairs(row, fill)` executes
- **Then**: Both column 2 and column 6 are replaced with `fill`, leaving no invalid continuation or orphan leader on the row

### R4: Consistent text extraction and selection parity with visual rendering — D4, D6

#### R4.1: `GridSnapshot::to_text` extracts orphan continuation cell as `' '` instead of omitting
- **Given**: A `GridSnapshot` containing an orphan `Cell::CONTINUATION` at column 1 preceded by single-width `'a'` at column 0 and followed by `'b'` at column 2
- **When**: Method `GridSnapshot::to_text` executes
- **Then**: The extracted string contains `"a b"`, preserving the column offset and matching the visual rendering

#### R4.2: `Grid::selection_text` extracts orphan continuation cell as `' '` instead of omitting
- **Given**: A text selection spanning a column holding an orphan `Cell::CONTINUATION`
- **When**: Method `Grid::selection_text` extracts the selected string
- **Then**: The extracted string contains a space `' '` for the orphan cell, ensuring clipboard copy matches the on-screen rendered column position

#### R4.3: Memory layout of `Cell` is unchanged
- **Given**: The `term_core::Cell` struct
- **When**: Checked at compile time via `std::mem::size_of::<Cell>()` and field offsets
- **Then**: `Cell` remains 16 bytes with `Cell::CONTINUATION == '\0'`, preserving cache alignment and `idle-cpu` hashing

## Tasks

### T1: Core grid repairs and renderer test API exports [infra]
- **Fulfills**: R0, R2, R3
- **Depends on**: (none)
- Expose `break_wide_pair_at(&mut self, col: u16, row: u16)` and `repair_wide_pairs(&mut self, row: u16, fill: Cell)` as `pub` methods on `Grid` in `crates/term-core/src/grid.rs` so integration tests can exercise them directly.
- Implement symmetric pair breaking in `break_wide_pair_at` ensuring robust O(1) partner clearing on `Grid::print`.
- Ensure `clear_line_from_cursor` (`CSI 0 K`), `clear_from_cursor` (`CSI 0 J`), and `clear_to_cursor` (`CSI 1 J`) repair wide pairs on boundary rows and handle `cursor.col >= cols` cleanly without skipping clears.
- In `crates/renderer/src/lib.rs`, export `RowChars` with public fields (`pub struct RowChars { pub chars: Vec<char>, pub char_col: Vec<u16> }`), `pub fn row_chars(...)`, and `pub fn cell_span(...)` so downstream acceptance tests (TT1) compile cleanly under TDD before T2 updates their logic.

### TT1: Acceptance tests for R1's GWTs (leader-validated continuation filtering in renderer) [tests]
- **Fulfills**: R1 (tests)
- **Depends on**: T1
- Creates new test file `crates/renderer/tests/tt1_renderer_continuation_tests.rs` testing `renderer::row_chars` and `renderer::cell_span` (exposed by T1) with valid pairs, orphan continuation cells, and single-width characters, asserting `row.chars` and `row.char_col`.

### TT2: Acceptance tests for R2's GWTs (wide-pair repair on clear and display operations) [tests]
- **Fulfills**: R2 (tests)
- **Depends on**: T1
- Creates new test file `crates/term-core/tests/tt2_clear_repair_tests.rs` testing `clear_line_from_cursor`, `clear_from_cursor`, and `clear_to_cursor` across wide character boundaries.

### TT3: Acceptance tests for R3's GWTs (hot-path pair breaking and bulk row repair in term-core) [tests]
- **Fulfills**: R3 (tests)
- **Depends on**: T1
- Creates new test file `crates/term-core/tests/tt3_pair_breaking_tests.rs` testing `Grid::break_wide_pair_at` and `Grid::repair_wide_pairs` on narrow-over-spacer, narrow-over-leader, and wide-over-split cases.

### TT4: Acceptance tests for R4's GWTs (consistent text extraction and selection parity) [tests]
- **Fulfills**: R4 (tests)
- **Depends on**: T1
- Creates new test file `crates/term-core/tests/tt4_text_extraction_parity_tests.rs` asserting `to_text`, `selection_text`, and `Cell` 16-byte memory layout.

### T2: Leader-validated continuation filtering and exact cell span in renderer [vertical]
- **Fulfills**: R0, R1
- **Depends on**: T1, TT1
- Modify `row_chars` in `crates/renderer/src/lib.rs` to only skip continuation cells when preceded by a valid double-width leader (`col > 0 && snapshot.cell(col - 1, row).map_or(false, |c| c.ch.width() == Some(2) && !c.is_continuation())`).
- Ensure orphan continuation cells are emitted as space `' '` at column `col`, preserving column alignment in `char_col`.
- Update `cell_span` in `crates/renderer/src/lib.rs` to verify that `col` is a double-width leader before granting a 2-cell span.

### T3: Text extraction and selection parity with visual rendering in term-core [vertical]
- **Fulfills**: R4
- **Depends on**: T1, TT4
- Update `GridSnapshot::to_text` in `crates/term-core/src/snapshot.rs` and `Grid::selection_text` in `crates/term-core/src/grid.rs` to mirror `row_chars`: only skip continuation cells when preceded by a valid double-width leader, emitting space `' '` for orphan continuation cells.

### T4: End-to-end TUI input box interactive prompt regression suite [vertical]
- **Fulfills**: R0
- **Depends on**: T2, T3, TT2, TT3
- Add end-to-end integration tests in `crates/term-core/tests/vt_tui.rs` (testing terminal grid state, `to_text`, and cursor positioning) and in `crates/renderer/tests/tui_inputbox.rs` (testing rendered `row_chars` and glyph instance layout) simulating bubbletea / Antigravity CLI input box flows: CJK prompt followed by consecutive typing, cursor backward/forward moves, line clearing, and line redraws asserting zero gap cells and zero stray trailing characters, respecting crate dependency direction.

## External Dependencies

### Pre-work
- (none)

### Post-work
- (none)

## Spec Review
- **Verdict**: PASS
- **Reviews**: 3
- **Fixed**:
  - [Coverage] T1, T2 / R0 — Added R0 to Fulfills for T1 and T2 so implementation tasks build the goal-level fix.
  - [Grounding] R1.1-R1.3, T1, T2 — Exported `RowChars` with public fields (`pub chars: Vec<char>`, `pub char_col: Vec<u16>`), `pub fn row_chars`, and `pub fn cell_span` in `crates/renderer/src/lib.rs` under infra task T1 before TT1 acceptance tests compile.
  - [Grounding] R3.4, TT3, T1 — Exported `Grid::break_wide_pair_at` and `Grid::repair_wide_pairs` as `pub` on `Grid` in `crates/term-core/src/grid.rs`.
  - [Consistency] R0.1, T4 — Split integration tests between `crates/term-core/tests/vt_tui.rs` (`GridSnapshot::to_text`) and `crates/renderer/tests/tui_inputbox.rs` (`renderer::row_chars`), maintaining crate dependency direction (`term-core` does not depend on `renderer`).
  - [Readiness] TT1 — Ensured `RowChars`, `row_chars`, and `cell_span` are exposed in infra task T1 so TT1 compiles cleanly under TDD.
- **Open**:
  (none)
