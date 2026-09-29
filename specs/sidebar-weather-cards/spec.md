# Spec: sidebar-weather-cards

## Meta
- **Created**: 2026-09-26
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-09-26
- **Design source**: `Sidebar Weather Cards.html` (repo root, untracked). A self-unpacking
  bundle: three boards — *After — weather-style cards*, *Before — current two-line cards*,
  *Card anatomy & states*. The After and Anatomy boards are the target; Before is the
  current implementation.

## Goal
I'd like to change card ui (you need to check file('Sidebar Weather Cards.html'))

## Non-goals
- The sidebar **header**. The After board's "Search tabs" pill and single collapse button are
  out of scope; the header stays as it is today. Tab search is a later spec.
- Line-1 title **precedence** (user > AI-task > program OSC > cwd, `sticky-tab-title`) —
  unchanged; this spec changes where and how that string is drawn, not which string it is.
- The ai-monitor right panel.
- New skins for Gemini and Aider — the design defines none; they keep the plain-shell look.
- A density setting. The design's Anatomy board fixes the card at 104 logical px; the
  compact/roomy values in the After board's props are a design-tool knob, not a feature.

## Confirmed Goal
Rebuild the sidebar tab cards so the running terminal matches the **After** and **Card
anatomy & states** boards of `Sidebar Weather Cards.html`, replacing today's 64px two-line
card (the **Before** board):

- **Geometry** — 104 logical px tall, corner radius 16, padding 12 / 16, 10px gap between
  cards.
- **Skins** — a saturated 160° three-stop gradient per agent: Claude (rust), Antigravity
  (blue → violet → wine), **Codex (dark teal, new — today Codex renders as the plain card)**,
  and plain shell (warm grey).
- **Four zones** — top-left: the resolved line-1 title (17px bold) over a folder icon + the
  live folder name; top-right: a pixel-art agent icon (Claude Code, Antigravity, Codex,
  default terminal); bottom-left: status icon + text (`Working`, `Ready`, `Waiting for
  input`, `Exited N`, `zsh`); bottom-right: branch icon + git branch.
- **States** — default (faint 1px white border), hover (brighter gradient, `≡` menu button
  appears left of the agent icon), selected (1.5px tinted border + 3px glow ring).
- **Status colours** come from the existing theme tokens (`mark_agent`, `mark_ready`,
  `waiting_fg`, `error_fg`, idle).

Inferred beyond the literal request: `Exited N` requires a card to know its tab's last
failed exit code — data that today reaches only the waiting band — so this is new data on
the card, not only new paint.

Done when the running terminal's cards visibly match the After and Anatomy boards; a
golden-frame test asserts the captured pixels (not only layout arithmetic); `cargo build`,
`cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` are
green; and `cargo mutants` on the changed files leaves no survivor that is not argued
equivalent.

## Research

### The design file (read by unpacking the bundle and rendering it in headless Chrome)
- The After board's card is `height: {{cardH}}px` with `cardH = {compact: 88, regular: 104,
  roomy: 120}[density]`, default `regular`. The Anatomy board fixes it: "316 × 104 logical
  px · radius 16 · padding 12 / 16 · gap 10".
- List metrics (After board): list padding `2px 14px 0 14px`, `gap: 10px` between cards;
  top-row `gap: 10px` between the title column and the icon. Today's metrics are
  `card_margin_x: 8`, `card_margin_y: 3` (a 6px gap), `card_radius: 13`, `card_pad_x: 15`,
  `title_px: 15` (`crates/renderer/src/sidebar_theme.rs:130`).
- Text: title `17px / 700`, `#f4efe6`; folder, status and branch `12.5px`; folder and branch
  in `rgba(244,239,230,0.78)`. The whole mock is monospace (`Menlo, 'JetBrains Mono', 'Apple
  SD Gothic Neo'`), as is the renderer.
- Skins (`linear-gradient(160deg, a 0%, b 55%, c 100%)`): the default gradient, then a
  brighter one for hover and selected. The selected border is `1.5px solid <tint>` plus
  `box-shadow: 0 0 0 3px <glow>`:

  | Skin | default | hover / selected | selected border | glow |
  |---|---|---|---|---|
  | Claude | `#7a3e22 #4e2818 #331c14` | `#8a4526 #5a2c1a #3a1f16` | `rgba(255,214,186,.62)` | `rgba(255,190,150,.13)` |
  | Antigravity | `#264578 #352866 #471e26` | `#2c4f86 #3d2f74 #52232c` | `rgba(196,212,255,.6)` | `rgba(160,180,255,.14)` |
  | Codex | `#24403b #1e2d2a #19201e` | `#2a4a44 #22332f #1c2422` | `rgba(200,236,223,.6)` | `rgba(160,220,200,.13)` |
  | Plain | `#3d3934 #2e2b27 #25221f` | `#46413b #34302b #292622` | `rgba(255,255,255,.5)` | `rgba(255,255,255,.10)` |

  Default border is `1px solid rgba(255,255,255,.10)`; hover is `rgba(255,255,255,.18)`.
- **Board disagreement:** the After board draws unselected cards with the *hover* gradient
  and a `.12` border, while the Anatomy board separates default and hover. Anatomy is the
  one titled "states".
- Status text colours: Claude Working `#ffd9bd`, Codex Working `#c9ecdf`, Ready `#b9cdf0`,
  Waiting `#f2c186` (bold), Exited `#ff9a90` (bold), `zsh` at `.78` white.
  - The Anatomy caption says these "come from the existing theme tokens" (`mark_agent`,
    `mark_ready`, `waiting_fg`, `error_fg`), but **none of the values equals the token**.
    The tokens are `#e2d9c8`, `#7796b8`, `#e0a866` and `#e0574d`
    (`sidebar_theme.rs:67`), and Working is tinted per skin.
  - So the caption names *which role* each colour plays, while the mock's hex values are
    lightened versions for saturated backgrounds.
- **Agent icons** are pixel art (`shape-rendering: crispEdges`), made only of filled
  rects:
  - **Sizes:** Claude is a 17×10 grid drawn at `round(1.2·iconSize) × …` (43×25 at the
    default `iconSize` 36). Antigravity, Codex and the terminal are 12×10 grids at 36×30.
  - **Colours:** Claude is one colour (`#e6714e`). Antigravity has 50 cells in 49 distinct
    colours, and its cells are 1.02 wide to hide seams. Codex is two colours; the
    terminal is four.
  - **Offsets:** `margin-top` is 4 for Claude and 2 for the rest.
- **Hover menu button:** 26×26, radius 8, background `rgba(255,255,255,.16)`, a 14px
  three-line glyph at stroke 2.2, and 8px left of the agent icon.
- **Status and zone icons** are 12–13px strokes at widths 2–3: a six-spoke asterisk, a
  diamond, a *filled* two-bar pause, an X, a `>_` prompt, a folder outline and a branch.
- **Before board = today.** Rendering it reproduces the current card: 64px, a `≡` on every
  card, and a faint 135° tint on agent cards only.

### Current card renderer
- `crates/renderer/src/sidebar_row.rs` draws the whole sidebar in one `DrawLayer`
  (`TabBar::record`, `:1177`). It uses two pipelines: `SidebarShapePipeline` for boxes and
  icons, and a local resident-buffer glyph pipeline for text.
- Card height is **derived from the grid cell height** (`layout_with`, `:291–:312`):
  `2·(border + pad_y) + cell_h + (show_git_branch ? gap + cell_h·meta_ratio : 0)`. The
  published `SidebarLayout` is what hit-testing reads (`sidebar_layout.rs:160`, `row_h` is an
  *input*), so changing the height in one place moves drawing and hit-testing together.
- **Text size is tied to the terminal font.** `push_glyphs` (`:1055`) draws atlas glyphs
  scaled by a factor: `1.0` for the title and `meta_px/title_px` for the sub-line.
  - There is **one atlas** at the grid's size: `ppem = font.size_pt × scale`
    (`lib.rs:360–365`), a 2048² R8 texture (`atlas.rs:221`) with linear filtering
    (`atlas.rs:73`).
  - So sidebar text is never rasterized at its own size. A factor above 1.0 upscales a
    bitmap. The default font is 14pt (`config.rs:135`), so the mock's 17px title would be a
    ×1.21 upscale.
- `resolve_card` (`:735`) maps skin × {Idle, Hovered, Selected} to fill and border. The
  `Generic` idle card emits **no instance** (transparent).
- The `≡` is drawn on **every** card (`:1403–:1421`) at `menu_button_in`
  (`sidebar_layout.rs:409`). That formula is shared by drawing and hit-testing
  (`BUTTON_PX 22`, `BUTTON_INSET 2`, `:119`). Whether it should be hover-only was left
  **explicitly open** by `sticky-tab-title` (`specs/sticky-tab-title/spec.md:361`).
- **Text on the card today:** line 1 is `RowMeta::line1`, resolved app-side. Line 2
  carries the cwd name only when `line1_is_custom`, and otherwise just the branch.
  - The rename field draws in place of line 1 (`:1428–:1480`) and assumes the title sits
    at `cell` size.
- Hover arrives through `TabModelHandle::set_hovered`, driven by one region hit-test in
  `refresh_hover` (`terminal_app.rs:1658–:1673`). Selection outranks hover.

### Shapes and icons
- The shape primitive already does what the skins need:
  - rounded boxes with radius and border;
  - capsules for strokes and rings;
  - **3-stop gradients in any direction**, with `t` projected over the bbox the way CSS
    does (`sidebar_shapes.wgsl:115–119`; `Fill::gradient`, `sidebar_shapes.rs:60`).
  - A 160° gradient is just a direction vector, and a 3px glow ring is a rounded box behind
    the card.
- Icons are **stroke-only, single-colour** tables in a 24-unit box at a fixed `STROKE_W
  1.7` (`sidebar_icons.rs:52`), emitted as capsules, rings and rounded boxes by
  `push_icon`. There is no filled-rect, per-cell-colour ("pixel") icon kind, and no folder
  or `>_` icon.
- `AgentSkin` has `Claude | Antigravity | Generic` (`sidebar_theme.rs:208`). The app maps
  `AgentKind::{Codex, Gemini, Aider}` → `Generic` (`sidebar_model.rs:149`), and the match is
  deliberately exhaustive (R9.3). `AgentKind::Codex` already exists
  (`tabmeta/mod.rs:148`).

### Data a card can reach
- `RowMeta` (`sidebar_row.rs:117`) carries `branch`, `mark`, `skin`, `line1`,
  `line1_is_custom` and `cwd_title` (the basename). `SidebarMetaSource::fill`
  (`sidebar_model.rs:60`) builds it from the worker-published `TabMeta`
  (`tabmeta/mod.rs:59`: `branch`, `agent{kind,state}`, `cwd` as a full path, `task`,
  `agent_pid`). **Neither carries the agent kind directly**: the renderer sees only
  `skin`, and Gemini and Aider collapse into `Generic`.
