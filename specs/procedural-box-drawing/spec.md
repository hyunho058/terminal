# Spec: procedural-box-drawing

## Meta
- **Created**: 2026-07-25
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-25
- **Branch**: `feat/procedural-box-drawing`
- **L2 review**: 2 adversarial passes (maker ≠ checker). Round 1 overturned D2's
  rationale and narrowed D3; round 2 caught the D5 stale premise and the dash
  collapse. The final end-clamp fix was verified by hand, not by a third review
  pass — the skill's 2-retry budget was spent.

## Goal
Intercept U+2500–257F (Box Drawing) and U+2580–259F (Block Elements) before the
per-face shaping run loop in `crates/renderer/src/lib.rs:1093` and emit them as
solid quads snapped to the cell rectangle `(col*cw, row*ch, cw, ch)`, reusing
`Atlas::solid_entry()` (`crates/renderer/src/atlas.rs:242`). Quadrant characters
become a 4-rect half-cell decomposition whose boundaries are `round()`-snapped so
neighbouring cells share integer pixel edges. Box art must tile seamlessly instead
of inheriting the font's per-glyph rasterization seams.

## Non-goals
- **Font-metric-driven cell sizing.** Replacing the hardcoded `0.6`/`1.2` in
  `cell_size_px` (`lib.rs:1268`) and `ascent = ch * 0.8` (`lib.rs:1076`) with
  `swash` `font.metrics()` values is a separate spec (별건). The tiling defect this
  spec targets is solved by the procedural path regardless of cell metrics.
- **U+1FB00–1FBFF (Symbols for Legacy Computing).** Currently renders blank because
  no installed macOS font covers the range. Documented as a follow-up, not built here.
- **Braille (U+2800–28FF), Powerline (U+E0B0–E0BF), and other pseudo-graphic ranges.**
  Out of range; unchanged.
- **`term-core` changes.** The grid, parser, and `GridSnapshot` are untouched — this
  is a renderer-local presentation change (`dependency-direction` rule).
- **New dependencies.** No new crates; geometry is hand-rolled arithmetic
  (`no-gui-crates` rule).

## Confirmed Goal
Box-drawing and block-element characters (U+2500–259F) are drawn procedurally by
the renderer as cell-snapped quads rather than shaped through the font, so that:

1. Multi-cell box art (tmux/btop frames, `─│┌┐└┘├┤┬┴┼` tables) renders as
   continuous lines — no hairline gaps at cell joins, no double-thick seams — at
   every font size and at both 1x and 2x scale factors.
2. Block elements (`█▀▄▌▐` + quadrants `▘▝▖▗▚▞▛▜▟▙`) fill exactly their cell region
   with integer-aligned edges shared with adjacent cells.
3. Shading blocks `░▒▓` (U+2591–2593) render as full-cell fills at 25%/50%/75%
   intensity, tiling seamlessly with their solid neighbours.
4. Every character outside U+2500–259F renders byte-identically to before — the
   ASCII/Latin/Hangul shaping path is not perturbed.
5. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
   stay green.

## Research

### The interception point
- `build_glyph_instances` (`crates/renderer/src/lib.rs:1066`) is the single function
  that turns a `GridSnapshot` into `Vec<GlyphInstance>`. Per row it calls
  `row_chars` → `atlas.resolve` per char → `split_runs(&faces)` → the run loop at
  `lib.rs:1093`. Intercepting *before* that loop means box chars never enter a
  `Run`, never reach `swash`, and never touch the face-resolution path.
- `row_chars` (`lib.rs:1007`) already drops double-width continuation spacers and
  carries `char_col[i]` = absolute column. Box/block chars are all narrow (span 1),
  so a procedural pass can key off `char_col` directly.
- `split_runs` (`lib.rs:968`) groups by `Option<face>`; a char with no covering face
  yields `face: None` and the run emits nothing (`lib.rs:1097`). Today an
  uncovered block char therefore renders **blank**, not tofu.

### The emission mechanism already exists
- The underline pass (`lib.rs:1172–1198`) is a working precedent for a non-shaped
  solid quad: it takes `atlas.solid_entry(queue)` (`atlas.rs:242`), samples the
  **center** of the 2×2 opaque block so linear filtering can't pick up a neighbour
  texel, and pushes a `GlyphInstance` with an arbitrary `pos`/`size` rect.
- `Atlas::solid_entry` packs a 2×2 block of coverage `255` and memoizes it as
  `Option<(GlyphEntry, u64 generation)>` (`atlas.rs:88, 242`), re-reserving it after
  an eviction bumps `generation`. Any partial-coverage variant would follow the
  same shape.
- `glyph.wgsl:46` computes `a = textureSample(...).r` and returns
  `vec4(color * a, a)` — premultiplied. So a **coverage value below 255 is exactly
  an alpha multiplier**, with no shader change needed.

### Constraint on per-instance alpha
- `GlyphInstance` is `pos/size/uv0/uv1/color:[f32;3]/_pad` (`lib.rs:77`) and the
  vertex layout stops at `5 => Float32x3` (`lib.rs:446`) — `_pad` is **not** wired
  to the shader. Per-instance alpha would require widening the attribute to
  `Float32x4` *and* editing `glyph.wgsl`. `GlyphInstance` is private to `lib.rs`
  (no sibling layer constructs it), so such a change is contained — but the
  partial-coverage-atlas-entry route avoids it entirely.

### Color semantics to reuse
- `glyph_color(cell)` (`lib.rs:928`) resolves REVERSE (fg/bg swap) and DIM (mix
  toward bg by `DIM_MIX = 0.45`) into a linear-light `[f32;3]`. Procedural quads
  must use this same function or box art would ignore reverse-video and dim.
- `build_bg_instances` (`lib.rs:897`) emits a bg quad only when the effective bg
  differs from `bg_default`; default-bg cells show the clear color. So a CPU-side
  "blend fg toward bg" trick for shading would need the *theme default*, not just
  `cell.bg` — a real argument against that route.

### Placement arithmetic and the seam
- Baseline is `row * ch + ascent` with `ascent = ch * 0.8` (`lib.rs:1076, 1091`),
  and glyphs place at `pen + g.x + entry.left, baseline - entry.top`. Every term
  there is unrounded float from swash's rasterizer — that fractional placement is
  precisely what makes adjacent `─` cells not meet. Cell size *is* integral
  (`cell_size_px` rounds, `lib.rs:1270`), so snapping rects to
  `(col*cw, row*ch, cw, ch)` and `round()`ing internal boundaries gives neighbours
  bit-identical shared edges.

