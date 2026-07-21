# Spec: sidebar-chrome-collapse

## Meta
- **Created**: 2026-07-20
- **Type**: dev
- **Status**: ready
- **Approved by**: batch mode — the human approved the partition (features, declared surfaces, `depends_on` edges, SD1–SD8) at decompose's single gate; specify's L0/L2/L3/L4 prompts are bypassed per `pre-approved-batch: yes`
- **Approved at**: 2026-07-20
- **Execution**: NOT handed off. Batch mode's deliverable is this file; `decompose` → `build-order` owns execution.
- **Set**: sidebar-redesign (`specs/sidebar-redesign/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single human gate)
- **Sibling reconciliation timestamp**: the two sibling specs were **read at three different points and changed under this spec twice** — they are authored concurrently in the same batch. `specs/sidebar-row-redesign/spec.md` and `specs/sidebar-interaction/spec.md` were absent at L1, present at review round 1, and `sidebar-interaction` had been **rewritten to consume this spec's own round-1 API** by review round 2. This version reconciles against the files **as of round 3**. Where this spec and a sibling disagree, the disagreement is named rather than resolved unilaterally (Known Gaps); `coherence-audit` is the arbiter.
- **L2 review**: 3 rounds (initial + 2 retries — the skill's maximum). Round 1 returned 7 blocking items, the largest being that both sibling specs had appeared on disk. Round 2 returned 5 blocking items, the largest being that round 1's fix had been written against a version of `sidebar-interaction` that a concurrent edit had already replaced — the rewrite had *diverged from* the dependent instead of converging on it. This round restores the API the dependent actually froze and applies every remaining item. **The round-3 fixes were applied without a further review pass, the retry budget being spent**; they are transcriptions of the reviewer's own prescribed remedies against files re-read at the time of writing, but they are unverified. `coherence-audit` is the next independent check.

## Declared Surface
- feature-id: sidebar-chrome-collapse
- depends_on: [sidebar-foundations, sidebar-row-redesign]
- declared-surface:
  - crates/renderer/src/sidebar_header.rs
  - crates/renderer/src/sidebar_layout.rs
  - crates/renderer/src/lib.rs

## Goal
Give the redesigned sidebar the chrome that surrounds its card list: a pinned header row carrying the new-tab and hide-sidebar buttons, a collapsed state that draws only a floating restore button over the terminal while the reclaimed width returns to the grid, and an overflow model so a list longer than the window scrolls and clips instead of running off the bottom into invisibility. All of the geometry is expressed as pure functions in one module, so what this feature draws and what `sidebar-interaction` hit-tests are provably the same arithmetic.

## Non-goals
- **The search / filter text input.** Deferred by SD8: a focus-stealing text field must win keystrokes away from PTY routing, which needs its own focus model. This feature reserves the region it will occupy and draws nothing in it (D1).
- **Any event handling or app-side wiring.** No winit, no click routing, no wheel handling, no `set_left_inset` call, no config mutation, no layer registration. All of it is `sidebar-interaction`'s; this feature supplies the geometry it calls and the pixels it toggles. The boundary is enumerated in R7.
- **The card's interior and the card's hover visual.** Mark glyph, title, branch line, agent skin, card background states — `sidebar-row-redesign`.
- **The panel background and right border.** `sidebar-row-redesign` draws both (its R1.8, `:519-521`: a `panel_bg` `#1e1b18` rect over `[0, 0, w, vh]` and a `panel_border` `#2e2a24` rect at the right edge).
- **The sidebar's width formula and the card's height formula.** `sidebar_row::sidebar_width_px(cfg, scale)` and `SidebarLayout.card_h` are `sidebar-row-redesign`'s (its D4, `:283-301`; its D6). This feature never computes either; `row_h` and `stride` are **inputs** it is handed.
- **Per-tab data** — `tab-metadata`.
- **Text rendering of any kind.** This feature draws shapes only; it never touches `ctx.atlas` (D12).
- **A keyboard shortcut for collapse**, and **animation** of any kind. Collapse and scroll are instantaneous.

## Confirmed Goal
Add `crates/renderer/src/sidebar_layout.rs` — a GPU-free, state-free module of pure free functions over a plain `SidebarLayoutInput` (all physical px) that names every sidebar region (header band, the three header buttons, the reserved-and-empty search slot, the list viewport, each card's slot, the collapsed restore button), owns the overflow model (`content_height`, `max_scroll`, `clamp_scroll`, `scroll_to_reveal`, `visible_rows`) and owns the ordered `hit_test` that resolves a physical point to a `SidebarRegion` — chrome first, cards last, never falling through the header. Add `crates/renderer/src/sidebar_header.rs` — one `DrawLayer` at `SIDEBAR_CHROME_Z = 30` with a device-free `build_shapes` core and two mutually exclusive modes selected by `ctx.left_inset`: expanded, drawing an opaque header band with the filter icon and the new-tab and hide buttons; collapsed, drawing only the floating restore button over the terminal. Add exactly two `pub mod` lines to `lib.rs`. Done when the header renders at the mock's metrics, the hide and restore buttons resolve through the same `chrome_hit` this layer draws its hover with, a card scrolled under the header is neither visible nor clickable, the overflow model is total (no NaN, no dead zone, no unreachable tab), and `cargo build` + `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` are green.

## Research

### The sibling specs — read at round 3, and they reshaped this spec twice
Every claim here was read from the files. They are authored concurrently, so each citation is a point-in-time read (see Meta).

**From `specs/sidebar-row-redesign/spec.md` (a `depends_on`; lands *before* this feature):**
- `crates/renderer/src/tab_bar.rs` is **not deleted** — rewritten as a thin façade (its D1), and **`TAB_BAR_Z: i32 = 10` is retained unchanged** (`:244`). A z-order assertion against it still compiles.
- Its layer's `record` **keeps the `ctx.left_inset <= 0.0` early return** (its D14, `:418`): *"`record` returns immediately when `ctx.left_inset <= 0.0` — which is exactly the `collapsed = true` case now that D4 makes the inset config-derived"*. Verified, not assumed; it is what makes D3 work.
- Its D7 (`:326`) publishes, in `crates/renderer/src/sidebar_row.rs` (struct at `:330-345`):
  `SidebarGeom { sidebar_w, viewport_h, cell, scale, show_branch, list_top, scroll_px }` — with `list_top` and `scroll_px` documented `0.0` there and reserved for **this** feature — and `SidebarLayout { geom, count, stride, card_h, … }` with `card_rect(i)`, `card_at(x, y)` and `visible_range()`. `card_at` inverts over the **stride band, not the drawn rect** (`:345`), explicitly so the mock's 3px card margins are not dead gutters.
- Its D13 (`:411`, quote at `:413`): *"`sidebar-chrome-collapse` fills two fields and adds clipping, changing nothing this feature defines"*; its Non-goals (`:26-27`) reserve `list_top` *"so that feature needs no signature change"*.
- Its D3 (`:262-281`) makes `SidebarModel` a **per-UI-thread singleton** — a `thread_local!` `Rc<RefCell<SidebarModel>>` in `sidebar_row.rs`, reached by `TabModelHandle::shared()`; `TabBar::new()` and `default()` return clones of the same `Rc`. Fields: `rows`, `active`, `meta`, `hovered: Option<usize>` (*"written by `sidebar-interaction`"*), `layout: Option<SidebarLayout>` (*"written by the layer **after drawing**"*), plus a comment reserving `scroll_px`, `list_top`, `collapsed` for this feature. **Borrow discipline is a hard rule**, not a style note.
- Its D4 (`:283-301`): `Renderer::set_left_inset(_requested_px)` **ignores its argument** and calls `sync_left_inset()` → `sidebar_width_px(&self.sidebar, self.scale_factor)`, `0.0` when `cfg.collapsed`. Called from `assemble` and at the end of `apply_config`.
- Its Post-work (`:889`) advises this feature to *"register its header layer at `z_order = 11`"*. **This spec chooses 30 instead**, for a reason that note did not consider (D4).

**From `specs/sidebar-interaction/spec.md` (the dependent; lands *after* this feature) — it now consumes this feature's round-1 API, so these are commitments, not assumptions:**
- Its Non-goals (`:27`): *"`sidebar-chrome-collapse` D2 chose reading B … the filter gets **no hover state and no hit region at all**, and `ChromeRegion` has no `Filter` variant."* Its D1 (`:266`) deleted the `Filter → Inert` row it once had, *"because a match arm for a variant that does not exist would not compile."*
- Its D1 (`:250-252`) publishes app-side wrappers taking `&SidebarLayoutInput`, and (`:267`) fixes: *"`collapsed` is an **argument to `hit_test`**, never a gate on whether to call it"* — because collapsed, the restore button floats over the terminal and an `x < sidebar_w` guard would make it permanently unclickable.
- Its D3 (`:278-297`) found a **real defect in this feature's round-1 API**: `SidebarLayoutInput::from_frame(ctx, …)` needs a `FrameCtx`, which holds a `&Device`, `&Queue`, `&mut CommandEncoder`, `&TextureView` and `&mut Atlas` — *"The app has none of these and must not acquire them … the published contract, as written, is unreachable from the only feature it names."* It closes the gap by assembling the struct literally, arithmetic-free, reading `card_h` from row-redesign's published `SidebarLayout` (`:282`). **D7 here adopts that as the documented app path.**
- Its D3 (`:295`) and Known Gaps (`:427`) flag as **BLOCKING cross-spec**: *"two parallel layout type families"* — `SidebarLayoutInput` + free functions here, `SidebarGeom`/`SidebarLayout` + methods there, over overlapping geometry. Named in Known Gaps below.
- Its D4 (`:302`) registers `SidebarChromeLayer` and captures its handle: *"the whole set's collapse feature is dark until this feature ships"*.
- Its D5 (`:306-320`) keeps live `collapsed` in a `Running`-owned `Config`, never written to disk, applied edge-triggered on reload — and rejects deriving it from `left_inset` because *"under row-redesign D4 the inset is **derived from** `collapsed`, so this would be a cycle"*. **D3 answers the cycle argument directly.**
- Its D6 (`:322-333`) **resolves** what an earlier round of this spec recorded as an unowned contradiction: collapse mutates the app's live `Config.sidebar.collapsed` and runs `settings::apply` → `apply_config` → `sync_left_inset` → `grid_size()` → `resize_all` → `clamp_scroll`. Since row-redesign D4 makes `set_left_inset`'s argument inert, this is the branch that works under both.
- Its D8 (`:365-369`): chrome hover pushes the **raw cursor** into `SidebarChromeHandle { cursor: Option<[f32;2]> }` and lets *this* layer resolve it via `chrome_hit`, *"explicitly so that the highlight and the click [cannot be] two independent resolutions that could disagree"*; the card index is resolved and debounced separately into row-redesign's `hovered`. The raw cursor is deliberately **not** debounced.
- Its D10 (`:388-397`) owns the scroll `f32`, both wheel unit systems, re-clamping on tab close and resize, and calls `scroll_to_reveal` from `do_tab_action` — *"the one place it cannot be forgotten"*.

### The dependency this feature consumes (`specs/sidebar-foundations/spec.md`, verified)
- `SidebarShapePipeline::new(device, format)` / `ensure(&mut Option<Self>, ctx) -> &mut Self` (lazy, rebuilds on format change) / `draw(&mut self, ctx, shapes: &[SidebarShape])`. `draw` **returns immediately on an empty slice** and uploads through `crate::upload_resident` — a grow-only resident buffer — never `create_buffer_init` per frame (its D3).
- `SidebarShape` is constructor-only (`rounded_box`, `capsule`, `ring`); constructors clamp radius and border to `min(w,h)/2` and return `None` for a non-finite or non-positive extent (its D9). Struct-literal construction does not compile.
- `SidebarTheme` (~15 colours, packed `u32`) and `SidebarMetrics` (~15 metrics, **logical px**) with `SidebarMetrics::scaled(scale)` as the only path to physical px (its D6). `pub fn linear(packed: u32, alpha: f32) -> [f32; 4]` lives in `sidebar_theme.rs`.
- `Icon` + `pub fn push_icon(out: &mut Vec<SidebarShape>, icon: Icon, x, y, size, color)` — a **sink**, not a returning `Vec` (its D7). `Icon::{Filter, Plus, Panel, GitBranch, Asterisk, Diamond}`; there is **no `Icon::Search`**.
- `FrameCtx` gained `pub sidebar: &'a SidebarConfig` and `pub scale_factor: f64`; `Renderer` gained `pub fn scale_factor()`; `headless()` was hoisted to a crate-root `#[cfg(test)] pub(crate) fn` (its D10).
- `SidebarMetrics` carries `header_pad` (14, 12, 12), `header_gap` 8, `button_pad` 6, `button_radius` 8, `card_margin_x` 8, `card_margin_y` 3, `card_radius` 13, `border_w` 1, `icon_stroke_w` 1.7, `mark_px` 13, `title_px` 15, `meta_px` 12.5. It has **no render size for the header-button icons** — `mark_px` is the `✳`/`◇` marks' size (its D8), not a button icon size. `SidebarTheme` carries neither `#2c2822` nor `#3b362e` (D15).
- `width_px` is clamped to **260–440 at load** and is never observable unclamped on any production path (its D2). D3 and D1 depend on this.

### The renderer seam (verified in this repo)
- `crates/renderer/src/lib.rs:113-131` — `FrameCtx { device, queue, encoder, view, viewport, cell, top_inset, left_inset, format, snapshot, atlas }`; `trait DrawLayer { z_order, record }` at `:137-142`; `Renderer::add_layer` at `:453` (doc at `:451-452`: *"no edit to this file is required to add a layer"*).
- Z-order constants, grep-verified: `cursor_layer.rs:18` `CURSOR_Z = 5`; `tab_bar.rs:71` `TAB_BAR_Z = 10`; `image_layer.rs:17` `IMAGE_Z = 20`; `overlay_layer.rs:42` `OVERLAY_Z = 100`. `lib.rs:717` sorts by `z_order` (a **stable** sort). `image_layer.rs:390-393` is the precedent for asserting betweenness.
- `crates/renderer/src/lib.rs:508` `set_left_inset` / `:514` `left_inset()`; `:1021-1033` `grid_dims` subtracts `left_inset` and floors cols at 1 (`:1029`).
- `crates/renderer/src/lib.rs:1010-1013` — `cell_size_px = (round(px·0.6), round(px·1.2))` where `px = size_pt · scale`; `crates/common/src/config.rs:55` — `size_pt` default `14.0`. Used in D4's occlusion arithmetic.
- `crates/renderer/src/lib.rs:716-737` — the registered-layer record loop, which sits **outside** the snapshot-equality branch (`:646-673`) and runs unconditionally every frame. This is what makes D10's hover free of redraw plumbing.
- `crates/renderer/src/tab_bar.rs:431-433` — `record` early-returns when `ctx.left_inset <= 0.0`; `:434` is `self.ensure_gpu(ctx)`.
- `crates/renderer/src/tab_bar.rs:98`/`:107` — `slot_y`/`tab_at`, the exact-inverse pair tested at `:667-675`; `:588-683` is the pure-geometry test idiom.
- `crates/renderer/src/tab_bar.rs:451-464` allocates two buffers with `create_buffer_init` **per frame** — the anti-pattern foundations D3 exists to avoid.
- **`set_scissor_rect` appears nowhere in the workspace** (grep-verified). Scissor is per-render-pass state and every `DrawLayer` records **its own** `LoadOp::Load` pass (`tab_bar.rs:466-482`; `lib.rs:715`), so a scissor set here cannot clip another layer's drawing (D5).

### The app seam (all of it `sidebar-interaction`'s)
- `crates/app/src/terminal_app.rs:353-357` — `CursorMoved` stores `last_cursor` and nothing else: **no hover state exists today**.
- `:359-376` — `MouseInput` Pressed/Left calls `tab_target(...)` → `do_tab_action(Jump)`, discarding the returned bool at `:374`. **There is no `MouseWheel` arm.**
- `:378-383` — `Resized` calls `grid_size()` then `tabs.resize_all(cols, rows)`.
- `:132-134` and `crates/app/src/settings.rs:28` — the two `set_left_inset(tab_bar::width_px(cell_w))` call sites, made inert by row-redesign D4 and deleted by `sidebar-interaction` D4.
- `:421-430` — the winit#2640 workaround: `RedrawRequested` re-arms `window.request_redraw()` unless occluded. **The app redraws continuously**, so per-frame allocation is paid ~60×/second (SD4).

### Constraints found in project docs
- `.claude/rules/no-gui-crates.md` — render stack is exactly `wgpu`, `swash`, `bytemuck`, `raw-window-handle`.
- `.claude/rules/dependency-direction.md` — `renderer` never imports `winit`; `term-core` untouched.
- `.claude/rules/macos-only.md` — no `#[cfg(target_os)]` branches.
- `CLAUDE.md` — no `unwrap()` outside tests; gates `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.
- `.claude/settings.json` — PostToolUse `rustfmt`; PreToolUse **blocks `Cargo.lock` edits**, so no new crate dependency is possible.

## L2 Checkpoints

Complexity: **Medium (3 signals)**
- **3+ independent subsystems**: the drawn chrome, the overflow/scroll model, and the pure hit-test geometry consumed by a sibling feature.
- **Data schema design**: `SidebarLayoutInput`, `SidebarRegion`/`ChromeRegion` and `SidebarChromeModel` are three new contracts a dependent feature already programs against.
- **Multi-actor interaction**: three sibling features write one `Rc<RefCell<SidebarModel>>` singleton at pointer-event rate, with a stated `RefCell` panic hazard — a genuine concurrency-shaped constraint even on one thread.

An earlier draft counted "a scroll offset carried across frames" as a state-machine signal. **Withdrawn**: D6 stores no offset here. The classification (Medium) is unchanged either way. Not counted: external resource loading, async resources.

Brownfield adjustment applied: Core 30% · Scope 20% · Error 25% · Data 15% · Implementation 10%.

### Core Behavior (30%) — 6/6 resolved
- [x] What occupies the header's dominant left region without a search field → D1
- [x] Header pinned vs. scrolling with the list → D1
- [x] The scroll model: unit, ownership, clamping, auto-reveal → D6
- [x] How collapsed is represented, and how the reclaimed width reaches the grid → D3
- [x] The exact pure-function API draw and hit-test both call → D7
- [x] **How the app reaches that API without a `FrameCtx`** → D7 *(added: the dependent proved round-1's only constructor unreachable from `crates/app`)*

### Scope Boundaries (20%) — 6/6 resolved
- [x] Search input out; the region it will occupy is reserved and empty → D1 (SD8)
- [x] Filter drawn, no hover, **no hit region** → D2 (SD8) *(the dependent has frozen this reading and deleted its `Filter` match arm)*
- [x] Event handling, inset mutation, grid re-fit and layer registration are `sidebar-interaction`'s → D3, D14 + R7
- [x] Card interior, panel background and panel border are `sidebar-row-redesign`'s → Non-goals *(verified against its R1.8)*
- [x] The width and card-height formulas are `sidebar-row-redesign`'s; `row_h` is an input here → D6, D7
- [x] The `lib.rs` edit is bounded to two `pub mod` lines → D14

### Error / Edge Cases (25%) — 8/8 resolved
- [x] Content shorter than the viewport (max scroll 0, no rubber band) → D6
- [x] Viewport shorter than the header (negative list height) → D6, D8
- [x] Zero tabs / one tab → D6
- [x] Non-finite or out-of-range scroll offset → D6
- [x] A card scrolled under the pinned header must be neither drawn-through nor clickable → D5, D9
- [x] The restore button being buried by another layer → D4 *(Tier-1: the image layer had zero decisions covering it)*
- [x] The restore button occluding terminal cells while collapsed → D4 *(Tier-3 vs SD8)*
- [x] **The restore button must be reachable when no card layout has ever been published** → D7 *(added: row-redesign publishes `SidebarLayout` only **after drawing**, and its layer does not draw while collapsed — so a session starting collapsed would have no layout at all)*

### Data Model (15%) — 5/5 resolved
- [x] `SidebarRegion` / `ChromeRegion` shape and ordering semantics → D7, D9
- [x] `SidebarLayoutInput`'s fields enumerated, and where `scaled()` is applied → D7
- [x] `SidebarChromeModel` — exactly one field, the raw cursor → D10, D11
- [x] Tokens the mock defines that neither `SidebarTheme` nor `SidebarMetrics` carries → D15
- [x] **A device-free shape-building seam, so the drawn output is assertable without a GPU** → D12 *(added: `record` needs a `FrameCtx`, so every "what is drawn" requirement would otherwise skip on adapterless CI)*

### Implementation (10%) — 5/5 resolved
- [x] Pipeline via `SidebarShapePipeline::{new, ensure, draw}`, instances via `upload_resident` → foundations D3 (`assumed: true`)
- [x] Icons via the `push_icon` sink → foundations D7 (`assumed: true`)
- [x] Pure-geometry and z-order tests need no device → `tab_bar.rs:588-683`, `image_layer.rs:390-393` (`assumed: true`)
- [x] App→renderer state on an `Rc<RefCell<_>>` handle → `tab_bar.rs:30-54`, `overlay_layer.rs:17-34` (`assumed: true`)
- [x] The per-frame shape buffer is a reused `Vec` field cleared each frame → D12 *(Tier-3 on SD4)*

**Composite: 1.00** (30/30 resolved; every dimension ≥ the 0.60 floor). Unknown/unknowns: 0 pending.

> Scoring honesty note. This table read 1.00 in both rounds an adversarial reviewer returned blocking defects — seven, then five. It measures "no checkpoint I can currently name is open". The residual risk is concentrated in D13 and in the one cross-spec conflict recorded in Known Gaps, neither of which this feature can close alone.

### Unknown/Unknown Detection

**Tier 1 (Actor).** Actors: the *user* (D1–D3, D6 ✓); *`sidebar-interaction`* as caller of every pure function (D3, D6, D7, D9, D10, D14 ✓ + R7); *`sidebar-row-redesign`* as the drawer of the cards this chrome surrounds — initially **0 decisions**, and the sweep that fixed that surfaced its published API and the `list_top`/`scroll_px` gap → **D7, D13**; the *image layer* and *ai-monitor overlay*, which also draw over the terminal and could bury the restore button — initially **0 decisions** → **+1 Error/Edge** → **D4**.

**Tier 2 (Implication).**
- D1 (a pinned header) → implies the card list's origin is no longer `y = 0`; the sibling reserved `SidebarGeom.list_top`, but nothing reads it → **D13**.
- D3 (collapse) → implies the grid must be re-fitted after the inset changes → `sidebar-interaction` D6, enumerated in **R7**.
- D5 (clip by opaque overdraw) → implies the band must be **opaque** and this layer's z strictly above the row layer's → **D4**, **D5**.
- D6 (a scroll offset) → implies someone must clamp it and someone must reveal the active tab → pure functions here, caller named in **R7**.
- D7 (a published function list) → implies the caller can *construct its argument*; the dependent proved it could not → **+1 Core checkpoint** → **D7**'s raw-literal path.
- D10 (a hover visual) → implies a redraw on cursor move, which the app does not do → answered by the continuous loop **and** by the record loop sitting outside the snapshot-equality branch (`lib.rs:716-737` vs `:646-673`).
- D3 (collapse over the terminal) → implies the restore button occludes live cells → Tier 3.
- D15 → implies a second home for design tokens → confined to one block with a stated destination.

**Tier 3 (Pair Tension).** **Runs** — SD4 is a performance constraint, D3/D4 are occlusion decisions, and the shared `RefCell` singleton is a reliability constraint.
- **D4 × SD8.** SD8 excludes the mock's terminal-pane chrome because it *"would permanently consume terminal rows"*. The restore button does exactly that, in miniature. Quantified in **D4** rather than hand-waved (≈15 cells; two earlier drafts said "~4" and "~2×2", both wrong).
- **D5 × D4.** Clipping-by-overdraw holds only while this layer's z exceeds the row layer's. `sidebar-row-redesign` retains `TAB_BAR_Z = 10` (`:244`), so the assertion has a stable constant — but it must be a **test**, because a card layer registered above 30 would silently invert the clipping.
- **D3 × D7 × row-redesign D3/D14.** Row-redesign publishes `SidebarLayout` *after drawing* and does not draw while collapsed, so a session that starts collapsed never publishes one. If this feature's restore geometry depended on that layout, collapse would be a **one-way trip** — the exact failure D4 spends a paragraph preventing at the z level. → **+1 Error/Edge checkpoint**; resolved by `restore_button_rect(scale)` taking a scale alone (D7).
- **D6 × SD4.** A `Vec`-returning visible-row query would allocate every frame → `visible_rows` returns a `Range<usize>`; **D12** covers the layer's own buffer.
- **D9 × D6.** Cards-before-chrome would make a card under the header clickable while invisible → **D9** fixes the order and tests it.
- **D10/D11 × row-redesign D3.** Two channels now carry hover — the raw cursor on this feature's handle, a resolved card index on the shared singleton — both written by one feature at pointer rate. → The asymmetry is deliberate and is the dependent's own design (its D8, `:368`); the `RefCell` borrow discipline is inherited as a hard rule (Constraints).

### Inversion Probe
*"What could make this fail even if every requirement here is met correctly?"*

**The two fields nobody is assigned to wire.** `SidebarGeom.list_top` and `.scroll_px` exist in `sidebar-row-redesign`'s struct, hardcoded to `0.0`, with its spec saying this feature *"fills two fields"*. The code that constructs the geom is in **`crates/renderer/src/sidebar_row.rs`**, outside this feature's declared surface, and that feature lands first. Every requirement here can pass with the header drawn perfectly over a first card that never moved. This is **D13**: the diff is sized exactly and escalated.

**Two layout type families in one sidebar.** This feature publishes `SidebarLayoutInput` + free functions; row-redesign publishes `SidebarGeom`/`SidebarLayout` + methods; both cover card placement. The dependent flags it BLOCKING (`:427`) and couples as narrowly as it can. Every requirement in all three specs can pass while the sidebar contains two answers to "where is card 3" — the precise failure SD2 exists to prevent, arrived at by partition rather than by carelessness. Recorded in Known Gaps with the resolution named.

*"You decided the layer resolves hover itself. Does that also mean the app must request a redraw on `CursorMoved`?"* — **No, verified twice**: `terminal_app.rs:421-430` keeps a frame always in flight, and the layer-record loop (`lib.rs:716-737`) runs outside the snapshot-equality branch (`:646-673`). The corollary is cost, not latency — hence D12's ban on per-frame allocation.

## Decisions

Inherited verbatim from `specs/sidebar-redesign/partition-manifest.md` → `## Shared Decisions`:

### SD1: Adapt the mock's visual language onto the hand-rolled wgpu renderer — never clone its DOM
- **Status**: resolved
- **Rationale**: `Mac Terminal.dc.html` is a CSS/DOM/SVG mock. We reproduce its *visual language* — the `#1e1b18` sidebar on `#24211d`, 13px-radius cards, 12/15px card padding, 3/8px card margins, two-line rows, the `✳`/`◇` mark glyphs, the branch sub-line, the per-agent gradient skins, the 340px default width — as instanced quads and WGSL on the existing renderer. Rejected: adding any GUI/widget crate to get flexbox/SVG for free (violates `.claude/rules/no-gui-crates.md`, the project's reason to exist). Rejected: pixel-perfect cloning — font rasterization, antialiasing and subpixel metrics differ between a browser and our swash atlas, so "matches the mock" means the design tokens are honored, not that a screenshot diff is empty.

### SD2: Sidebar chrome lives in physical-px space, with ONE geometry source of truth shared by draw and hit-test
- **Status**: resolved
- **Rationale**: The mock's paddings, radii and gaps are not cell multiples, so cell-space layout cannot express them. All sidebar layout stays in physical px as pure functions in the renderer, and the app hit-tests by calling those same functions — extending the existing `slot_y`/`tab_at` exact-inverse pair (`crates/renderer/src/tab_bar.rs:98,107`) that `input::tab_target` already delegates to. The sidebar's px width reaches the terminal grid only through `Renderer::set_left_inset` (`crates/renderer/src/lib.rs:508`), which already shifts the grid in the vertex shaders and drives `grid_dims`. Rejected: expressing sidebar layout in cells; rejected: duplicating layout math on the app side (guarantees drift between what is drawn and what is clickable).

### SD3: `term-core` stays GPU-free AND tab-free; tab/git/agent state crosses to the renderer as plain data
- **Status**: resolved
- **Rationale**: `.claude/rules/dependency-direction.md` makes the headless core load-bearing. Tabs are already an `app`-only concept (`crates/app/src/tabs.rs`); git and agent state are likewise app concerns. The renderer keeps receiving them over the existing `Rc<RefCell<TabBarModel>>` side-channel (`tab_bar.rs:30-54`), extended to carry the richer per-tab payload — never on `GridSnapshot`. Rejected: adding tab/git/agent fields to `GridSnapshot` (it is the versioned grid contract and would pull non-terminal state into `term-core`).

### SD4: Per-tab metadata is polled off the UI thread and cached; the render path performs no I/O
- **Status**: resolved
- **Rationale**: `Tabs::titles()` already costs a `proc_pidinfo` syscall per tab per call, and the app runs a continuous redraw loop (`terminal_app.rs:428-430`, the winit#2640 workaround). Adding a filesystem read and a process-tree walk per tab per frame would compound that into a measurable idle-CPU regression — the exact debt G1 records as already paid down once. So a background worker refreshes branch + agent state keyed by `TabId`, and pushes results in via the existing `EventLoopProxy` wake path that PTY output already uses. Rejected: computing branch/agent inline in `sync_tab_model()`.

### SD5: Git branch comes from reading `.git` directly — no git library, no `git` subprocess
- **Status**: resolved
- **Rationale**: We need exactly one string per tab: the current branch. Walking up from the tab's cwd to the first `.git` and reading `HEAD` (`ref: refs/heads/<name>` → branch name; a raw SHA → short detached-HEAD form) is a couple of small file reads. Rejected: `git2` (drags in a libgit2 C build for one field). Rejected: `gix` (large dependency tree for one field). Rejected: shelling out to `git rev-parse` (a process spawn per tab per poll, on a poll loop).

### SD6: AI-agent detection reads the PTY child's descendant processes; `ai-monitor` supplies the Claude-specific detail
- **Status**: resolved
- **Rationale**: `Pty::child_pid()` (`crates/term-core/src/pty.rs:63`) already gives the zsh pid, and the cwd path already uses macOS `libc::proc_pidinfo` (`session.rs:142`) — so a descendant scan via `proc_listchildpids` + `proc_pidpath`, matched against a known-agent table, is the same class of macOS-native call we already depend on (`.claude/rules/macos-only.md` permits this freely). The existing `crates/ai-monitor` JSONL reader supplies the richer Claude Code state where present. Rejected: scraping the terminal's own scrollback for agent banners (fragile, breaks on any CLI release). Rejected: an OSC escape handshake (agent CLIs do not emit one).

### SD7: Sidebar appearance is config-driven, with the mock's values as the defaults
- **Status**: resolved
- **Rationale**: The sidebar's colors, `PAD_X`, `BORDER_W` and the `14.0`/`1.5` cell multipliers are hardcoded in `tab_bar.rs` today, and `common::config` has zero sidebar fields. The mock itself exposes `sidebarWidth` (260–440, default 340), `showGitBranch` and `accent` as knobs, so those are the natural user-facing settings. A new `[sidebar]` section follows its siblings' `#[serde(default, deny_unknown_fields)]` shape and rides the existing live-reload path (`settings::apply`, `settings.rs:23-34`). Rejected: leaving the values hardcoded (the mock's own props argue they are settings).

### SD8: Scope is the SIDEBAR only — the mock's terminal-pane chrome is out, and the search field is deferred
- **Status**: resolved
- **Rationale**: The user's ask is "redesign the left tab", plus git branch and AI agent. The mock's right-hand pane also shows a top-right icon row, a bottom status/footer bar, and an "Use agent" banner — all of which would permanently consume terminal rows from a real PTY grid, and the agent welcome block in the mock is output the agent CLI itself prints (not our chrome). The header's search/filter input is real work of a different kind: a focus-stealing text field must win keystrokes away from PTY routing, which needs its own focus model. Both are deferred to separate future specs rather than smuggled in here. The header's **new-tab** and **hide-sidebar** buttons ARE in scope (they are pure click targets); the filter button is drawn but inert.

---

Feature-local decisions:

### D1: The header is **pinned**, keeps the mock's box exactly, and its vacated left region is a **reserved, empty `search_slot`**
- **Status**: resolved
- **Rationale**: Two questions, one answer.
  **Pinned, not scrolling.** The header holds the only affordances that create a tab and hide the sidebar. If it scrolled with the list, then at 40 open tabs — precisely the situation the overflow model exists for — the user would have to scroll *up* to reach the button that would have let them stop scrolling. A pinned header also makes the list viewport a function of the window and the header height alone, which is what lets D5's clip-by-overdraw be a static z-order fact rather than a per-frame computation, and what lets `list_top` be a constant the sibling's `SidebarGeom` can hold. Rejected: a header that scrolls away (unreachable collapse); rejected: a header that shrinks on scroll (an animation state machine for a 54px band).
  **The vacated region stays empty and named.** SD8 defers the search field, so the mock's header row loses the flex child that filled it. Rejected: **a search-shaped placeholder** — a text input's silhouette with no input behind it, a strictly worse lie than an inert icon (a user *will* click a search box). Rejected: **an invented brand/section label** ("TABS", a tab count) — the mock specifies no such element, foundations' D1 rejected the identical move as fabricating design, and it would drag a text path into a feature that otherwise draws none (D12). Rejected: **shrinking the header to the buttons** — it changes the `padding: 14px 12px 12px` box that SD1 says we honor, and it would have to be un-changed when the search field lands.
  So the header keeps its full-width box, the three icons are laid out **right-aligned** at the trailing edge (`sidebar_w − header_pad.x` inward, in the mock's source order filter → new-tab → hide, so hide sits at the sidebar's top-right corner), and everything left of them is `search_slot_rect()` — a named region this feature **draws nothing into**. It reads as header padding on the panel background, it is forward-compatible with the deferred field dropping in with no other geometry moving, and it is testable.
  **Header height** = `header_pad.top(14) + button_h + header_pad.bottom(12)` where `button_h = ICON_PX + 2·button_pad = 16 + 12 = 28` → **54 logical px**. That is the value `SidebarGeom.list_top` must receive (D13).
  **The width budget, shown rather than asserted** (an earlier draft's figure did not reproduce): three boxes at `16 + 2·6 = 28` px is 84; two `header_gap`s of 8 is 16; the row is **100 px**. At the clamped minimum width of 260 (foundations D2), the content box is `260 − 2·12 = 236`, leaving a **136 px** `search_slot` — 128 if a third gap is charged before the first icon. Positive across the whole clamp band.

### D2: Three icons are drawn; `NewTab` and `Hide` are live regions, the filter has **no hover and no hit region**
- **Status**: resolved
- **Rationale**: SD8 says "the filter button is drawn but inert", which admits two readings, and the difference is user-visible. Reading A: draw it with the mock's hover treatment (`background:#2a2722`, `color:#c9c2b2`) and make the click a no-op. Reading B: draw only the icon at `icon_fg`, with no hover state and no region. **B is chosen**, because a hover highlight is a promise that a click will do something, and honouring the promise with a no-op is the interaction equivalent of a dead link — the user clicks it repeatedly and concludes the terminal is broken. Under B the filter reads as an icon in the header: inert *looking* as well as inert *behaving*.
  **This is now a frozen cross-spec contract, not a preference.** `sidebar-interaction` has adopted it: its Non-goals (`:27`) state *"`ChromeRegion` has no `Filter` variant"*, and its D1 (`:266`) records that it **deleted** an earlier `Filter → Inert` match arm *"because a match arm for a variant that does not exist would not compile."* An intermediate draft of this spec briefly reintroduced a `Filter` variant, reasoning that a hit region prevents click fall-through; that reasoning is **withdrawn**, because D9 already gives every point inside the sidebar a `Panel` result, so a press on the filter icon is swallowed exactly like any other dead space — the fall-through concern was already answered, and adding the variant would have broken a dependent's total match.
  `filter_icon_rect(inp)` is still published, because the other two icons' positions depend on the filter occupying its slot in the row, and because the deferred search spec will want it.
  Rejected: reading A (a hover affordance with no action); rejected: a `Filter` region (breaks the dependent's frozen match for a problem `Panel` already solves); rejected: omitting the filter entirely (SD8 says it is drawn) — though see Known Gaps, where a human may prefer omission.

### D3: **Collapsed is an input, not an inference** — the layer reads `ctx.left_inset`, the hit-test takes a `collapsed` argument, and neither is a source of truth
- **Status**: resolved
- **Rationale**: Two call sites need collapsedness and they are not symmetric, which is why one rule for both would be wrong.
  **Drawing**: the layer reads `ctx.left_inset`. It is the number already in the `FrameCtx`, it is the number `grid_dims` (`lib.rs:1021-1033`) subtracts, and it is the number the row layer's own early return uses (`tab_bar.rs:431-433`, retained by row-redesign D14 `:418`) — so the row layer stops drawing while this one starts, with no coordination and no second flag. The reclaimed width reaches the grid **by construction** rather than by remembering.
  **Hit-testing**: `collapsed` is an explicit `bool` argument to `hit_test`/`chrome_hit`. This is not a stylistic difference — `sidebar-interaction` D1 (`:267`) establishes it as a correctness requirement: collapsed, the only live region is the restore button, which floats *over the terminal*, so any guard shaped like today's `x < sidebar_w` (`tab_bar.rs:107-113`) would make it permanently unclickable and the sidebar could be hidden but never brought back. *"`collapsed` is an **argument to `hit_test`**, never a gate on whether to call it."*
  **The steelman, answered.** `sidebar-interaction` D5 (`:320`) rejects deriving `collapsed` from the inset: *"a float comparison standing in for a boolean, and under row-redesign D4 the inset is **derived from** `collapsed`, so this would be a cycle."* The cycle objection is correct about a *source of truth* and is why this decision claims none. The causal chain is strictly one-directional: the app's live `Config.sidebar.collapsed` (owned by `sidebar-interaction` D5, mutated by its D6) → `Renderer.sidebar` via `apply_config` → `sync_left_inset` → `left_inset` → what this layer draws. This feature sits at the **downstream end** of that chain and never writes any link of it; reading the last one to decide what to draw is reading a shadow, not closing a loop. And for hit-testing it does not even read the shadow — it takes the upstream boolean as a parameter, which is precisely what the objection asks for.
  An earlier draft of this spec did claim `left_inset` as the single source of truth and removed the parameter. That was wrong on both counts and is withdrawn.
  **The layer's own guard is the opposite one**: this layer draws *more* when the inset is zero, so its `record` must not copy the row layer's early return.
  Rejected: `collapsed` as this feature's own state (nothing here can observe a click); rejected: reading `ctx.sidebar.collapsed` for drawing (the config value refreshed only on `apply_config`, which is exactly what `left_inset` already reflects, with an extra hop that can be stale between them); rejected: gating the hit-test on the sidebar band (makes collapse a one-way trip — the dependent's D1 argument).

### D4: `SIDEBAR_CHROME_Z = 30` — above `IMAGE_Z`, below `OVERLAY_Z`; ≈15 occluded cells accepted; the sibling's suggested `11` is declined with a reason
- **Status**: resolved
- **Rationale**: Three constraints hold at once. **(1) Above `TAB_BAR_Z = 10`** — D5's clipping is overdraw, so the opaque band must paint over any card scrolled under it. Row-redesign retains that constant (`:244`). **(2) Above `IMAGE_Z = 20`** — collapsed, the restore button is drawn over the terminal's top-left, exactly where an inline image pasted at row 0 lands; burying the **only** affordance that brings the sidebar back would leave no recovery short of editing the config file. **(3) Below `OVERLAY_Z = 100`** — the ai-monitor overlay is a full-window panel that should win. Any value in `(20, 100)` satisfies all three; **30** is chosen. Following `image_layer.rs:390-393`, this is a **test** (`> TAB_BAR_Z`, `> IMAGE_Z`, `< OVERLAY_Z`), not a comment, because a card layer registered above 30 would silently invert D5's clipping.
  **`sidebar-row-redesign`'s Post-work (`:889`) advises `z_order = 11`** — *"so it records after this feature's `TAB_BAR_Z = 10` panel"*. That satisfies constraint (1), which is the only one that note was reasoning about, and **fails constraint (2)**: 11 < `IMAGE_Z` = 20, so an inline image would bury the restore button. The advice is declined for that reason, and the divergence is named here rather than left for someone to discover as a conflict.
  **The accepted cost, quantified.** While collapsed the restore button sits at logical `top: 12, left: 14` with a `2·8 + 16 = 32` box, spanning `x ∈ [14, 46]`, `y ∈ [12, 44]`. At the default `size_pt = 14.0`, `cell_size_px` (`lib.rs:1010-1013`) rounds to `(8, 17)` at scale 1 — so the button covers **columns 1–5 × rows 0–2, ≈15 cells**, approximately scale-invariant (approximately, because that `round()` makes the cell size non-linear in the scale factor). Two earlier drafts said "~4 cells" and "~2×2"; both were wrong by roughly 4×, and since this decision's whole claim is that the cost is *named rather than discovered*, the number has to be right. It is accepted because both alternatives are worse: reserving a row or column consumes grid capacity **permanently, for every user**, including those who never collapse — what SD8 rejects — whereas this costs ≈15 cells only while collapsed and only for the user who chose it; and drawing nothing makes collapse a one-way trip.
  Rejected: `z = 11` and `z = 12` (an inline image buries the restore button); rejected: `z > 100` (the ai-monitor overlay must win); rejected: reserving grid space (a permanent cost for a transient state).

### D5: Overflow is clipped by **opaque overdraw from a higher-z pinned header**, plus the framebuffer's own bottom edge
- **Status**: resolved
- **Rationale**: The obvious mechanism, `RenderPass::set_scissor_rect`, **cannot work from this layer**: scissor is per-render-pass state and every `DrawLayer` records its own `LoadOp::Load` pass (`tab_bar.rs:466-482`; `lib.rs:715`). The cards are drawn in `sidebar-row-redesign`'s pass; a scissor set here clips nothing of theirs.
  Two nearer alternatives exist, and they are rejected for an honest reason rather than a technical one:
  - **Scissor inside the row layer's own pass** — would clip partial cards exactly, need no z contract, waste no rasterisation. Rejected because `sidebar_row.rs`/`tab_bar.rs` are **another feature's declared surface**.
  - **A per-instance clip rect on `SidebarShape`, discarded in the fragment stage** — cleaner still, and extensible to text. Rejected because `sidebar_shapes.rs`/`.wgsl` are **`sidebar-foundations`' declared surface**, and that feature has shipped.
  So the choice is **partition-forced, not intrinsically superior**; claiming otherwise would overstate it. What makes it *sufficient* is the geometry:
  - **Top edge**: the band is drawn **opaque** in `panel_bg` `#1e1b18` at z=30, strictly above the row layer's z=10, so a card scrolled up past the list viewport's top is painted over. The opacity is a **correctness property, not decoration** — stated so nobody later "improves" it into a translucent band.
  - **Bottom edge**: the sidebar is full height, so the list viewport's bottom **is** the framebuffer's bottom; overflow past it is clipped by the render target with no mechanism at all.
  - No left/right overflow: cards are inset by `card_margin_x` inside a band narrower than the window.
  **The overdraw cost is already bounded by the sibling**: `sidebar-row-redesign` D13 has its layer emit shapes and text only for `visible_range()`. A card straddling `list_top` *is* emitted — the one overdraw actually paid for; cards entirely outside are never uploaded. `visible_rows(scroll, inp)` is published here for the same purpose in this feature's own coordinate vocabulary, as a **cost measure only**: correctness never depends on it, so a caller that ignores it renders identically.
  Rejected: scissor in this pass (cannot cross a pass boundary); rejected: scissor in the row layer's pass and a per-instance clip rect (both out of declared surface); rejected: whole-instance culling *as the correctness mechanism* (a card straddling the header edge would pop out entirely — a visible jump); rejected: a stencil buffer (no depth/stencil attachment exists).

### D6: Scroll is a clamped **physical-px offset** owned by the app; reveal-active-tab is event-triggered, never derived per frame
- **Status**: resolved
- **Rationale**: **Unit: physical px, not row index.** A row-index offset cannot express a partially-scrolled row, so a trackpad's continuous delta would snap card-by-card — jarring on ~70px cards and irreconcilable with macOS momentum scrolling. Px is also the unit `SidebarGeom.scroll_px` declares.
  **Model.** `row_stride(inp) = inp.row_h + 2·card_margin_y`; `content_height(inp) = inp.row_count · row_stride(inp)`; `max_scroll(inp) = max(0, content_height − list_viewport(inp).h)`; `clamp_scroll(s, inp) = if !s.is_finite() { 0.0 } else { s.clamp(0.0, max_scroll(inp)) }`. When the content fits, `max_scroll == 0` and the offset pins to 0 — **no rubber-banding, no overscroll**, because a bounce is animation and this feature has no per-frame state.
  **`row_h` is an input, never computed here.** The card's height is `sidebar-row-redesign`'s (its D6); the app reads it from the published `SidebarLayout.card_h` and puts it in `SidebarLayoutInput` (`sidebar-interaction` D3, `:282`). Recomputing it here from `SidebarMetrics` would be a second card-height formula — a textbook SD2 violation, and one the dependent explicitly rejects for itself.
  **Ownership.** The offset is app state. `sidebar-interaction` D10 owns the `f32`, mutates it on `MouseWheel` in both unit systems, re-clamps it on tab close and window resize (both shrink `max_scroll`), and passes it to every function that needs it. **This feature stores no scroll offset** — the header is pinned and the restore button is fixed, so nothing it draws depends on the value.
  **Reveal.** `scroll_to_reveal(index, scroll, inp) -> f32` returns the minimal offset bringing row `index` fully into the list viewport: unchanged if visible, `index · stride` if above, `(index + 1) · stride − viewport.h` if below, then clamped. It exists because without it a chord tab-switch (`Cmd+9`) to a tab below the fold highlights a card the user cannot see — a correctness bug, not a nicety. It is deliberately **not** applied inside the layout every frame: a user who scrolls away from the active tab would be snapped back 60 times a second. So it is a pure function the app calls **on tab change**; the dependent's D10 (`:397`) puts the call in `do_tab_action`, *"the one place it cannot be forgotten"*, which discharges the residual risk this decision hands over.
  Rejected: row-index offsets (no partial rows); rejected: computing `row_h` here (a second formula, SD2); rejected: storing the offset in `Renderer` (`lib.rs` churn on an ordered-shared surface, duplicating a value the app holds); rejected: applying reveal inside the layout (snap-back every frame); rejected: rubber-band overscroll (animation state).

### D7: `sidebar_layout.rs` is **pure, state-free and GPU-free**, over an explicitly-enumerated `SidebarLayoutInput` the app can build by hand
- **Status**: resolved
- **Rationale**: This is SD2's core. The module holds no state, allocates nothing, imports no `wgpu` type, and every function is a `pub fn` of its arguments — so the drawing here and the hit-testing in `sidebar-interaction` are literally the same call, and the tests need no device (`tab_bar.rs:588-683`).
  **The input, enumerated** (an earlier draft named only one field, which made the module underivable):
  ```rust
  #[derive(Clone, Copy, Debug)]
  pub struct SidebarLayoutInput {
      pub sidebar_w: f32,      // physical px; 0.0 while collapsed
      pub viewport: [f32; 2],  // physical px, framebuffer size
      pub scale: f64,          // window scale factor
      pub row_h: f32,          // physical px, from SidebarLayout.card_h — NOT computed here (D6)
      pub row_count: usize,
      pub metrics: SidebarMetrics, // ALREADY scaled: SidebarMetrics::scaled(scale)
  }
  ```
  **Two constructors, and the app path is the plain literal.** `SidebarLayoutInput::from_frame(ctx, row_h, row_count)` is a renderer-side convenience that scales the metrics once. It is **not** the app's path: `sidebar-interaction` D3 (`:280`) proved it unreachable — `FrameCtx` holds a `&Device`, `&Queue`, `&mut CommandEncoder`, `&TextureView` and `&mut Atlas`, *"The app has none of these and must not acquire them"* — and closes the gap by assembling the struct literally from accessors, arithmetic-free. So **every field is `pub` and the raw struct literal is the documented app constructor**, with `metrics` doc-commented *"the result of `SidebarMetrics::scaled(scale)` — never `SidebarMetrics::DEFAULT`"*. That is a real weakening of the unit guarantee, and it is accepted knowingly: a private field would have made the type unconstructible from the only feature that needs it. Foundations D6 already records that `scaled` is centralisation, not a type-level guarantee.
  **`restore_button_rect(scale: f64)` takes a scale alone, deliberately.** The restore button must be hit-testable when **no card layout has ever been published**: row-redesign publishes `SidebarLayout` *after drawing* (its D3, `:262-281`) and its layer does not draw while collapsed (its D14), so a session started with `collapsed = true` has `SidebarModel.layout == None` forever. If the restore geometry needed a layout, collapse would be a one-way trip — the exact failure D4 prevents at the z level. A scale-only signature makes it unconditionally reachable. This does not reintroduce two scaling boundaries, because the restore rect uses only `tokens` values (D15) and no `SidebarMetrics` field, so there is nothing to double-scale.
  **The published API — this list *is* the contract with `sidebar-interaction`:**

  | Function | Purpose |
  |---|---|
  | `SidebarLayoutInput::from_frame(ctx, row_h, row_count)` | renderer-side convenience; scales metrics once |
  | `header_h(scale) -> f32` | the pinned band's height — the value `SidebarGeom.list_top` must receive |
  | `header_rect(inp) -> Rect` | the opaque band |
  | `button_rect(ChromeRegion, inp) -> Rect` | `NewTab` / `Hide` boxes |
  | `filter_icon_rect(inp) -> Rect` | the inert filter's box (D2) |
  | `search_slot_rect(inp) -> Rect` | the reserved, empty region (D1) |
  | `list_viewport(inp) -> Rect` | the band cards live in |
  | `row_stride(inp) -> f32`, `content_height(inp) -> f32`, `max_scroll(inp) -> f32` | the overflow extent (D6) |
  | `clamp_scroll(s, inp) -> f32`, `scroll_to_reveal(i, s, inp) -> f32` | the scroll model (D6) |
  | `row_rect(i, s, inp) -> Rect`, `row_at(y, s, inp) -> Option<usize>` | card slots and their inverse (D8) |
  | `visible_rows(s, inp) -> Range<usize>` | culling, a cost measure only (D5) |
  | `restore_button_rect(scale) -> Rect` | the collapsed affordance, layout-free |
  | `chrome_hit(p, inp, collapsed) -> Option<ChromeRegion>` | chrome-only, scroll-free — what the layer calls for hover |
  | `hit_test(p, s, inp, collapsed) -> Option<SidebarRegion>` | the ordered full test (D9) |

  `Rect { x, y, w, h }` is a plain `Copy` struct with `contains(p)`. `ChromeRegion { NewTab, HideSidebar, Restore }` — no `Filter` (D2). `SidebarRegion { Chrome(ChromeRegion), Card(usize), Panel }`, where `Panel` means "inside the sidebar, on no target", so the app swallows the press instead of letting it fall through.
  Rejected: methods on a struct built per frame (an allocation on the continuous loop for three-line functions); rejected: taking `&FrameCtx` in every function (pulls `wgpu` into the hit-test path — the hazard the dependent hit); rejected: private fields with only `from_frame` (unconstructible from `app`, the round-1 defect); rejected: taking `&sidebar_row::SidebarLayout` instead (unavailable while collapsed, see above — this is the fix for the one-way-trip bug, at the cost named in Known Gaps).

### D8: `row_at` is **stride-tiling, not strict containment** — the inter-card gap belongs to the nearer card
- **Status**: resolved
- **Rationale**: The precedent pair (`slot_y`/`tab_at`) is trivially invertible because slots tile with no gap. These cards do not: `margin: 3px 8px` insets each card 3px top and bottom inside its stride, so a strict `contains()` inverse would leave a **6px dead band at every card boundary** where a click does nothing. On a list where every pixel looks clickable that is the same class of defect SD2 exists to prevent, phrased as a dead zone rather than an offset error.
  So `row_at(y, s, inp)` computes `floor((y − list_viewport(inp).y + s) / row_stride(inp))` and returns `Some(i)` when `i < row_count`; the gap above a card belongs to it and the gap below to the next. `row_rect(i, s, inp)` returns the **drawn** rect, inset by `card_margin_y` inside that band. The two are therefore *not* a bijection, and saying so is better than a comment claiming they are: the relationship is four named, tested properties (R6) — round-trip on centres (the direct analogue of `slot_y_and_tab_at_are_exact_inverses`), total coverage with no holes below `content_height`, invariance under fractional scroll offsets, and agreement with `visible_rows`.
  **This matches `sidebar-row-redesign`'s `card_at` by design**, which inverts over the stride band for the identical reason (its D7, `:345`) — the two must agree, and Known Gaps records that they are two implementations of one `floor`.
  Rejected: strict containment (6px dead bands); rejected: expanding the drawn card to fill the stride (deletes the mock's 3px margin, a visible part of the design SD1 honors); rejected: nearest-centre snapping (identical results for a uniform stride, at more arithmetic).

### D9: `hit_test` is **ordered chrome-first**, so a card hidden under the header is neither drawn-through nor clickable
- **Status**: resolved
- **Rationale**: D5 hides an overflowing card by painting the header over it. If the hit-test consulted cards first — or consulted them at all inside the header band — the user could click a tab that is invisible and switch to it. Invisible-but-clickable is worse than either alone, because nothing on screen explains what happened.
  So `hit_test(p, s, inp, collapsed)` is an ordered cascade with explicit early returns:
  1. **If `collapsed`**: return `Some(Chrome(Restore))` if `p` is in `restore_button_rect(inp.scale)`, else `None` — every other point belongs to the terminal.
  2. **Outside** `[0, sidebar_w) × [0, viewport.y)`: `None`.
  3. **Inside `header_rect`**: return the button region hit, else `Some(Panel)` — **never falling through to a card**. This is the decision's whole point, asserted directly (R6.4).
  4. **Otherwise**: `Some(Card(i))` from `row_at`, or `Some(Panel)` when the list is exhausted.
  `Some(Panel)` rather than `None` for sidebar dead space is load-bearing and is the dependent's design: its D1 (`:257-266`) maps `Panel` to a swallow and reserves `None` for "the pointer was over the grid", so a future terminal-selection feature will not start a drag on a press that visually landed on the sidebar. Its D1 (`:255`) also commits to **no app-side dispatch order**, precisely so this cascade is the only answer.
  Rejected: an unordered "first rect that contains the point" scan (order becomes an accident of declaration order); rejected: returning `None` for sidebar dead space (clicks leak to the grid).

### D10: The hover **visual** is drawn here and resolved here; the hover **event** is `sidebar-interaction`'s; the card's hover is `sidebar-row-redesign`'s
- **Status**: resolved
- **Rationale**: The app has no hover state today (`terminal_app.rs:353-357`). Ownership follows the pixels: this feature draws the header buttons and the restore button, so their `{background:#2a2722, color:#c9c2b2}` hover is here; `sidebar-row-redesign` draws the cards, so the card's `#2a2722` hover is theirs (its model carries `hovered: Option<usize>` for exactly that); `sidebar-interaction` owns every winit type, so tracking the cursor is theirs.
  **The handle carries the raw cursor point, not a resolved region.** `SidebarChromeModel { cursor: Option<[f32; 2]> }` — physical px, `None` when the pointer leaves the window. The layer resolves it by calling `chrome_hit`, *the same function `sidebar-interaction` calls on a press*. If the app resolved the region and pushed an enum, the highlight and the click would be two independent resolutions that could disagree — the divergence SD2 forbids. The dependent has adopted this verbatim (its D8, `:365`), and it deliberately does **not** debounce the raw cursor: *"it must be exact for `chrome_hit` to resolve a button edge, and it costs nothing"*, while the card index *is* region-debounced on the other channel. A middle draft of this spec inverted this into an app-resolved enum; that is withdrawn — it contradicted the dependent and lost the by-construction property.
  **A finding that removes work**, verified twice: because the app redraws continuously (`terminal_app.rs:421-430`) *and* because the layer-record loop (`lib.rs:716-737`) runs outside the snapshot-equality branch (`:646-673`), pushing the cursor is sufficient — the highlight appears on the next frame with no `request_redraw` plumbing and no stale-highlight failure mode. The corollary is cost, not latency, which is why D12 bans per-frame allocation.
  Rejected: the app pushing a resolved region (two resolutions of one question; contradicts the dependent); rejected: this feature drawing the card hover (it does not own the card's background states); rejected: no hover at all (the mock specifies it, and a macOS button with no hover reads as disabled).

### D11: `SidebarChromeHandle` is the **third** instance of the established handle idiom, carrying exactly one field
- **Status**: resolved
- **Rationale**: `TabModelHandle` (`tab_bar.rs:30-54`) and `OverlayHandle` (`overlay_layer.rs:17-34`) are two independent instances of one pattern: `#[derive(Clone, Default)] pub struct XHandle(Rc<RefCell<XModel>>)`, `pub fn set(&self, model)` on the app side, private `fn snapshot(&self)` on the layer side, single-threaded and never `Send`. This is the third; copying it costs ~20 lines and adds no concept, whereas a `FrameCtx` field plus a `Renderer` setter would put live UI state on the ordered-shared `lib.rs` surface D14 keeps to two lines.
  It carries **one field**, `cursor: Option<[f32; 2]>` (D10). Everything else that might have gone here does not need to: `collapsed` is an argument and a `FrameCtx` value (D3), the scroll offset is a parameter (D6), the tab list is row-redesign's model. A one-field model is a smell worth naming rather than hiding — it is justified because the field is per-frame UI state only the app can observe, and because a handle that starts with one field is the cheapest place for `sidebar-interaction` to add a second. It has already taken that invitation seriously: its D3 (`:293`) names *"having the chrome layer publish the `SidebarLayoutInput` it drew with, on the second handle field D11 explicitly invites"* as the strictly better SD2 form and the recommended follow-up, deferred only because it needs an edit to `sidebar_header.rs`.
  **Why not put chrome state on row-redesign's shared `SidebarModel` singleton** — which its D3 (`:262-281`) built partly for this feature, listing it as *"a fourth writer"*? Because the two things this feature needs from the app have different shapes and different rates: the cursor is unresolved, undebounced and written at 120 Hz, while `SidebarModel` is a snapshot-and-replace model whose writers take short borrows around whole-model updates. Mixing a high-rate raw input into it would put every pointer move inside the same `RefCell` the row layer borrows during `record`, on a path its own spec calls a *"self-deadlock at 120 Hz"* hazard. The handle keeps the hot path off that lock. `list_top` — a value written **once per frame by the layer itself**, not per event — does ride the shared model (D13), which is the right split.
  Rejected: a `FrameCtx` field (live UI state on the shared surface); rejected: `Cell` instead of `RefCell` (marginally cheaper, gratuitously different from both precedents); rejected: putting the cursor on `SidebarModel` (a 120 Hz writer on the row layer's borrow path).

### D12: This feature draws **no text**, builds its shapes in a **device-free** function, and reuses one `Vec`
- **Status**: resolved
- **Rationale**: Three properties, each fixing something a requirement would otherwise be unable to assert.
  **No text**: with no label in the header, every pixel is a `SidebarShape` — the band, the button backgrounds, the icons via `push_icon`. The layer never touches `ctx.atlas`, never needs truncation, never depends on `font.family`.
  **A device-free core**: `record` takes `&mut FrameCtx`, which requires a `Device`, `Queue`, `CommandEncoder`, `TextureView` and `Atlas` — so any requirement phrased as "when `record` runs, these shapes are emitted" could only be tested with a GPU, and would silently skip wherever `headless()` returns `None`, i.e. exactly the adapterless CI R0.4 promises to keep green. So the layer splits: `fn build_shapes(&self, inp: &SidebarLayoutInput, collapsed: bool, hover: Option<ChromeRegion>, out: &mut Vec<SidebarShape>)` is a pure method with no `wgpu` type in its signature, and `record` is a thin wrapper that assembles the input, calls it, and hands the slice to `SidebarShapePipeline::draw`. Every R2/R3/R8 assertion is written against `build_shapes`. `sidebar-row-redesign` reaches the same conclusion for its own layer (its R phrasing *"when chrome shapes are built"*).
  **No per-frame allocation**: the layer holds `shapes: Vec<SidebarShape>` as a field and calls `clear()` at the top of `record`, so the backing allocation is reused for the process's life. This is the `Vec`-level analogue of foundations' `upload_resident`, and it matters for the reason SD4 gives: the continuous redraw loop would otherwise pay ~60 allocations/second forever, in the codebase whose G1 idle-CPU debt is why SD4 exists. `push_icon`'s sink signature exists for exactly this.
  Rejected: a local `Vec::new()` per `record`; rejected: `SmallVec`/arena (a new dependency, hook-blocked); rejected: testing only through `record` (GPU-gated assertions for pure arithmetic); rejected: a header label "for balance" (D1).

### D13: Two fields on a sibling's struct have **no assigned owner** — the diff is sized exactly and escalated
- **Status**: **provisional** — the resolution is a human decision, not a technical one
- **Rationale**: `sidebar-row-redesign` reserves `SidebarGeom.list_top` and `.scroll_px`, hardcodes both to `0.0`, and states this feature *"fills two fields … changing nothing this feature defines"* (its D13, `:413`). But the code that **constructs** the geom is in `crates/renderer/src/sidebar_row.rs`, and the struct that would carry the values across is `SidebarModel` in the same file — **both outside this feature's declared surface**, and that feature lands **first**, so it cannot import a module that does not yet exist. The manifest's partition assigns the connecting edit to neither feature. Without it, this feature's header is drawn opaquely over a first card that never moved, and scrolling moves nothing.
  Not resolved by duplicating the placement math here (SD2's central prohibition) nor by guessing. Escalated, with the diff sized so the human decision is cheap:
  1. `SidebarModel` (in `sidebar_row.rs`) gains `list_top: f32` and `scroll_px: f32` — the two of the three fields its own comment already reserves that this design actually uses. (`collapsed` is **not** taken: D3 claims no source of truth, and a stored copy would create one.)
  2. The `SidebarGeom` construction in that layer's `record` reads them from the shared model instead of the two literal `0.0`s.
  **Four lines, one file, no behaviour change while the fields are zero** — which is what they are until this feature's layer is registered (which `sidebar-interaction` D4 does, after both). The cheapest resolution: fold the four lines into `sidebar-row-redesign`'s own implementation, where the change is inert and its spec already anticipates it in a struct comment.
  This feature writes `list_top` once per frame from `record` through `TabModelHandle::shared()`, under the inherited borrow discipline; `sidebar-interaction` writes `scroll_px` (its D10).
  **Everything else this feature depends on from that sibling is verified, not assumed** — `TAB_BAR_Z` retained, the `left_inset <= 0.0` early return retained, `card_h` published, panel background and border drawn. The remaining exposure is these four lines.
  Rejected: duplicating row placement here (guaranteed drift, two card positions in one binary); rejected: editing `sidebar_row.rs` from this feature unilaterally (the partition was approved at the human's single gate); rejected: shipping without it (the feature's headline behaviour would be visibly broken).

### D14: The `lib.rs` edit is **exactly two `pub mod` lines**, and this feature registers nothing
- **Status**: resolved
- **Rationale**: `crates/renderer/src/lib.rs` is an **ordered-shared** surface: `sidebar-foundations` spent its budget there (three `pub mod` lines, `Renderer.sidebar`, `scale_factor()`, two `FrameCtx` fields, the `headless()` hoist), and `sidebar-row-redesign` spends more (`pub mod sidebar_row;`, `sync_left_inset()`, the `set_left_inset` body, its two call sites, one test rewrite). This feature needs **nothing** beyond visibility: `pub mod sidebar_layout;` and `pub mod sidebar_header;`. Every decision above was shaped partly to keep it that way — collapse read from an existing field and passed as an argument (D3), scroll passed as a parameter (D6), chrome state on a handle (D11).
  Registration happens in `crates/app/src/terminal_app.rs`, `sidebar-interaction`'s surface — `add_layer`'s own doc says *"no edit to this file is required to add a layer"* (`lib.rs:451-452`) — and that feature has accepted it (its D4, `:302`), noting *"the whole set's collapse feature is dark until this feature ships"*. So this feature ships a layer that is constructible and tested but **registered by nobody**, exactly as foundations shipped a primitive nobody drew. `SidebarChromeLayer::new()` returns `(Self, SidebarChromeHandle)`, matching `TabBar::new()`'s tuple shape (`terminal_app.rs:134`) — the app cannot write the cursor into a handle it was never given.
  Rejected: a `Renderer::set_sidebar_scroll` accessor (renderer state duplicating an app value, plus churn on a shared surface); rejected: registering from `lib.rs` (against `add_layer`'s documented contract); rejected: `new() -> Self` alone (leaves the handle unreachable — an intermediate draft's error).

### D15: Tokens the mock defines for surfaces only this feature draws live in one documented `tokens` block
- **Status**: resolved
- **Rationale**: Three values are absent from foundations' enumerated token sets: the restore button's fill `#2c2822` and border `#3b362e` (`SidebarTheme` lists neither), and a **render size for the header-button icons**. That last claim is narrowed from an over-broad earlier version: `SidebarMetrics` *does* carry `mark_px: 13`, used as an icon render size for the `✳`/`◇` marks (foundations D8) — but that is the marks' size, not the buttons'. `sidebar_theme.rs` is **outside this feature's declared surface**, so none can be added where they belong.
  They live in one `mod tokens` block at the top of `sidebar_layout.rs`, each doc-commented with the mock value and the note that it belongs in `SidebarTheme`/`SidebarMetrics`: `RESTORE_BG = 0x2C2822`, `RESTORE_BORDER = 0x3B362E`, `RESTORE_TOP = 12.0`, `RESTORE_LEFT = 14.0`, `RESTORE_PAD = 8.0`, `RESTORE_RADIUS = 9.0`, `ICON_PX = 16.0` — all logical px, multiplied by `scale` at the single point of use.
  **`ICON_PX = 16.0` is a judgement, not a transcription.** The mock gives icons a 24-unit viewBox and 1.6–2.0 strokes, but the render size was not captured in foundations' research. 16 makes `button_pad: 6` produce a 28px button and leaves a 136px `search_slot` at the clamped 260px minimum (D1's shown arithmetic). Flagged in Known Gaps as the one number a human with the mock open should confirm.
  Confining them to one block with a stated destination is the least-bad option, but it *is* a second home for design tokens — the drift SD2 exists to prevent — so it is a decision, not an oversight. Rejected: editing `sidebar_theme.rs` (out of declared surface); rejected: scattering the constants at use sites (unfindable at migration time); rejected: approximating with existing tokens (`card_bg_active` `#302c26` is not `#2c2822`).

## Constraints
- No new crate dependency — `Cargo.lock` edits are hook-blocked. `wgpu` and `bytemuck` suffice; `sidebar_layout.rs` needs neither.
- No GUI/widget/vector-graphics crate (`.claude/rules/no-gui-crates.md`).
- `renderer` must not import `winit`; `term-core` untouched. This feature contains zero winit types and zero event handling.
- No `#[cfg(target_os = ...)]` branches; no `unwrap()` outside `#[cfg(test)]`.
- The `crates/renderer/src/lib.rs` edit is exactly two `pub mod` lines (D14).
- **No file outside the declared surface is edited.** `tab_bar.rs`, `sidebar_row.rs`, `sidebar_theme.rs`, `sidebar_shapes.rs` and all of `crates/app/**` are off-limits. D13's four-line exception is escalated to a human, not taken unilaterally.
- `row_h` (the card height) and the sidebar width formula are **inputs read from `sidebar-row-redesign`, never recomputed** (SD2, D6).
- `SidebarMetrics::scaled` is applied exactly once, when `SidebarLayoutInput` is built; no function multiplies a metric by a scale factor again (D7).
- Nothing is allocated per frame in `record` (D12); instances reach the GPU through foundations' `upload_resident`, never `create_buffer_init`.
- The header band's fill is **opaque** — a correctness property of D5's clipping, not a style choice.
- Every "what is drawn" assertion goes through the device-free `build_shapes`, so it runs on adapterless CI (D12).
- `RefCell` borrows of the shared `SidebarModel` are short and never held across a call into `sidebar-foundations`, wgpu or the atlas (inherited from `sidebar-row-redesign` D3).
- All three gates green: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

## Known Gaps
- **Four lines in `crates/renderer/src/sidebar_row.rs` have no assigned owner (D13).** Two `SidebarModel` fields plus two `SidebarGeom` assignments. Without them the header draws over an unmoved first card and scrolling moves nothing. First Pre-work item.
- **Two parallel layout type families — flagged BLOCKING by the dependent** (`sidebar-interaction` `:427`). This feature publishes `SidebarLayoutInput` + free functions including `row_rect`/`row_at`; `sidebar-row-redesign` publishes `SidebarGeom`/`SidebarLayout` + `card_rect`/`card_at`. **Both invert over the same stride band by the same `floor()`** (D8), so they agree today — but they are two implementations of one formula in one sidebar, which is what SD2 exists to prevent. It was not chosen; it is what the partition produced by giving two features overlapping geometry. The resolution named by both sides: have this feature's `hit_test` delegate its card step to `SidebarLayout::card_at`, which requires `SidebarLayoutInput` to carry a `&SidebarLayout` — **blocked today** because no layout is published while collapsed (D7), so it needs the collapsed path split out first. `coherence-audit` should arbitrate; a human should decide whether to re-cut the partition instead.
- **`SidebarLayoutInput`'s `metrics` field is `pub` and the raw literal is the app's constructor (D7).** A caller can pass unscaled metrics and every rect will be half-size on a retina Mac. It was accepted because a private field made the type unconstructible from `crates/app`. The dependent's D3 (`:293`) names the fix — publish the exact input the layer drew with, on `SidebarChromeHandle`'s second field — as the recommended follow-up.
- **`ICON_PX = 16.0` is a judgement, not a measurement (D15)**, as is D1's right-alignment of the icon row (inferred from a flex-row reading of a mock not checked into this repo). A human with the mock open should confirm both.
- **The inert filter is a defensible-but-debatable reading of SD8 (D2).** Drawn, no hover, no region. A human may prefer not drawing it until the search field lands; that is a one-line change here and none in the dependent.
- **The restore button occludes ≈15 terminal cells while collapsed (D4).** Accepted with reasoning. Note `sidebar-interaction` `:267` cites this spec's earlier, wrong figure ("~2×2 cells") — a stale citation, not a design disagreement.
- **This spec declines `sidebar-row-redesign`'s suggested `z_order = 11`** (`:889`) in favour of 30 (D4). The reason (11 sits below `IMAGE_Z`) is sound, but the divergence is a cross-spec disagreement a reviewer should confirm.
- **`scroll_to_reveal` can be forgotten.** It is a pure function the app must call on tab change (D6). The dependent mitigates by putting it in `do_tab_action` (`:397`), but nothing here enforces it.
- **Nothing here proves the chrome *looks* right.** Every test is pure geometry, a shape-list assertion or a z-order comparison. First visual confirmation is `/run-term` after `sidebar-interaction` registers the layer (D14).
- **The sibling specs are moving targets.** They were rewritten under this spec twice during authoring (Meta). Every reconciliation above is a point-in-time read; a fourth round could invalidate parts of D2, D7 or D10 again.

## Requirements

### R0: The feature is correct, bounded, and gate-green

#### R0.1: No file outside the declared surface is edited
- **Given**: the completed feature's diff
- **When**: the changed file list is compared against the declared surface
- **Then**: it contains only `crates/renderer/src/sidebar_layout.rs`, `sidebar_header.rs` and `lib.rs` — and if D13's four-line change to `sidebar_row.rs` was made, it carries an explicit human approval reference

#### R0.2: All three gates are green
- **Given**: the completed feature
- **When**: `cargo build`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings` are run
- **Then**: all three exit 0, with no new warnings and no `unwrap()` outside `#[cfg(test)]`

#### R0.3: The layer is constructible but registered by nobody
- **Given**: the completed feature
- **When**: the workspace is grepped for `SidebarChromeLayer`
- **Then**: it is constructed only in this feature's own tests — no `add_layer` call is added anywhere, so the running terminal is byte-identical before and after (D14)

#### R0.4: Every assertion in this spec runs without a GPU
- **Given**: a machine where `crate::headless()` returns `None`
- **When**: `cargo test --workspace` is run
- **Then**: every `sidebar_layout` test, every `build_shapes` test and the z-order test still run and pass — because no requirement here is phrased against `record` (D12) — and the run is green

### R1: The header's box and regions (D1, D15)

#### R1.1: The header height is the mock's box
- **Given**: `SidebarMetrics::DEFAULT` and a scale factor of 1.0
- **When**: `header_h(1.0)` is evaluated
- **Then**: it is `14 + (16 + 2·6) + 12 == 54.0`, and `header_h(2.0)` is `108.0`

#### R1.2: The three icon boxes are right-aligned in the mock's order
- **Given**: an input with `sidebar_w = 340`, scale 1.0
- **When**: `button_rect(HideSidebar, ..)`, `button_rect(NewTab, ..)` and `filter_icon_rect(..)` are computed
- **Then**: `HideSidebar`'s right edge is at `340 − 12`, `NewTab` sits one `header_gap` (8) to its left, the filter one gap left of that, each `28×28`, all with `y == 14`

#### R1.3: The search slot is reserved, non-empty, and never drawn into
- **Given**: any sidebar width in the clamped 260–440 band, at scale 1.0
- **When**: `search_slot_rect` is computed and `build_shapes` is run
- **Then**: its width is at least 128 px (136 at 260 charging two inter-icon gaps), it does not intersect any icon box, and no emitted `SidebarShape` overlaps it

#### R1.4: The regions are disjoint and contained
- **Given**: widths swept across 260–440 and scale factors 1.0 and 2.0
- **When**: the three icon boxes and the search slot are compared pairwise
- **Then**: no two intersect, all lie inside `header_rect`, and `header_rect` lies inside `[0, sidebar_w) × [0, viewport.y)`

#### R1.5: The header/list boundary is exact
- **Given**: any input
- **When**: `header_rect(inp).h` and `list_viewport(inp).y` are compared
- **Then**: they are equal and both equal `header_h(inp.scale)` — no gap, no overlap (D8)

### R2: The header's shapes and hover (D2, D5, D10, D12)

#### R2.1: The band is built opaque in `panel_bg`
- **Given**: an expanded input (`sidebar_w > 0`) with no hover
- **When**: `build_shapes` is called
- **Then**: the first shape is a `panel_bg` `#1e1b18` rounded box with alpha 1.0 covering `header_rect` exactly — opacity being a correctness property of D5, not a style choice

#### R2.2: All three icons are emitted at `icon_fg`
- **Given**: an expanded input with no hover
- **When**: `build_shapes` is called
- **Then**: `Icon::Filter`, `Icon::Plus` and `Icon::Panel` are each pushed once at their box's centre with size `ICON_PX × scale` and colour `icon_fg` `#8f887a`, and no button background is emitted

#### R2.3: Hovering a live button emits its background and brightens its icon
- **Given**: `hover == Some(ChromeRegion::NewTab)`
- **When**: `build_shapes` is called
- **Then**: a `#2a2722` rounded box of radius `button_radius` (8) is emitted behind the new-tab box and its icon uses `icon_fg_hover` `#c9c2b2`, while the other two icons stay at `icon_fg`

#### R2.4: The filter can never be a hover target
- **Given**: a cursor at the exact centre of `filter_icon_rect`
- **When**: `chrome_hit` resolves it
- **Then**: it returns `None`, so no highlight is emitted and the press resolves to `Panel` via `hit_test` — `ChromeRegion` has no `Filter` variant to return (D2)

#### R2.5: The shape buffer is reused, not reallocated
- **Given**: the layer has recorded 100 frames
- **When**: the `Vec`'s capacity is observed across frames
- **Then**: it never shrinks and is allocated at most once after warm-up — `record` begins with `clear()` on a field, not a fresh `Vec` (D12)

#### R2.6: The layer draws no text
- **Given**: `sidebar_header.rs`
- **When**: it is grepped for `atlas`, `glyph_id` and `get_or_insert`
- **Then**: there are no matches — every emitted primitive is a `SidebarShape` (D12)

#### R2.7: The shape builder needs no GPU type
- **Given**: `build_shapes`'s signature
- **When**: it is inspected
- **Then**: it mentions no `wgpu` type and no `FrameCtx`, so every R2/R3/R8 assertion is callable with no device (D12)

### R3: The collapsed mode and the restore button (D3, D4, D7, D15)

#### R3.1: A collapsed input emits only the restore button
- **Given**: `collapsed == true`
- **When**: `build_shapes` is called
- **Then**: no header band and no icon box is emitted; only the restore button's shapes are

#### R3.2: An expanded input emits no restore button
- **Given**: `collapsed == false` with `sidebar_w == 680.0`
- **When**: `build_shapes` is called
- **Then**: the band and three icons are emitted and the restore button is not

#### R3.3: The restore button matches the mock
- **Given**: scale 1.0
- **When**: `restore_button_rect(1.0)` and the emitted shapes are inspected
- **Then**: the rect is `[14, 12, 32, 32]`, filled `#2c2822` with a `1px` `#3b362e` border and radius 9, carrying `Icon::Panel` at `icon_fg`

#### R3.4: The restore button is reachable with no published card layout
- **Given**: a session that started collapsed, so `SidebarModel.layout` has always been `None` and `row_count == 0`
- **When**: `hit_test((20, 20), 0.0, &inp, true)` is called
- **Then**: it returns `Some(Chrome(Restore))` — because `restore_button_rect` takes only a scale and depends on no card geometry (D7), so collapse is not a one-way trip

#### R3.5: Every other point is the terminal's while collapsed
- **Given**: a collapsed input
- **When**: `hit_test` is called at `(200, 300)` and at `(2, 2)`
- **Then**: both return `None`, so the press reaches whatever handles the grid (D9 step 1)

#### R3.6: The restore button does not resolve while expanded
- **Given**: an expanded input with `sidebar_w == 680.0`
- **When**: `hit_test((20, 20), 0.0, &inp, false)` is called
- **Then**: it returns a header region or `Panel`, never `Restore`

#### R3.7: The layer's z-order sits above the cards and images, below overlays
- **Given**: `SIDEBAR_CHROME_Z`
- **When**: it is compared with the other layer constants
- **Then**: `SIDEBAR_CHROME_Z > tab_bar::TAB_BAR_Z`, `> image_layer::IMAGE_Z`, and `< overlay_layer::OVERLAY_Z` — asserted in a test following `image_layer.rs:390-393` (D4)

### R4: The overflow model is total (D6)

#### R4.1: A list that fits cannot scroll
- **Given**: 3 rows whose total stride height is less than the list viewport
- **When**: `max_scroll(inp)` and `clamp_scroll(500.0, inp)` are evaluated
- **Then**: both are `0.0` — no rubber band, no overscroll

#### R4.2: A list that overflows clamps to its content
- **Given**: 40 rows of `row_h` 69 (stride 75) in a 600px-high list viewport
- **When**: `clamp_scroll(99999.0, inp)` and `clamp_scroll(-50.0, inp)` are evaluated
- **Then**: they are `40·75 − 600 == 2400.0` and `0.0` respectively

#### R4.3: A non-finite offset becomes zero
- **Given**: `f32::NAN` and `f32::INFINITY`
- **When**: each is passed to `clamp_scroll`
- **Then**: both return `0.0`, so no NaN reaches `SidebarGeom.scroll_px` and no row is placed at a NaN y

#### R4.4: An empty list is a valid input
- **Given**: an input with `row_count == 0`
- **When**: `max_scroll`, `content_height` and `clamp_scroll(10.0, inp)` are evaluated
- **Then**: all are `0.0` and none panics or divides by zero

#### R4.5: Revealing a row below the fold scrolls the minimum
- **Given**: 40 rows at stride 75, viewport 600, current scroll `0.0`, target index 20
- **When**: `scroll_to_reveal(20, 0.0, inp)` is evaluated
- **Then**: it returns `21·75 − 600 == 975.0` — the row's bottom flush with the viewport's, not centred and not maximal

#### R4.6: Revealing a row above the fold scrolls to its top
- **Given**: 40 rows at stride 75, current scroll `2000.0`, target index 3
- **When**: `scroll_to_reveal(3, 2000.0, inp)` is evaluated
- **Then**: it returns `225.0`

#### R4.7: Revealing an already-visible row changes nothing
- **Given**: a row fully inside the current viewport
- **When**: `scroll_to_reveal(i, scroll, inp)` is evaluated
- **Then**: it returns `scroll` unchanged, so a tab switch never jolts a list the user is already reading

#### R4.8: The card height is read, never derived
- **Given**: `sidebar_layout.rs`
- **When**: it is grepped for `card_pad_y`, `title_px`, `meta_px` and any card-height formula
- **Then**: there are no matches — the height enters only as `SidebarLayoutInput.row_h` (SD2, D6)

### R5: Overflow is clipped, and what is hidden is also unclickable (D5, D9)

#### R5.1: A row scrolled above the list viewport is painted over
- **Given**: a scroll offset placing row 0 partly above `list_viewport(inp).y`, with the chrome layer at z=30 and the row layer at z=10
- **When**: a frame is composed
- **Then**: the band's opaque fill covers the portion of row 0 above the boundary, so no card pixel appears inside `header_rect`

#### R5.2: A row scrolled under the header is not clickable
- **Given**: the same scroll offset
- **When**: `hit_test` is called at a point inside `header_rect` that lies over the hidden part of row 0
- **Then**: it returns a chrome region or `Panel` — never `Card(0)` (D9 step 3)

#### R5.3: Overflow past the bottom needs no mechanism
- **Given**: a list whose content exceeds the window height
- **When**: `list_viewport(inp).y + list_viewport(inp).h` is compared with `inp.viewport[1]`
- **Then**: they are equal, so the render target clips the excess and no scissor or stencil is used

#### R5.4: Culling agrees with placement and is optional
- **Given**: any scroll offset
- **When**: `visible_rows(s, inp)` is compared with the set of `i` whose `row_rect(i, s, inp)` intersects `list_viewport(inp)`
- **Then**: the two sets are equal, and a caller that ignores the range renders an identical image — culling is a cost measure only (D5)

### R6: The inverse and ordering properties (D7, D8, D9)

#### R6.1: Each chrome region round-trips through `chrome_hit`
- **Given**: inputs at scale 1.0 and 2.0
- **When**: the centre of `button_rect(NewTab)`, `button_rect(HideSidebar)` and (collapsed) `restore_button_rect` is passed to `chrome_hit`
- **Then**: each returns exactly that variant — the analogue of `slot_y_and_tab_at_are_exact_inverses` (`tab_bar.rs:667-675`)

#### R6.2: Each row slot round-trips through `row_at`
- **Given**: 5 rows at an arbitrary scale, swept over scroll offsets including fractional ones
- **When**: the vertical centre of `row_rect(i, s, inp)` is passed to `row_at`
- **Then**: it returns `Some(i)` for every `i` and every offset (D8)

#### R6.3: The list band has no dead zone
- **Given**: a list whose content fills the viewport
- **When**: `row_at` is evaluated at every integer y from `list_viewport.y` to the bottom of the last row
- **Then**: every call returns `Some(i)` for exactly one `i`, the sequence is monotonically non-decreasing, and there is no `None` hole in the inter-card margins (D8)

#### R6.4: The header never falls through to a row
- **Given**: any scroll offset, including one placing a row under the header
- **When**: `hit_test` is called at every integer y inside `header_rect`
- **Then**: no call returns `SidebarRegion::Card(_)` (D9)

#### R6.5: A point outside the sidebar is not a hit
- **Given**: an expanded input with `sidebar_w = 340`
- **When**: `hit_test` is called at `x = 341` and at `y = −1`
- **Then**: both return `None` (D9 step 2)

#### R6.6: Dead space inside the sidebar is consumed, not leaked
- **Given**: a point inside the header on no button, and a point in the list below the last row
- **When**: `hit_test` is called at each
- **Then**: both return `Some(Panel)`, which `sidebar-interaction` swallows — `None` is reserved for "over the grid"

#### R6.7: The module is pure and device-free
- **Given**: `crates/renderer/src/sidebar_layout.rs`
- **When**: its imports and item list are inspected
- **Then**: it imports no `wgpu` type, declares no `struct` holding mutable state, and every public item is a `pub fn` or a `Copy` data type

#### R6.8: The input is constructible from `crates/app` with no renderer context
- **Given**: `SidebarLayoutInput`
- **When**: a caller with no `FrameCtx`, `Device` or `Queue` writes a struct literal
- **Then**: it compiles — every field is `pub` — so `sidebar-interaction` can assemble it from accessors (D7)

#### R6.9: Metrics are scaled exactly once
- **Given**: every function in the module
- **When**: its body is inspected
- **Then**: none calls `SidebarMetrics::scaled` and none multiplies a metric by `inp.scale` — the scaling happened when the input was built (D7)

### R7: The seam — what each neighbour must do (D3, D6, D10, D11, D13, D14)

#### R7.1: `sidebar-interaction` resolves every pointer event through this module
- **Given**: a left press at physical `(x, y)`
- **When**: its `MouseInput` arm handles it
- **Then**: it calls `sidebar_layout::hit_test(p, scroll, &inp, collapsed)` and matches the returned `SidebarRegion` — computing no region itself (SD2)

#### R7.2: The hit-test is consulted even when the sidebar reserves no width
- **Given**: a collapsed sidebar
- **When**: a press arrives anywhere in the window
- **Then**: `hit_test` is still called with `collapsed = true` rather than gated on `x < sidebar_w`, or the restore button would be permanently unclickable

#### R7.3: `sidebar-interaction` owns the collapse sequence
- **Given**: a press resolving to `Chrome(HideSidebar)`
- **When**: it is handled
- **Then**: the app mutates its live `Config.sidebar.collapsed` and runs `settings::apply` → `apply_config` → `sync_left_inset` → `grid_size()` → `resize_all` → re-clamp — this feature performs none of those steps and contains no winit type

#### R7.4: The app pushes the raw cursor and this layer resolves it
- **Given**: a `CursorMoved` event
- **When**: it is handled
- **Then**: the app writes `Some([x, y])` into `SidebarChromeHandle` undebounced, and this layer calls `chrome_hit` on it — the same function the press path calls (D10)

#### R7.5: The header height reaches the cards through the shared model
- **Given**: an expanded sidebar
- **When**: this layer's `record` runs
- **Then**: it writes `header_h(ctx.scale_factor)` into the shared model's `list_top` in a short borrow, so the row layer's next `SidebarGeom` places row 0 below the header (D13)

#### R7.6: The scroll offset reaches the cards through the shared model
- **Given**: a wheel event over the sidebar
- **When**: `sidebar-interaction` updates the offset
- **Then**: it writes `clamp_scroll(new, &inp)` into the shared model's `scroll_px`, which the row layer reads into `SidebarGeom.scroll_px` — one clamped value, one writer

#### R7.7: A tab switch reveals the active row
- **Given**: an active tab whose row lies below the fold
- **When**: the switch completes in `do_tab_action`
- **Then**: the offset becomes `scroll_to_reveal(active, scroll, &inp)`, so the highlighted card is on screen (D6)

#### R7.8: The handle is reachable at construction
- **Given**: `SidebarChromeLayer::new()`
- **When**: `sidebar-interaction` registers the layer
- **Then**: the call yields `(layer, handle)` so the app can keep the handle it must write the cursor into (D14)

#### R7.9: Borrows of the shared model are short
- **Given**: this layer's `record`
- **When**: its borrow scopes are inspected
- **Then**: each `borrow_mut()` ends before the next begins and none is held across a call into `sidebar-foundations`, wgpu or the atlas — a `RefCell` double-borrow would panic a terminal a user is typing into

### R8: Degenerate geometry never panics and never emits garbage (D6, D8)

#### R8.1: A window shorter than the header still builds
- **Given**: a viewport of `[340, 30]` with a 54px header
- **When**: `list_viewport(inp)` and `build_shapes` are evaluated
- **Then**: the list viewport's height is `0.0` rather than negative, the band is emitted (to be clipped by the framebuffer), and nothing panics

#### R8.2: A zero-size viewport emits nothing
- **Given**: a viewport of `[0, 0]`
- **When**: `build_shapes` is called
- **Then**: no shape is emitted, so `SidebarShapePipeline::draw` returns on the empty slice and no zero-size buffer is created

#### R8.3: A sidebar narrower than the icon row yields no negative slot
- **Given**: a pathological `sidebar_w` of 80 (below the clamp band, reachable only in a hand-built input)
- **When**: `search_slot_rect` is computed
- **Then**: its width is clamped at `0.0` rather than going negative, and the icon boxes either stay inside the band or are dropped by foundations' constructors

#### R8.4: A non-finite scale or width emits nothing
- **Given**: an input whose `scale` or `sidebar_w` is NaN
- **When**: `build_shapes` is called
- **Then**: every derived rect is non-finite and every shape is dropped by foundations' `SidebarShape` constructors, so nothing reaches the buffer and no NaN propagates across the frame

#### R8.5: A scroll offset past the content does not produce a negative row index
- **Given**: an unclamped offset larger than `content_height`
- **When**: `row_at` is called inside the list viewport
- **Then**: it returns `None` rather than an out-of-range index, so a caller that skipped `clamp_scroll` cannot index past the tab list

### R9: The shared surface stays small (D14)

#### R9.1: `lib.rs` gains exactly two lines
- **Given**: the diff to `crates/renderer/src/lib.rs`
- **When**: it is reviewed
- **Then**: it contains only `pub mod sidebar_layout;` and `pub mod sidebar_header;` — no `FrameCtx` field, no `Renderer` method, no test change

## Tasks

### T1: `sidebar_layout.rs` — regions, overflow model, ordered hit-test, and the `lib.rs` mod lines [infra]
- **Fulfills**: R1, R4, R5.3, R5.4, R6, R8.1, R8.3, R8.5, R9
- **Depends on**: (none)

Adds both `pub mod` lines to `lib.rs` (R9.1) with a compiling stub behind `sidebar_header`, then writes `sidebar_layout.rs` in full: the `tokens` block (D15), `Rect`, `ChromeRegion`, `SidebarRegion`, the enumerated `SidebarLayoutInput` with **every field `pub`** (R6.8 — the dependent assembles it by literal), every function in D7's table, and the whole test suite. All pure and device-free (`tab_bar.rs:588-683`). Three traps: no card-height formula may appear here (R4.8); no function may call `scaled` or multiply by `inp.scale` (R6.9); and `restore_button_rect` takes a scale, not an input, so collapse is not a one-way trip (R3.4, D7).

### T2: `SidebarChromeLayer`, `SidebarChromeHandle`, and the device-free `build_shapes` [vertical]
- **Fulfills**: R2, R3.1, R3.2, R3.3, R7.4, R7.8, R7.9
- **Depends on**: T1

Writes the layer: `new() -> (Self, SidebarChromeHandle)` (R7.8), the `Option<SidebarShapePipeline>` slot driven by `SidebarShapePipeline::ensure`, the reused `shapes: Vec<SidebarShape>` field cleared per frame (R2.5), and the pure `build_shapes` core (R2.7) that emits the opaque band, the three icons through `push_icon`, and the hover highlight resolved from the handle's raw cursor via `chrome_hit`. `record` is a thin wrapper: assemble the input, call `build_shapes`, hand the slice to `draw`. The band's alpha is 1.0 by contract, not by taste (D5).

### T3: Collapsed mode, the restore button, and the z-order invariant [vertical]
- **Fulfills**: R3.4, R3.5, R3.6, R3.7, R5.1, R5.2, R8.2, R8.4
- **Depends on**: T2

Adds the `ctx.left_inset <= 0.0` mode switch — the **opposite** guard to the row layer's, since this layer draws more when the inset is zero (D3) — the restore button's shapes from the `tokens` block, and `SIDEBAR_CHROME_Z = 30` with the betweenness test (R3.7). R5.1–R5.2 are verified here because they are properties of the z relationship and the opaque band, which exist only once this task lands.

### T4: The seam — publish `list_top`, document the contract, and resolve D13 [vertical]
- **Fulfills**: R7.1, R7.2, R7.3, R7.5, R7.6, R7.7, R0
- **Depends on**: T3

Writes `header_h(ctx.scale_factor)` into the shared model's `list_top` (R7.5) and documents R7.1–R7.7 in `sidebar_layout.rs`'s module doc — the one place `sidebar-interaction`'s implementer will look. **This task begins by checking whether `SidebarModel` actually carries `list_top` and `scroll_px`.** If not, D13's escalation fires: stop, surface the four-line diff to the human, do **not** edit `sidebar_row.rs` unilaterally and do **not** duplicate the placement math here.

**On R0's placement**: R0 is a whole-feature property, not a unit of work. R0.2 (the gates) is run by every task per CLAUDE.md; R0.1 (surface discipline) is most at risk in exactly this task, which is why it is attached here. It is not a disguised final-verify task — holistic verification belongs to the execute pipeline.

**DAG**: T1 → T2 → T3 → T4. Linear, and honestly so: T2–T4 all edit `crates/renderer/src/sidebar_header.rs`, so the L4 parallelism rule's file-overlap condition forbids concurrency. The feature is one pure module plus one layer; a wide DAG would be fiction.

**Horizontal-split justification**: T1 is pure infrastructure with no UI counterpart (the L4 rule's first exception). T2–T4 are vertical in the only sense available to a renderer feature that ships unregistered (D14) — each delivers a complete, testable visual behaviour end-to-end from model to emitted shapes.

## External Dependencies

### Pre-work
- **Assign the four-line diff in `crates/renderer/src/sidebar_row.rs` (D13).** Two `SidebarModel` fields (`list_top: f32`, `scroll_px: f32`) and two `SidebarGeom` assignments reading them instead of literal `0.0`. The partition assigns it to neither feature. Cheapest resolution: fold it into `sidebar-row-redesign`'s implementation, where it is inert while the fields are zero and where its spec already anticipates it in a struct comment. **Blocks T4 and the feature's headline behaviour.**
- **Have `coherence-audit` (or a human) arbitrate the two parallel layout type families** (Known Gaps) — `SidebarLayoutInput` + `row_rect`/`row_at` here versus `SidebarGeom`/`SidebarLayout` + `card_rect`/`card_at` there, two implementations of one stride `floor()`. The dependent already flags it BLOCKING (`:427`). Blocks nothing mechanically; it is the set's clearest SD2 risk.
- **Confirm `ICON_PX = 16.0` and the icon row's right-alignment against the mock** (Known Gaps). The mock is not checked into this repo. Blocks nothing; cheap to correct later.
- **Note for `sidebar-row-redesign`'s implementer**: this spec declines its `z_order = 11` advice (`:889`) in favour of 30, because 11 sits below `IMAGE_Z = 20` and an inline image would bury the restore button (D4). No action needed unless that reasoning is wrong.

### Post-work
- **`/run-term` after `sidebar-interaction` lands.** This feature registers no layer (D14), so its first pixel is drawn by that feature. The header's metrics, the hover treatment and the restore button's placement have no visual confirmation before then.
- **Publish the drawn `SidebarLayoutInput` on `SidebarChromeHandle`'s second field**, making the app's hit-test byte-identical to the draw rather than equal by construction — named by `sidebar-interaction` D3 (`:293`) as the recommended follow-up and the stronger SD2 form.
- **Retire the `tokens` block** (D15) into `SidebarTheme`/`SidebarMetrics` whenever `sidebar_theme.rs` is next open under a spec that owns it.
- **Correct `sidebar-interaction:267`'s stale "~2×2 cells" citation** to ≈15 (D4).