- **Exit codes.** zsh emits `OSC 133;C` before a command and `133;D;<exit>;<ms>` after it
  (`crates/term-core/assets/shell-integration/zshrc.zsh`).
  - The parser consumes **only `D`**; `C` is recognised and dropped
    (`term-core/src/parser.rs:380–385`).
  - `Session::drain` keeps the latest `CommandEnd` (`session.rs:142`). `terminal_app.rs:2224`
    takes it and hands it to `WaitingState::on_command_end` (`waiting.rs:119`), which keeps
    only a `WaitingKind` and a timestamp. **The exit code is discarded.**
  - The band's error rule ignores `{0, 1, 130}` (`waiting.rs:33`) and is
    acknowledged-on-activate: the active tab never shows in the band.
- A per-tab value that lives on the UI thread already has a precedent for reaching the
  card: `SidebarRow::program` (the OSC 0/2 program title) comes from the `Session` through
  `Tabs::titles`, and `sync_tab_model` refreshes it on change (`terminal_app.rs:2239`).
- **The design's folder line is inconsistent:**
  - Some cards show a basename (`terminal`), others a `~/…` path (`~/work/api-server`,
    `~/.config`).
  - The Anatomy caption says "the live folder name".
  - `TabMeta::cwd` is a full path, so either rendering is derivable.
  - With a basename, an un-renamed card would print its folder twice (title = folder
    basename).

### Config and toggles
- `[sidebar]`: `width_px 340` (matches the mock), `show_git_branch`, `show_agent_status`,
  `collapsed`, `accent` (`config.rs:113–171`). `font.size_pt 14`.

### Tests and verification
- `crates/renderer/tests/golden_frame.rs` captures real frames (`Renderer::capture`,
  `lib.rs:1123`; `CapturedFrame::pixel`) but only asserts **grid** cells. **No test asserts
  a sidebar pixel.** The `sidebar_row.rs` GPU fixtures (`:2117–:2300`) record frames but
  check only bind-group identity.
- `Renderer::add_layer` (`lib.rs:535`) plus the default config (sidebar expanded) are enough
  to draw a `TabBar` into a captured frame. A `MetaSource` test double can set skins and
  marks.
- Existing unit tests pin today's geometry and will be rewritten, not merely updated:
  - `card_height_reserves_the_branch_line_by_config` (`:1775`)
  - `card_height_is_independent_of_the_task_label` (`:2039`)
  - the line-2 repo/branch budget tests (`:1986–:2032`)
  - `metrics_default_are_mock_values` (`sidebar_theme.rs`)
- **Pre-existing defect on the cards:** sidebar card text lays out at a **fixed per-char
  advance** (`push_glyphs`, `:1055`; `truncate_to_width`, `:371`), so every Hangul glyph (2
  columns) overlaps its neighbour. The column-based helpers that fixed this for the
  ai-monitor panel and the preedit layer (`tab_bar.rs` `char_cols` / `advance_px` /
  `pen_layout` / `truncate_cols`, "Hangul fix #6") were never adopted by `sidebar_row.rs`.
  The mock's titles are Korean (`사이드바 카드 리디자인`, `auth 마이그레이션`).

## Decisions

### D1: Card geometry is the Anatomy board's, at a fixed height
- **Status**: resolved (auto, from the design file)
- **Decision**: every card is **104 logical px** tall, with corner radius 16, padding 12
  (vertical) / 16 (horizontal), and a 1px border (1.5px when selected). Cards sit **14px**
  from each sidebar edge with a **10px** gap between them, and the top row keeps a 10px gap
  between the title column and the agent icon. Everything is scaled once through
  `SidebarMetrics::scaled`, as today. The list's top edge (below the header and the
  waiting band) is unchanged.
- **Amended during verification (2026-09-27, user-approved):** the card's values live in
  their own `CardMetrics` (`sidebar_theme.rs`), scaled once through the pure
  `CardMetrics::scaled`; the rest of the sidebar keeps `SidebarMetrics::scaled`. The rule
  the sentence above states — one logical-to-physical conversion, no ad-hoc scaling —
  holds for each; only the name of the struct differs from the one written here.