### Testability precedent
- `lib.rs` has both pure-function unit tests (`center_offset`, `split_runs`,
  `row_chars` — `lib.rs:1622–1705`) and GPU tests guarded by `headless()`
  (`lib.rs:1309`), which return early with `eprintln!("skipping: no GPU/font")` when
  no adapter/font exists. `ascii_placement_matches_single_font_shaping`
  (`lib.rs:1721`) is the exact template for a "non-box output is unchanged"
  regression test.
- The renderer has **no** `tests/` dir; all renderer tests live in `#[cfg(test)] mod
  tests` inside each module. `term-core` owns the insta snapshots.

### Cost / caching
- `draw_to` re-shapes only when `last_snapshot != snapshot` (`lib.rs:746`), then
  re-presents resident vertex buffers. Procedural rects are built inside
  `build_glyph_instances`, so they inherit that cache — no per-frame cost added at
  idle (protects G1 / the idle-CPU fix).

### Environment
- Default font is **Menlo** at 14pt, ligatures off (`crates/common/src/config.rs:86–92`).
  Menlo covers most of U+2500–257F, so those chars render today (with seams);
  coverage stops mattering once they're intercepted.
- Build/test/lint: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings` (CLAUDE.md).
- Rules in force: `no-gui-crates` (hand-rolled geometry, no new deps),
  `dependency-direction` (`term-core` untouched), `macos-only`.

## Decisions

### D1: Stem thickness is a fraction of cell HEIGHT, with heavy = 2× light
- **Status**: resolved
- **Rule**: `light = max(1.0, (ch * 0.075).round())`; `heavy = 2.0 * light`;
  a double stem is `light` + `light` gap + `light`. One scalar derived from `ch`
  feeds **both** axes, so the arms of `┼` are equal weight by construction.
  Worked values: 14pt @2x (cell 17×34) → light 3px, heavy 6px, double 3+3+3;
  14pt @1x (cell 8×17) → light 1px, heavy 2px, double 1+1+1.
- **Rationale**: line weight has to track font size — a fixed logical hairline
  (rejected) looks spindly against 24pt text, and tying thickness to cell *width*
  (rejected) makes the weight jump if a font's aspect ratio differs from Menlo's.
  `max(1.0, …)` is also what keeps a stem visible at tiny cell sizes (E3).

### D2 (revised after L2 review): Intercept all of 2500–257F except the three diagonals, plus all of 2580–259F
- **Status**: resolved
- **In (procedural)**: **`2500–2570` and `2574–257F`** — solid stems, corners, tees
  and crosses (light/heavy/double), **dashed variants** (`2504–250B`, `254C–254F`),
  **rounded corners** (`256D–2570`, drawn *square*), and single-arm stubs — plus
  **all** of `2580–259F` (halves, eighths, full block, shading, quadrants).
- **Out (keeps shaping through `swash`, exactly as today)**: the diagonals
  `2571–2573` (`╱╲╳`) only.
- **Dash geometry**: `n` dashes across the cell extent, `n` from the codepoint
  (2 for `254C–254F`, 3 for `2504–2507`, 4 for `2508–250B`; orientation comes from
  `Arms`, so the vertical members of each range are covered by the same rule).
  Dash `i` spans `snap(i/n) .. snap(i/n + 2/(3n))` through D7's shared helper, so the
  pattern is identical in every cell — the same per-cell repetition the font produces.
- **Dash degradation guard (added after L2 re-review)**: that formula silently
  collapses at small extents. At `cw = 8` (14pt @1x) with `n = 4`, `┈` becomes
  1px-on-1px-off; at `cw = 4` the dashes come out contiguous and `┈` renders
  *identical to* `─` — a regression, since the font hints these legibly today.
  Guard, in two parts — the count bound alone is necessary but **not sufficient**:
  1. `n_eff = max(1, min(n, ⌊extent / 2⌋))`.
  2. **Clamp each dash end against the next dash's start**:
     `end_i = min(snap(i/n_eff + 2/(3·n_eff)), start_{i+1} - 1)`, with
     `start_{n_eff} = extent`.
  Part 2 is not belt-and-braces. `start` and `end` round *independently*, so `end_i`
  can round up **onto** `start_{i+1}` and silently merge two dashes — which no count
  bound can prevent. Worked failures at reachable sizes
  (`cw = round(pt · scale · 0.6)` → 7 at 11–12pt/1x, 9 at 15pt/1x):
  `extent 7, n=4` → `n_eff=3`, starts `0,2,5`, ends `2,4,6` → `[0,2][2,4][5,6]`:
  dashes 0 and 1 touch, 1 gap where 2 were promised. `extent 9, n=4` →
  `[0,2][2,4][5,6][7,8]`: 2 gaps where 3 were promised. With the clamp, extent 7
  gives `[0,1][2,4][5,6]` (2 gaps ✓) and extent 9 gives 3 gaps ✓, while extents
  4, 8, 17 and 34 are **unchanged** — the clamp only ever fires where the pattern
  was already broken.
  Post-condition: every dashed character yields at least `n_eff - 1` internal gaps
  of ≥1px, plus the trailing gap.
- **`n_eff == 1` is a special case, not a formula input.** It fires for
  `extent ≤ 3` (not "extent < 2" — `⌊3/2⌋ = 1` too) and must draw the **full-extent
  solid stem** explicitly. Feeding `n_eff = 1` through the dash formula would yield
  `[0, round(2·extent/3)]` — a stub that stops short of the cell edge and leaves an
  inter-cell gap, which is the opposite of the intended degradation.
- **Why the bound is `2n` and not `2n-1`**: each dash carries a *trailing* gap, so
  `n` dashes need `2n` px, not `n + (n-1)`. The trailing gap is not padding — the
  `snap(i/n) .. snap(i/n + 2/(3n))` form leaves each segment two-thirds dash and
  one-third gap precisely so the last dash ends *before* the cell edge. Without it,
  the final dash of one cell would butt against the first dash of the next and merge
  into one long dash across the boundary — a seam of a different kind, and exactly
  what D7's cell-independence otherwise buys us.
- **Rounded corners cost no new geometry**: `╭╮╯╰` map to the *same* `Arms` value
  as `┌┐┘└`. They lose their roundness; they gain alignment.
- **Rationale (rewritten — the original rationale was wrong).** The first version
  claimed leaving hard cases on the font path "can only ever remove seams, never add
  a regression." That is false: the font path anchors a stroke at
  `baseline - entry.top` where `baseline = row*ch + 0.8*ch`, while a procedural stem
  sits at `ch/2` — **two different anchors**, and `ascent = ch * 0.8` is itself an
  admitted approximation (see Non-goals). So a font-drawn `╭` or `┄` joining a
  procedural `─` could show a *vertical step that does not exist today*, which is
  precisely the defect class this spec exists to remove. Dashes are exactly
  rect-expressible, so moving them in is pure gain. Rounded corners trade roundness
  for alignment — accepted deliberately, because nvim/lazygit/starship draw rounded
  frames whose stems would otherwise step. Diagonals stay on the font path because
  they never meet a stem mid-edge, so they cannot exhibit the mismatch, and
  stair-stepping them (rejected) looks strictly worse than Menlo's antialiased arc.

### D3 (narrowed after L2 review): BOLD promotes light→heavy only for uniformly-light characters
- **Status**: resolved
- **Rule**: with `CellAttrs::BOLD` set, a character **all** of whose arms are
  `Light` draws every arm at `heavy` thickness. A character that already contains a
  `Heavy` or `Double` arm is left **completely unchanged**, as are block/shading
  characters (no stems) and dashed characters (thickness only, pattern unchanged).
  DIM and REVERSE are unaffected — they flow through the existing `glyph_color()`
  (`lib.rs:928`) exactly as they do for shaped glyphs.
- **Why the narrowing**: the original "promote every light stem" rule silently
  rewrote the mixed-weight characters D2 admits — `╀` (light horizontal, heavy up)
  would render as `╋`, destroying the very distinction the codepoint encodes. The
  uniform-light guard keeps the emphasis signal for the frames that actually use it
  while leaving deliberately-mixed art alone.
- **Rationale**: TUIs bold the border of the focused pane and users read that as
  emphasis; a font would render it as a heavier stroke, so dropping it (rejected)
  would silently delete a signal. A `+1px` bump (rejected) is invisible at large
  cells and disproportionate at small ones.

### D4: Box chars are an arm-weight table; one shared routine turns arms into rects
- **Status**: resolved
- **Rule**: each intercepted box char maps to
  `Arms { up, down, left, right, dashes: Option<u8> }` where each arm field is
  `None | Light | Heavy | Double`. A single routine computes a band **per arm** from
  *that arm's own weight* (D7) — not one band per axis — and draws the arm from its
  band to the cell edge. `┴`'s left, right and up arms all come out of the same
  routine, so the junction is generated, never transcribed. Block/shading characters
  carry no arms; they take the fraction path (D7).
- **Per-arm, not per-axis**: D2 admits characters with two weights on one axis
  (`╀ ╂ ╃…╊ ╼ ╽ ╾ ╿`, and the mixed tees in `250C–254B`). A single `t` per axis
  cannot express those. Per-arm bands can, and D7's containment property is what
  makes the light-meets-heavy junction come out flush.
- **Rationale**: junction correctness becomes structural rather than a property of
  the table being typed correctly. A literal per-char rect list (rejected) needs
  ~128 hand-verified entries where a single typo notches one corner and nothing
  catches it; an inline `match ch → Vec<Rect>` in pixels (rejected) duplicates the
  rounding arithmetic per arm and can't be unit-tested as a plain value.

### D5 (re-argued after L2 re-review): No config toggle — the procedural path is unconditional
- **Status**: resolved
- **Rationale**: the original argument — "D2 intercepts only what rects reproduce
  exactly, so a toggle could only select the worse rendering" — died with the D2
  revision, which renders `╭╮╯╰` square on purpose. The honest argument is about
  cost, not incoherence. An OFF path *would* be coherent: it is today's code
  verbatim, everything anchored the same way. But it buys exactly one thing —
  corner roundness — in exchange for a permanent config axis, a doubled test matrix
  (every geometry assertion needing a font-path counterpart), and a second supported
  rendering mode on a solo daily-driver terminal. Not worth it. Default-off
  (rejected) additionally ships the fix dark behind a TOML edit.
- **If this is ever revisited**, the cheaper move is a rounded-corner escape hatch
  rather than a global toggle — it isolates the one real loss (see Known Gaps).

### D6: ░▒▓ use partial-coverage atlas entries, not a shader change
- **Status**: resolved
- **Rule**: add `Atlas::shade_entry(&mut self, queue, level) -> Option<GlyphEntry>`
  beside `solid_entry` (`atlas.rs:242`), packing 2×2 blocks at coverage
  `64 / 128 / 191` for `░ / ▒ / ▓`, memoized as `[Option<(GlyphEntry, u64)>; 3]`
  with the same generation-based re-reservation. It returns `Option` and the caller
  skips the quad on `None` — it does **not** copy `solid_entry`'s
  `.expect("a 2x2 solid block must fit the atlas")` (`atlas.rs:257`), which would
  violate this spec's own no-`unwrap()`-outside-tests constraint. `Option` also
  matches `get_or_insert_styled`'s existing contract. `glyph.wgsl:46` already returns
  `vec4(color * a, a)`, so a sub-255 coverage *is* the alpha — correct
  premultiplied output with **no** shader or vertex-layout edit. UVs sample the
  block center, exactly as the underline pass does (`lib.rs:1188`).
- **Rationale**: widening `GlyphInstance.color` to `Float32x4` (rejected) edits the
  shared glyph pipeline and shader for one feature's benefit. A CPU-side
  `mix(cell_bg, fg, level)` opaque quad (rejected) is only correct when something
  opaque is beneath — an inline image (`image_layer.rs`) under the cell would be
  painted over instead of showing through.

### D7: All geometry goes through two snapping helpers, never ad-hoc arithmetic
- **Status**: assumed (derived from L1; no user question)
- **Rule**:
  ```
  band(origin, extent, t)  -> s = origin + ((extent - t) * 0.5).round();  (s, s + t)
  snap(origin, extent, f)  -> origin + (extent * f).round()
  ```
  Cell origin is `(col * cw, row * ch)` and `cell_size_px` rounds (`lib.rs:1270`),
  so both helpers are a *pure function of `(extent, t|f)`* — identical for every
  cell. Consequences: two horizontally adjacent `─` land on the same `by0/by1`, and
  their x-extents `[x0, x0+cw]` and `[x0+cw, x0+2cw]` abut exactly. `▀` and `▄` are
  exact complements because both call `snap(.., 0.5)`. Quadrants split on the same
  two lines the halves use, so `▘▝▖▗` tile with `▀▄▌▐`. Eighth blocks use
  `f = n/8`. Double stems place two `light` bands separated by a `light` gap inside
  a `3 * light` span centered by the same rule.
- **Two invariants govern cross-weight junctions (added after L2 review).** Write
  `L` for the light thickness, so `heavy = 2L` and a double occupies a `3L` span.
  - **light ⊆ heavy — containment, *not* concentricity.** A light and a heavy band
    are not concentric and cannot be: an odd-width and an even-width band have
    centers half a pixel apart on an integer lattice, and no rounding rule fixes
    that. Worked: `ch=34` → light `L=3` gives `[16,19]` (center 17.5), heavy `2L=6`
    gives `[14,20]` (center 17.0). What `band` *does* guarantee is containment —
    `s_light - s_heavy ∈ {⌊L/2⌋, ⌈L/2⌉} ⊆ [0, L]` across all four parities of
    `(extent, L)`, so the light band always sits fully inside the heavy one. `╾`
    widens into a clean T with no gap and no step; residual asymmetry is ≤0.5px,
    the integer-lattice minimum.
  - **light ∥ double — concentricity.** The double-line crossings (`╪ ╫ ╞ ╟ ╡ ╢ ╤
    ╥ ╧ ╨`) depend on a *different* property: the light band must be centered in the
    `3L` double span, or the crossing looks lopsided. It is, because `L` and `3L`
    share parity — `e=34, L=3`: light `[16,19]` center 17.5, double span `[13,22]`
    center 17.5; `e=17, L=1`: both 8.5. Unlike containment this one *is* exact.
  Both are testable properties of `band` alone, and both must be tests, not comments.
- **Rationale**: this is the only way "이웃 셀과 정수 엣지 공유" is a theorem rather
  than a hope. Rounding at each call site independently (rejected) is what produces
  the off-by-one notches this spec exists to remove.

### D8: Interception marks the char's face `None`; the existing run machinery skips it
- **Status**: assumed (derived from L1; no user question)
- **Rule**: in `build_glyph_instances`, after `faces` is built (`lib.rs:1087`), set
  `faces[i] = None` for every char the procedural table covers and record
  `(char, char_col[i])` for the procedural pass. `split_runs` then emits a
  `face: None` run that the loop already skips (`lib.rs:1097`) — `text`,
  `byte_col`, and `char_col` stay in perfect sync, untouched.
- **Rationale**: `lib.rs:1108–1120` documents that a row-wide byte→column map reads
  the wrong column for every run after the first, so *deleting* chars from the
  shaping input (rejected) would desync `char_col`. Crucially, `pen` is reset per
  cluster to `col * cw + offset` and `center_offset(0, ..) == 0.0` (`lib.rs:1048`),
  so splitting a face-0 run at a box char moves **no** surviving glyph — which is
  what makes "non-box output is unchanged" provable rather than hoped for.
- **The proof extends to fallback faces too (added after L2 review)**: for
  `face > 0`, `center_offset` is a function of the *cluster's own* advance sum and
  `cell_span(snapshot, col, row)` at the cluster's lead column (`lib.rs:1143–1144`).
  Both are per-cluster values read from the snapshot, not run-relative ones, so a
  run boundary introduced next to a box char cannot move a Hangul cluster either.
  The unchanged-output guarantee therefore covers the whole fallback chain, not just
  ASCII — and the L4 tests must assert it for both.
- **Bounded ligature caveat**: with `font.ligatures = true`, splitting a run removes
  the box char from that run's `liga`/`calt` context. A ligature spanning a box char
  is not a real sequence, and Menlo is monospaced with no kerning, so no observable
  change is expected — but the claim is "no ligature spans a box char", not "run
  splitting is free", and the L4 test runs with ligatures **on** to pin it.

### D9: Main grid only — sibling text layers are untouched
- **Status**: assumed (derived from L1; no user question)
- **Rule**: only `build_glyph_instances` intercepts. `sidebar_row.rs:815`,
  `preedit_layer.rs:358`, the ai-monitor overlay, and the tab bar keep resolving
  their own glyphs through `atlas.resolve` + `get_or_insert_styled`.
- **Rationale**: those layers draw chrome text at their own non-cell-snapped
  positions, so cell-snapped rects have no meaning there — the tiling defect is
  specific to the grid. `cursor_layer.rs:3` and `selection_layer.rs:9` are
  translucent overlays that never re-render the cell's character, so they tint a
  procedural quad correctly with no change at all.

### D10: Pin the integer-cell-size invariant with a test rather than defending against its loss
- **Status**: resolved (from the Inversion Probe)
- **Rule**: add a unit test asserting `cell_size_px` returns integral `(cw, ch)`
  across representative sizes and scale factors, commented as *"D7's shared-edge
  guarantee depends on this; the font-metrics spec must preserve it."*
- **Rationale**: D7's crispness (uniform stem width) rests on integer `cw`/`ch`,
  which holds today only because `cell_size_px` rounds (`lib.rs:1270`) — and the
  deferred `font.metrics()` cell-sizing spec is precisely the change that could
  break it. Note the failure mode is *uneven stem width*, not returned seams: both
  quads still compute the same shared-edge coordinate, and the pipeline is
  single-sampled (`multisample: default()`, `lib.rs:1252`), so center-sampling gives
  no double-blended boundary pixel. Defending internally (rejected) would put box
  art on an integer lattice while glyphs sat on a fractional one, drifting them
  apart by up to a pixel. Accepting the coupling silently (rejected) trades a
  one-line test for a defect nobody would trace back to here.

## Constraints
- **`term-core` is untouched.** No grid, parser, or `GridSnapshot` change — this is
  renderer-local presentation (`dependency-direction` rule).
- **No new dependencies.** Geometry is hand-rolled arithmetic (`no-gui-crates`).
- **No shader or vertex-layout change.** `glyph.wgsl` and the
  `vertex_attr_array![… 5 => Float32x3]` layout (`lib.rs:446`) stay as they are —
  D6 exists to make that possible.
- **Non-intercepted output must be byte-identical.** Every character outside D2's
  set — ASCII, Hangul, and the diagonals `╱╲╳`, which after the D2 revision are the
  *only* remaining fallthroughs — keeps its exact current `pos/size/uv/color`.
  This is the analogue of the existing R0.3 hard
  constraint that `ascii_placement_matches_single_font_shaping` (`lib.rs:1721`)
  guards.
- **No idle-CPU regression.** Procedural rects are built inside
  `build_glyph_instances`, so they live under the `last_snapshot` cache
  (`lib.rs:746`) and cost nothing on an unchanged frame (protects the G1 fix).
- **Atlas eviction safety.** `shade_entry` must re-reserve on a `generation` bump
  exactly as `solid_entry` does; the mid-frame `generation_at_entry` compare
  (`lib.rs:738`) is the existing guard and is not to be worked around.
- **No `unwrap()` outside tests** (CLAUDE.md); `cargo clippy --workspace
  --all-targets -- -D warnings` must stay clean.
- **Containment invariant** (D7): for every cell extent and every light thickness
  `L`, `band(o, e, L) ⊆ band(o, e, 2L)`. This is what makes light-meets-heavy
  junctions flush; it must be a test, not a comment.
- **Double-concentricity invariant** (D7): **for every cell extent and every `L`
  where `3L ≤ extent`**, the light band and the `3L` double span share a center.
  This is what makes `╪ ╫ ╞ ╢` crossings symmetric, and it is a *separate* property
  from containment. The domain restriction is not cosmetic: `band` clamps thickness
  to the cell (`t.min(extent)`), so once `3L > extent` the double span is truncated
  and its center necessarily moves. That only happens at `cw ≤ 2` — reachable solely
  via `font.size_pt ≈ 1` — and the clamp is the lesser evil, because without it a
  heavy stem in a 1px cell starts at `-1` and paints into the row above. Containment
  (`light ⊆ heavy`) holds **unrestricted**, including in the clamped region.
- **Dashes must stay dashed** (D2): every dashed character at every reachable cell
  extent yields at least `n_eff - 1` gaps of ≥1px, or falls to the documented solid
  degradation at `n_eff == 1`. A dashed character must never silently render as its
  solid counterpart.
- **Only diagonals may fall through.** After the D2 revision, `╱╲╳` are the sole
  characters in `2500–259F` still shaped by the font. Any future decision to hand
  another character back to the font reopens the anchor-mismatch defect (see D2's
  rewritten rationale) and must be argued explicitly.

## Known Gaps
- **`╭╮╯╰` render square.** A deliberate, user-approved trade: alignment over
  roundness. Rounded frames in nvim/lazygit/starship will look square. This is a
  visible content change and the most likely thing to want revisiting — the fix
  path is procedural stair-stepped arcs (considered, rejected as visibly chunkier
  than Menlo's antialiased arc at a 3px stem).
- **The font-vs-procedural anchor offset was never measured.** D2's revision removes
  the exposure by intercepting nearly everything rather than by quantifying it. The
  actual delta between `baseline - entry.top` and `ch/2` for Menlo's box glyphs is
  still unknown, and it stays unknown — the sibling `font.metrics()` spec is where
  that number would come from.
- **U+1FB00–1FBFF (Symbols for Legacy Computing)** remains blank — no installed
  macOS font covers it, and D2 keeps it out of scope. The D4 arm/fraction model
  does not extend to sextants/octants without a new fraction basis (1/2 × 1/3 and
  1/2 × 1/4 grids). Documented follow-up.
- **Font-metric-driven cell sizing** (`cell_size_px` 0.6/1.2, `ascent = ch * 0.8`)
  is a separate spec, now coupled to this one through D10's invariant test.
- **Light/heavy stems are non-concentric by ≤0.5px** (D7). Unavoidable on an integer
  lattice; containment makes it invisible in practice, but it is a real residual.
- **Degenerate cell sizes (`cw ≤ 2`, i.e. `font.size_pt ≈ 1`) degrade rather than
  render faithfully.** Three floors keep characters visible instead of vanishing or
  bleeding: `band` clamps thickness to the cell, `span` keeps a stub a pixel wide
  from either side, and `floor_to_pixel` grows a rounded-away fill back to 1px. The
  costs, all confined to that range: double-concentricity no longer holds (the span
  is truncated), and `▌`/`▐` both collapse to the single available pixel, so
  half-block complementarity degrades. A test pins that the clamp is inert at every
  cell size reachable from 8–32pt at 1x/2x, so none of this touches real rendering.

## Requirements

Reference cells throughout: **`(cw, ch) = (17, 34)`** (14pt @2x) and
**`(8, 17)`** (14pt @1x), both from `cell_size_px` (`lib.rs:1268`).

### R0: Box art tiles seamlessly — the goal-level behavior
*From: Confirmed Goal*

#### R0.1: Adjacent horizontal stems share an exact edge
- **Given**: `─` at columns 3, 4 and 5 of the same row, cell `(17, 34)`
- **When**: `build_glyph_instances` produces their quads
- **Then**: all three carry the identical y-band, and their x-extents are
  `[51,68] [68,85] [85,102]` — abutting exactly, with no gap and no overlap

#### R0.2: A closed frame renders continuous
- **Given**: a 3×3 frame `┌─┐ │ │ └─┘` in the grid
- **When**: the frame is drawn
- **Then**: every corner's arms meet its adjacent edge cell's stem on the same band,
  leaving no unpainted pixel anywhere along the frame's outline

#### R0.3: Vertically adjacent stems share an exact edge
- **Given**: `│` at rows 1 and 2 of the same column, cell `(17, 34)`
- **When**: their quads are produced
- **Then**: both carry the identical x-band, and their y-extents are `[34,68]` and
  `[68,102]` — abutting exactly

---

### R1: Character classification and the interception set
*From: D2*

#### R1.1: Classification is total and exact over U+2500–259F
- **Given**: every codepoint in `U+2500..=U+259F`
- **When**: passed to the classifier
- **Then**: exactly `U+2571..=U+2573` (`╱╲╳`) return "font path"; every other
  codepoint returns a procedural spec — no codepoint in the range is unhandled

#### R1.2: An intercepted character is never shaped
- **Given**: a row containing `─` at column 3, primary face covering U+2500
- **When**: `build_glyph_instances` runs
- **Then**: column 3's face slot is `None`, `split_runs` yields a `face: None` run
  covering it, and no `swash`-derived atlas entry is emitted for that column

#### R1.3: Diagonals still shape through the font
- **Given**: a row containing `╱` (U+2571)
- **When**: `build_glyph_instances` runs
- **Then**: that column's emitted instance is field-identical (`pos`, `size`,
  `uv0`, `uv1`, `color`) to what the pre-change shaping path produced

#### R1.4: Rounded corners reuse their square counterpart's arms
- **Given**: `╭` (U+256D) and `┌` (U+250C)
- **When**: each is classified
- **Then**: both yield `Arms { down: Light, right: Light, up: None, left: None }` —
  equal values, so `╭` renders as `┌`

---

### R2: Thickness, weight, and the BOLD rule
*From: D1, D3*

#### R2.1: Light thickness at 2x
- **Given**: cell `(17, 34)`
- **When**: light thickness is computed as `max(1, round(ch · 0.075))`
- **Then**: `3.0`, and heavy is `6.0`

#### R2.2: Light thickness at 1x
- **Given**: cell `(8, 17)` → `round(17 · 0.075) = round(1.275)`
- **When**: light thickness is computed
- **Then**: `1.0`, and heavy is `2.0`

#### R2.3: Thickness never reaches zero
- **Given**: a degenerate cell height `ch = 4` → `round(0.3) = 0`
- **When**: light thickness is computed
- **Then**: `1.0` — the `max(1.0, …)` floor holds, and no zero-area rect is emitted

#### R2.4: BOLD promotes a uniformly-light character
- **Given**: a cell holding `┼` (all four arms light) with `CellAttrs::BOLD` set,
  cell `(17, 34)`
- **When**: its rects are generated
- **Then**: every arm is drawn at thickness `6.0`, not `3.0`

#### R2.5: BOLD leaves a mixed-weight character untouched
- **Given**: a cell holding `╀` (U+2540 — light horizontal, heavy up) with BOLD set
- **When**: its rects are generated
- **Then**: the arm weights are byte-identical to the same cell without BOLD — the
  light horizontal is *not* promoted, so `╀` does not become `╋`

#### R2.6: BOLD is a no-op on block characters
- **Given**: a cell holding `█` with BOLD set
- **When**: its rect is generated
- **Then**: identical to the same cell without BOLD

---

### R3: Geometry primitives and their invariants
*From: D7*

#### R3.1: `band` is cell-independent
- **Given**: extent `34`, thickness `3`, origins `0` and `34`
- **When**: `band` is called for each
- **Then**: both return the same offset from their origin, so the two bands abut
  exactly at `34`

#### R3.2: Containment — light ⊆ heavy
- **Given**: every extent `e` in `1..=64` and every light `L` in `1..=e/2`
- **When**: `band(0, e, L)` and `band(0, e, 2L)` are compared
- **Then**: `s_heavy ≤ s_light` **and** `s_light + L ≤ s_heavy + 2L` for all pairs —
  the light band is always fully inside the heavy one

#### R3.3: Concentricity — light ∥ double
- **Given**: the same sweep of `e` and `L`
- **When**: the light band's center and the `3L` double span's center are compared
- **Then**: they are equal for all pairs (exact, not within-tolerance)

#### R3.4: `snap` produces exact complements
- **Given**: extent `17` (odd) and `34` (even)
- **When**: `snap(0, e, 0.5)` is used as both the upper half's end and the lower
  half's start
- **Then**: `upper.end == lower.start` exactly — no gap, no overlap, at both extents

#### R3.5: No zero-area rects
- **Given**: any intercepted character at any cell size from `(1,1)` to `(64,128)`
- **When**: its rects are generated
- **Then**: every emitted rect has width ≥ 1 and height ≥ 1

---

### R4: The arm model produces notch-free junctions
*From: D4*

#### R4.1: A T-junction leaves no unpainted row
- **Given**: `┴` at cell `(17, 34)`, light bands `band_x` and `band_y`
- **When**: its rects are generated
- **Then**: the up-arm rect spans `band_x × [0, band_y.end]` and the crossbar spans
  `[0, cw] × band_y` — `band_y.end` appears in both, so no row between the stem and
  the crossbar is left unpainted

#### R4.2: Mixed-weight arms get their own bands
- **Given**: `╾` (U+257E — **heavy** left, **light** right) at cell `(17, 34)`
- **When**: its rects are generated
- **Then**: two horizontal rects of thickness `3.0` and `6.0`, and the light band is
  contained in the heavy band (R3.2), so the junction widens into a clean T

#### R4.3: A plain stem spans the full cell
- **Given**: `─` at cell `(17, 34)`
- **When**: its rects are generated
- **Then**: the painted x-extent is exactly `[0, 17]` relative to the cell origin

#### R4.4: A stub arm stops at the band
- **Given**: `╴` (U+2574 — light left arm only)
- **When**: its rect is generated
- **Then**: the rect spans from the cell's left edge to `band_x.end` only — it does
  not cross to the right edge

#### R4.5: A double stem is two bands with a light gap
- **Given**: `═` at cell `(17, 34)`, light `L = 3`
- **When**: its rects are generated
- **Then**: two rects of height `3` separated by a `3`-px gap, together spanning the
  `9`-px double span, centered per R3.3

---

### R5: Dashed characters stay dashed
*From: D2 (dash geometry + two-part guard)*

#### R5.1: Dash count comes from the codepoint
- **Given**: `╌` (U+254C), `┄` (U+2504), `┈` (U+2508)
- **When**: each is classified
- **Then**: dash counts `2`, `3`, `4` respectively, with orientation taken from
  `Arms` so the vertical members of each range use the same rule

#### R5.2: Gaps survive at a small cell width
- **Given**: `┈` (n=4) at `cw = 8` → `n_eff = 4`
- **When**: its rects are generated
- **Then**: 4 dashes with 3 internal gaps of ≥1px — it does not render as `─`

#### R5.3: The clamp fires where independent rounding would merge dashes
- **Given**: `┈` (n=4) at `cw = 7` → `n_eff = 3`, starts `0,2,5`, unclamped ends
  `2,4,6`
- **When**: the end-clamp `end_i = min(snap(…), start_{i+1} − 1)` is applied
- **Then**: rects `[0,1] [2,4] [5,6]` — 2 internal gaps, and no two dashes touch
  (unclamped, dashes 0 and 1 would merge at x=2)

#### R5.4: The clamp is a no-op where the pattern was already correct
- **Given**: `┈` at `cw` of `4`, `8`, `17` and `34`
- **When**: rects are generated with and without the clamp
- **Then**: identical output at all four extents

#### R5.5: Degenerate extents draw a full solid stem
- **Given**: `┈` at extent `3` → `n_eff = 1`
- **When**: its rect is generated
- **Then**: exactly one rect spanning `[0, 3]` — the **full** extent, not the
  `[0, round(2·3/3)]` stub the dash formula would give

#### R5.6: Dashes do not merge across a cell boundary
- **Given**: two horizontally adjacent `┈` cells at `cw = 17`
- **When**: their rects are generated
- **Then**: the last dash of the left cell ends strictly before `x = 17`, so a
  trailing gap separates it from the right cell's first dash

---

### R6: Blocks, eighths, quadrants, and shading
*From: D6, D7*

#### R6.1: Half blocks are exact complements
- **Given**: `▀` and `▄` at cell heights `34` and `17`
- **When**: their rects are generated
- **Then**: `▀.end == ▄.start` at both heights — together they cover the cell
  exactly once, with no gap and no double-painted row

#### R6.2: Quadrants split on the same lines as the halves
- **Given**: `▘▝▖▗` and `▀▄▌▐` at cell `(17, 34)`
- **When**: their rects are generated
- **Then**: every quadrant edge coincides with a half-block edge, so quadrants tile
  with halves and with each other

#### R6.3: Eighth blocks are monotonic and reach full height
- **Given**: `▁▂▃▄▅▆▇█` at cell height `34`
- **When**: their rects are generated
- **Then**: heights are non-decreasing, and `█` spans exactly `[0, 34]`

#### R6.4: Left/right blocks are complementary on x
- **Given**: `▌` and `▐` at cell width `17`
- **When**: their rects are generated
- **Then**: `▌.end == ▐.start`, covering the cell width exactly once

#### R6.5: Shading uses a shade entry, not the solid entry
- **Given**: `▒` (U+2592)
- **When**: its quad is generated
- **Then**: one full-cell rect referencing the level-2 shade entry (coverage `128`),
  **not** `solid_entry` — so the shader's `color * a` yields 50% intensity

---

### R7: Atlas shade entries
*From: D6*

#### R7.1: Three distinct memoized entries
- **Given**: a headless device/queue and an `Atlas`
- **When**: `shade_entry` is called for levels 0, 1, 2 and then again for each
- **Then**: three distinct entries the first time; the second round returns the same
  entries without re-packing

#### R7.2: Re-reserved after an eviction
- **Given**: `shade_entry` has returned an entry, then the atlas `generation` bumps
- **When**: `shade_entry` is called again for that level
- **Then**: a freshly packed entry is returned — the stale pre-eviction UV is not
  handed out

#### R7.3: Failure returns `None`, never a panic
- **Given**: a pack failure for a shade block
- **When**: `shade_entry` is called
- **Then**: it returns `None`, the caller emits no quad, and no `unwrap`/`expect`
  is reached

#### R7.4: `solid_entry` behavior is unchanged
- **Given**: the underline pass calling `solid_entry`
- **When**: a frame is drawn
- **Then**: underline quads are unchanged from before this feature

---

### R8: Integration into the shaping pipeline
*From: D8, D9*

#### R8.1: Procedural quads carry the cell's resolved color
- **Given**: a `│` cell with `CellAttrs::REVERSE` set
- **When**: its quad is emitted
- **Then**: `color == glyph_color(cell)` — the fg/bg-swapped value, identical to
  what a shaped glyph in that cell would receive

#### R8.2: DIM applies to procedural quads
- **Given**: a `│` cell with `CellAttrs::DIM` set
- **When**: its quad is emitted
- **Then**: `color` is mixed toward the background by `DIM_MIX = 0.45`

#### R8.3: UVs sample the block center
- **Given**: any procedural quad
- **When**: its `uv0`/`uv1` are inspected
- **Then**: both equal the entry's center UV, so linear filtering cannot pick up a
  neighbouring atlas texel (matching `lib.rs:1188`)

#### R8.4: Quads live under the snapshot cache
- **Given**: `draw_to` called twice with equal snapshots
- **When**: the second call runs
- **Then**: `build_glyph_instances` is not re-entered — the resident buffers are
  re-presented, so idle CPU is unchanged

#### R8.5: Sibling layers are untouched
- **Given**: a sidebar row whose text contains `─`
- **When**: the sidebar renders
- **Then**: it still resolves that char via `atlas.resolve` +
  `get_or_insert_styled` and draws the font glyph

#### R8.6: Cursor and selection composite over procedural quads
- **Given**: a `│` cell that is both inside a selection and under the cursor
- **When**: the frame is drawn
- **Then**: the selection tint and cursor block composite above the procedural quad
  exactly as they do above a shaped glyph — no layer change required

---

### R9: Non-intercepted output is byte-identical
*From: D8 + the byte-identical constraint*

#### R9.1: ASCII in a row containing a box char is unmoved
- **Given**: the row `abc─def` with `font.ligatures = true`
- **When**: `build_glyph_instances` runs
- **Then**: every instance for `a b c d e f` matches the single-run reference path
  field for field (`pos`, `size`, `uv0`, `uv1`, `color`) — the run split at `─`
  moves nothing

#### R9.2: Fallback-face clusters are unmoved
- **Given**: the row `한글─한글`
- **When**: `build_glyph_instances` runs
- **Then**: every Hangul cluster's instance matches the reference path field for
  field — `center_offset`'s per-cluster `adv` and `cell_span` are unaffected by the
  new run boundary

#### R9.3: Diagonals are unmoved
- **Given**: the row `─╱─`
- **When**: `build_glyph_instances` runs
- **Then**: the `╱` instance is field-identical to the pre-change path

#### R9.4: Wide-char columns are unaffected
- **Given**: a row mixing a double-width char and a box char, e.g. `한─한`
- **When**: `build_glyph_instances` runs
- **Then**: `char_col` still maps each char to its true column and the box quad
  lands on its own column, not the continuation spacer's

---

### R10: Invariants and config surface
*From: D5, D10*

#### R10.1: `cell_size_px` returns integral cell dimensions
- **Given**: sizes `10.0`, `14.0`, `24.0` pt at scale factors `1.0` and `2.0`
- **When**: `cell_size_px` is called
- **Then**: both `cw` and `ch` are integral at every combination — the invariant
  D7 depends on, which the deferred `font.metrics()` spec must preserve

#### R10.2: No new configuration key
- **Given**: `Config::default()` and a `config.toml` with no box-drawing key
- **When**: the renderer starts
- **Then**: no box-drawing field exists on `FontConfig`, and the procedural path is
  active unconditionally

#### R10.3: Lint and error-handling constraints hold
- **Given**: the completed change
- **When**: `cargo clippy --workspace --all-targets -- -D warnings` runs
- **Then**: it exits clean, and no `unwrap()`/`expect()` was added outside `#[cfg(test)]`

