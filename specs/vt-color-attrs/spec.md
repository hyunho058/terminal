# Spec: vt-color-attrs

## Meta
- **Created**: 2026-07-18
- **Type**: dev
- **Status**: planned (decompose batch — L2-reviewer PASS; execution owned by build-order)
- **Set**: post-pr2-backlog (`specs/post-pr2-backlog/partition-manifest.md`)

## Goal
Close the VT color/attribute gaps so real TUIs render correctly. Today
`parser.rs::handle_sgr` supports only the 16 basic ANSI colors — no 256-color
(`38;5`), no truecolor (`38;2`), no bold/dim/italic/underline/reverse, and no
default-reset (`39`/`49`). vim, bat, `ls --color`, and most TUIs look wrong. Add
full SGR color + attribute parsing in `term-core` and render those attributes in
the renderer.

## Non-goals
- Blink, conceal, strikethrough, and other rarely-used SGR attributes (only
  bold/dim/italic/underline/reverse are in scope this pass).
- Per-style re-shaping of ligatures across a style boundary (kept as a known gap;
  shaping stays per-row, glyphs rasterize with the lead cell's style).
- Idle-CPU caching behavior (owned by the sibling `idle-cpu`); this feature only
  must not *break* its snapshot-equality cache (see D5 / R6).
- Doc/known-gaps updates removing "256-color/truecolor SGR" from the deferred
  lists (owned by the sibling `housekeeping`).

## Confirmed Goal
`handle_sgr` parses 256-color (`38;5;n` / `48;5;n`), truecolor
(`38;2;r;g;b` / `48;2;r;g;b`, incl. the colon-subparam forms), the attributes
bold/dim/italic/underline/reverse (set codes 1/2/3/4/7 and their clears
22/23/24/27), and default-reset (`39`/`49`) — stamping them into each `Cell`. The
renderer displays those attributes: reverse swaps fg/bg, dim scales the color,
bold/italic select a style-synthesized glyph (style-aware atlas key), and
underline draws an extra rule. `Cell`'s `PartialEq` stays derived and every
render-affecting attribute lives inside `Cell`, so the sibling `idle-cpu`'s
snapshot-equality cache remains correct.

## Declared Surface
- feature-id: vt-color-attrs
- depends_on: [idle-cpu]
- declared-surface:
  - crates/term-core/src/parser.rs
  - crates/term-core/src/grid.rs
  - crates/renderer/src/lib.rs
  - crates/renderer/src/glyph.rs
  - crates/renderer/src/atlas.rs

## Research
- `handle_sgr` matches only `0`, `30..=37`, `40..=47`, `90..=97`, `100..=107` and
  drops everything else via a simple per-param `for p in params.iter()` loop — no
  multi-param consumption, so `38;5;n` / `38;2;r;g;b` cannot be handled
  (`crates/term-core/src/parser.rs:142-156`).
- `ansi_16(index)` holds the 16-color palette and returns `Color(u32)`
  (`crates/term-core/src/parser.rs:212-217`).
- `Color(pub u32)` is already 24-bit RGB: `Color::rgb(r,g,b)`, `.r()/.g()/.b()`,
  `to_srgb_f32`, `to_linear_f32` — truecolor needs **no type change**
  (`crates/term-core/src/grid.rs:13-45`).
- `Cell { ch, fg, bg }` derives `Clone, Copy, PartialEq, Eq, Debug` — adding an
  attrs field keeps the derive and (critically) keeps `GridSnapshot`'s derived
  `PartialEq` field-complete (`crates/term-core/src/grid.rs:54-59`).
- Grid pen state: `pen_fg`, `pen_bg`, `default_fg`, `default_bg`; `set_pen(Option
  fg, Option bg)`, `reset_pen()` (→ defaults); `print` stamps `pen_fg/pen_bg` into
  the cell (`crates/term-core/src/grid.rs:88-90,144-157,182-192`).
- Renderer glyph color comes from `c.fg.to_linear_f32()` inside
  `build_glyph_instances`; `build_bg_instances` reads `cell.bg`
  (`crates/renderer/src/lib.rs:665-…,729`).
- Shaping is per-row: a row string is shaped once via `swash::ShapeContext` (for
  ligatures) and clusters map back to columns; each glyph rasterizes via
  `atlas.get_or_insert(queue, g.id)` (`crates/renderer/src/lib.rs:705-760`).
- The atlas caches on `glyph_id: u16` only: `cache: HashMap<u16,
  Option<GlyphEntry>>`, `get_or_insert(queue, glyph_id)` — bold/italic synthesized
  from the same glyph id would collide unless the key includes style
  (`crates/renderer/src/atlas.rs:36,103-112`).
- `GlyphRasterizer::rasterize_glyph(glyph_id)` uses `swash` Render/ScaleContext;
  swash supports synthesis (embolden / skew) at scale time
  (`crates/renderer/src/glyph.rs:9-11,68`).
- The sibling `idle-cpu` keys a renderer skip-reshape cache on `GridSnapshot`
  equality, which requires all render-affecting cell data to live in `Cell` with a
  derived `PartialEq` (`specs/idle-cpu/spec.md` — SD2, Constraints "Cache-key
  completeness").

## Decisions

<!-- SD1–SD4 inherited verbatim from specs/post-pr2-backlog/partition-manifest.md -->
### SD1: Preserve the enforced crate dependency direction; `term-core` stays GPU/winit-free
- **Status**: resolved
- **Rationale**: The workspace graph `common ← term-core ← renderer ← app` is a hard
  invariant (`.claude/rules/dependency-direction.md`). New per-cell data (VT color +
  attributes) lives in `term-core` (`Cell`/`Color`/`parser`); the renderer only ever
  reads it through `GridSnapshot`. Rejected: pushing any wgpu/winit type into
  `term-core`, or letting the renderer see a live `Grid` — either would make the
  engine un-testable headlessly and break the rule.

### SD2: `GridSnapshot` is the versioned renderer contract AND the frame-equality basis
- **Status**: resolved
- **Rationale**: `GridSnapshot` already derives `PartialEq` and is the immutable
  contract the renderer consumes (never a live `Grid`). The idle-CPU fix keys its
  skip-reshape cache on **snapshot equality**; VT attributes are added as `Cell`
  fields, so `Cell: PartialEq`/`Eq` makes attribute-only changes invalidate that
  cache automatically. This is the load-bearing coordination point between the two
  code features. Rejected: a side-channel dirty flag in `app` (splits the source of
  truth from the snapshot), or comparing rendered text (misses color/attr-only
  changes).

### SD3: No GUI/widget crates, macOS-ARM64-only; attribute rendering is hand-rolled via `swash`
- **Status**: resolved
- **Rationale**: `no-gui-crates` + `macos-only` rules. bold/dim/italic/underline/
  reverse are drawn on the existing hand-rolled two-pass cell renderer — synthesized
  through `swash` (embolden/skew) or font faces, with underline/reverse composed in
  the instance builders. Rejected: any widget/text-layout toolkit (egui/iced/…) and
  any `#[cfg(target_os = "…")]` portability branch.

### SD4: Keep the continuous-redraw loop (winit#2640); cut idle CPU by making each idle frame cheap
- **Status**: resolved
- **Rationale**: On this wgpu/winit/macOS stack the compositor drops the presented
  frame the moment the run loop sleeps, so a frame must always be in flight (see the
  `RedrawRequested` comment in `terminal_app.rs`). The idle-CPU fix therefore keeps
  the continuous redraw and makes each idle frame cheap (skip re-shape, re-present
  cached buffers) — it does **not** disable redraws. VT-attribute work must not
  reintroduce unconditional per-frame reshaping. Rejected: on-demand / `ControlFlow::
  Wait`-only redraws (blank window) or dropping `request_redraw` (regresses #2640).

<!-- feature-local decisions -->
### D5: Attributes are a `CellAttrs(u8)` bitflag newtype stored INSIDE `Cell`, `PartialEq` stays derived
- **Status**: resolved
- **Rationale**: Add `attrs: CellAttrs` to `Cell`, where `CellAttrs(u8)` carries bit
  masks BOLD/DIM/ITALIC/UNDERLINE/REVERSE and derives `Clone, Copy, PartialEq, Eq,
  Debug, Default`. `Cell` keeps its **derived** `PartialEq`/`Eq`, and every
  render-affecting attribute lives in `Cell` — so the sibling `idle-cpu`'s
  snapshot-equality cache still invalidates on an attribute-only change (honors SD2 /
  idle-cpu "Cache-key completeness"). This is the load-bearing cross-feature contract
  of the ordered share on `renderer/src/lib.rs`. Rejected: a `bitflags` crate
  dependency (a `u8` + const masks is enough, keeps the dep surface minimal); a
  struct of 5 `bool`s (larger, no benefit); storing reverse/attrs *outside* `Cell` or
  hand-writing `PartialEq` (would silently stale idle-cpu's cache — explicitly
  forbidden by that spec's constraint).

### D6: Reuse `Color(u32)` for 256-color and truecolor — no color-type change
- **Status**: resolved
- **Rationale**: `Color` is already 24-bit RGB with `Color::rgb` and
  `to_linear_f32`. 256-color resolves via a new `ansi_256(n)` — 0..=15 delegate to
  `ansi_16`, 16..=231 the 6×6×6 cube, 232..=255 the 24-step grayscale ramp — and
  truecolor is `Color::rgb(r,g,b)` directly. Rejected: an `enum Color {
  Indexed(u8), Rgb(u8,u8,u8) }` — it would churn the renderer, `snapshot`, and
  idle-cpu's cache for zero gain, since `u32` truecolor is lossless and already wired
  through `to_linear_f32`.

### D7: Rewrite `handle_sgr` to consume multi-param sequences, with a two-form 38/48 dispatch
- **Status**: resolved
- **Rationale**: Replace the per-param match with an iterator that dispatches `38`/`48`
  by whether the param carries **subparams** (vte delivers the two forms differently):
  - **Semicolon form** (`38;5;n`, `38;2;r;g;b`) — the selector and values arrive as
    *separate* `Param`s. Consume the next param for the selector: `5` → one more param
    = index → `ansi_256`; `2` → three more params = `Color::rgb(r,g,b)`.
  - **Colon-subparam form** (`38:5:n`, `38:2::r:g:b`) — the whole directive is ONE
    `Param` with subparams; resolve *in-param*. For `:2:` (truecolor) the T.416 layout
    puts an optional **color-space-id** slot right after the `2` (empty in the common
    `38:2::r:g:b`, which vte yields as `[38,2,0,r,g,b]`), so the parser MUST skip that
    slot and take the LAST three subparams as r,g,b — not the first three. The `:5:n`
    indexed form has no such slot.
  Add the attribute set codes (1 bold, 2 dim, 3 italic, 4 underline, 7 reverse), their
  clears (22 normal-intensity clears bold+dim, 23 not-italic, 24 not-underline, 27
  not-reverse), and default-reset (39 fg, 49 bg). Keep the existing
  30-37/40-47/90-97/100-107 arms. Unknown codes are ignored without consuming
  unrelated params. Rejected: the current independent per-param loop (structurally
  cannot read a `38;5;n` sequence); treating both forms identically as "read next 3"
  (misparses the colon truecolor form's color-space-id slot → wrong color).

### D8: Extend the grid pen with attributes + per-channel default reset
- **Status**: resolved
- **Rationale**: Add `pen_attrs: CellAttrs` to the grid; `print` stamps it into the
  cell alongside `pen_fg`/`pen_bg`. Add methods to set/clear attribute bits, and to
  reset fg / bg *individually* to `default_fg` / `default_bg` for SGR 39 / 49 (the
  grid owns the defaults; the parser does not). `reset_pen` (SGR 0) also clears
  `pen_attrs`. Rejected: threading defaults into the parser (parser has no access to
  the configured defaults; the grid is the right owner).

### D9: Apply reverse / dim / underline at draw time in the renderer instance builders
- **Status**: resolved
- **Rationale**: The stored `Cell` keeps its *logical* fg/bg; the renderer applies
  attributes when building instances: REVERSE swaps fg↔bg in both `build_bg_instances`
  and `build_glyph_instances`; DIM scales the glyph color toward the background;
  UNDERLINE emits an extra thin bg-quad at the cell baseline. Rejected: baking the
  reverse swap into the stored cell (loses the logical color, corrupts copy/paste and
  makes the snapshot semantics style-dependent).

### D10: Style-aware glyph atlas — internal `GlyphKey`, but keep the `u16` entry point (surface-safe)
- **Status**: resolved
- **Rationale**: Key the atlas cache on `GlyphKey { id, bold, italic }` internally and
  pass the style into `rasterize_glyph`, which applies swash embolden (bold) / skew
  (italic) synthesis at scale time — so bold/italic cache and pack separately from the
  regular glyph. **The existing `get_or_insert(queue, glyph_id: u16)` signature is
  KEPT** as a thin wrapper delegating to the styled path with
  `GlyphKey { id, bold:false, italic:false }`; a new `get_or_insert_styled(queue,
  GlyphKey)` carries the style. The terminal pass (in-surface `lib.rs`) calls the
  styled method; every out-of-surface caller keeps compiling unchanged.
- **Blast-radius constraint (why the wrapper, not a signature change)**: two sibling-
  owned `DrawLayer`s call the bare `u16` form —
  `crates/renderer/src/tab_bar.rs:363` (owned by `specs/tabs`) and
  `crates/renderer/src/overlay_layer.rs:265` (owned by ai-monitor / glyph-rendering).
  Neither file is in this feature's declared surface. Changing `get_or_insert`'s
  signature would break their compile or force edits into another feature's partition,
  so the change is deliberately confined to `atlas.rs` behind the preserved `u16`
  entry point. Rejected: replacing the `u16` signature outright (partition breach);
  keying on `glyph_id` only (bold/regular collide → wrong weight); a second atlas
  texture per style (wastes atlas space, duplicates eviction).

### D11: Keep per-row shaping; rasterize each glyph with its lead cell's style
- **Status**: resolved
- **Rationale**: Row shaping (for ligatures) stays as-is; each shaped glyph is
  rasterized with the style of the cluster's lead cell (looked up via the existing
  `byte_col` map). A ligature that straddles a bold/regular boundary uses the lead
  cell's style — rare and acceptable for this pass. Rejected: per-style-run
  re-shaping now (a larger rework of the shaping loop; deferred as a known gap).

## Constraints
- `term-core` gains no GPU/winit dependency; `Color`, `Cell`, `CellAttrs`, and the
  SGR parser stay headless-testable (SD1).
- **`Cell` must keep a derived, field-complete `PartialEq`/`Eq`, and every
  render-affecting attribute must live inside `Cell`** — the sibling `idle-cpu`'s
  snapshot-equality cache depends on it (SD2, idle-cpu "Cache-key completeness"). No
  hand-written `PartialEq`; no reverse/dim/etc. stored outside the snapshot.
- No new GUI/widget crate; macOS-ARM64 only; attribute glyphs come from `swash`
  synthesis, not a text-layout toolkit (SD3).
- Unknown / unsupported SGR codes are ignored and must not corrupt parsing of the
  surrounding parameters or the byte stream.
- The renderer change must not add unconditional per-frame work that defeats
  `idle-cpu` — attribute reads happen inside the existing instance builders, which
  only run on a cache miss (SD4).
- **Stay inside the declared surface.** The `Atlas::get_or_insert(queue, u16)`
  signature must be preserved (as a wrapper), because sibling-owned `DrawLayer`s
  (`tab_bar.rs`, `overlay_layer.rs`) call it and are NOT in this feature's surface.
  All atlas changes stay within `renderer/src/atlas.rs`; no edit lands in another
  feature's partition (D10).

## Known Gaps
- A ligature spanning a style boundary (e.g. a bold `=>` next to regular text)
  renders in the lead cell's style (D11); per-run re-shaping is deferred.
- Blink / conceal / strikethrough / overline SGR codes are parsed-as-ignored this
  pass (non-goal); adding them later is additive to `CellAttrs`.
- The 6×6×6 / grayscale 256-color ramp uses the standard xterm values; a
  user-configurable palette is out of scope.
- Dim is rendered by scaling the glyph color toward the background; exact
  perceptual matching with other terminals is not guaranteed.

## Requirements

### R0: Real TUIs render with correct colors and attributes
#### R0.1: A colored TUI matches its intent
- **Given**: `vim`, `bat`, or `ls --color` running in the terminal
- **When**: it emits 256-color / truecolor / bold / underline / reverse SGR
- **Then**: the on-screen colors and attributes match what the program requested
  (no fallback to the 16-color-only approximation, no dropped attributes)

### R1: 256-color SGR
#### R1.1: Indexed foreground `38;5;n`
- **Given**: the SGR sequence `ESC[38;5;196m` (bright red, cube index 196)
- **When**: `handle_sgr` processes it and a char is printed
- **Then**: the cell's `fg` equals `ansi_256(196)` (the xterm cube color), not a
  16-color approximation
#### R1.2: Indexed background `48;5;n`
- **Given**: the SGR sequence `ESC[48;5;21m`
- **When**: `handle_sgr` processes it and a char is printed
- **Then**: the cell's `bg` equals `ansi_256(21)`
#### R1.3: Ramp coverage (0–15 / 16–231 / 232–255)
- **Given**: indices 7 (base), 46 (cube), and 244 (grayscale)
- **When**: each resolves through `ansi_256`
- **Then**: 0–15 delegate to `ansi_16`, 16–231 map to the 6×6×6 cube, and 232–255
  map to the 24-step grayscale ramp (distinct, monotonic gray)

### R2: Truecolor SGR
#### R2.1: 24-bit foreground `38;2;r;g;b`
- **Given**: the SGR sequence `ESC[38;2;255;128;0m`
- **When**: `handle_sgr` processes it and a char is printed
- **Then**: the cell's `fg` equals `Color::rgb(255,128,0)`
#### R2.2: 24-bit background `48;2;r;g;b`
- **Given**: the SGR sequence `ESC[48;2;0;40;80m`
- **When**: `handle_sgr` processes it and a char is printed
- **Then**: the cell's `bg` equals `Color::rgb(0,40,80)`
#### R2.3: Colon-subparam form (with the T.416 color-space-id slot)
- **Given**: the colon truecolor form `ESC[38:2::255:128:0m` (vte delivers the param
  as `[38,2,0,255,128,0]` — the empty color-space-id becomes a `0` slot) and the
  colon indexed form `ESC[38:5:196m`
- **When**: `handle_sgr` reads the param's subparams
- **Then**: the truecolor form skips the color-space-id slot and resolves to
  `Color::rgb(255,128,0)` (the LAST three subparams, not `Color::rgb(0,255,128)`); the
  indexed form resolves to `ansi_256(196)` — both equal to their semicolon
  equivalents

### R3: Text attributes parse and stamp into cells
#### R3.1: Set codes
- **Given**: `ESC[1m` (bold), `ESC[2m` (dim), `ESC[3m` (italic), `ESC[4m`
  (underline), `ESC[7m` (reverse), each followed by a printed char
- **When**: `handle_sgr` processes each
- **Then**: the printed cell's `attrs` has the corresponding bit set (and combining
  e.g. bold+underline sets both bits)
#### R3.2: Clear codes
- **Given**: bold+dim+italic+underline+reverse are active
- **When**: `ESC[22m` (normal intensity), `ESC[23m`, `ESC[24m`, `ESC[27m` are
  processed
- **Then**: 22 clears BOTH bold and dim, 23 clears italic, 24 clears underline, 27
  clears reverse — leaving the other bits untouched
#### R3.3: SGR 0 resets attributes too
- **Given**: any attributes + non-default colors are active
- **When**: `ESC[0m` is processed
- **Then**: `pen_attrs` is cleared and fg/bg return to the configured defaults

### R4: Default-reset 39 / 49
#### R4.1: `39` resets only foreground
- **Given**: a non-default fg, a non-default bg, and bold active
- **When**: `ESC[39m` is processed
- **Then**: `fg` returns to `default_fg` while `bg` and the bold bit are unchanged
#### R4.2: `49` resets only background
- **Given**: a non-default bg, a non-default fg, and underline active
- **When**: `ESC[49m` is processed
- **Then**: `bg` returns to `default_bg` while `fg` and the underline bit are
  unchanged

### R5: Renderer displays the attributes
#### R5.1: Reverse swaps fg/bg
- **Given**: a cell with `REVERSE` set, logical fg=white bg=black
- **When**: the renderer builds bg + glyph instances for that cell
- **Then**: the background quad uses the fg color and the glyph uses the bg color
  (visual swap), while the stored `Cell` keeps its logical fg/bg. The
  `build_bg_instances` "skip when bg == bg_default" optimization is evaluated on the
  *effective* (post-swap) background, so a reversed cell whose logical fg differs from
  the default still emits its quad, and the glyph uses the logical bg even when that
  equals the default
#### R5.2: Bold / italic select a synthesized glyph via a style-aware atlas key
- **Given**: a bold cell and a regular cell showing the same character
- **When**: the renderer rasterizes each glyph through the new
  `get_or_insert_styled(queue, GlyphKey{ id, bold, italic })`
- **Then**: the two cache separately (distinct `GlyphKey`s) and the bold glyph is
  emboldened (heavier) via swash synthesis
#### R5.5: Existing `u16` atlas callers are unaffected
- **Given**: `tab_bar.rs` and `overlay_layer.rs` call `atlas.get_or_insert(queue,
  glyph_id)` with a bare `u16`
- **When**: the atlas gains the styled path
- **Then**: the `get_or_insert(queue, u16)` signature still exists (delegating to
  `GlyphKey{ id, bold:false, italic:false }`), those files compile unchanged, and no
  edit lands outside this feature's declared surface
#### R5.3: Underline draws a rule
- **Given**: a cell with `UNDERLINE` set
- **When**: the renderer builds instances for that cell
- **Then**: an extra thin quad is emitted at the cell's baseline row in the fg color
#### R5.4: Dim scales the color
- **Given**: a cell with `DIM` set
- **When**: the glyph instance color is computed
- **Then**: the glyph color is scaled toward the background (visibly dimmer than the
  same non-dim color)

### R6: The idle-cpu snapshot cache stays correct (cross-feature contract)
#### R6.1: Attribute-only change invalidates the equality cache
- **Given**: two snapshots identical except one cell's `attrs` (e.g. bold toggled)
- **When**: `GridSnapshot` `PartialEq` compares them
- **Then**: they compare unequal (because `Cell`'s derived `PartialEq` includes
  `attrs`), so `idle-cpu`'s renderer cache treats it as a miss and re-shapes
#### R6.2: `Cell` `PartialEq` is derived and field-complete
- **Given**: the `Cell` type after adding `attrs`
- **When**: the code is reviewed / compiled
- **Then**: `Cell` still `#[derive(PartialEq, Eq)]` (no hand-written impl) and no
  render-affecting attribute is stored outside `Cell`

### R7: Malformed / unknown SGR is handled safely
#### R7.1: Unknown code is ignored, surrounding params intact
- **Given**: `ESC[38;5;196;9;1m` (a valid 256-color set, an unsupported `9`
  strikethrough, then bold)
- **When**: `handle_sgr` processes the sequence
- **Then**: fg = `ansi_256(196)`, bold is set, and the unsupported `9` is skipped
  without consuming `1` or corrupting the pen
#### R7.2: Truncated multi-param sequence does not panic
- **Given**: a truncated `ESC[38;5m` or `ESC[38;2;255m` (missing trailing params)
- **When**: `handle_sgr` processes it
- **Then**: it does not panic and leaves the pen unchanged (or applies a documented
  safe default), skipping the incomplete directive

## Tasks

### T1: Color + attribute model in term-core (Cell, CellAttrs, ansi_256, pen) [infra]
- **Fulfills**: R1, R2, R3, R4, R6, R7
- **Depends on**: (none)
- Add `CellAttrs(u8)` (BOLD/DIM/ITALIC/UNDERLINE/REVERSE masks; derive Copy/Clone/
  PartialEq/Eq/Debug/Default) and an `attrs: CellAttrs` field on `Cell` (keep the
  derive). Add `ansi_256(n)`. Extend the grid pen (`pen_attrs`, set/clear-bit
  methods, per-channel default reset for 39/49); `print` stamps `pen_attrs`;
  `reset_pen` clears it. Rewrite `handle_sgr` with a peekable param iterator
  consuming `38/48;5|2;…` (semicolon + colon subparam forms), attribute set/clear
  codes, and 39/49, keeping the existing 16-color arms and ignoring unknown codes
  safely (R7). Pure `term-core` — GWTs verifiable via `insta` grid snapshots.

### T2: Renderer displays attributes (reverse/dim/underline + style-aware atlas) [vertical]
- **Fulfills**: R0, R5
- **Depends on**: T1   (consumes `Cell.attrs`; and, per the manifest, this whole
  feature `depends_on: [idle-cpu]` — the `renderer/src/lib.rs` edits layer on
  idle-cpu's caching, which invalidates on the R6 equality change)
- In `build_bg_instances` / `build_glyph_instances`, read `cell.attrs`: swap fg/bg
  on REVERSE (evaluate the bg-default skip on the effective bg, R5.1), scale color on
  DIM, emit an underline quad on UNDERLINE, and select a styled glyph on BOLD/ITALIC.
  In `atlas.rs`, add a `GlyphKey { id, bold, italic }` cache key and a
  `get_or_insert_styled(queue, GlyphKey)` method; **keep** `get_or_insert(queue, u16)`
  as a thin wrapper (`GlyphKey{ id, bold:false, italic:false }`) so `tab_bar.rs` /
  `overlay_layer.rs` compile unchanged (R5.5) — no edit outside the declared surface.
  Pass style into `rasterize_glyph` (swash embolden/skew). Keep per-row shaping;
  rasterize each glyph with its lead cell's style (D11).

## External Dependencies

### Pre-work
- (none)

### Post-work
- Eyeball with a real TUI (`vim`, `bat`, `ls --color`, or a truecolor test script
  like `printf '\e[38;2;255;100;0mX\e[0m'`) to confirm colors + attributes render —
  the R0 acceptance evidence, verified in the execute pipeline (`/run-term`).