- **Rationale**: the Anatomy board states "316 × 104 logical px · radius 16 · padding 12 / 16
  · gap 10". The After board's list uses `padding: 2px 14px 0 14px; gap: 10px`.
  - **Rejected: deriving the height from the grid cell, as `layout_with` does today.** After
    D2 no card text depends on `font.size_pt`, so there is nothing variable left to derive
    from, and a fixed height is what the mock draws.
  - **Rejected: a density setting.** Non-goal; compact and roomy exist only as a
    design-tool knob.
  - This **reverses `sidebar-row-redesign` D6** ("the branch line is reserved by
    `show_branch`"): the height no longer toggles with config (see D13).

### D2: Card text is rasterized at the design's sizes by sidebar-owned atlases
- **Status**: resolved (user)
- **Decision**:
  - **Sizes:** the title is **17 logical px bold**; the folder, status and branch text is
    **12.5 logical px** (bold for `Waiting for input` and `Exited N`).
  - **Atlases:** the sidebar layer owns **two glyph atlases**, built with the existing
    `Atlas::new(device, fonts, ppem)` at `17 × scale` and `12.5 × scale`. They use the same
    `FontSet` fallback chain and the real bold faces (`specs/real-bold-faces`).
  - **Independence:** card text therefore **no longer follows `font.size_pt`**.
  - **Wiring:** `FrameCtx` gains the `FontSet` it needs to build them.
  - **Rebuilds:** an atlas is rebuilt when its ppem changes (a scale-factor change) or the
    font set changes (a config reload of `font.family`).
  - **Scope:** only **card** text moves. The waiting band, the card-menu popup and the
    header keep drawing from the grid atlas at today's sizes (the band is out of scope), so
    the layer issues one text draw per atlas.
  - **Amended during verification (2026-09-27, user-approved):** one text draw per atlas
    holds on the normal path. When a card atlas cannot be built (R2.6), that face's text
    falls back to the grid atlas and is a **second** grid-atlas draw, submitted with the
    card text and **under** the popup. It cannot share the band/popup draw: the popup's
    item text must be drawn over the popup and stays on the grid atlas (R2.7), while card
    text must be drawn under it — with one draw, a degraded card title painted over an
    open menu.
- **Rationale**: chosen by the user as "design sizes, own atlas". The mock's title is 17px
  against a 14pt terminal font. The single grid atlas (`lib.rs:360–365`) can only produce
  that by upscaling a bitmap ×1.21.
  - **Rejected: follow the terminal font.** No new atlas, but at the default 14pt every
    card string is ~18% smaller than the mock, and the card height would have to follow
    the font again.
  - **Rejected: stretch the bitmap.** Exact size, but a soft title, and the title is the
    card's most prominent element.
  - **Cost, accepted:** 2 × 2048² R8 textures = 8 MB of GPU memory, plus a font/scale
    rebuild path.
  - **Rejected: one sidebar atlas at 17px** with the 12.5px text downscaled from it (×0.74
    bilinear). Softer secondary text, for 4 MB saved.
  - **Rejected: adding a size dimension to the shared `GlyphKey`.** That touches the grid's
    hot path and the atlas-swap invariants for a sidebar-only need.
  - **Rejected: signed-distance-field (SDF/MSDF) glyphs**, one size-independent atlas scaled
    in the shader. This is raised by the L2 review as the strongest alternative, since it is
    the standard answer to "crisp text at several sizes", and it was rejected for four
    reasons:
    1. **Weakest where this spec needs it most.** SDF text is weakest at small sizes, and
       12.5px is exactly the one this spec needs most. A distance field loses the hinting and
       coverage detail swash's rasterizer keeps, and the dense strokes of Hangul syllables
       need a high-resolution field to survive.
    2. **Costs more than it saves.** Generating fields needs either a new dependency (an
       msdfgen port) or a new rasterization path over swash's outlines, plus a new text
       shader, for a two-size, sidebar-only need.
    3. **Card text would stop matching the grid.** It would render by a different technique
       than the grid, so the two would stop matching, including the real bold faces
       `specs/real-bold-faces` just fixed.
    4. **It removes neither D18 nor D20.** An SDF atlas is still an atlas that evicts, so it
       would remove one of the two textures, not the eviction handling or the cache key.
    - The 8 MB it saves is not worth that on this hardware.
  - **Confirmed at the inversion probe:** raising `font.size_pt` (for example to 18 for
    readability) enlarges terminal text and leaves card text at 17 / 12.5. The sidebar is
    chrome, like the header.
  - **Rejected: scaling card sizes by `size_pt / 14`.** Crisp, but it brings back a
    font-dependent card height, which D1 removes.

### D3: Card text lays out by display column, not by char
- **Status**: resolved (auto, from L1's pre-existing defect)
- **Decision**:
  - Every card string advances through the same column rule the ai-monitor and preedit
    layers use (`tab_bar::char_cols` / `advance_px` / `pen_layout`): a wide char (Hangul,
    CJK) takes two columns.
  - Truncation budgets in **columns** (`truncate_cols` semantics, then the ellipsis), never
    half-draws a wide glyph, and never splits a codepoint.
  - A column's width at each text size is the same `0.6 × px` (rounded) the grid uses
    (`cell_size_px`, `lib.rs:1906`), computed from that atlas's ppem.
- **Rationale**: the mock's titles are Korean. Today's fixed per-char advance overlaps
  every Hangul glyph on a card (L1). The fix already exists and is shared precisely so the
  rule cannot drift into copies (`tab_bar.rs` module doc). Rejected: keeping
  `truncate_to_width`'s char count, which is the defect.

### D4: Card states follow the Anatomy board
- **Status**: resolved (auto, from the design file)
- **Decision**:
  - **Default:** the skin's default gradient and a 1px `rgba(255,255,255,.10)` border.
  - **Hover:** the skin's brighter gradient, a 1px `rgba(255,255,255,.18)` border, and the
    `≡` button (D11).
  - **Selected:** the brighter gradient, a 1.5px border in the skin's tint, and a 3px glow
    ring in the skin's glow colour.
  - **Precedence:** selection outranks hover, as today (`resolve_card`).
  - **Glow:** the ring is a rounded box drawn **behind** the card, grown by 3px on every
    side, with radius `16 + 3`, filled with the glow colour. The card's own box is painted
    over it.
  - **Plain cards:** every skin, including the plain one, is drawn in every state.
- **Rationale**: the Anatomy board is the one titled "states"; the After board draws
  unselected cards with the hover gradient, a mock shortcut that Anatomy overrides.
  - Drawing the glow as a box behind the card is how a CSS `box-shadow: 0 0 0 3px` spread
    renders, and needs no new primitive (`SidebarShape::rounded_box`).
  - This **reverses `sidebar-row-redesign` D12's** "a `Generic` idle card emits no
    instance". The design gives every card a body, so the plain card is no longer
    transparent.

### D5: Four skins; Codex gets its own
- **Status**: resolved (auto, from the design file)
- **Decision**:
  - **Variants:** `AgentSkin` gains `Codex`. `Generic` keeps its name, and its colours
    become the design's plain-shell skin.
  - **Colours:** each skin carries the design's default gradient, brighter gradient,
    selected tint and glow (the L1 table), as linear-RGBA constants in `sidebar_theme.rs`.
    Stops sit at 0 / 55 / 100%, with direction **160° CSS = `(0.3420, 0.9397)`** in
    top-left-origin space.
  - **Mapping:** `skin_for` maps `AgentKind::Codex → Codex`. Gemini, Aider and no-agent
    still map to `Generic`, and the match stays exhaustive.
- **Rationale**: Codex already exists as an `AgentKind` (`tabmeta/mod.rs:148`) and the
  design gives it a skin. Gemini and Aider are non-goals, since the design defines no skin
  for them. The shader already projects `t` over the bbox the way CSS does
  (`sidebar_shapes.wgsl:115–119`), so a 160° gradient is only a direction vector.

### D6: Agent icons are pixel art, drawn as snapped filled cells
- **Status**: resolved (auto; no existing icon kind fits)
- **Decision**:
  - **Icon kind:** a new pixel icon kind, a const table of `(x, y, w, h, colour)` cells in
    grid units, emitted as zero-radius `rounded_box` shapes.
  - **Which icon:** chosen by skin:
    - Claude → the 17×10 crab, drawn at 43×25 logical px.
    - Antigravity → the 12×10 multicolour "A".
    - Codex → the 12×10 `>_` bubble.
    - Generic → the 12×10 terminal window.
    - The three 12×10 icons draw at 36×30 logical px.
  - **Placement:** top-right, `margin-top` 4 (Claude) or 2 (the others).
  - **Cell size:** each icon has **one uniform cell size in whole physical pixels**,
    `cell_px = max(1, round(design_cell_logical × scale))`, where `design_cell_logical` is
    43/17 for Claude and 36/12 for the rest. The icon is then `cols × cell_px` by
    `rows × cell_px`, anchored at the icon origin, and that origin is snapped to a whole
    physical pixel.
  - **Result:** every cell is the same width, cells tile without seams, and the mock's
    1.02-wide overlap is unnecessary. This is the renderer's analogue of
    `shape-rendering: crispEdges`.
  - **Tolerance:** the drawn icon is within ±0.5 physical px per cell of the design size.
    Claude at 2× is 85×50 physical against the design's 86×50.
  - Gemini and Aider tabs get the terminal icon.
- **Rationale**: the icons are multicolour pixel grids. The existing icon tables are
  single-colour strokes (`sidebar_icons.rs:52`), and Antigravity alone has 50 cells in 49 distinct colours.
  - **Rejected: an image texture pipeline**, which is a new asset path and a new pipeline for
    four tiny icons.
  - **Rejected: approximating with stroke icons**, which cannot carry the colours.
  - **Why snap:** unsnapped, a Claude cell is 2.53 logical px wide, and SDF antialiasing
    would blur every internal edge.
  - **Rejected: snapping each cell *edge* independently.** This version was found by L2's
    inversion probe. On a 2.53px pitch it yields a mix of 2px and 3px cells (at 1×) or 5px
    and 6px cells (at 2×), which is a visibly irregular crab. A uniform integer cell costs
    at most half a pixel per cell of size fidelity.

### D7: The status zone is derived app-side with a fixed precedence
- **Status**: resolved (user chose "Running"; the precedence is derived from it)
- **Decision**:
  - **Type:** a renderer-owned `CardStatus` replaces `RowMark` on `RowMeta`, and the app
    resolves it, keeping the policy app-side like `line1`.
  - **Precedence**, first match wins:
    1. an agent is present → its state: `Busy | Unknown → Working`, `Idle → Ready`,
       `Waiting → Waiting for input`;
       **Superseded in part by `specs/agent-card-status`:** `Unknown` now shows no status
       (its D3, `CardStatus::Hidden`), and a blocking dialog on the tab's screen reads
       `Waiting for input` ahead of the agent's state (its D8).
    2. a foreground command is **running** (OSC 133 `C` seen, no `D` since) → `Running`;
    3. the last finished command failed (D12) → `Exited N`;
    4. otherwise → `zsh`.
  - **Icons:**
    - `Working`: a six-spoke asterisk.
    - `Ready`: a diamond.
    - `Waiting for input`: a filled two-bar pause.
    - `Exited N`: an X.
    - `Running` and `zsh`: the `>_` prompt glyph.
- **Rationale**: the user chose "clear on start, show Running".
  - **Why `Running` outranks `Exited`:** this ordering is what implements "clear on
    start" without any clearing logic. The next command's `C` makes `Running` win; its `D`
    replaces the remembered result. So `Exited N` shows exactly between a failing command
    and the next start.
  - **Why this survives mixed chunks:** it holds even when one PTY read carries both
    markers. A fast `ls` puts `C` then `D` in one chunk, and typeahead can put an old `D`
    then a new `C` in one chunk. Only the parser's final state is read, never event order
    across a drain.
  - **Why an agent outranks everything:** a `claude` started from zsh leaves `Running`
    true for its whole session, and the agent's state is the more specific truth.
  - **`Running` is not in the mock.** It was added by the user's choice because `zsh`
    ("idle, no agent" in the Anatomy caption) would be false while a command runs.

### D8: term-core records the command-start marker as running state
- **Status**: resolved (user; implementation derived)
- **Decision**:
  - **Parser:** handles `OSC 133 ; C` by setting a grid-level `command_running = true`, and
    `133 ; D` sets it `false`. Both are processed in byte order within `advance`.
  - **Exposure:** `Terminal` exposes the flag as a plain accessor, not a one-shot take.
  - **The existing `take_command_end` one-shot is untouched**, so the waiting band's
    consumption is unchanged.
  - **Session:** the `Session` keeps a separate, non-consuming `last_end: Option<CommandEnd>`
    for the card, updated on every `D`.
  - **Delivery:** both values reach the renderer on `SidebarRow`, the same UI-thread route
    `program` takes (`Tabs::titles` → `sync_tab_model`). A change in either triggers the
    same row resync a program-title change does.
- **Rationale**:
  - **Why `C` needs parsing:** the parser drops `C` today (`parser.rs:380–385`), and without
    it neither `Running` nor clear-on-start is observable.
  - **Why a flag and not an event:** a state flag rather than a second one-shot event keeps
    D7's "read the final state" property. An event stream would reintroduce the ordering
    problem.
  - **Why `SidebarRow`:** `RowMeta` is filled from the off-thread metadata worker, which
    never sees PTY bytes, so `RowMeta` is the wrong carrier.
  - **Rejected: reading `WaitingState`.** It keeps no exit code, and it is
    acknowledged-on-activate, so the active tab would never show `Exited`.
  - `term-core` stays policy-free: it records markers, and the app decides what they mean
    (D21 of `specs/left-sidebar-waiting-area/spec.md`).

### D9: Status colours are new role tokens holding the mock's values
- **Status**: resolved (auto; resolves the design file's own disagreement)
- **Decision**:
  - **New tokens** in `SidebarTheme`: `card_title_fg #f4efe6` and `card_meta_fg
    #f4efe6 @ .78`. The folder, branch, `zsh` and `Running` text use `card_meta_fg`.
  - **Status tokens:** `status_working_fg #ffd9bd` (with a Codex override `#c9ecdf`),
    `status_ready_fg #b9cdf0`, `status_waiting_fg #f2c186`, `status_error_fg #ff9a90`.
  - **The existing tokens** (`mark_agent_fg`, `mark_ready_fg`, `waiting_fg`, `error_fg`,
    `title_fg`, `meta_fg`) are **unchanged**; the waiting band and the menu popup keep
    using them.
  - **Uncovered skins:** Working on Antigravity or Generic cards uses the default `#ffd9bd`,
    because the mock shows no Working card for either.
- **Rationale**: the Anatomy caption says status colour "comes from the existing theme
  tokens", but none of the mock's values equals a token (L1). The caption names the
  **role**; the hex values are the tokens lightened to stay legible on saturated gradients.
  - Adding role tokens honours both, and leaves the band, which is out of scope, visually
    untouched.
  - **Rejected: reusing the tokens.** `#e0574d` red on a `#3d3934` card is the contrast
    problem the mock's lightening solves.
  - **Rejected: changing the tokens in place**, which would repaint the waiting band.

### D10: The folder line is the `~`-abbreviated live cwd, truncated from the left
- **Status**: resolved (user)
- **Decision**:
  - **Content:** the line under the title shows the tab's **live** working directory
    (`TabMeta::cwd`, the same value `card-live-cwd` made live).
  - **Abbreviation:** `$HOME` is shown as `~` (`~/IdeaProjects/terminal`); a path outside
    home stays absolute (`/opt/homebrew`).
  - **Where the string is built:** app-side, on the metadata pull, into a new
    `RowMeta::cwd_display`, with `$HOME` read at the point of use as `clipboard.rs:92`
    does. Deriving it there keeps it on the pull that already refreshes the folder name
    live.
  - **Truncation:** when the path is wider than its budget (D14), it is cut **from the
    left** by columns (D3) and prefixed with `…`: `…/IdeaProjects/terminal`. The tail,
    the part that names the folder, survives.
  - **Fallback:** before the first metadata fill, the line shows the row's assembly-time
    folder name (`SidebarRow::title`), the fallback that D2 of
    `specs/card-live-cwd/spec.md` (not this spec's D2) already established.
  - **Retirement:** `RowMeta::line1_is_custom` is retired. Its one consumer was line 2's
    "move the repo name down beside the branch" layout (tab-identity R2.2), which this
    layout replaces, because the folder line is now always present.
    `sidebar_model::resolve_line1_tier` keeps its signature, since `terminal_app.rs:915`
    still calls it.
- **Rationale**: chosen by the user.
  - A path never repeats an un-renamed tab's title, which a basename does (`terminal` over
    `terminal`), and it matches most of the mock's cards (`~/work/api-server`, `~/blog`,
    `~/.config`).
  - Left truncation is the only direction that keeps the part a person reads first.
  - **Rejected: always a basename.** It is the Anatomy caption's literal words, but it
    doubles the title on every un-renamed card.
  - **Rejected: a basename hidden when it equals the title.** A card whose second line
    appears and vanishes with a rename.

### D11: The `≡` menu button appears on hover, beside the agent icon
- **Status**: resolved (auto, from the design file; settles an open question)
- **Decision**:
  - **Visibility:** the button is drawn only on the **hovered** card. It is also drawn on
    the card whose menu is **open** and on the card being **renamed**, even after the
    pointer leaves, so an open popup is never orphaned from its anchor.
  - **Look:** 26×26 logical px, radius 8, fill `rgba(255,255,255,.16)`, with the existing
    hamburger glyph at 14px.
  - **Position:** vertically aligned with the agent icon's top, 8px to its left.
  - **One formula:** `sidebar_layout::menu_button_in` is rewritten to this position, and it
    stays the **one** formula both the draw and the hit-test read (SD2 of
    `specs/sidebar-redesign/partition-manifest.md`).
  - **Clicks:** unchanged. A press can only land on the button while the pointer is over
    its card, which is exactly when the card is hovered.
- **Rationale**:
  - **Not a reversal:** `sticky-tab-title` left this explicitly **open** ("whether the
    hamburger is always drawn on every card or only on the hovered/active card is
    unspecified", `specs/sticky-tab-title/spec.md:361`). The design settles it: "The ≡ menu
    button appears on hover, left of the icon".
  - **Discoverability:** that concern was the reason for `sticky-tab-title` D2 (the
    hamburger as a second rename entry), and it is still met. Rename also has its chord
    (`sticky-tab-title` D1), and every card reveals the button under the pointer.
  - **Rejected: always drawn.** The design removes it, and on a 104px card with an icon
    beside it, it competes with the icon.
- **Found during implementation (T4): the button anchors to a fixed icon slot.**
  - **The problem:** "8px left of the icon" cannot hold on every card *and* keep the
    drawn button identical to the hit-tested one. The icon's width depends on the skin
    (the Claude crab is 43 logical wide, the others 36). But the hit-test computes the
    button from `SidebarLayoutInput` (`terminal_app.rs:1247`), a `Copy` struct with no
    per-card data, so it cannot know which icon a card shows.
  - **The fix:** the icon is right-aligned in a slot as wide as **the widest icon as
    drawn** at the frame's scale (`sidebar_card::icon_slot_w(scale)`), and the button's
    right edge sits 8px left of that **slot**, at a skin-independent
    `sidebar_card::menu_button(card, scale)`. `sidebar_layout::menu_button_in` becomes
    that same function.
  - **Result:** the button sits the mock's 8px from the Claude crab at every scale, and
    further left on the narrower icons. Its top is the mock's 3px (the 26px button
    centred against a 30px icon row), which is 1.5px low on Claude cards.
  - **Amended during verification (2026-09-27, user-approved).** The slot was first a
    constant 43 logical px — the crab's *design* width. D6's whole-pixel cells draw the
    crab wider than that at several scales: 51px at 1× (`round(43/17) = 3` × 17 cells)
    and about 45 logical at 1.5× and 3×. So the button touched the crab at 1× (a 0px gap)
    and sat 5.7 logical px from it at 1.5× and 3×. The Gate-3 checker found it; the fix
    measures the slot instead of assuming it, and stays skin-independent (the widest
    icon over all skins), so R11.4 still holds. Rejected: flooring the crab's cells to
    fit 43px — 34px at 1× breaks R6.3's half-a-pixel-per-cell bound.
  - **Rejected: carrying per-card skins into `SidebarLayoutInput`.** It breaks the
    struct's `Copy` and makes the app assemble per-row renderer data for one button.
  - **Rejected: a button that moves by skin while the hit-test uses one rect.** That
    violates R11.4.

### D12: A card shows `Exited N` by exactly the waiting band's rule
- **Status**: resolved (user)
- **Decision**:
  - **Which codes:** a finished command counts as failed iff its exit code is **outside
    `{0, 1, 130}`**. So `make`'s 2 and `cargo test`'s 101 show; `grep`'s no-match 1 and a
    Ctrl-C 130 read `zsh`.
  - **One source of truth:** `waiting.rs`'s private `ERROR_IGNORE` becomes one
    `pub(crate) fn is_reportable_exit(code: i32) -> bool`, read by both the band and the
    card status (D7).
- **Rationale**: chosen by the user. The card and the band then always agree about whether
  a command failed; the band ignores these codes for reasons argued in its own spec (D7 of
  `specs/left-sidebar-waiting-area/spec.md`: `grep` no-match, Ctrl-C).
  - **Why one function:** two copies of the list could drift, so one function makes the
    agreement structural rather than hoped for.
  - **Accepted cost:** a tool that reports test failure with exit 1 (pytest, `npm test`)
    leaves the card at `zsh`. The user accepted this over a card and a band that disagree
    about exit 1.

### D13: The display toggles hide zones; they no longer change the height
- **Status**: resolved (auto; follows from D1)
- **Decision**:
  - `[sidebar].show_git_branch = false` leaves the bottom-right zone empty.
  - `[sidebar].show_agent_status = false` leaves the bottom-left status zone empty.
  - The agent **icon** is always drawn, because it is the tab's identity, not its status.
    The skin is always applied.
  - The card height stays D1's 104 in every combination.
- **Rationale**:
  - **Why the icon stays:** the toggles exist and keep their meaning (show the branch / the
    status), but the design has no layout for a card without its icon. The icon and the
    skin carry the same identity, and the skin was never toggled.
  - **Rejected: shrinking the card when both toggles are off.** It reintroduces the
    config-dependent height that D1 removes, for a combination the design never draws.

### D14: Each zone truncates within its own budget
- **Status**: resolved (auto)
- **Decision**:
  - **Top-left title:** gets the width up to the agent icon, or up to the `≡` when it is
    drawn, minus the 10px gap. It is truncated by columns with a trailing ellipsis (D3).
  - **Top-left folder line:** the same right edge as the title, but its text starts **after
    the 13px folder icon and a 6px gap** (D21). The path's column budget is therefore the
    title's width minus 19 logical px (scaled), and D10's left truncation works within
    that. At the narrowest sidebar (260 logical px) the icon always fits, so, unlike the
    branch, the folder line is never dropped. It can only shorten to `…` plus the final
    component.
  - **Bottom row, status:** always gets its natural width. The longest status, `Waiting for
    input`, is 17 columns at 12.5px, well inside the ~280px inner width.
  - **Bottom row, branch:** right-aligned into what remains after a 10px gap, and truncated
    with a trailing ellipsis. If not even its icon fits, the branch is dropped, as today
    (`line2_branch_origin`).
  - `Exited N` prints `N` in full, since an `i32` is at most 11 columns.
- **Rationale**: the status is the zone that carries meaning; a clipped `Waiting for inp…`
  is worse than a clipped branch name. Today's rule already prefers dropping the branch
  over overrunning the edge (`sidebar_row.rs:487`), and this keeps that rule. Rejected:
  proportional sharing, which clips both.

### D15: The rename field takes the title's place at the title's size
- **Status**: resolved (auto)
- **Decision**:
  - **Where:** the in-place rename (`sticky-tab-title` R2.1) draws its field box and text
    exactly over the title line: 17px bold from the title atlas.
  - **Caret:** its windowing and the select-all highlight measure with the title atlas's
    column width, not the grid cell.
  - **Folder line:** stays visible below the field.
  - **Width:** the field's box and text end at **D14's title budget**. The `≡` is always
    drawn during a rename (D11), so the field always stops 10px short of it and never
    overlaps it. Today's code already ends the field at `line1_right`, the button-aware
    budget (`sidebar_row.rs:1425`), and this keeps that.
- **Rationale**: the field today assumes the title sits at `cell` size (`sidebar_row.rs:1428–1480`).
  After D2 that assumption is wrong, and the caret would drift from the glyphs it follows.

### D16: Card composition is a pure function; the layer only submits
- **Status**: resolved (auto; follows the ai-monitor precedent)
- **Decision**:
  - **New module** `crates/renderer/src/sidebar_card.rs`, pure and GPU-free:
    `build_card(input) -> (shapes, text runs)`.
    - **Input:** the card rect, skin, state, status, strings, show toggles, scale and the two
      atlases' column widths.
    - **Output:** positioned `(atlas, string, x, y, colour, bold)` text runs that
      `sidebar_row::TabBar::record` resolves through the matching atlas.
  - **What `sidebar_row.rs` keeps:** the model bus, the layout and hit-test publication,
    the waiting band, the menu popup and the pipelines.
  - **New icon data:** the pixel icon tables (D6) and the new stroke icons (folder, `>_`,
    six-spoke asterisk, filled pause) live in `sidebar_icons.rs`.
- **Rationale**: `sidebar_row.rs` is already 2,321 lines. A pure builder is what makes the
  card's geometry and colours assertable, and mutation-testable, without a GPU, the way
  `monitor_layout` is split from `monitor_panel`. Rejected: growing `record` in place,
  which keeps the card's decisions reachable only through a captured frame.

### D17: Verification is by captured pixels plus mutation testing
- **Status**: resolved (auto, from the Confirmed Goal)
- **Decision**:
  - **New golden-frame test:** `crates/renderer/tests/golden_frame.rs` gains a sidebar
    test. It adds a `TabBar` layer with a stub `MetaSource` (one card per skin, one of them
    selected, one hovered) and asserts, **in the captured frame**:
    - each card's sampled body colour lies on its skin's gradient;
    - the selected card has a glow ring outside its edge, which an unselected card lacks;
    - the `≡` backplate is present on the hovered card only;
    - the title band carries ink;
    - each agent icon's sampled cell has its table colour.
  - **Unit tests:** `sidebar_card::build_card`, the D7 status precedence, the D8 parser
    flag and the D10 folder derivation are unit-tested.
  - **Mutation testing:** `cargo mutants -f` runs on every changed file, and each survivor
    is killed or argued equivalent.
- **Rationale**: CLAUDE.md is explicit that a render-path change "is not covered until
  something asserts what reached the screen". No test today asserts a single sidebar pixel
  (L1), so the redesign could ship blank and stay green.

### D18: The sidebar atlases guard their own eviction and swap
- **Status**: resolved (auto; surfaced by round 1's implication check on D2)
- **Decision**:
  - **Bind groups:** the card text pipeline keys its bind groups on **each sidebar atlas's
    own `epoch()`**, the same guard every text layer already uses against the grid atlas
    (`lib.rs:842–849`). A rebuilt sidebar atlas therefore rebinds on its next frame.
  - **Mid-frame eviction:** if an atlas's `generation()` changes **during** a `record`, an
    eviction invalidated UVs already pushed that frame. The layer then rebuilds that
    atlas's text runs once before submitting.
- **Rationale**: `Atlas::pack` evicts when full and invalidates the UVs of instances already
  emitted (`atlas.rs:218`). The grid guards this by re-shaping. The sidebar's glyph set is
  small (a few hundred distinct glyphs against a 2048² texture), so eviction is rare, but
  "rare" is how a one-frame garbage title ships unnoticed. Rejected: relying on the 60 fps
  redraw to heal it next frame. That is the "correct by accident" reasoning
  `specs/atlas-swap-bind-invalidation` recorded as a Known Gap.

### D19: A foreground program that never returns reads `Running`, by design
- **Status**: resolved (user)
- **Decision**:
  - **The rule:** a plain tab whose foreground command is long-lived (`tmux`, `vim`, `less`,
    `top`) shows `Running` for as long as that command runs. That follows directly from
    D7/D8, because its shell emitted `C` and no `D` yet.
  - **Agents still win:** an agent detected inside it, including inside a tmux pane (which
    `tabmeta` already re-roots to), still outranks `Running` with its own status (D7).
  - **Nothing is added** to special-case these programs.
- **Rationale**: chosen by the user. `Running` is literally true, since the tab's shell is
  busy and cannot take a command.
  - **Rejected: showing the program's OSC 0/2 title while running.** It duplicates line 1
    whenever that title already won the line-1 precedence, which for tmux and vim is the
    common case.
  - **Rejected: showing `zsh` for long runs.** It would need a duration threshold that
    loses the `Running` signal for every command, and still reads "idle" while vim is open.

### D20: The layer composes only when an input changed
- **Status**: resolved (user; surfaced by L2's inversion probe)
- **Decision**:
  - **What is cached:** `TabBar` keeps the composed shape instances and text runs of its
    last frame, cards, waiting band and menu popup alike, under a **compose key**.
  - **The key:**
    - `rows_rev`, the metadata `generation`, `active`, `hovered`;
    - the open menu (row and items) and the edit state (tab, text, caret, selection);
    - the waiting rows, `list_top`, `scroll_px`, `sidebar_w`, the viewport height and
      `scale_factor`;
    - `show_git_branch` / `show_agent_status`;
    - **the `(epoch, generation)` of all three atlases it samples**: the grid atlas, used
      by the band and the popup, and the two card atlases (D2).
  - **On a hit:** the layer skips composition and glyph resolution and **re-submits the
    resident buffers without re-uploading them**. On a miss, it composes, uploads and
    stores the key.
  - **Test hook:** a `compose_count` counter lets a test pin "a steady frame composes
    nothing".
- **Rationale**: chosen by the user.
  - **Why now:** the window redraws at 60 fps while visible (`docs/architecture.md` C6),
    and today the sidebar re-composes and re-uploads every shape every frame
    (`sidebar_shapes.rs:460–469`). The pixel icons multiply the instance count (Antigravity
    alone is 50 cells), and nothing guards idle CPU (G1). The ai-monitor panel already
    carries the same kind of change-keyed cache for the same reason.
  - **Why the grid atlas is in the key:** the grid atlas evicts on **terminal** output, not
    sidebar state. Without it, a cached band title would sample UVs from a repacked
    texture. That is D18's stale-UV hazard, reached through the cache instead of within a
    frame.
  - **Rejected: measuring without caching.** A number in a document guards nothing, and G1
    already records that one.
  - **Rejected: caching only the cards.** The band and the popup share the same resident
    buffers, so a partial cache still re-uploads them every frame.
- **Implementation notes (found during verification, 2026-09-27):**
  - **When each atlas is sampled.** The card atlases enter the key as composing *left*
    them, because a mid-frame eviction there is already resolved again (D18). The grid
    atlas enters it as composing *found* it: nothing re-resolves the band's, popup's or
    fallback text, so an eviction caused by the sidebar's own composing must make the
    next frame miss. Keyed on the after-state, that frame hit and the stale UVs stayed
    on screen for as long as the sidebar stayed still.
  - **A steady frame compares in place.** The key's scalars are compared by value, and
    the model's waiting rows, open menu and rename state are compared under a borrow
    against the key's copies; rows and those copies are cloned only for a frame that is
    composed. That is what makes "no per-frame allocation in a steady frame" (the
    Constraints) true of this layer — before this spec the layer cloned the rows every
    frame.

### D21: Status and zone icons take their text's colour, at the mock's sizes and strokes
- **Status**: resolved (auto, from the design file; added after L2 review finding 2)
- **Decision**:
  - **Colour:** every in-card icon is drawn in **the colour of the text it precedes**. This
    is the mock's `stroke/fill="currentColor"` inside each span, so each status icon takes
    its D9 status token (a Codex card's Working asterisk is `#c9ecdf`), and the folder and
    branch icons take `card_meta_fg`. The `≡` glyph is `card_title_fg` on its `.16`
    backplate (D11).
  - **Icons, sizes and strokes (logical px; stroke width in 24-unit space, as the mock's
    `viewBox 0 0 24 24`):**

    | Icon | Size | Stroke width | Shape |
    |---|---|---|---|
    | Working | 13 | 2.2 | six-spoke asterisk |
    | Ready | 12 | 2.2 | diamond outline |
    | Waiting | 12 | filled | two bars at x = 6 and 14, y = 4, each 4 × 16 with radius 1, **in 24-unit box space** (so ≈ 2 × 8 logical px at size 12) |
    | Exited | 12 | 3 | X |
    | `Running` / `zsh` | 13 | 2 | `>_` prompt |
    | folder | 13 | 2 | folder outline |
    | branch | 12 | 2 | branch |
    | `≡` glyph | 14 | 2.2 | hamburger |

  - **Stroke width per icon:** the stroke width becomes a **per-icon** value; today it is
    the fixed `STROKE_W = 1.7`, `sidebar_icons.rs:52`.
  - **Filled bars:** the icon vocabulary gains a filled-rect primitive (a `rounded_box`
    with a fill, where `RectOutline` is border-only) for the Waiting bars.
  - **Placement:** each icon is vertically centred on its text line, with a 6px gap to its
    text (5px for the branch, as the mock).
  - **The band's icons are untouched:** these are **new** card-only icon variants. The
    waiting band keeps its existing `Pause` / `Check` / `Cross` icons exactly as they are,
    because the band is out of scope.
- **Rationale**: D9 fixed text colours only and D6 only the agent icons, so without this a
  requirement writer has no source for an icon's colour or weight.
  - **Why the numbers are the mock's:** the user directed that the file is the source of
    truth, and the table is read straight from its SVGs.
  - **Why new variants:** they are separate from the band's icons because the mock's
    Working mark is a six-spoke asterisk and its pause is filled, while today's are
    eight-spoke and stroked. Changing those in place would repaint the band.
  - **Rejected: one global stroke width.** The mock uses 2, 2.2 and 3 deliberately; its X
    is heavier so it reads as an error.

## Constraints
- **`term-core` stays GPU-free and policy-free** (`dependency-direction`). The parser
  records only the OSC 133 `C`/`D` state (D8), and what a code *means* stays in the app
  (D12).
- **No GUI crates and no new dependencies** (`no-gui-crates`). Every card element is drawn
  through the existing `SidebarShapePipeline` and the sidebar text pipeline;
  `unicode-width` is already a renderer dependency.
- **The draw and the hit-test read one formula** for card rects and the `≡` button (SD2
  of `specs/sidebar-redesign/partition-manifest.md`), and the published `SidebarLayout`
  stays the hit-test's source.
- **Unchanged surfaces**, each argued as out of scope:
  - the header;
  - the waiting band (content, colours and grid-atlas text);
  - the card-menu popup;
  - line-1 precedence (`sidebar_model::resolve_line1_tier`);
  - the rename chord and menu items (`sticky-tab-title`);
  - `take_command_end`'s one-shot semantics (the waiting model).
- **No per-frame allocation in a steady frame** (D10 of
  `specs/sidebar-row-redesign/spec.md`: "a steady state allocates nothing"). Buffers stay
  resident, and D20 additionally skips re-upload.
- **Graceful degradation, never a blank card:**
  - no shell integration → the status reads `zsh`;
  - no metadata yet → the plain skin, the row's folder name and the terminal icon;
  - no bold face in the font → the existing synthesis fallback (`docs/architecture.md`
    A3);
  - a card atlas that fails to build → card text falls back to the grid atlas at its
    size, with a `tracing::warn!`, rather than drawing nothing.
- **macOS ARM64 only** (`macos-only`): no `#[cfg]` platform branches.

## Known Gaps
- **Idle CPU remains unguarded globally** (`docs/architecture.md` G1). D20 adds a
  compose-skip test for the sidebar only; no fps or CPU budget is introduced.
- **`Running` for long-lived foreground programs** (tmux, vim) is deliberate (D19), not a
  defect.
- **Exit 1 from test runners** (pytest, `npm test`) leaves the card at `zsh` (D12, accepted
  for band/card agreement).
- **Working colour for Antigravity and plain cards** is not in the mock. It uses the default
  `#ffd9bd` (D9).
- **The After board's header** (the "Search tabs" pill and a single collapse button) is not
  implemented, which is a non-goal. It is a candidate follow-up spec.
- **A3 is inherited:** if the first-choice font loses its bold face, card titles return to
  synthesized bold, and for Hangul that means erosion (`docs/architecture.md` G7/A3).

## L2 Review Log
- **Round 1: NEEDS_FIX, 5 findings.** Each was checked against the document and the code
  before being accepted.
  1. **Complexity is really 4 signals (a second, atlas/cache state machine). Rejected.** The
     rubric counts signal *categories*, and the atlas/cache lifecycle (D2/D18/D20) falls
     under the same "state machine" category as card state × status. Reclassifying would
     change only the checkpoint count, and every dimension already resolved 100%.
  2. **No colour or stroke width for the new status and zone icons. Accepted:** D21.
  3. **The rename field vs the visible `≡`. Accepted:** D15 now ends the field at D14's
     button-aware budget.
  4. **`card-live-cwd` D2 reads as this spec's D2. Accepted, and it was broader:** every
     cross-spec citation now names its file. The sweep also found a **wrong citation**.
     "No per-frame allocation" had been attributed to `sidebar-row-redesign` SD4, but that
     SD4 belongs to `specs/sidebar-redesign/partition-manifest.md` and says something else
     (metadata is polled off the UI thread). The rule is `sidebar-row-redesign` D10.
  5. **Steelman: SDF/MSDF glyphs as the alternative to D2's two atlases. Accepted:** a
     rejected-alternative entry in D2.
- **Round 2: NEEDS_FIX, 2 findings, both introduced by round 1's D21.** Both were
  checked and accepted.
  1. **D21's folder icon was never subtracted from D14's folder-line budget.** D14 now
     starts the path after the icon and its gap.
  2. **D21's Waiting bars "4×16" had no unit against a 12px size.** They are now stated in
     24-unit box space, as the mock's `viewBox` is.
  - The reviewer confirmed the round-1 fixes and the finding-1 rejection, and found D20's
    compose key complete against D15 and D21.
- **Round 3 (final, scoped to round 2's fixes): PASS.** The reviewer re-derived the
  narrowest-sidebar budget independently. At 260 logical px the content width is 200 and
  the title zone is 147 (113 with the `≡`), which leaves the folder path ≥ 94px after its
  icon. That matches this spec's own check.

## Requirements

Units: "logical px" are design values. Tests run at `scale_factor = 2.0` unless a
sub-requirement names another scale, so "104 logical" is asserted as 208 physical.
`card i` is the i-th row of `TabBarModel.titles`.

### R0: The running sidebar draws the After / Anatomy cards (Confirmed Goal)

#### R0.1: A captured frame shows four distinct skinned cards
- **Given**: a headless `Renderer` with default config and a `TabBar` layer. Its stub
  `MetaSource` yields four cards: Claude (active), Antigravity (hovered), Codex, and
  Generic.
- **When**: `Renderer::capture` renders a frame.
- **Then**: each card's sampled body pixel differs from the panel background and from
  the other three cards' samples, and matches its own skin (R16.1).

#### R0.2: The workspace gates are green
- **Given**: the implementation is complete.
- **When**: `cargo build`, `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` run.
- **Then**: all three exit 0.

#### R0.3: Mutation testing leaves no unargued survivor
- **Given**: the implementation is complete.
- **When**: `cargo mutants -f <file> -j 4` runs on every file this spec changed.
- **Then**: every surviving mutant is either killed by an added test or argued equivalent
  in writing, with its survivor count recorded (CLAUDE.md), and no `exclude_re` is added.

### R1: Fixed card geometry (D1)

#### R1.1: Card height is 104 logical px regardless of font and toggles
- **Given**: any `font.size_pt` in {10, 14, 24} and any combination of
  `show_git_branch` / `show_agent_status`.
- **When**: the layer computes its `SidebarLayout`.
- **Then**: `card_h == 208.0`.

#### R1.2: Cards are inset 14 and spaced 10
- **Given**: a 340-logical-px sidebar with 3 rows and no scroll.
- **When**: `card_rect(0)` and `card_rect(1)` are read.
- **Then**:
  - each rect's `x == 28.0` and its `w == 680.0 − 56.0`;
  - `card_rect(1).y − card_rect(0).y == (104 + 10) × 2`.

#### R1.3: Hit-testing reads the published geometry
- **Given**: a published layout from R1.2.
- **When**: `card_at` is asked for a point 1 physical px below card 0's bottom edge (in
  the gap).
- **Then**:
  - it returns the same card the stride-band rule assigns (the gap belongs to its nearer
    card, `sidebar_row.rs` D7);
  - `card_rect(card_at(p))` contains or borders `p`.

### R2: Sidebar-owned atlases at design sizes (D2, D18)

#### R2.1: Card text is rasterized at 17 and 12.5 whatever the terminal font
- **Given**: the layer's two card atlases, built at `font.size_pt = 14`, then again at
  `font.size_pt = 20`.
- **When**: the title atlas resolves `'A'` bold and the meta atlas resolves `'a'`.
- **Then**:
  - the title atlas's ppem is `17 × scale` and the meta atlas's is `12.5 × scale` in both
    builds;
  - the glyph entries' pixel extents are equal between the 14pt and 20pt builds.

#### R2.2: `FrameCtx` carries the font set
- **Given**: a `Renderer` constructed with a `FontSet`.
- **When**: a layer's `record` runs.
- **Then**: `ctx.fonts` is that font set (the same chain `Atlas::new` was given).

#### R2.3: A scale-factor change rebuilds and rebinds the card atlases
- **Given**: a frame recorded at `scale_factor = 2.0`.
- **When**: the next frame records at `scale_factor = 1.0`.
- **Then**:
  - both card atlases are rebuilt at `17 × 1.0` and `12.5 × 1.0`;
  - the card text bind groups reference the new atlases' `epoch()`, and not the old ones.

#### R2.4: A font-family reload rebuilds the card atlases
- **Given**: card atlases built from font set A.
- **When**: the renderer's font set is replaced by set B (a config reload of
  `font.family`).
- **Then**: the next `record` builds both card atlases from set B.

#### R2.5: A mid-frame eviction does not ship stale UVs
- **Given**: a card atlas whose `generation()` bumps during `record`, with some runs
  already resolved against the old generation.
- **When**: `record` finishes.
- **Then**: every text instance it submits was resolved at the atlas's final generation
  (the frame's runs are rebuilt once).

#### R2.6: A card atlas that cannot be built degrades, not blanks
- **Given**: `Atlas::new` returns `None` for a card size.
- **When**: the layer records.
- **Then**:
  - that size's card text is drawn from the grid atlas at cell size;
  - one `tracing::warn!` is emitted per failed build (not per frame);
  - the title band still carries ink.

#### R2.7: Band and popup text stay on the grid atlas
- **Given**: a model with two waiting rows and an open card menu.
- **When**: the layer records.
- **Then**: the waiting-row and menu-item glyph instances are resolved from the grid
  atlas at scale 1.0 of `ctx.cell` (today's size), and not from a card atlas.

### R3: Column-based card text (D3)

#### R3.1: Wide characters advance two columns
- **Given**: the title `사이드바 카드` and a title column width `c`.
- **When**: its pen positions are laid out from `x0`.
- **Then**: the pens are `x0, x0+2c, x0+4c, x0+6c, x0+8c` (the space takes one column),
  `x0+9c, …`.

#### R3.2: Truncation counts columns and never splits a wide glyph
- **Given**: `auth 마이그레이션` and a budget of 9 columns.
- **When**: it is truncated with `…`.
- **Then**:
  - the result is at most 9 columns wide, including the `…`;
  - it ends on a whole character;
  - it is `auth 마…`.

#### R3.3: A column's width derives from its atlas's ppem
- **Given**: title ppem `P`.
- **When**: the title column width is computed.
- **Then**: it equals `max(1, round(0.6 × P))`, the rule `cell_size_px` applies to the
  grid.

### R4: Card states (D4)

#### R4.1: The default state
- **Given**: a Claude card that is neither active nor hovered.
- **When**: `build_card` runs.
- **Then**:
  - its body is a rounded box of radius 32 carrying Claude's default gradient;
  - its border is 2px `rgba(255,255,255,.10)`;
  - no glow shape is emitted.

#### R4.2: The hover state
- **Given**: the same card, hovered and not active.
- **When**: `build_card` runs.
- **Then**: the body carries Claude's hover gradient, the border is 2px
  `rgba(255,255,255,.18)`, and there is no glow.

#### R4.3: The selected state draws a glow behind the card
- **Given**: the same card, active.
- **When**: `build_card` runs.
- **Then**:
  - a glow box is emitted **before** the body: the card rect grown by 6 on every side,
    radius 38, filled with Claude's glow colour;
  - the body carries the hover gradient with a 3px border in Claude's selected tint.

#### R4.4: Selection outranks hover
- **Given**: a card that is both active and hovered.
- **When**: `build_card` runs.
- **Then**: its shapes equal the selected-only case (R4.3).

#### R4.5: A plain idle card has a body
- **Given**: a `Generic` card that is neither active nor hovered.
- **When**: `build_card` runs.
- **Then**: a body shape with the plain default gradient is emitted.

### R5: Four skins (D5)

#### R5.1: Codex maps to its own skin
- **Given**: `Agent { kind: Codex, .. }`, `Gemini`, `Aider`, and no agent.
- **When**: `skin_for` maps each.
- **Then**:
  - the results are `Codex`, `Generic`, `Generic`, `Generic`;
  - the match has no `_` arm.

#### R5.2: Skin constants equal the design table
- **Given**: each `AgentSkin`.
- **When**: its states are resolved.
- **Then**:
  - the default and hover stops equal `linear(hex, 1.0)` of the L1 table at mid `0.55`;
  - the direction is `(0.3420, 0.9397)` (±1e-4);
  - the selected tint and the glow equal the table's RGBA.

### R6: Pixel agent icons (D6)

#### R6.1: The icon follows the skin
- **Given**: cards with skins Claude, Antigravity, Codex and Generic (including a Gemini
  tab).
- **When**: `build_card` runs.
- **Then**:
  - they emit the crab (17×10), the "A" (12×10), the bubble (12×10) and the terminal
    (12×10) respectively;
  - the Gemini card emits the terminal.

#### R6.2: Cells are uniform whole pixels
- **Given**: any icon at `scale_factor` 1.0, 1.5 or 2.0.
- **When**: its cells are emitted.
- **Then**:
  - every cell's `w` and `h` equal `cell_px = max(1, round(design_cell × scale))`;
  - every cell's `x`/`y` is an integer;
  - adjacent cells share edges exactly, with no gap and no overlap.

#### R6.3: The drawn size is within half a pixel per cell of the design
- **Given**: the Claude icon.
- **When**: it is emitted at scale 2.0 and at scale 1.0.
- **Then**:
  - at 2.0 its extent is 85×50 physical (design 86×50);
  - at 1.0 it is 51×30 (design 43×25.3);
  - each is within `0.5 × cols` px of the design width.

#### R6.4: Cells carry their table colours
- **Given**: the Antigravity icon.
- **When**: it is emitted.
- **Then**: it contains 50 cells, and cell `(5,0)` has colour `linear(#ff8c00)`.

#### R6.5: Icons are placed at the top-right
- **Given**: a card rect.
- **When**: its icon is emitted.
- **Then**:
  - the icon's right edge is the card's inner right edge (16 logical of padding in);
  - its top is the inner top plus 4 logical (Claude) or 2 (the others).

### R7: Status derivation (D7, D12, D19)

#### R7.1: An agent's state wins
- **Given**: agent `Busy`, `Unknown`, `Idle` and `Waiting`, each combined with
  `running = true` and a failed `last_end`.
- **When**: the app resolves `CardStatus`.
- **Then**: the results are `Working`, `Working`, `Ready` and `WaitingForInput`.

#### R7.2: Running outranks a remembered failure
- **Given**: no agent, `running = true`, and `last_end.exit_code = 101`.
- **When**: the status is resolved.
- **Then**: it is `Running`.

#### R7.3: A failed last command shows its code
- **Given**: no agent, `running = false`, and `last_end.exit_code = 101`.
- **When**: the status is resolved.
- **Then**: it is `Exited(101)`.

#### R7.4: Ignored codes and no history read `zsh`
- **Given**: no agent, not running, and `last_end` with exit 0, 1 or 130, or no
  `last_end` at all.
- **When**: the status is resolved.
- **Then**: it is `Shell` (drawn `zsh`).

#### R7.5: The band and the card share one exit rule
- **Given**: exit codes `{-1, 0, 1, 2, 101, 130}`.
- **When**: `waiting::is_reportable_exit` is evaluated, and the band's
  `on_command_end` receives each code.
- **Then**:
  - the function returns `true` exactly for `{-1, 2, 101}`;
  - the band raises an Error for exactly those codes.

#### R7.6: renderer: each status draws its label, weight, colour and icon
- **Given**: a non-Codex card, with each `CardStatus` in turn.
- **When**: `build_card` runs.
- **Then**: the bottom-left zone emits exactly:

  | Status | Text | Weight | Colour | Icon |
  |---|---|---|---|---|
  | `Working` | `Working` | regular | `status_working_fg` | asterisk |
  | `Ready` | `Ready` | regular | `status_ready_fg` | diamond |
  | `WaitingForInput` | `Waiting for input` | **bold** | `status_waiting_fg` | filled pause |
  | `Exited(101)` | `Exited 101` | **bold** | `status_error_fg` | X |
  | `Exited(-1)` | `Exited -1` | **bold** | `status_error_fg` | X |
  | `Running` | `Running` | regular | `card_meta_fg` | `>_` prompt |
  | `Shell` | `zsh` | regular | `card_meta_fg` | `>_` prompt |

### R8: The command-start marker, end to end (D8)

#### R8.1: term-core: `C` sets running and `D` clears it
- **Given**: a `Terminal`.
- **When**: it advances `ESC]133;C BEL`, then `ESC]133;D;0;5 BEL`.
- **Then**:
  - `command_running()` is `true` after the first and `false` after the second;
  - `take_command_end()` still yields `{0, 5}` exactly once.

#### R8.2: term-core: only the final state within one chunk counts
- **Given**: a `Terminal`.
- **When**: it advances one buffer `C…D`, and separately one buffer `D…C`.
- **Then**: `command_running()` is `false` after the first and `true` after the second.

#### R8.3: term-core: a malformed `D` leaves the state untouched
- **Given**: `command_running() == true`.
- **When**: `ESC]133;D;notanumber;5 BEL` is advanced.
- **Then**: `command_running()` is still `true`, and no command end is recorded (the
  parser's existing drop-whole-event contract).

#### R8.4: app: the session keeps a non-consuming last end
- **Given**: a `Session` whose PTY emits `133;D;101;40` and then nothing.
- **When**: `drain` runs, and then the waiting path calls `take_command_end`.
- **Then**: `Session::last_end()` still returns `{101, 40}`, and `command_running()`
  mirrors the terminal's flag.

#### R8.5: app: a change reaches the sidebar row
- **Given**: a background tab whose `running` flips `false → true`, or whose `last_end`
  changes.
- **When**: its output is drained.
- **Then**:
  - `sync_tab_model` runs (the same path a program-title change takes);
  - that tab's `SidebarRow` carries the new `running` / `last_exit`;
  - the resolved `CardStatus` changes without a tab switch.

### R9: Colours and role tokens (D9, D21)

#### R9.1: The new role tokens hold the mock's values
- **Given**: `SidebarTheme::DEFAULT`.
- **When**: the new tokens are read.
- **Then**:
  - `card_title_fg == #f4efe6`, and `card_meta_fg` is `#f4efe6` at alpha .78;
  - `status_working_fg == #ffd9bd`, `status_working_codex_fg == #c9ecdf`;
  - `status_ready_fg == #b9cdf0`, `status_waiting_fg == #f2c186`,
    `status_error_fg == #ff9a90`.

#### R9.2: Existing tokens are unchanged
- **Given**: `SidebarTheme::DEFAULT`.
- **When**: the pre-existing tokens are read.
- **Then**: every one equals its value before this spec (`theme_default_carries_mock_values`
  passes unmodified).

#### R9.3: Working is tinted per skin
- **Given**: a Working status on a Codex card and on Claude, Antigravity and Generic
  cards.
- **When**: `build_card` runs.
- **Then**: the Codex status text and icon are `#c9ecdf`, and all others are `#ffd9bd`.

#### R9.4: Every icon matches its text
- **Given**: any card.
- **When**: `build_card` runs.
- **Then**: each status, folder and branch icon's fill colour equals the colour of the
  text run it precedes.

### R10: The folder line (D10)

#### R10.1: Home is abbreviated
- **Given**: `HOME=/Users/x`.
- **When**: `cwd_display` is derived for each of the following cwds.
- **Then**:
  - `/Users/x/IdeaProjects/terminal` → `~/IdeaProjects/terminal`;
  - `/Users/x` → `~`;
  - `/Users/xy/p` → `/Users/xy/p` (a component-wise prefix, not a string prefix).

#### R10.2: No home, or outside home, stays absolute
- **Given**: `HOME` unset, and separately the cwd `/opt/homebrew`.
- **When**: `cwd_display` is derived.
- **Then**: it is the absolute path.

#### R10.3: A long path keeps its tail
- **Given**: `~/IdeaProjects/terminal-feat-improve-card-ui` and a 20-column budget.
- **When**: the folder line is laid out.
- **Then**: it is `…` followed by the last 19 columns of the path, and it is at most 20
  columns wide.

#### R10.4: The fallback before the first fill
- **Given**: a row with `RowMeta::default()` (no metadata yet) and title `terminal`.
- **When**: the card is built.
- **Then**: the folder line reads `terminal`.

#### R10.5: Line 1 no longer drives line 2
- **Given**: the tree after this spec.
- **When**: `RowMeta` is inspected.
- **Then**:
  - it has no `line1_is_custom` field;
  - the card always draws `line1` as the title and `cwd_display` as the folder line;
  - `resolve_line1_tier`'s signature is unchanged.

### R11: The hover-only `≡` (D11)

#### R11.1: No button at rest
- **Given**: a card that is not hovered, has no open menu and is not being edited.
- **When**: `build_card` runs.
- **Then**: no backplate and no hamburger shapes are emitted for it.

#### R11.2: Hover shows it beside the icon
- **Given**: a hovered card.
- **When**: `build_card` runs.
- **Then**:
  - a 52×52 physical backplate (radius 16, `rgba(255,255,255,.16)`) is emitted;
  - its right edge is 16 physical left of the 85-physical icon **slot** — the crab as
    drawn at 2× (D11's implementation note) — and its top is 6 physical below the
    padding box's top;
  - the hamburger glyph is inside it.

#### R11.3: An open menu or a rename pins the button
- **Given**: card 2 with an open menu, or card 2 being renamed, and the pointer over
  card 0.
- **When**: the layer records.
- **Then**: card 2 draws the button.

#### R11.4: The drawn button is the hit-tested button
- **Given**: any card rect.
- **When**: the row layer's drawn backplate rect and `sidebar_layout::menu_button_rect`
  (the hit-test's route) are compared.
- **Then**: they are equal.

#### R11.5: The gap to the crab holds at every scale
- **Given**: a hovered Claude card at 1×, 1.25×, 1.5×, 2×, 2.5× and 3×.
- **When**: `build_card` runs.
- **Then**:
  - the crab's left edge is `8 × scale` physical px (±0.5, its snapped origin) right of
    the backplate's right edge;
  - the backplate is drawn at `sidebar_card::menu_button`'s rect, the hit-tested one.
- Added during verification with D11's amendment (2026-09-27).

### R12: Toggles (D13)

#### R12.1: Branch hidden
- **Given**: `show_git_branch = false` and a card with branch `main`.
- **When**: `build_card` runs.
- **Then**: no branch icon or branch text is emitted, and the height is still 104.

#### R12.2: Status hidden, identity kept
- **Given**: `show_agent_status = false` and a Working Claude card.
- **When**: `build_card` runs.
- **Then**: no status icon or text is emitted, and the crab icon and Claude skin are.

### R13: Zone budgets (D14)

#### R13.1: The title stops before the icon, or before the `≡`
- **Given**: a long title on a card that is not hovered, then the same card hovered.
- **When**: `build_card` runs.
- **Then**: the title's right edge is ≤ the icon's left edge − 20 physical, then ≤ the
  backplate's left edge − 20.

#### R13.2: The folder text starts after its icon
- **Given**: any card.
- **When**: `build_card` runs.
- **Then**:
  - the folder text's first pen is at inner-left + 38 physical;
  - its budget is the title budget − 38.

#### R13.3: Status first, branch second
- **Given**: the status `Waiting for input` and the branch `claude/eloquent-knuth` on a
  card at the minimum sidebar width (260 logical).
- **When**: `build_card` runs.
- **Then**:
  - the status text is drawn in full;
  - the branch is right-aligned in the remainder and ends with `…`, or is omitted entirely
    when its icon cannot fit.

#### R13.4: The narrowest sidebar keeps every zone legible
- **Given**: a 260-logical-px sidebar, a hovered Claude card and the path `~/a/b/c`.
- **When**: `build_card` runs.
- **Then**: the title budget is ≥ 111 logical, and the folder text budget is ≥ 92 logical.
- **Corrected during T4:** the L2 figures (113 / 94, which the round-3 reviewer also
  derived) took the content width as 200. It is 198: 232 − 2×1 (border) − 2×16
  (padding). Both reviewers computed without the 1px border.

### R14: Rename at title size (D15)

#### R14.1: The field uses the title atlas
- **Given**: card 1 being renamed, with text `사이드바` and the caret at the end.
- **When**: the layer records.
- **Then**:
  - the field's glyphs come from the title atlas, in bold;
  - the caret's x equals the title pen after 8 title columns.

#### R14.2: The field stops before the `≡`
- **Given**: the same edit.
- **When**: the layer records.
- **Then**: the field box's right edge is ≤ the backplate's left edge − 20 physical.

#### R14.3: The folder line stays
- **Given**: the same edit.
- **When**: the layer records.
- **Then**: card 1's folder text is still emitted.

### R15: A pure card builder (D16)

#### R15.1: `build_card` needs no GPU and is deterministic
- **Given**: a `CardInput` value constructed in a unit test, with no device or queue.
- **When**: `sidebar_card::build_card` runs twice on it.
- **Then**: it returns identical `(shapes, text runs)` both times.

### R16: Pixel verification (D17)

#### R16.1: Body colours lie on their skin's gradient
- **Given**: R0.1's frame.
- **When**: each card's body is sampled at its centre.
- **Then**: the sample is within ±6/255 per channel of the skin gradient evaluated at that
  point.

#### R16.2: Only the selected card glows
- **Given**: R0.1's frame.
- **When**: the pixel 2 physical px outside each card's right edge, at mid-height, is
  sampled.
- **Then**: it differs from the panel background only for the selected card.

#### R16.3: Only the hovered card shows its backplate
- **Given**: R0.1's frame.
- **When**: each card's button-rect centre is sampled.
- **Then**: only the hovered card's sample differs from its own body gradient there.

#### R16.4: Titles reach the screen
- **Given**: R0.1's frame.
- **When**: each card's title band is scanned.
- **Then**: it contains pixels that differ from the body sample.

#### R16.5: Icons reach the screen
- **Given**: R0.1's frame.
- **When**: the centre of the crab's cell `(0,4)` on the Claude card is sampled.
- **Then**: it is within ±6/255 of `#e6714e`.

#### R16.6: Skip vs fail
- **Given**: no GPU adapter, or no system font.
- **When**: the sidebar golden test runs.
- **Then**: it skips through the shared `testing::headless` `None`. With both present, any
  mismatch fails the test.

### R17: Compose only on change (D20)

#### R17.1: A steady frame composes nothing
- **Given**: a recorded frame.
- **When**: `record` runs again with identical inputs.
- **Then**: `compose_count` is unchanged, and no instance buffer is written.

#### R17.2: Every key input invalidates
- **Given**: a recorded frame.
- **When**: exactly one of the following changes before the next `record`:
  - `hovered`, `active` or the meta `generation`;
  - `scroll_px` or `sidebar_w`;
  - the open menu or the edit caret;
  - the waiting rows or `scale_factor`;
  - the grid atlas's `generation`, or either card atlas's `generation`.
- **Then**: `compose_count` increments by exactly 1.

#### R17.3: A cached frame looks the same
- **Given**: two consecutive captures with identical inputs.
- **When**: they are compared.
- **Then**: their pixels are identical, so the second (cached) frame is not blank.

### R18: Status and zone icons (D21)

#### R18.1: Sizes and strokes follow the mock
- **Given**: each new card icon.
- **When**: it is emitted at size `S` with 24-unit stroke `w`.
- **Then**:
  - its stroke shapes have a thickness of `w × S / 24`;
  - the sizes and stroke widths are Working 13 / 2.2, Ready 12 / 2.2, Exited 12 / 3,
    prompt 13 / 2, folder 13 / 2, branch 12 / 2.

#### R18.2: The Waiting icon is two filled bars
- **Given**: the card Waiting icon at size 12.
- **When**: it is emitted.
- **Then**:
  - it is exactly two filled rounded boxes;
  - each is 2×8 logical at x = 3 and x = 7 (in its box), y = 2, with radius 0.5 logical.

#### R18.3: The band's icons are untouched
- **Given**: `Icon::Pause`, `Icon::Check` and `Icon::Cross` (the band's).
- **When**: they are emitted.
- **Then**: their shapes equal those before this spec, and `sidebar_icons`' existing tests
  pass unmodified.

### Traceability (decision → requirements)

| Decision | Requirements |
|---|---|
| D1 geometry | R1 |
| D2 card atlases | R2.1–R2.4, R2.6, R2.7 |
| D3 columns | R3, R14.1 |
| D4 states | R4, R16.2 |
| D5 skins | R5, R16.1 |
| D6 pixel icons | R6, R16.5 |
| D7 precedence | R7.1–R7.4, R7.6 |
| D8 `C` marker | R8 (term-core R8.1–3 → session R8.4 → row R8.5) |
| D9 colours | R9.1–R9.3 |
| D10 folder line | R10, R13.2 |
| D11 hover `≡` | R11, R16.3 |
| D12 exit rule | R7.4, R7.5 |
| D13 toggles | R12 |
| D14 budgets | R13 |
| D15 rename | R14 |
| D16 pure builder | R15 |
| D17 verification | R0, R16 |
| D18 atlas guards | R2.3, R2.5 |
| D19 long runs | R7.2, R8.1 (`Running` holds until `D`) |
| D20 compose cache | R17 |
| D21 icons | R9.4, R18 |

## Tasks

**Gate for every task:**
- `cargo build`, `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` are green at the end of every
  task (R0.2).
- `cargo mutants -f <file> -j 4` runs on each file the task changed, and each survivor
  is killed or argued equivalent, with its count recorded (R0.3).
- **Hooks:** `rustfmt` runs through the Edit/Write hook only, so a shell-applied `.rs` edit
  must be followed by `cargo fmt`.

**File-overlap map, which drives the `Depends on` edges:**
- **T1:** term-core only.
- **T2 then T3:** both touch `sidebar_row.rs`. T2 adds the Codex arm in `resolve_card`;
  T3 edits the test `FrameCtx` literal at `:2177`.
- **T3 then T4:** both touch the renderer's `lib.rs` (`pub mod` lines).
- **T6 ∥ T7 ∥ T8 after T5:** these three have disjoint file sets.

### T1: term-core records the OSC 133 command-start marker [infra]
- **Fulfills**: R8.1, R8.2, R8.3
- **Depends on**: (none). Can run in parallel with T2 through T6.
- **Scope:**
  - `crates/term-core/src/parser.rs`: `133;C` sets the flag and a well-formed `133;D`
    clears it, in byte order.
  - `crates/term-core/src/grid.rs`: the flag's storage.
  - `crates/term-core/src/lib.rs`: the `Terminal::command_running()` accessor.
  - `take_command_end` is unchanged.

### T2: Sidebar design data (skins, role tokens, pixel and card icons) [infra]
- **Fulfills**: R5.2, R6.2, R6.3, R6.4, R9.1, R9.2, R18.1, R18.2, R18.3
- **Depends on**: (none). Can run in parallel with T1.
- **Scope:**
  - `sidebar_theme.rs`:
    - `AgentSkin::Codex` and a card-skin struct holding default and hover gradients, the
      selected tint and the glow for all four skins (D5);
    - the D9 role tokens;
    - the D1 metrics: radius 16, padding 12/16, inset 14, gap 10, card height 104, title
      17, meta 12.5.
  - `sidebar_icons.rs`:
    - per-icon stroke width;
    - a filled-rect primitive;
    - the D21 card icons (asterisk, diamond, filled pause, X, prompt, folder, branch);
    - the D6 pixel-icon tables with uniform-cell emission.
  - `sidebar_row.rs`: **only** the `resolve_card` arm that keeps the exhaustive match
    compiling.
  - The band's `Pause`, `Check` and `Cross` are untouched.

### T3: Sidebar-owned card atlases [infra]
- **Fulfills**: R2.1, R2.2, R2.3, R2.4, R2.5, R2.6, R3.3
- **Depends on**: T2 (file overlap: `sidebar_row.rs`)
- **Scope:**
  - `crates/renderer/src/lib.rs`:
    - `FrameCtx.fonts: &FontSet`, threaded through `draw_to`;
    - the 14 `FrameCtx` literals, in `lib.rs`, `monitor_panel.rs`, `preedit_layer.rs` and
      the `sidebar_row.rs` test at `:2177`;
    - `pub mod sidebar_text`.
  - **New** `crates/renderer/src/sidebar_text.rs`, holding `CardAtlases`:
    - builds the 17 and 12.5 atlases through `Atlas::new`;
    - rebuilds on a ppem or font-set change;
    - epoch-keyed bind groups (D18);
    - mid-frame generation-bump detection;
    - the grid-atlas fallback with a warn-once;
    - the column width `max(1, round(0.6 × ppem))`.
  - It is not yet wired into `TabBar`.

### T4: Pure card builder [infra]
- **Fulfills**: R3.1, R3.2, R4.1, R4.2, R4.3, R4.4, R4.5, R6.1, R6.5, R7.6, R9.3, R9.4,
  R10.3, R10.4, R11.1, R11.2, R12.1, R12.2, R13.1, R13.2, R13.3, R13.4, R15.1
- **Depends on**: T2 (tokens and icons), T3 (file overlap: `lib.rs`)
- **Scope:**
  - **New** `crates/renderer/src/sidebar_card.rs`, with `pub mod` in `lib.rs`:
    - the `CardStatus` enum;
    - `CardInput`, a plain value built from rect, skin, state, status, title, folder
      display, branch, the toggles, scale and the two column widths;
    - `build_card(&CardInput) -> (Vec<SidebarShape>, Vec<TextRun>)`, whose text runs are
      tagged `Title` / `Meta` for atlas selection.
  - It covers the glow and body, the icons, the status zone, the folder line with left
    truncation and its icon offset, the zone budgets, and the hover `≡`.
  - Column layout reuses `tab_bar::char_cols` / `pen_layout`; no private copy.
  - Headless unit tests only.

### T5: The new card on screen, with its app-side data [vertical]
- **Fulfills**: R1.1, R1.2, R1.3, R2.7, R5.1, R7.1, R7.2, R7.3, R7.4, R7.5, R10.1,
  R10.2, R10.5, R11.3, R11.4, R14.1, R14.2, R14.3
- **Depends on**: T4 (and, transitively, T2 and T3)
- **Scope, renderer:**
  - `sidebar_row.rs`:
    - the `RowMeta` contract: `status: CardStatus` and `cwd_display: String` are added;
      `mark` and `line1_is_custom` are removed;
    - `SidebarRow` gains `running: bool` and `last_exit: Option<i32>`;
    - `layout_with` gives a fixed 104 with the new insets;
    - `TabBar::record` composes each card through `build_card` and draws card runs through
      `CardAtlases`, while the band and the popup stay on the grid atlas;
    - the `≡` stays pinned by an open menu or a rename;
    - rename is drawn at title size.
  - `sidebar_layout.rs`: `menu_button_in` moves to D11's position (26 square, left of the
    icon), still shared with the hit-test.
- **Scope, app:**
  - `sidebar_model.rs`:
    - `skin_for(Codex) → Codex`;
    - `resolve_status(agent, running, last_end)`, per D7's precedence;
    - `cwd_display`, component-wise `~` abbreviation;
    - `RowMark` / `mark_for` / `line1_is_custom` removed.
  - `waiting.rs`: `pub(crate) fn is_reportable_exit`, now also used by the band.
  - `tabs.rs`: populates the new `SidebarRow` fields with `false` / `None` until T7.
- `Running` / `Exited` stay unreachable in the live app until T7, but are unit-tested
  here.

### T6: Compose only on change [infra]
- **Fulfills**: R17.1, R17.2
- **Depends on**: T5. Can run in parallel with T7 (disjoint files).
- **Scope:**
  - `sidebar_row.rs`: the D20 compose key, including all three atlases'
    `(epoch, generation)`.
  - A cache hit re-submits the resident buffers without composing or writing them.
  - A `compose_count` counter for tests.
  - `sidebar_shapes.rs` and the text pipeline gain a "draw what is resident" path if one
    is missing.

### T7: Running and last-exit reach the card, end to end [vertical]
- **Fulfills**: R8.4, R8.5
- **Depends on**: T1, T5. Can run in parallel with T6 and T8 (disjoint files).
- **Scope:**
  - `session.rs`: a non-consuming `last_end`, and `command_running()` mirrored from the
    terminal on `drain`.
  - `tabs.rs`: `titles()` fills `SidebarRow.running` / `last_exit` from the session.
  - `terminal_app.rs`: a change in either value triggers `sync_tab_model`, beside the
    existing `title_changed` resync (`:2241`).

### T8: The sidebar golden frame [verification]
- **Fulfills**: R0.1, R16.1, R16.2, R16.3, R16.4, R16.5, R16.6, R17.3
- **Depends on**: T5, T6. Can run in parallel with T7.
- **Scope:**
  - `crates/renderer/tests/golden_frame.rs`: a `TabBar` layer with a stub `MetaSource`
    (four skins, one active, one hovered), captured through `Renderer::capture`.
  - Pixel assertions for R16.1–R16.5, and a second capture compared for R17.3.
  - It skips through `testing::headless`.
  - **Falsify before trusting:** break the skin fill, drop the glow, and always-draw the
    `≡`, and confirm each assertion goes red; then revert.

## External Dependencies

### Pre-work
- (none required).
- **Optional:** decide what happens to `Sidebar Weather Cards.html` (10 MB, untracked, at
  the repo root). It is this spec's design source. Committing it as-is adds about 10 MB,
  99% of which is embedded fonts; a font-stripped extract (about 48 KB) or leaving it
  untracked are the alternatives.

### Post-work
- **Eyeball the result with `/run-term`** against the After and Anatomy boards:
  - a Claude tab, a Codex tab, and a plain tab where `false; (exit 101)` shows
    `Exited 101`;
  - a long `sleep 30` showing `Running`;
  - a tmux tab (D19);
  - a Korean tab title (D3);
  - hover (the `≡` appears) and a rename;
  - at 2× (Retina) and, if one is available, a 1× external display (D6's cell sizes).