---

### Coverage check

| Decision | Covered by |
|----------|-----------|
| D1 thickness rule | R2.1, R2.2, R2.3 |
| D2 interception set + dashes | R1.1–R1.4, R5.1–R5.6 |
| D3 BOLD narrowing | R2.4, R2.5, R2.6 |
| D4 arm model | R4.1–R4.5 |
| D5 no toggle | R10.2 |
| D6 shade entries | R6.5, R7.1–R7.4 |
| D7 band/snap + invariants | R3.1–R3.5, R0.1, R0.3, R6.1–R6.4 |
| D8 interception mechanism | R1.2, R9.1–R9.4 |
| D9 main grid only | R8.5, R8.6 |
| D10 integral cell size | R10.1 |

No orphan decisions. Every sub-requirement carries Given/When/Then.

## Tasks

New module: `crates/renderer/src/box_draw.rs`. Touched: `crates/renderer/src/lib.rs`
(one `mod box_draw;`, the interception hook, tests) and
`crates/renderer/src/atlas.rs` (shade entries). No other file changes.

### T1: Geometry primitives and the thickness model [infra]
- **Fulfills**: R3.1, R3.2, R3.3, R3.4, R3.5, R2.1, R2.2, R2.3
- **Depends on**: (none)
- **Files**: `box_draw.rs` (new), `lib.rs` (+`mod box_draw;`)
- `Rect`, `band(origin, extent, t)`, `snap(origin, extent, f)`, and
  `light_thickness(ch)` / `heavy`. Ships with the swept invariant tests —
  containment and concentricity over every extent `1..=64` × every light
  `1..=extent/2`. Pure functions, no GPU: these tests run on any machine.

### T2: `Atlas::shade_entry` [infra]
- **Fulfills**: R7.1, R7.2, R7.3, R7.4
- **Depends on**: (none) — **parallel with T1** (disjoint files, no shared interface)
- **Files**: `atlas.rs`
- `shade_entry(&mut self, queue, level) -> Option<GlyphEntry>` packing 2×2 blocks at
  coverage 64/128/191, memoized as `[Option<(GlyphEntry, u64)>; 3]` with
  generation-based re-reservation. Returns `Option` — does **not** copy
  `solid_entry`'s `.expect()`. Tests are `headless()`-guarded per the existing idiom.

### T3: Box-drawing character model — classification, arms, rects
- **Fulfills**: R1.1, R1.4, R4.1, R4.2, R4.3, R4.4, R4.5, R2.4, R2.5, R2.6
- **Depends on**: T1
- **Files**: `box_draw.rs`
- `Arms { up, down, left, right, dashes }`, the classification table over
  `U+2500..=U+259F` (totality test incl. `╱╲╳` → font path, `╭`≡`┌`), per-arm band
  generation, and the BOLD rule *with* its uniform-light guard. Solid orthogonal
  characters only — dashes are T4, blocks are T5.

### T4: Dashed characters and the two-part guard
- **Fulfills**: R5.1, R5.2, R5.3, R5.4, R5.5, R5.6
- **Depends on**: T3 — *file overlap on `box_draw.rs`, not a true interface
  dependency; it could run beside T3 if the module were split*
- **Files**: `box_draw.rs`
- Dash counts from codepoint, `n_eff` bound, **and** the end-clamp. Tests must
  include the `cw = 7` and `cw = 9` merge cases and assert the clamp is a no-op at
  4/8/17/34, plus `n_eff == 1` → full-extent solid stem.

### T5: Block elements, eighths, quadrants, shading
- **Fulfills**: R6.1, R6.2, R6.3, R6.4, R6.5
- **Depends on**: T4 — *file overlap on `box_draw.rs`, same caveat as T4*
- **Files**: `box_draw.rs`
- The fraction path via `snap`: halves as exact complements at odd and even heights,
  quadrants on the half lines, monotonic eighths, and `▒` referencing the shade
  level rather than solid.

### T6: Wire interception into the shaping pipeline [vertical]
- **Fulfills**: R0.1, R0.2, R0.3, R1.2, R1.3, R8.1, R8.2, R8.3, R8.4, R8.5, R8.6
- **Depends on**: T2, T5
- **Files**: `lib.rs`
- Set `faces[i] = None` for classified chars after `lib.rs:1087`, collect
  `(char, col)`, and emit quads in a pass beside the underline pass — reusing
  `glyph_color`, center-sampled UVs, and `solid_entry`/`shade_entry`. This is the
  task where box art first renders end-to-end.

### T7: Non-regression and invariant suite
- **Fulfills**: R9.1, R9.2, R9.3, R9.4, R10.1, R10.2, R10.3
- **Depends on**: T6
- **Files**: `lib.rs` (tests)
- `abc─def` with ligatures **on**, `한글─한글` through the fallback chain, `─╱─`,
  and `한─한` for wide-char columns — each compared field-for-field against the
  single-run reference path, following `ascii_placement_matches_single_font_shaping`
  (`lib.rs:1721`). Plus the `cell_size_px` integrality test carrying the comment that
  names the `font.metrics()` spec as the thing that must preserve it.

### DAG

```
T1 (primitives) ──┐
                  ├──> T3 (box model) ──> T4 (dashes) ──> T5 (blocks) ──┐
T2 (shade_entry) ─┘                                                     ├──> T6 ──> T7
                  └─────────────────────────────────────────────────────┘
```

T1 ∥ T2 is the only true parallelism. T3→T4→T5 serialize on `box_draw.rs`; splitting
that module three ways purely to widen the DAG would be contorting the design for the
scheduler, so it is left cohesive.

## External Dependencies

### Pre-work
- (none) — no new crates, no external services, no font installation. Every input
  already exists in the workspace.

### Post-work
- **Visual confirmation via `/run-term`** against a real TUI (tmux panes or btop).
  The rect-coverage tests prove geometry; they do not prove it *looks* right, and
  `╭╮╯╰` rendering square is a deliberate visible change worth eyeballing before
  this is called done.
- **File the `font.metrics()` follow-up spec**, noting that T7's `cell_size_px`
  integrality test is its gate (D10).
