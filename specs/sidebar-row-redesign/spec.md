# Spec: sidebar-row-redesign

## Meta
- **Created**: 2026-07-20
- **Type**: dev
- **Status**: ready
- **Approved by**: batch mode — the human approved the partition (features, declared surfaces, `depends_on` edges, SD1–SD8) at decompose's single gate; specify's L0/L2/L3/L4 prompts are bypassed per `pre-approved-batch: yes`
- **Approved at**: 2026-07-20
- **Set**: sidebar-redesign (`specs/sidebar-redesign/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single human gate; `pre-approved-batch: yes`)
- **Execution**: NOT handed off. Batch mode's deliverable is this file; `decompose` → `build-order` owns execution.
- **L2 review**: 3 rounds (initial + 2 retries — the skill's maximum). Round 1 returned NEEDS_FIX on 3 BLOCKING + 9 non-blocking source-verified defects; round 2 confirmed all twelve fixes landed and found 2 further BLOCKING defects (`sidebar-chrome-collapse` could not write the `SidebarModel` fields deferred to it; `apply_config` silently reverting the runtime setters round 1 had just added) plus 7 non-blocking. All were fixed. Round 3 returned **PASS** with no blocking items and six non-blocking ones (the sharpest: `record` had two disagreeing readings of the sidebar width once the override existed — `ctx.left_inset` vs `ctx.sidebar.width_px` — and R0.3's enumerated `lib.rs` diff had not been updated to admit the override the round-2 fix mandates). **All six were applied after the retry budget was spent**; like `sidebar-foundations`' round-3 fixes, they are direct transcriptions of the reviewer's own prescribed remedies rather than novel designs, but they are unverified by a further review pass. Ten L2 checkpoints in this spec exist because these reviews put them there. The cross-spec `coherence-audit` at decompose-end is the next independent check — it should pay particular attention to the two cross-feature obligations this spec records in External Dependencies (`sidebar-interaction` owning the `add_layer` registration site for `sidebar-chrome-collapse`'s header, and `tab-metadata`'s read API).

## Declared Surface
- feature-id: sidebar-row-redesign
- depends_on: [sidebar-foundations, tab-metadata]
- declared-surface:
  - crates/renderer/src/tab_bar.rs
  - crates/renderer/src/sidebar_row.rs
  - crates/app/src/sidebar_model.rs
  - crates/app/src/tabs.rs
  - crates/renderer/src/lib.rs
  - crates/app/src/main.rs

## Goal
Replace the current flat one-line-per-tab sidebar with the design mock's rounded two-line **card**: a `13px`-radius, `1px`-bordered box with `12px 15px` padding and `3px 8px` margins, whose first line is an optional agent/ready mark plus the tab title and whose second line (when `show_git_branch`) is a git-branch icon plus the branch name — selected cards brighter and bordered, agent-backed cards carrying their gradient skin, on a sidebar whose width comes from `[sidebar].width_px`.

## Non-goals
- **The header row** (new-tab / hide / filter buttons) and the **collapsed restore button** — `sidebar-chrome-collapse`. This feature reserves `list_top` as a layout input so that feature needs no signature change.
- **Scrolling and clipping** when tabs overflow the viewport — also `sidebar-chrome-collapse`. This feature culls off-viewport cards and reserves `scroll_px` as an input (D13).
- **Wiring clicks, hover tracking, collapse toggling, and the settings/inset call sites in `terminal_app.rs` / `input.rs` / `settings.rs`** — `sidebar-interaction`. This feature renders the hover state and publishes the hit-test geometry; it never touches those three files (D1, D7).
- **Producing** per-tab git/agent data — `tab-metadata`. This feature only *consumes* it through one adapter (D10) and owns the identity→skin mapping (D11, foundations D13).
- **Defining** shapes, icons, colour/metric tokens, the `[sidebar]` config schema, or `linear()` — all `sidebar-foundations`. This feature is their first consumer and adds no token of its own.
- The mock's terminal-pane chrome, the search field, and the agent welcome block (SD8).
- OSC-title support. `Session::set_title` stays `#[allow(dead_code)]`; titles keep coming from the cwd basename.
- A second glyph atlas, a font-family change, or any new text-rasterisation path (D5).

## Confirmed Goal
Rewrite `crates/renderer/src/tab_bar.rs` into a thin compatibility façade over a new `crates/renderer/src/sidebar_row.rs` that draws the mock's card: `SidebarShape` chrome (panel, right border, per-card rounded box + border, mark and git-branch icons) through `sidebar-foundations`' `SidebarShapePipeline`, and two-size text through a new resident-buffer text pipeline built on `crate::build_pipeline` / `crate::QUAD` / `crate::upload_resident`. Card geometry is one pure function returning a `SidebarLayout` that the layer both draws from and publishes, so hit-testing consumes the exact values that were drawn (SD2). The per-tab view model is assembled app-side in a new `crates/app/src/sidebar_model.rs` — `Tabs::titles()` yields `SidebarRow { tab_id, title }`, and `tab-metadata`'s `TabId`-keyed cache reaches the render path through a single generation-gated `MetaSource` adapter installed from `main.rs`, which also owns the agent-identity → `AgentSkin` + mark mapping. The sidebar's width becomes `[sidebar].width_px × scale_factor`, owned by the renderer. Done when the terminal draws the mock's cards, `cargo build` / `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` are green, **no file outside the declared surface is edited**, and the per-frame `create_buffer_init` the old sidebar performed twice a frame is gone.

## Research

### The design source (token inventory inherited from `specs/sidebar-foundations/spec.md` › `## Research`)
- Card (`rowStyle`): `padding: 12px 15px`, `margin: 3px 8px`, `borderRadius: 13px`, `border: 1px solid <border>`, `gap: 6px`, `flexDirection: column`.
- Plain card: bg `transparent` / hover `#2a2722` / selected `#302c26`; border `transparent` unselected, `#46413a` selected.
- Agent skins are 3-stop 135° gradients — `antigravity` bg `rgba(58,134,224,.14) → rgba(122,79,214,.13) @55% → rgba(224,69,58,.11)`, hi `.32/.27/.22`, border `rgba(122,79,214,.45)`, dim `rgba(160,150,220,.16)`; `claude` is 2-stop `rgba(255,157,92,.15) → rgba(214,82,31,.12)` (hi `.32/.26`), border `rgba(224,120,60,.5)`, dim `rgba(224,140,90,.18)`. **The mock defines exactly two skins.**
- Text: title `15px/600` `#e8e2d2`; branch line `12.5px` `#8f887a`; mark `13px`.
- Marks: `agent` = `✳` (U+2733) `#e2d9c8`, `ready` = `◇` (U+25C7) `#7796b8`. `sidebar-foundations` D8 draws both as **icons, not font glyphs**.
- Sidebar panel `#1e1b18`, `borderRight: 1px solid #2e2a24`; `sidebarWidth` default **340**, range 260–440.
- **Not captured in the mock inventory:** the *horizontal* gap between the mark and the title on line 1, and between the branch icon and the branch name on line 2. Only the column `gap: 6px` was recorded (Known Gaps).

### The API this feature consumes from `sidebar-foundations` (its spec is the contract)
- `SidebarShape` + the `rounded_box` / `capsule` / `ring` constructors — the **only** way to build an instance; they clamp radius and border to `min(w,h)/2`, drop non-finite/zero extents, replace a degenerate gradient direction with the 135° default, and clamp `mid` into `[0.05, 0.95]` (foundations D9).
- `SidebarShapePipeline::{new, ensure, draw}` — `ensure(slot, ctx)` because layers are constructed device-free; `draw` returns immediately on an empty slice; instances go through `crate::upload_resident`, **never** per-frame `create_buffer_init` (foundations D3).
- `SidebarTheme` (~15 colours incl. `accent`, unused here) + `SidebarMetrics` in **logical px**, with `SidebarMetrics::scaled(scale)` the only way to physical px — called **exactly once per frame** (foundations D6).
- `AgentSkin { Claude, Antigravity, Generic }`, each resolving `bg` / `hi` / `border` / `dim`; `Generic` resolves to the plain-card treatment (foundations D6/D13).
- `Icon` + `push_icon(&mut Vec<SidebarShape>, icon, x, y, size, color)` — a **sink**, never a returning `Vec` (foundations D7).
- `pub fn linear(packed: u32, alpha: f32) -> [f32; 4]` in `sidebar_theme.rs` — the surviving copy of `tab_bar.rs:578` (foundations D6).
- `FrameCtx.sidebar: &SidebarConfig` and `FrameCtx.scale_factor: f64`; `Renderer::scale_factor()`; `#[cfg(test)] pub(crate) fn crate::headless()` hoisted to the crate root (foundations D10).

### Today's sidebar — `crates/renderer/src/tab_bar.rs` (683 lines), this feature's to rewrite
- Colour consts `:58-62`; `PAD_X` `:65`; `BORDER_W` `:68`; `TAB_BAR_Z: i32 = 10` `:71`.
- `width_px(cell_w) = (cell_w * 14.0).round().max(1.0)` `:77`; `tab_height_px(cell_h) = (cell_h * 1.5).round().max(1.0)` `:82`; `height_px` `:91` (**dead** — grep-verified, no caller); `slot_y(index, tab_h)` `:98`; `tab_at(x, y, sidebar_w, tab_h, count)` `:107` — the exact inverse of `slot_y` (SD2).
- `TabBarModel { titles: Vec<String>, active: usize }` `:30-36`; `TabModelHandle(Rc<RefCell<TabBarModel>>)` `:42` with `set()` `:47` and a private `snapshot()` `:52`.
- `rects()` `:358` (strip + active highlight + right border); `text()` `:381` — the shared glyph atlas via `ctx.atlas.glyph_id` / `get_or_insert`, left-aligned at `PAD_X`, monospace advance `ctx.cell[0]`, `ascent = ch * 0.8` hardcoded, `truncate` by char count `:583`.
- `record()` `:429` reads `ctx.left_inset` for the sidebar width and **calls `create_buffer_init` TWICE PER FRAME** (`:451-464`) on an app that redraws continuously (`terminal_app.rs:428-430`, the winit#2640 workaround). `upload_resident` (`lib.rs:748`) is the codebase's own fix for exactly this.
- Private helpers that die with the file: `QUAD` `:121`, `Vertex` `:114`, `Uniforms` `:130`, `RECT_WGSL`, `TEXT_WGSL`, `PREMULTIPLIED` `:499`, `uniform_entry` `:512` (already `VERTEX_FRAGMENT`), `build_pipeline` `:525`, `linear` `:578`, `truncate` `:583`. The crate-root twins (`lib.rs:58, 101, 748, 935, 949`) are private-at-crate-root and therefore visible to descendant modules — `sidebar_row.rs` calls them as `crate::build_pipeline(...)` with no visibility change (foundations D11 established this).
- `ensure_gpu` `:242` rebuilds only on a **format** change — so a live font-size change (`apply_config` replaces `Renderer.atlas` wholesale, `lib.rs:545-560`) leaves the text bind group pointing at the **old atlas texture**. A latent bug in the code this feature replaces (D9).

### The out-of-surface callers that must keep compiling (grep-verified — these are the whole compatibility problem)
- `crates/app/src/terminal_app.rs:17` — `use renderer::tab_bar::{self, TabBar, TabBarModel, TabModelHandle};`
- `:132-135` — `let bar_w = tab_bar::width_px(renderer.cell_size().0); renderer.set_left_inset(bar_w); let (bar, tab_model) = TabBar::new(); renderer.add_layer(Box::new(bar));`
- `:218-222` — `fn sync_tab_model(&mut self) { self.tab_model.set(TabBarModel { titles: self.tabs.titles(), active: self.tabs.active_index() }); }` — **a struct literal**, so `TabBarModel` may neither gain nor lose a field, and both field names are frozen.
- `:368-373` — `tab_target(inner.last_cursor, inner.renderer.cell_size(), inner.renderer.left_inset(), inner.tabs.len())`.
- `crates/app/src/input.rs:51-64` — `tab_target` delegates to `tab_bar::tab_height_px(cell.1)` + `tab_bar::tab_at(x, y, sidebar_w, tab_h, count)`.
- `crates/app/src/settings.rs:28` — `renderer.set_left_inset(tab_bar::width_px(renderer.cell_size().0));` **after** `renderer.apply_config(cfg)`.
- **All four files (`terminal_app.rs`, `input.rs`, `settings.rs`, plus `session.rs`) are outside this feature's declared surface.** `terminal_app.rs`, `input.rs` and `settings.rs` belong to `sidebar-interaction`; editing them here would be a declared-surface overlap and a `coherence-audit` BLOCK.

### The app side
- `crates/app/src/tabs.rs:138-151` — `titles() -> Vec<String>`, derived from the cwd basename (`Session::cwd()` → a `libc::proc_pidinfo` syscall per tab per call, `session.rs:142`), falling back to `Tab {n}`. Its **only** caller is `sync_tab_model`, which runs on tab actions — not per frame.
- `crates/app/src/tabs.rs:16-23` — `Tabs { sessions, active, next_id, fg, bg, proxy }`; `Tabs::new(first, fg, bg, proxy)` is called from `terminal_app.rs`, so its signature is frozen too. Pure index helpers `wrap_next` `:160` / `wrap_prev` `:169` / `active_after_close` `:179` with tests at `:191, :203`.
- `crates/app/src/session.rs:115,122` — `Session.title` / `set_title` exist but are `#[allow(dead_code)]`; `Session.id: TabId` (`TabId(u64)`) is the stable key `tab-metadata` is specified to cache against (SD4).
- `crates/app/src/seam.rs` — the SD4 extension seam. `App::register_overlay` / `register_handler` are callable from `main.rs`; `Overlay::poll()` is driven by `UserEvent::OverlayPending` → `overlays.poll_all()` (`terminal_app.rs:333-336`) — the **only** generic background-data hook reachable without editing `terminal_app.rs`. `AppCtx` exposes `tab_count` / `active` / `active_cwd` but **not** the tab list, and `Overlay::poll(&mut self)` receives nothing at all — so no seam hook can see the per-tab inventory.
- `crates/app/src/main.rs:10-18, 44-46` — the `mod` list and the three `register_*` calls. This feature's only edits here are one `mod sidebar_model;` line and one installation call (D10).

### Renderer plumbing
- `FrameCtx` (`lib.rs:113-131`) + `trait DrawLayer { z_order, record }`; z-order glyph=0 < tab bar=10 < image=20 < overlay=100. `layers.sort_by_key(|l| l.z_order())` (`:717`) is a **stable** sort, so equal z-orders keep registration order.
- `Renderer::set_left_inset` (`:508`) → `self.left_inset = px.max(0.0)` + `write_uniforms()`; `left_inset()` `:514`; `grid_dims` `:1021-1033` subtracts it. `lib.rs:1123` `set_left_inset_clamps_and_narrows_grid` asserts the clamp and the narrowing on a live `Renderer`.
- Existing layers bake the inset into their own geometry (`cursor_layer.rs:247`, `image_layer.rs:378`); the sidebar draws at `x = 0` and must **not** (`tab_bar.rs:155-175`'s px→clip has no inset term).
- `Renderer::apply_config` (`:526-560`) re-derives `cell_size`, `ppem = font.size_pt * scale_factor`, and **replaces the whole `Atlas`**. `Renderer.font: Arc<Vec<u8>>` (`:161`) is private with **no accessor** — a second atlas would need a new `lib.rs` accessor (D5).
- `crates/renderer/src/atlas.rs` — one texture at ONE `ppem`. `glyph_id(ch) -> u16` `:122` (0 = `.notdef`), `get_or_insert(queue, gid)` `:129`, `get_or_insert_styled(queue, GlyphKey { id, bold, italic })` `:133` (**synthesized bold is already supported** — the mock's `600` weight needs no new machinery), `GlyphEntry { uv0, uv1, px_w, px_h, left, top }` `:36`, `generation()` `:114` (bumped on eviction only — a *fresh* `Atlas` restarts at 0, so generation alone cannot detect the `apply_config` swap), `view()`/`sampler()` `:107,110`, `FilterMode::Linear` on both min and mag `:89-90`.
- `upload_resident` (`lib.rs:748-770`) — grow-only, `queue.write_buffer`, returns 0 and touches nothing for an empty slice.
- `build_pipeline` (`lib.rs:949`) already hardcodes vertex buffer 0 as the `Float32x2` corner; `uniform_entry` (`:935`) is `ShaderStages::VERTEX` only, so a fragment-stage uniform needs a local descriptor (foundations D11 hit the same wall).

### Constraints found in project docs
- `.claude/rules/no-gui-crates.md`, `.claude/rules/dependency-direction.md` (renderer never imports `winit`; `term-core` untouched), `.claude/rules/macos-only.md` (no `#[cfg(target_os)]`), `.claude/rules/zsh-local-only.md`.
- `CLAUDE.md` — `thiserror` errors, no `unwrap()` outside tests; gates `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.
- `.claude/settings.json` — PostToolUse `rustfmt`; PreToolUse **blocks `Cargo.lock` edits** → no new crate dependency.
- Test idiom: pure geometry asserted headlessly (`tab_bar.rs:588-683` — `slot_y_and_tab_at_are_exact_inverses`, `truncate_is_char_safe`); `crate::headless()` for guarded GPU tests.
- **`crates/app/src/input.rs:122-137` contains a *test*, not just a call** — `tab_target_maps_click_to_tab_and_delegates_geometry` asserts `tab_target((40.0, 65.0), (8.0, 20.0), 160.0, 3) == Some(2)` with `tab_h = tab_height_px(20.0) = 30`. It is out of surface and it runs in `cargo test --workspace`, so the compatibility contract covers **behaviour**, not merely compilation (D7).
- `crates/renderer/src/lib.rs:1123` `set_left_inset_clamps_and_narrows_grid` asserts `left_inset() == 0.0` for a default `Config` — it *must* be rewritten by D4. `grid_dims` floors cols/rows at ≥1 (`:1029, :1031`), so `draw_to_skips_reshape_on_unchanged_snapshot` (`:1328`, a 256px-wide headless renderer that would now carry a 340px inset) still yields a drawable 1-column grid and asserts only `reshape_count` — expected to pass unchanged (R0.3).
- **`thread_local!` isolation is NOT guaranteed by the test harness.** libtest spawns a thread per test only in the parallel default; under `--test-threads=1` / `RUST_TEST_THREADS=1` tests run serially on the caller's thread and a `thread_local!` singleton **leaks between them** (D3).

## L2 Checkpoints

Complexity: **Complex (4 signals)**
- *3+ independent subsystems*: card layout geometry · two GPU pipelines (foundations' shape pipeline + a new text pipeline) · the app-side view-model assembly + metadata adapter · the hit-test/compatibility bridge.
- *Multi-actor interaction*: four independent writers to one sidebar model — `terminal_app`'s `sync_tab_model`, the layer publishing its own layout, and (in siblings) a hover index and a scroll offset. Counted **once**, not twice: an earlier draft also counted `tab-metadata`'s off-thread cache as a separate "async resources" signal, which double-counts the same actor from two angles and would contradict `sidebar-foundations` (`:93`), which declined to count consumers as concurrent actors.
- *Data-schema design*: `SidebarRow` / `SidebarModel` / `SidebarLayout` / `RowMeta` are new contracts consumed by `sidebar-chrome-collapse` and `sidebar-interaction`.
- *Cross-feature contract surface*: three files driving this feature are owned by a sibling that has not been built, so every public name, signature **and out-of-surface test assertion** is frozen (D1, D7).

Not counted: external resource loading (the font/atlas already exist), state machine (selection is an index, not a machine).

Brownfield adjustment applied: Core 30% · Scope 20% · Error 25% · Data 15% · Implementation 10%.

### Core Behavior (30%) — 8/8 resolved
- [x] What a card looks like and how its states resolve → D12
- [x] **Two text sizes from a single-ppem atlas** (the feature's biggest unknown) → D5
- [x] Card height: fixed vs content-dependent → D6
- [x] Where card geometry lives and how draw and hit-test stay identical → D7
- [x] Title truncation in px space → D8
- [x] What `TabBarModel` becomes, given a frozen struct literal in an out-of-surface file → D2
- [x] Where the sidebar's width comes from and who owns the inset → D4
- [x] How *fresh* metadata reaches a frame without per-frame I/O or allocation → D10

### Scope Boundaries (20%) — 9/9 resolved
- [x] **Every field or method a sibling must WRITE exists on a surface that sibling can reach** → D3, D4, D13 *(added by the L2 re-review: `hovered` was added for this reason while `list_top`/`scroll_px` were deferred to a feature that owns neither `sidebar_row.rs` nor a write path — the same defect D4 had already fixed for `sidebar-interaction`)*
- [x] **No file outside the declared surface may be edited** → D1 (the constraint that shapes half this spec)
- [x] **Out-of-surface *tests* must keep passing, not just compiling** → D7 *(added by the L2 review: `input.rs:122-137` asserts a concrete `tab_at` result, and `input.rs` is `sidebar-interaction`'s — which builds **last**, so a red workspace would persist across two features)*
- [x] Header / collapse / restore button belong to `sidebar-chrome-collapse` → D13
- [x] Scrolling and clipping belong to `sidebar-chrome-collapse`; overflow must still not misdraw → D13
- [x] Clicks, hover tracking, and the `settings.rs` inset call site belong to `sidebar-interaction` → D1, D4, D12
- [x] `tab-metadata` produces the data; this feature only consumes it → D10
- [x] The agent-identity → `AgentSkin` mapping is **this feature's**, per foundations D13 → D11
- [x] `term-core` untouched and tab-free; no `GridSnapshot` change → SD3

### Error / Edge Cases (25%) — 14/14 resolved
- [x] **A runtime setter followed by an unrelated config reload** (`apply_config` reassigns `Renderer.sidebar` wholesale, per foundations D10) → D4 *(added by the L2 re-review)*
- [x] **A runtime `set_sidebar_width` bypassing foundations D2's 260–440 clamp** → D4 *(added by the L2 re-review)*
- [x] Zero / negative sidebar width, and `collapsed = true` → D4, D14
- [x] **A negative / non-finite `width_px` reaching the inset** now that D4 removes `set_left_inset`'s `max(0.0)` guard and foundations D2's clamp is bypassable via `toml::from_str` → D4 *(added by the L2 review)*
- [x] **A hit-test before the first frame publishes a layout** → D7 *(added by the L2 review — this is what breaks `input.rs`'s test)*
- [x] **The `thread_local!` singleton leaking between tests under `--test-threads=1`** → D3 *(added by the L2 review)*
- [x] A card taller than the viewport, or more cards than fit → D13
- [x] `active` out of range (a stale index after a close) → D12
- [x] A tab with no branch while `show_git_branch = true` → D6
- [x] A title longer than the card, and a title of CJK / combining characters → D8
- [x] A missing ellipsis glyph in the user's font → D8
- [x] The glyph atlas being **replaced** under a cached bind group (live font-size change) → D9
- [x] `meta` and `rows` disagreeing in length or id (a tab closed between assembly and draw) → D10
- [x] A `RefCell` double-borrow, **including reentrancy through `MetaSource::fill` back into `shared()`** → D3 *(the reentrancy half added by the L2 review)*
- [x] **Two `TabBar` layers writing `SidebarModel.layout` on one thread** → D3 *(split out by the L2 review; resolved as last-writer-wins plus the statement that exactly one is registered)*

### Data Model (15%) — 10/10 resolved
- [x] **`TabModelHandle`'s public method set — including the `with` / `with_mut` closures that are the only borrow entry points** → D3 *(added by the L2 re-review: R6.5 implied a `borrow()` while R6.8 implied a `set_meta_source`, and D3 pinned neither)*
- [x] `SidebarRow` shape and who writes it → D2
- [x] `RowMeta` shape (branch, mark, skin) and **who owns the buffer it is written into** → D10 *(corrected by the L2 review: the layer owns it, not the model — otherwise `fill` needs a simultaneous `borrow` + `borrow_mut`)*, D11
- [x] `SidebarModel` — the single bus, and the fields siblings will add → D3
- [x] **`meta_source` has a home on the model** → D3 *(added by the L2 review: D10's install call had no field to write to)*
- [x] **The second pull gate — how a *row* change is detected without an O(n) `String` compare per frame** → D10 *(added by the L2 review: `rows_rev`)*
- [x] `SidebarLayout` — the published draw/hit-test contract → D7
- [x] Text instance layout + the atlas bind group's rebuild key → D9
- [x] `tab_id: u64` rather than an app `TabId` (SD3 — plain data across the boundary) → D2
- [x] Which foundations tokens map to which card surface, incl. the unassigned `dim` → D12

### Implementation (10%) — 5/5 resolved
- [x] Reuse the crate-root `build_pipeline` / `QUAD` / `upload_resident`; local `VERTEX_FRAGMENT` uniform entry → D9 (`assumed: true`, foundations D11 precedent)
- [x] Pure-geometry `#[cfg(test)] mod tests`, guarded `crate::headless()` for GPU → L1 `tab_bar.rs:588-683` (`assumed: true`)
- [x] Inline `const WGSL` rather than a `.wgsl` file (no `.wgsl` in this declared surface) → D9
- [x] No new crate dependency (`Cargo.lock` hook-blocked) → Constraints
- [x] The per-frame `create_buffer_init` debt is paid off, not reproduced → D9, D14

**Composite: 1.00** (46/46 resolved; every dimension ≥ 0.60). Unknown/unknowns: 0 pending.

> Scoring honesty note, inherited from `sidebar-foundations`: a 1.00 means "no checkpoint I can currently name is open", not "no gap exists". The L2-reviewer, not this table, is the gate — and it earned that framing twice here. This table first reached 1.00 before Tier-1 found that `Overlay::poll()` cannot see the tab list (which forced D3 and D10 into their current shape); reached 1.00 **again** before L2 review round 1 found six real defects (an out-of-surface *test* D7 would have turned red across two features, a runtime-collapse write path D4 had removed and then explicitly forbidden, a `RowMeta` buffer D3 and D10 each claimed to own, a missing `meta_source` field, a `thread_local!` isolation claim false under `--test-threads=1`, and an unguarded non-finite `width_px`); and reached 1.00 a **third** time before round 2 found two more (the same unwritable-field defect repeated for `sidebar-chrome-collapse`, and `apply_config` silently reverting the very setters round 1 had just added). Ten of the checkpoints above were added by those reviews. The lesson is recorded rather than smoothed over: this table measures *articulated* coverage, and the adversarial pass is what finds the rest.

### Unknown/Unknown Detection

**Tier 1 (Actor).** Actors: the *user* reading the sidebar (D5, D6, D8, D12 ✓); the *draw path* (D7, D9, D13 ✓); *`terminal_app`'s `sync_tab_model`* as a model writer (D2 ✓); *`tab-metadata`'s background worker* as a second, asynchronous writer (initially 0 decisions) → **+1 Core checkpoint**: how does a metadata update that arrives *between* tab actions reach the screen, given `sync_tab_model` only runs on a tab action? Investigating showed `Overlay::poll()` — the only seam hook — receives no arguments and `AppCtx` exposes no tab list, so no seam-driven patch can rebuild rows → resolved by **D10** (a generation-gated pull, not a push). *`sidebar-interaction`* as a third writer (hover index) and *`sidebar-chrome-collapse`* as a fourth (scroll offset, header height) → **+1 Data checkpoint** → **D3** gives them named slots on one model rather than four private channels.

**Tier 2 (Implication).**
- D2 (`TabBarModel.titles` changes element type) → implies `Tabs::titles()`'s return type changes in lockstep, and that its *name* cannot change → D2 states both, and records the naming debt.
- D4 (config-driven width) → implies the app's two `set_left_inset(width_px(cell_w))` call sites become wrong, and **both live outside the declared surface** → **+1 Core checkpoint** → D4 moves inset ownership into `Renderer`.
- D5 (one atlas, two sizes) → implies a per-line advance, a per-line ascent, and a per-line truncation width, none of which the current `ascent = ch * 0.8` / `max_chars` code expresses → D5 + D8.
- D6 (uniform card height) → implies what a two-line card shows when a tab has no branch → D6.
- D7 (publish the drawn layout) → implies a reader that is a *free function* with a frozen signature and no handle parameter → **+1 Data checkpoint** → D3's per-thread singleton is what makes `tab_at` able to read it at all.
- D9 (cached atlas bind group) → implies detecting an atlas **swap**, which `generation()` alone cannot → **+1 Error checkpoint** → D9's composite rebuild key.
- D10 (a `MetaSource` pulled per frame) → implies a `String` clone per tab per frame on a continuously-redrawing loop → Tier 3 below.
- D12 (hover state) → implies a hover *index* this feature does not produce → D12 renders it and leaves `hovered: None` until `sidebar-interaction` sets it.

**Tier 3 (Pair Tension).** Runs — SD4 is an explicit performance constraint, and D3 introduces shared mutable state.
- **D10 × SD4.** A naive `MetaSource::meta_for(id) -> RowMeta` returning an owned branch `String` allocates once per tab per frame at ~60 fps, in the codebase whose G1 idle-CPU debt is why SD4 exists. → D10 gates the pull on a cheap `generation() -> u64` and fills a **layer-owned reused `Vec`**, so a steady state allocates nothing.
- **D3 × D7.** The layer *writes* the published layout inside `record`, while `snapshot()` *reads* the model in the same `record` — a `RefCell` double-borrow panic in a cosmetic layer would kill the terminal. → D3 mandates that every access is a short, non-overlapping `borrow`/`borrow_mut` that returns before the next begins, with no borrow held across a call into foundations or wgpu, and a unit test that drives read-then-write in one `record`.
- **D4 × D13.** `collapsed = true` sets the inset to 0; the layer must then draw nothing at all rather than a zero-width panel (foundations D9 would drop the shapes, but the text path has no such guard). → D14's early return.
- **D2 × D6.** A `Vec<SidebarRow>` written on a tab action and a `Vec<RowMeta>` refreshed asynchronously can disagree in length the instant a tab closes. → D10 joins by `tab_id` with a `RowMeta::default()` fallback, never by index and never by indexing past the end.
- **D5 × D8.** A scaled sub-line has a *different* advance from the title, so one shared `max_chars` would truncate the branch line at the wrong column. → D8's truncation takes the advance as a parameter.
- **D3 × D10** *(added by the L2 review — this is where the panic lived)*. `MetaSource::fill` is a call into **app-crate code** that can itself reach `TabModelHandle::shared()`. If the layer held a borrow across it — or if `meta` lived on the model, requiring `fill(&model.rows, &mut model.meta)` to borrow the same `RefCell` immutably and mutably at once — the result is a `RefCell` panic in a cosmetic layer running ~60×/second. → D3 moves `meta` to layer ownership, extends the borrow rule to cover *any* `MetaSource` call, and requires the `Rc<dyn MetaSource>` to be cloned out and the borrow dropped **before** `fill` runs. R6.5 drives a re-entrant `fill`.
- **D7 × the out-of-surface test suite** *(added by the L2 review)*. `input.rs:122-137` asserts `tab_at` returns `Some(2)` for a concrete point — and it is `sidebar-interaction`'s file, the **last** feature in the build order. A published-layout-only shim returns `None` there and leaves `cargo test --workspace` red across two features. → D7 keeps the legacy uniform formula as the pre-first-frame fallback.
- **D4 × `sidebar-interaction`'s accept-seed** *(added by L2 review round 1)*. D4 removed the only public write path to the inset, while the manifest promises that feature "collapsing returns the reclaimed width to the terminal grid and expanding restores it" — on a declared surface (`input.rs`/`terminal_app.rs`/`settings.rs`/`main.rs`) that **does not include `lib.rs`**. → D4 now adds the runtime setters itself.
- **D4 × foundations D10** *(added by L2 review round 2)*. Foundations D10 has `apply_config` execute `self.sidebar = cfg.sidebar.clone()`, and D4 calls `sync_left_inset()` at the end of it — so setters that mutated `self.sidebar` would be silently reverted by any unrelated config save, and the hidden sidebar would reappear on its own. → D4's setters write a `SidebarOverride` that `apply_config` preserves and `sync_left_inset` consults first, with the precedence stated and R5.6 asserting it.
- **D13 × the partition** *(added by L2 review round 2)*. `sidebar-chrome-collapse` owns `sidebar_header.rs` / `sidebar_layout.rs` / `lib.rs` — **not** `sidebar_row.rs`. Fields "that sibling adds later" are fields it cannot add without a declared-surface overlap, the exact defect D4 had just fixed for the other sibling. → `list_top` and `scroll_px` are declared on `SidebarModel` now, with a setter, and `record` reads them (R10.4).

### Inversion Probe
*"What could make this fail even if every requirement is met correctly?"* — The cards could be pixel-correct and the feature still **ship a broken terminal**, because the three files that drive it (`terminal_app.rs`, `input.rs`, `settings.rs`) are owned by a feature that has not been built yet. Every compatibility decision here (D1, D2, D3, D4, D7) exists to make that impossible: the module path, the six referenced items, both `TabBarModel` field names, `Tabs::new`'s signature and `Tabs::titles`'s name are all frozen; `set_left_inset`'s argument is *deliberately* ignored so the two stale call sites become harmless rather than wrong, with `set_sidebar_collapsed`/`set_sidebar_width` added so the sibling that needs a write path has one on a surface it owns; and the hit-test shims fall back to the *legacy formula* so an out-of-surface **test** keeps passing, not merely compiling. R9, R4.6 and R4.7 assert exactly this.
Residual, and stated plainly: between this feature and `sidebar-interaction`, **clicking still works** (D7's published layout makes the legacy `tab_at` correct for the new geometry) but **hover does nothing** (nobody sets `hovered`) and **collapse is config-only** (no button exists yet). Neither is a regression against today; both are visible incompleteness for one feature's duration.
Second residual: nothing here proves the result *looks* like the mock. `/run-term` is the first eyeball, and this feature is where `sidebar-foundations`' entire visual output is seen for the first time — so an icon-table or SDF error from that feature surfaces *here*.

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

### D1: `tab_bar.rs` becomes a compatibility façade; the implementation moves to `sidebar_row.rs`
- **Status**: resolved
- **Rationale**: Three files reference `renderer::tab_bar` — `terminal_app.rs`, `input.rs` and `settings.rs` — and **all three belong to `sidebar-interaction`**, a feature that builds *after* this one. Editing them would be a declared-surface overlap and a `coherence-audit` BLOCK; not editing them means the module path and every item they name must survive verbatim. So `tab_bar.rs` is rewritten into a ~60-line façade whose entire job is source compatibility, `pub use`-ing from `sidebar_row.rs` (which holds the real implementation, ~600 lines):
  - `TabBar` (the `DrawLayer`) and `TabBar::new() -> (TabBar, TabModelHandle)` — same names, same tuple.
  - `TabBarModel` and `TabModelHandle` — see D2, D3.
  - `TAB_BAR_Z: i32 = 10` — the layer's z-order, unchanged.
  - `width_px(cell_w: f32) -> f32`, `tab_height_px(cell_h: f32) -> f32`, `tab_at(x, y, sidebar_w, tab_h, count) -> Option<usize>` — same signatures; new meanings (D4, D7), and behaviour that still satisfies `input.rs`'s existing assertions (D7's fallback).
  The façade re-exports; the *canonical* paths are `renderer::sidebar_row::{TabBar, TabBarModel, TabModelHandle, SidebarRow, …}`, and `crates/app/src/main.rs` uses those directly (it is new code, so it need not go through the façade) to reach `TabModelHandle::shared()` and `set_meta_source` (R6.8).
  - `slot_y` and `height_px` are **deleted**: `height_px` is dead (grep-verified) and `slot_y` had no caller outside `tab_bar.rs`'s own tests. `PAD_X`, `BORDER_W` and the five colour consts are deleted — `SidebarMetrics` / `SidebarTheme` own them now (foundations D6).
  The façade is not free: it keeps a module named for a UI element that no longer exists, and three functions whose names now misdescribe them. That cost is bounded and explicitly assigned — `sidebar-interaction` owns all three call sites and deletes the façade when it rewires them (External Dependencies). Rejected: editing the three call sites here (declared-surface overlap — the one thing the partition exists to prevent, and it would silently take work away from a sibling's accept-seed); rejected: keeping the whole implementation in `tab_bar.rs` (the file is `sidebar-chrome-collapse`'s reference point for the card list, and a file named `tab_bar` holding the card renderer would misname the surviving code for the rest of the project's life); rejected: `#[deprecated]` on the façade items (`-D warnings` turns every out-of-surface call site into a build failure, which is exactly the breakage this decision avoids).

### D2: `TabBarModel.titles` changes **element type**, not shape — `Vec<String>` → `Vec<SidebarRow>`, matched by `Tabs::titles()`
- **Status**: resolved
- **Rationale**: The card needs a `tab_id` per row so asynchronous metadata can be joined to it (D10); a title alone cannot carry that. But `terminal_app.rs:219-222` builds the model with a **struct literal** — `TabBarModel { titles: self.tabs.titles(), active: self.tabs.active_index() }` — so the type may not gain, lose or rename a field, and no `Default`/builder trick evades a missing-field error. The one degree of freedom left is the *element type of an existing field*, and both ends of it are inside this feature's surface: `renderer::tab_bar::TabBarModel` and `crates/app/src/tabs.rs`. So:
  ```rust
  pub struct SidebarRow { pub tab_id: u64, pub title: String }          // renderer, plain data (SD3)
  pub struct TabBarModel { pub titles: Vec<SidebarRow>, pub active: usize }
  // app: pub fn titles(&self) -> Vec<SidebarRow>
  ```
  and `sync_tab_model` compiles **byte-unchanged**. `tab_id` is a plain `u64`, not `session::TabId`, because `renderer` must not know app types (SD3, `dependency-direction`).
  The field and method keep the name `titles` although they no longer carry titles. That is a deliberate, documented cost of the frozen call site, not an oversight: renaming either one is a one-line edit in `terminal_app.rs` that `sidebar-interaction` will make, and it is recorded in Known Gaps and External Dependencies so it is not lost. `SidebarRow` deliberately carries **only** what `Tabs` owns — id and title — while branch/mark/skin live in a separate `RowMeta` (D10), so the two writers never contend for the same field.
  `Tabs::titles()` keeps its existing cwd-basename derivation (`tabs.rs:138-151`) including its per-tab `proc_pidinfo` syscall: it is called on tab actions, not per frame, so SD4's no-I/O-per-frame rule is not engaged. Rejected: adding a field to `TabBarModel` (does not compile — missing field at the frozen literal); rejected: `#[non_exhaustive]` (forbids the external literal outright — strictly worse); rejected: a second parallel handle for the ids (three writers on two channels, and the ids could desynchronise from the titles they key); rejected: encoding branch/agent into the title string (stringly-typed, unparseable, and it would break truncation).

### D3: `TabModelHandle` becomes a per-UI-thread singleton over one `SidebarModel` — the bus every party shares
- **Status**: resolved
- **Rationale**: Four parties must see one model, and **only one of them can be handed a handle**. `terminal_app` receives its handle from `TabBar::new()`; but (a) the metadata adapter is installed from `main.rs`, before `Running` exists; (b) `sidebar-interaction` will publish a hover index; (c) `sidebar-chrome-collapse` will publish a scroll offset and a header height; and (d) `tab_at` — the hit-test entry point frozen by D1 — is a **free function with no handle parameter and a signature that cannot change**. There is no seam that closes this: `App` exposes only `register_overlay`/`register_handler`, `Overlay::poll(&mut self)` takes no arguments, and `AppCtx` exposes `tab_count`/`active`/`active_cwd` but never the tab list. Threading a handle would require editing `terminal_app.rs` — forbidden by D1.
  So the model becomes a **per-thread singleton**: a `thread_local!` `Rc<RefCell<SidebarModel>>` inside `sidebar_row.rs`, reached by `TabModelHandle::shared()`; `TabModelHandle::default()` and `TabBar::new()` both return clones of that same `Rc`, so `terminal_app`'s existing wiring is unchanged and every other party calls `shared()`. This is defensible precisely here and nowhere else: the app is single-window and single-UI-thread by construction (`event_loop.run_app` runs on the calling thread), and the renderer's `DrawLayer`s are `!Send` and already communicate through `Rc<RefCell<_>>` (`tab_bar.rs:42`).
  ```rust
  pub struct SidebarModel {
      pub rows: Vec<SidebarRow>,                    // written by terminal_app via set()  (D2)
      pub active: usize,                            //   "
      pub rows_rev: u64,                            // bumped by set(); D10's second pull gate
      pub hovered: Option<usize>,                   // written by sidebar-interaction; None here (D12)
      pub list_top: f32,                            // written by sidebar-chrome-collapse; 0.0 here (D13)
      pub scroll_px: f32,                           // written by sidebar-chrome-collapse; 0.0 here (D13)
      pub layout: Option<SidebarLayout>,            // written by the layer after drawing (D7)
      meta_source: Option<Rc<dyn MetaSource>>,      // installed once from main.rs        (D10)
  }
  // pub fn shared() -> TabModelHandle;  the handle — NOT the bare Rc — is the public reach.
  // TabModelHandle carries: set(TabBarModel) [bumps rows_rev] · set_meta_source(Rc<dyn MetaSource>)
  //   · set_hovered(Option<usize>) · set_list_scroll(list_top, scroll_px) · layout() -> Option<SidebarLayout>
  //   · with<R>(f: impl FnOnce(&SidebarModel) -> R) -> R  and  with_mut<R>(…)  — the ONLY borrow entry
  //   points, each taking and releasing the borrow inside the call so no caller can hold one across
  //   another (D3's discipline becomes structural rather than advisory).
  ```
  **Every field a sibling needs is declared HERE, now.** `hovered`, `list_top` and `scroll_px` are all written by features whose declared surfaces (`input.rs`/`terminal_app.rs`/`settings.rs`/`main.rs` for `sidebar-interaction`; `sidebar_header.rs`/`sidebar_layout.rs`/`lib.rs` for `sidebar-chrome-collapse`) **do not include `sidebar_row.rs`** — so a field "the sibling adds later" is a field the sibling cannot add at all without a declared-surface overlap. An earlier draft deferred `list_top`/`scroll_px` to `sidebar-chrome-collapse` while adding `hovered` for exactly this reason, which was inconsistent. `record` copies `list_top`/`scroll_px` out of the model into `SidebarGeom` rather than hardcoding `0.0`, so a sibling that writes them changes the drawn *and* hit-tested geometry together (R10.3, R10.4) with no edit to this file.
  **`Vec<RowMeta>` is deliberately NOT on this struct — the layer owns it.** An earlier draft put it here, which would have made D10's refresh `fill(&model.rows, &mut model.meta)`: an immutable and a mutable borrow of the *same* `RefCell` at once, i.e. the exact panic the next paragraph exists to prevent. The layer keeps its own reusable `Vec<RowMeta>` and reads `rows` out under a short borrow.
  **Borrow discipline is structural, not advisory**: a `RefCell` double-borrow panics, and this layer runs ~60×/second in a terminal a user is typing into. `with` / `with_mut` are the **only** borrow entry points and each takes and releases its borrow inside the call, so no caller can hold one across another — the rule is enforced by the API rather than by convention. The one thing still forbidden by hand is calling a `MetaSource` *inside* a `with` closure, and that is the dangerous case precisely because it is not hypothetical: `MetaSource` is implemented in the *app* crate (D10) and can itself reach `TabModelHandle::shared()`. So `record` copies what it needs out first — `rows`, `active`, `rows_rev`, `list_top`, `scroll_px`, and a clone of the `Rc<dyn MetaSource>` — in one `with`, and calls `fill` **after** that closure returns. `record`'s shape is therefore: `with` (copy out) → refresh meta → build `SidebarGeom` from `ctx.left_inset` + the copied `list_top`/`scroll_px` → draw → `with_mut` (publish `layout`). R6.5 drives exactly this, including a `MetaSource` whose `fill` re-enters `shared()`.
  **Test isolation is explicit, not inherited from the harness.** An earlier draft claimed `cargo test` isolates the singleton because each `#[test]` gets its own thread. That is only true under the parallel default: with `--test-threads=1` or `RUST_TEST_THREADS=1`, libtest runs tests serially on the caller's thread and the singleton — including a published `layout` and an installed `MetaSource` — leaks between them, making R4.6 and R6.3 order-dependent. So `sidebar_row.rs` provides `#[cfg(test)] pub(crate) fn reset_shared_model()`, and **every test that reads or writes the singleton calls it first**.
  **Two `TabBar` layers on one thread** would both publish `layout`; the rule is last-writer-wins, and since `layers.sort_by_key` is stable that means registration order. Exactly one `TabBar` is registered (`terminal_app.rs:134-135`), so this is a documented invariant rather than a supported configuration; a second one is a bug, not a mode.
  Rejected: threading a handle through `App`/`Running` (requires editing `terminal_app.rs` — D1); rejected: a `static` + `Mutex` (the layer is `!Send`; a mutex would be per-frame ceremony guarding data only one thread can reach, and it would make a poisoned lock a terminal-killing panic); rejected: leaving `tab_at` on the stale uniform geometry and accepting mis-targeted clicks for one feature (a *user-visible regression* shipped deliberately, and D7's whole point is that draw and hit-test must never diverge); rejected: passing the layout back through `set()` (it flows the wrong way — the layout is produced by the renderer and consumed by the app).

### D4: The renderer owns the sidebar's width; `set_left_inset`'s argument is deliberately ignored
- **Status**: resolved
- **Rationale**: SD7 makes the width `[sidebar].width_px` (logical px, default 340, clamped 260–440 at load by foundations D2), so the physical inset is `width_px × scale_factor` — and **neither of the two call sites can compute it**. `terminal_app.rs:132` and `settings.rs:28` both call `renderer.set_left_inset(tab_bar::width_px(renderer.cell_size().0))`, and `width_px`'s only input is the cell width: the config is not in scope and the scale factor is not recoverable from a cell size without the font metrics. Both files are outside this feature's surface. Leaving them as-is would set the inset to `cell_w × 14` (≈238 physical px at default settings) while the sidebar draws 680 — the panel would cover the terminal's first columns.
  So `Renderer` becomes the owner:
  ```rust
  // sidebar_row.rs — pure, unit-tested
  pub fn sidebar_width_px(cfg: &SidebarConfig, scale: f64) -> f32   // 0.0 when collapsed
  // lib.rs
  // lib.rs — a runtime override that survives apply_config, consulted BEFORE the config
  struct SidebarOverride { collapsed: Option<bool>, width_px: Option<f32> }
  fn effective_sidebar(&self) -> SidebarConfig { /* self.sidebar with the override's Some() fields applied */ }
  fn sync_left_inset(&mut self) { self.left_inset = sidebar_width_px(&self.effective_sidebar(), self.scale_factor); self.write_uniforms(); }
  pub fn set_left_inset(&mut self, _requested_px: f32) { self.sync_left_inset(); }   // compat no-op
  pub fn set_sidebar_collapsed(&mut self, c: bool) { self.sidebar_override.collapsed = Some(c); self.sync_left_inset(); }
  pub fn set_sidebar_width(&mut self, logical_px: f32) { self.sidebar_override.width_px = Some(logical_px.clamp(260.0, 440.0)); self.sync_left_inset(); }
  ```
  **The setters write an override, not the config field — otherwise `apply_config` would silently undo them.** Foundations D10 has `apply_config` execute `self.sidebar = cfg.sidebar.clone()`, and D4 calls `sync_left_inset()` at the end of it. Had the setters mutated `self.sidebar` directly, this sequence would defeat the whole point of adding them: the user hides the sidebar with the button, then saves `config.toml` for any unrelated reason (a theme tweak) → `settings::apply` → `apply_config` overwrites `self.sidebar` with the file's `collapsed = false` → `sync_left_inset` → **the sidebar reappears by itself**, and `settings.rs:28`'s now-no-op `set_left_inset` cannot undo it. So the precedence is stated rather than accidental: **a runtime setter wins over the file for the rest of the session**, `apply_config` leaves the override untouched, and the config supplies whichever fields the override has not claimed. The consequence — after a runtime collapse, editing `[sidebar] collapsed` in the TOML does nothing until restart — is deliberate (the thing the user just clicked should outrank a file they edited for another reason) and is recorded in Known Gaps. R5.6 asserts the sequence, and R5.3 still asserts that a reload moves the width when no override has claimed it.
  **`ctx.left_inset` — not `ctx.sidebar.width_px` — is the sidebar's width inside `record`.** The override creates two readings of the same quantity: `FrameCtx.sidebar` (foundations D10) points at `Renderer.sidebar`, the **pre-override config**, while `left_inset` comes from `effective_sidebar()`. After `set_sidebar_width(260.0)` on a 340 config they disagree — 680 vs 520 physical at 2× — and a layer that built `SidebarGeom.sidebar_w` from `ctx.sidebar` would draw 680-wide cards in a 520-wide band and publish a `card_at` that disagrees with the real band: an SD2 violation manufactured by this decision. So the rule is explicit: **`SidebarGeom.sidebar_w` derives from `ctx.left_inset`** (which is also what `tab_bar.rs:430` does today, `let w = ctx.left_inset;`), and `ctx.sidebar` is authoritative **only** for `show_git_branch` and `show_agent_status`, neither of which the override touches.
  **`set_sidebar_width` clamps to foundations D2's 260–440 band.** That decision titles itself "never observable unclamped" and names `width_px = 100000` as the failure it exists to prevent; a public runtime setter — the obvious backing for a drag-resize in `sidebar-interaction` — is a new door into exactly that, and it does not go through `Config::load_from`. The clamp belongs here and **not** in `sidebar_width_px`, which must keep returning `0.0` for a collapsed or zero width (R5.8).
  `sync_left_inset` is called from `assemble` (so the startup value is right before the first frame) and at the end of `apply_config` (so a live `[sidebar]` edit re-fits the grid through the existing `settings::apply` → `grid_size` → `resize_all` path). The two stale call sites then become **harmless no-ops instead of wrong writes** — which is the entire point: the parameter is ignored *so that* out-of-surface code cannot corrupt the invariant.
  **`set_sidebar_collapsed` / `set_sidebar_width` are added here, and this is not optional.** An earlier draft rejected "a `Renderer::set_sidebar_width` added alongside" on the grounds that nothing outside the surface would call it — which was exactly backwards, and is the strongest argument against D4: neutering `set_left_inset` removes the *only* public write path to the inset, and the manifest promises `sidebar-interaction` that "collapsing returns the reclaimed width to the terminal grid and expanding restores it" on a declared surface (`input.rs` / `terminal_app.rs` / `settings.rs` / `main.rs`) that **does not contain `lib.rs`**. Fixing a call-site defect by deleting an API contract is only defensible when the *future* users are enumerated too, not just the stale ones. So the runtime setters land here, in-surface, and appear in R0.3's enumerated `lib.rs` diff; `sidebar-interaction` then calls `set_sidebar_collapsed` from its hide/restore button and needs no `lib.rs` edit at all.
  The remaining counterargument is real and is bounded rather than dismissed: a public method that silently discards its argument is a footgun, and someone reading `set_left_inset(240.0)` will reasonably expect 240. Four things bound it — the parameter is named `_requested_px`; the doc comment says plainly that the sidebar owns the band, that the argument is retained only for source compatibility, and that `set_sidebar_width` is the real setter; the two remaining callers are enumerated here and in External Dependencies with `sidebar-interaction` named as the feature that deletes them; and after that deletion `set_left_inset` itself can go while `set_sidebar_*` remain. `set_top_inset` is untouched, so the top-inset API keeps its ordinary meaning.
  **`sidebar_width_px` clamps defensively**: `w.max(0.0)` with a non-finite input mapped to the default 340. Foundations D2 clamps `width_px` to 260–440 inside `Config::load_from`, but its own rationale admits `toml::from_str` and a hand-built `Config::default()`-then-mutate both bypass that door — and D4 has just removed `set_left_inset`'s `px.max(0.0)` from the path, so this function is now the last guard before a negative or `NaN` width reaches `grid_dims`.
  `lib.rs`'s existing `set_left_inset_clamps_and_narrows_grid` test (`:1123`) asserts `left_inset() == 0.0` on a default `Config` and is rewritten in place to assert the new contract — a config-driven width narrows the grid, a stale `set_left_inset` cannot move it, and `set_sidebar_collapsed(true)` returns the band. That is the only test edit expected in `lib.rs`. Two other tests were checked and are unaffected: `left_inset_reserves_cols_for_the_sidebar` (`:1086`) calls the pure `grid_dims` directly, and `draw_to_skips_reshape_on_unchanged_snapshot` (`:1328`) now runs a 256px-wide headless renderer with a 340px inset but `grid_dims` floors cols at ≥1 (`:1029`) and it asserts only `reshape_count` — if either does need a change, it joins R0.3's enumerated list rather than being fixed silently.
  Rejected: keeping `set_left_inset` authoritative and correcting the width on the *next* `apply_config` (startup and every non-reload frame would be wrong, and `settings.rs:28` runs *after* `apply_config`, so even a reload would end on the wrong value); rejected: redefining `width_px(cell_w)` to return the config width (it cannot see the config, and returning the unscaled logical 340 renders a half-width sidebar on every retina Mac — the exact failure foundations D1 calls out); rejected: deriving the scale factor from `cell_h / (size_pt × line_ratio)` (font- and config-dependent arithmetic standing in for a value the renderer already holds); rejected: leaving runtime collapse to a `Config` round-trip through `settings::apply` (it would require writing the user's TOML file to toggle a button, and the reload path is debounced and asynchronous).

### D5: Two text sizes from ONE atlas — the title at native ppem, the sub-line scaled by the mock's `meta/title` ratio
- **Status**: resolved
- **Rationale**: **This is the feature's central unknown.** The glyph atlas is rasterised at exactly one ppem (`Renderer.ppem = font.size_pt × scale_factor`, rebuilt only in `apply_config`), and it is the workspace's only text path. The mock asks for a 15px title and a 12.5px branch line. Four options were weighed against the default config (`size_pt = 13`, retina `scale = 2` → `ppem = 26` physical; the mock's 15 logical px = 30 physical, its 12.5 = 25):
  1. *Scale both lines to absolute mock px.* Title `k = 30/26 = 1.15` — an **upscale**, so the largest, most-read text in the sidebar becomes the blurriest thing on screen, and it gets worse as the user lowers `font.size_pt`.
  2. *A second `Atlas` at the title ppem.* Sharpest, but it needs a `Renderer::font()` accessor added to `lib.rs` (the `Arc<Vec<u8>>` is private), a second 1024² R8 texture (+1 MB, ×2 if the sub-line gets its own), a second bind group, and invalidation on every `apply_config` — a lot of machinery, and a third texture for 12.5px would follow.
  3. *One size for both lines.* Kills the two-line hierarchy, which is the visible point of the card.
  4. **Chosen: the title draws at the atlas's native size with no scaling at all, and the sub-line draws scaled by `meta_ratio = metrics.meta_px / metrics.title_px = 12.5 / 15 = 0.8333`.**
  Option 4 wins on three counts. The dominant text is rasterised at exactly the ppem it is drawn at — pixel-sharp, never resampled. The only scaled text is a **downscale** of secondary text through the atlas's already-`Linear` sampler (`atlas.rs:89-90`), which is the benign direction. And the sidebar's type scales with the user's `font.size_pt` instead of pinning 15px regardless — at `size_pt = 20` a hardcoded 15px title would look wrong next to the terminal it labels.
  The cost, stated plainly: at default settings the title renders at 26 physical px where the mock says 30 — **13% smaller than the mock**. SD1 governs this ("the design tokens are honored, not that a screenshot diff is empty"), and what is honored is the *ratio* between the two lines and the hierarchy it creates, which is what the eye reads. The mock's absolute `title_px` / `meta_px` tokens survive **only** as the source of `meta_ratio` — an earlier draft also claimed they fed "the card's padding arithmetic", which is false: D6's card height takes its line boxes from `cell_h` and its padding, gap and border from `SidebarMetrics::scaled`, so `title_px` feeds nothing else. The consequence is that card *proportions* drift from the mock as `font.size_pt` rises (logical-px padding around cell-derived line boxes); D6 states why that is the intended trade.
  Mechanically, per line: `advance = ctx.cell[0] × s`, `line_h = ctx.cell[1] × s`, `ascent = line_h × 0.8` (preserving `tab_bar.rs:404`'s existing constant), and each `GlyphEntry`'s `px_w`, `px_h`, `left`, `top` multiplied by `s` — `s = 1.0` for the title, `meta_ratio` for the sub-line. The title additionally uses `atlas.get_or_insert_styled(queue, GlyphKey { id, bold: true, italic: false })`, since the atlas already synthesizes bold (`atlas.rs:133`) and the mock specifies weight 600; the sub-line uses the plain key. Rejected: options 1–3 above, each for the reason given.

### D6: One card height per frame — the branch line is reserved by config, never by per-tab presence
- **Status**: resolved
- **Rationale**: A tab's branch can appear or vanish at any moment: `tab-metadata`'s worker polls off-thread (SD4) and the user `cd`s in and out of repositories constantly. If card height tracked *per-tab* branch presence, every card below a changing one would shift by ~20px at an arbitrary moment — including in the instant between a user aiming at a card and pressing the button. That is a worse defect than a little empty space, and it is unfixable at the hit-test layer because the movement is genuine.
  So the card is two lines iff `ctx.sidebar.show_git_branch` is true, for **every** card, regardless of whether that tab has a branch; a tab with no branch simply draws no icon and no text on its reserved second line. With `show_git_branch = false` every card is one line and the sidebar is denser. Height is therefore a per-frame constant:
  ```
  card_h = 2*(border_w + pad_y) + line1_h + (show_branch ? gap + line2_h : 0)
  stride = card_h + 2*margin_y
  ```
  with `line1_h = cell_h`, `line2_h = cell_h × meta_ratio` (D5), and every metric from `SidebarMetrics::scaled(ctx.scale_factor)` — called **exactly once per frame**, per foundations D6.
  **Padding, gap, radius and border are therefore font-size-independent by design, while the line boxes are not.** A card at `size_pt = 20` has taller text inside the same 12/15px padding, so it looks proportionally tighter than the mock at 13pt. That is deliberate: the mock's paddings are *chrome* metrics that should stay constant as the window's other chrome does, and scaling them with the font would make a large-font sidebar consume a quarter of the window. R2.5 asserts only that nothing clips or overlaps, because "looks right at every font size" is a judgement for `/run-term`, not an assertion.
  The secondary win is that hit-testing stays a single `floor()` over a uniform stride, so SD2's exact-inverse property survives the redesign intact rather than becoming a search over non-uniform rects. The mock's own flexbox would collapse the empty line; this deliberately does not. Rejected: per-card height (layout jitter driven by an asynchronous writer, as above); rejected: reserving the line only when *some* tab has a branch (the whole list would still jump the moment the first or last branch appears — the same defect, merely rarer and therefore harder to reproduce); rejected: showing a placeholder like `—` on branch-less tabs (invents a token the mock does not have, and adds visual noise to the common non-repo case).

### D7: One pure layout function; the layer draws from it **and publishes it**, and the hit-test reads the published values
- **Status**: resolved
- **Rationale**: SD2 requires draw and hit-test to share one source of truth, and today that is an *inverse-pair* contract (`slot_y` forward, `tab_at` backward). The redesign breaks the pair's precondition: the new stride depends on the scale factor, `show_git_branch` and the cell size, and `tab_at(x, y, sidebar_w, tab_h, count)` — whose signature D1 freezes — can see none of them. `input::tab_target` supplies `tab_h` from `tab_height_px(cell.1)`, which likewise cannot see the config or the scale.
  So the contract is **strengthened rather than weakened**: geometry is computed once, by one pure function, and the layer publishes the exact `SidebarLayout` it drew into the shared model (D3). Hit-testing then consumes the same *values*, not merely the same *formula* — drift becomes impossible rather than merely unlikely.
  ```rust
  pub struct SidebarGeom {   // every input, all physical px
      pub sidebar_w: f32, pub viewport_h: f32, pub cell: [f32; 2],
      pub scale: f64, pub show_branch: bool,
      pub list_top: f32,   // 0.0 here; sidebar-chrome-collapse supplies the header height
      pub scroll_px: f32,  // 0.0 here; sidebar-chrome-collapse supplies the scroll offset
  }
  #[derive(Clone, Debug, PartialEq)]   // Clone is required: TabModelHandle::layout() returns it owned
  pub struct SidebarLayout { pub geom: SidebarGeom, pub count: usize, pub stride: f32, pub card_h: f32, /* … */ }
  pub fn layout(geom: &SidebarGeom, count: usize) -> SidebarLayout;   // pure
  impl SidebarLayout {
      pub fn card_rect(&self, i: usize) -> Option<[f32; 4]>;          // forward
      pub fn card_at(&self, x: f32, y: f32) -> Option<usize>;         // exact inverse
      pub fn visible_range(&self) -> std::ops::Range<usize>;          // D13
  }
  ```
  `card_at` inverts over the **stride band**, not the drawn rect: the mock's `3px` vertical margins are visual, so a click in a margin selects the card whose stride band contains it rather than falling in a dead zone. Horizontally the whole `[0, sidebar_w)` band is live, matching today's behaviour.
  **The shims degrade to the legacy formula rather than to `None`.** `tab_bar::tab_at` reads `SidebarModel.layout`; when one is published it delegates to `card_at` (using `sidebar_w` for the x-bound and `count` as an upper bound, and **ignoring `tab_h`**, whose value the caller can no longer compute correctly). When no layout has been published it falls back to today's `floor(y / tab_h)` with today's bounds checks, and `tab_height_px(cell_h)` returns the published stride when one exists and today's `(cell_h * 1.5).round().max(1.0)` otherwise.
  This is not defensive padding — an earlier draft returned `None` before the first frame, and that would have turned a **test in a file this feature may not edit** red: `input.rs:122-137` asserts `tab_target((40.0, 65.0), (8.0, 20.0), 160.0, 3) == Some(2)`, and no `record` has ever run on a unit-test thread, so `layout` is `None` there by construction. Because `input.rs` belongs to `sidebar-interaction` — the **last** feature in the build order — `cargo test --workspace` would have stayed red across two whole features. The fallback keeps that assertion true, and it degrades honestly: pre-first-frame the guarantee weakens from "the same *values*" to "the same *formula* as before the redesign", which is exactly what today's code already provides.
  Rejected: keeping a formula-only inverse pair (the formula needs three inputs the frozen signature cannot carry); rejected: a signature change on `tab_at` (breaks `input.rs`, out of surface); rejected: returning `None` pre-first-frame (breaks an out-of-surface test, as above); rejected: adding `input.rs` to this declared surface (a partition change requiring a re-run of `coherence-audit` and taking work out of a sibling's accept-seed); rejected: hit-testing against the drawn rect including margins (3px dead gutters between every card, which feels broken under a fast pointer); rejected: having the app recompute the layout from `left_inset` and a guessed scale (precisely the duplicated-math drift SD2 forbids).

### D8: Truncation is measured in px and ends in an ellipsis, as a pure function per line
- **Status**: resolved
- **Rationale**: Today's `truncate(s, max_chars)` (`tab_bar.rs:583`) cuts at a char count derived from one shared cell width and gives no visual signal that anything was cut. The card has two lines with **different advances** (D5), a content width reduced by the border, padding, mark and icon, and titles that are frequently long directory names — so truncation must be px-based, per line, and visible:
  ```rust
  pub fn truncate_to_width(s: &str, advance: f32, max_w: f32, ellipsis: char) -> String
  ```
  It fits as many `chars()` as `floor(max_w / advance)` allows; when the string does not fit, it keeps one fewer and appends the ellipsis, so the result is never wider than `max_w`. It iterates `chars()`, never bytes, so it cannot split a codepoint (`truncate_is_char_safe`, `tab_bar.rs:676`, is the precedent to extend). `max_w ≤ 0` yields `""` and `advance ≤ 0` yields `""` rather than dividing by zero.
  The ellipsis is `…` (U+2026) when the font provides it and `.` × 3 otherwise, decided **once per frame** by `ctx.atlas.glyph_id('…') != 0` (`atlas.rs:122` documents 0 as `.notdef`) — a missing ellipsis would otherwise render as a tofu box on the most-read text in the sidebar, and Menlo-alikes are not universal.
  Honest limits, since this is a monospace-advance layout and not a shaper: a CJK title advances at one cell per character here but is typically double-width on screen, so a CJK title truncates *late* and can overflow its card by up to a factor of two; and a combining mark counts as its own `char`, so a decomposed string truncates early. Both are pre-existing properties of the `tab_bar.rs:381` text path, neither is made worse, and fixing them means routing the sidebar through the wide-char logic `term-core` owns for the grid — out of scope and recorded in Known Gaps. Rejected: char-count truncation (wrong the moment the two lines have different advances); rejected: a hard clip rect (needs scissor state per card, and a title sliced mid-glyph reads as a rendering bug rather than as elision); rejected: measuring with `swash` shaping (a second text-measurement path diverging from the monospace advance the same code then draws with).

### D9: The layer owns **two** pipelines — foundations' shape pipeline plus a new resident-buffer text pipeline
- **Status**: resolved
- **Rationale**: `SidebarShapePipeline` (foundations D3) draws every non-text surface — panel, right border, card box, card border, mark icon, git-branch icon — through one instance format. It draws no text, and the only text path in the workspace is the swash atlas. So `sidebar_row.rs` also owns a `SidebarTextPipeline` mirroring foundations' API exactly (`new(device, format)` / `ensure(slot, ctx)` / `draw(ctx, &[TextInstance])`, early-returning on an empty slice), built on `crate::build_pipeline`, `crate::QUAD`, `crate::Vertex` and `crate::upload_resident` — the crate-root helpers, visible to descendant modules with no visibility change (foundations D11 established this). Only one descriptor is local: `crate::uniform_entry` is `ShaderStages::VERTEX` (`lib.rs:935`) while the text shader samples in the fragment stage, so the layer writes its own `VERTEX_FRAGMENT` entry — the same divergence `tab_bar.rs:512` already carries. The instance layout is today's `TextInstance { rect: [f32;4], uv0, uv1, color }` with `PREMULTIPLIED` blending, unchanged and re-declared here because its old home is deleted. The WGSL is an inline `const` (this feature's declared surface contains no `.wgsl` file, and the shader is ~25 lines — foundations' argument for a separate file was its 150-line SDF shader, which does not apply).
  **Instances go through `upload_resident`, not `create_buffer_init`.** The code being replaced allocates two GPU buffers *every frame* (`tab_bar.rs:451-464`) on an app that redraws continuously — the exact debt `G1` records as paid down once, one order of magnitude above the `Vec` allocation foundations D7 already refused. Reproducing it would be choosing a regression.
  **The atlas bind group is rebuilt on a composite key, not on format alone.** `apply_config` replaces `Renderer.atlas` wholesale (`lib.rs:545-560`), so a cached bind group holding the old `TextureView` survives a live font-size change and samples a dead texture — a latent bug in `tab_bar.rs:242`, which rebuilds only when `ctx.format` changes. `Atlas::generation()` alone does not detect it either (a fresh `Atlas` restarts at 0). So the key is `(ctx.format, ctx.atlas.generation(), ctx.cell)`: `ctx.cell` changes exactly when `apply_config` re-derives the cell metrics that accompany the atlas swap, and `generation` still catches an in-place eviction. This is a real behavioural fix, not a refactor, and R8.4 tests it. Residual: a config reload that changes *only* `theme.background` also rebuilds the atlas without changing `cell` or `generation` — the bind group would then be stale. `apply_config`'s own doc records that font *family* swaps are not applied; this narrower case is recorded in Known Gaps rather than papered over. Rejected: rebuilding the bind group every frame (a GPU object allocation per frame, the very thing this decision removes); rejected: `create_buffer_init` (see above); rejected: adding an atlas-swap counter to `atlas.rs` (outside the declared surface).

### D10: Fresh metadata reaches the frame by a **generation-gated pull** through one adapter — never a per-frame push or clone
- **Status**: resolved
- **Rationale**: The branch and agent state come from `tab-metadata`'s off-thread cache (SD4), but the model that reaches the renderer is only rebuilt when `sync_tab_model` runs — i.e. on a tab action. A branch discovered ten seconds after the tab opened would therefore never appear. And the seam offers no fix: `Overlay::poll(&mut self)` takes no arguments and `AppCtx` never exposes the tab list, so nothing reachable from `main.rs` can rebuild the rows; the only writer that *can* is `terminal_app`, which is out of surface (D1).
  So the direction inverts — the layer **pulls**:
  ```rust
  // renderer (sidebar_row.rs) — the trait is defined here, implemented in the app
  pub trait MetaSource {
      fn generation(&self) -> u64;                                   // cheap, allocation-free
      fn fill(&self, rows: &[SidebarRow], out: &mut Vec<RowMeta>);   // clears + refills a caller-owned Vec
  }
  pub struct RowMeta { pub branch: Option<String>, pub mark: RowMark, pub skin: AgentSkin }
  // app (sidebar_model.rs): impl MetaSource for SidebarMetaSource { … }  // reads tab-metadata's TabId cache
  // app (main.rs): TabModelHandle::shared().set_meta_source(Rc::new(SidebarMetaSource::new()));
  ```
  The `Vec<RowMeta>` is **owned by the layer**, not by `SidebarModel` — putting it on the model would make this call `fill(&model.rows, &mut model.meta)`, borrowing one `RefCell` immutably and mutably at once (D3).
  **Two gates, both O(1).** The layer refreshes when `source.generation()` differs from the value it last folded in, **or** when `SidebarModel.rows_rev` differs — a `u64` bumped by `TabModelHandle::set()`. The second gate is not optional and not free-form: without it the layer would have to detect a row change by comparing `Vec<SidebarRow>` element-wise every frame (an O(n) `String` compare per frame, against SD4's whole point) or would silently miss a tab open/close. In the steady state both counters are unchanged, so a frame does zero allocation and zero I/O — while an update still appears within one frame of the worker publishing it. This mirrors two idioms already in the codebase: `Atlas::generation()` and the renderer's `last_snapshot` equality gate that paid down G1.
  `fill` is app-crate code that can reach `TabModelHandle::shared()`, so the layer **clones the `Rc<dyn MetaSource>` out and drops its borrow before calling it** (D3's reentrancy rule).
  `fill` **joins by `tab_id`, never by index**, and emits `RowMeta::default()` (no branch, no mark, `AgentSkin::Generic`) for any row it does not recognise — a tab closed between assembly and draw must degrade to a plain card, never to a panic or a mis-attributed branch. With no `MetaSource` installed (renderer unit tests, and any window between startup and installation) every row gets the default, so the sidebar renders as plain titled cards.
  **This is the coordination point with a sibling being specified in parallel.** `tab-metadata`'s cache API is decided in *its* spec, not here; `SidebarMetaSource` in `sidebar_model.rs` is deliberately the **single** place that names it, so an API mismatch is one adapter to fix rather than a spread. If `tab-metadata` ships no synchronous read accessor, the fallback is stated in Known Gaps and External Dependencies. Rejected: pushing rows from `Overlay::poll()` (the hook cannot see the tab list, and it would also make freshness depend on `tab-metadata` choosing `UserEvent::OverlayPending` specifically); rejected: an ungated per-frame `meta_for(id) -> RowMeta` (a `String` clone per tab per frame at ~60 fps — the G1 anti-pattern); rejected: filling `RowMeta` inside `Tabs::titles()` (stale between tab actions, which is the defect this decision exists to remove); rejected: the app owning a `Vec<RowMeta>` it writes into the model (a fourth writer, and it would still need a push hook that does not exist).

### D11: The agent identity → `AgentSkin` + `RowMark` mapping lives in `sidebar_model.rs`, total by construction
- **Status**: resolved
- **Rationale**: `sidebar-foundations` D13 assigns this mapping to this feature explicitly, because `tab-metadata` is a partition root that cannot reference `renderer::sidebar_theme::AgentSkin` (SD3 forbids an app-side metadata provider knowing renderer types) and this is the only feature that `depends_on` both. The mapping is two small total functions in `crates/app/src/sidebar_model.rs`:
  - `fn skin_for(agent: Option<&AgentIdentity>) -> AgentSkin` — Claude Code → `AgentSkin::Claude`, Antigravity → `AgentSkin::Antigravity`, **every other value and `None` → `AgentSkin::Generic`**, which foundations D6 defines as the plain-card treatment. An unrecognised agent therefore renders as a normal tab rather than failing.
  - `fn mark_for(agent: Option<&AgentIdentity>) -> RowMark` — an agent that is *working* → `RowMark::Agent` (the `✳` icon, `#e2d9c8`); an agent that is *idle/ready* → `RowMark::Ready` (the `◇` icon, `#7796b8`); no agent → `RowMark::None` and line 1 starts at the padding edge with no reserved mark column.
  Both are written with an explicit `match` and no catch-all *pattern* over the agent enum where the sibling's type permits it, so adding an agent to `tab-metadata` produces a compile error at the one site that must decide — which is the whole reason foundations D13 rejected a stringly-typed seam. Where the sibling's type is non-exhaustive or stringly-typed, the fallback arm is `Generic`/`None`, and R7.3 asserts it.
  `ctx.sidebar.show_agent_status = false` short-circuits **both** to `Generic`/`None` before the mapping runs, so the toggle removes the mark *and* the skin rather than leaving a tinted card with no explanation. The marks are drawn as `Icon::Asterisk` / `Icon::Diamond` (foundations D8), never as font glyphs, so they cannot become tofu in a font lacking U+2733. Rejected: putting the mapping in `tab-metadata` (inverts the dependency direction — foundations D13); rejected: a mapping table in `common` (a colour concept in the UI-free config crate); rejected: deriving the skin from the agent's *name string* at the draw site (loses exhaustiveness exactly where a new agent must be handled).

### D12: Card visual states resolve through one table, evaluated per card per frame
- **Status**: resolved
- **Rationale**: Three inputs — selection, hover and skin — must combine deterministically, and scattering that across the draw code is how a state gets missed. One function, one table (all colours from `SidebarTheme`, all gradients from `AgentSkin`, per foundations D6):

  | skin | state | background | border |
  |------|-------|-----------|--------|
  | `Generic` | idle | *(no instance emitted — transparent)* | *(none)* |
  | `Generic` | hovered | `card_bg_hover` `#2a2722` | *(none)* |
  | `Generic` | selected | `card_bg_active` `#302c26` | `card_border_active` `#46413a` |
  | `Claude` / `Antigravity` | idle | `skin.bg` gradient | `skin.dim` |
  | `Claude` / `Antigravity` | hovered | `skin.hi` gradient | `skin.dim` |
  | `Claude` / `Antigravity` | selected | `skin.hi` gradient | `skin.border` |

  Selected outranks hovered. A transparent plain idle card emits **no** shape instance at all rather than an alpha-0 one — fewer instances, and it keeps the panel background visibly continuous.
  Two readings are recorded as readings, not as facts. First, the mock lists a `dim` colour per skin without stating which element it paints; this spec assigns it to the **idle border of an agent-backed card**, so agent tabs are always faintly outlined and the selected state escalates to the full `skin.border` — a plausible and self-consistent use for an otherwise orphaned token, flagged in Known Gaps for a human with the mock open. Second, the mock specifies a single `title_fg` `#e8e2d2` and no dimmed variant, so **inactive titles are drawn at full brightness** and selection is carried entirely by the card's background and border. That is a deliberate departure from today's behaviour (`tab_bar.rs:58-62` dims inactive titles to `#8A93A0`) and follows SD1's "honor the tokens".
  `hovered` stays `None` throughout this feature — nothing sets it until `sidebar-interaction` lands — so the hover row is implemented and unit-tested but never observed at runtime yet. An `active` index outside `0..count` (a stale index after a close) simply selects nothing, preserving `no_highlight_when_active_out_of_range` (`tab_bar.rs:625`). Rejected: hardcoding the six combinations at the draw site (six chances to miss one, and no way to unit-test the resolution separately from the GPU); rejected: dimming inactive titles as today (invents a token the mock does not specify); rejected: skipping hover until `sidebar-interaction` (the *rendering* is this feature's, and leaving a hole would put card-colour logic in two features).

### D13: Overflow is culled, not scrolled — `list_top` and `scroll_px` are layout inputs from day one
- **Status**: resolved
- **Rationale**: `sidebar-chrome-collapse` owns the header, scrolling and clipping. This feature must still behave when the cards exceed the viewport, and — critically — must leave that sibling a way to act **on a surface it owns**. Its declared surface is `sidebar_header.rs`, `sidebar_layout.rs` and `lib.rs`; it does **not** include `sidebar_row.rs`, so anything it must write has to exist here already. An earlier draft deferred these two fields to it, which would have made them unwritable without a declared-surface overlap — the same defect D4 had to fix for `sidebar-interaction`. So `SidebarModel` carries `list_top` and `scroll_px` **now**, with a `TabModelHandle::set_list_scroll(list_top, scroll_px)` setter (D3), `record` copies them into `SidebarGeom` instead of hardcoding `0.0`, and `SidebarLayout::visible_range()` returns the index range whose stride bands intersect `[list_top, viewport_h)`. The layer emits shapes and text only for that range — so a hundred tabs cost a handful of instances, not a hundred cards' worth drawn off-screen — and `card_at` returns `None` outside it, so a click below the last visible card is a no-op rather than a hit on an invisible one. The boundary is therefore drawn at the *data* level, not by a `TODO`: `sidebar-chrome-collapse` **writes two fields that already exist** and adds clipping, editing nothing this feature owns.
  The consequence, named: until that feature lands, tabs past the viewport bottom are **unreachable by pointer** and reachable only by `Cmd+Shift+[` / `]` / `Cmd+1..9`. Today's flat sidebar has exactly the same limitation (`tab_at` rejects `y` below the last slot) with a smaller slot height, so the reachable count drops from roughly `viewport_h / (1.5·cell_h)` to roughly `viewport_h / stride` — about a third as many tabs. That is a real, temporary narrowing, and it is recorded in Known Gaps. Rejected: implementing scrolling here (declared-surface overlap with `sidebar-chrome-collapse`); rejected: compressing cards to fit (destroys the mock's fixed geometry the moment a fourth tab opens); rejected: drawing every card and letting the GPU clip (unbounded instance growth, and `card_at` would then return indices for cards nobody can see).

### D14: The layer draws nothing at all when the band is empty, and the old sidebar's dead weight is deleted
- **Status**: resolved
- **Rationale**: `record` returns immediately when `ctx.left_inset <= 0.0` — which is exactly the `collapsed = true` case now that D4 makes the inset config-derived — before touching either pipeline. Foundations D9 would drop degenerate *shapes*, but the text path has no such guard and would still walk every row, hit the atlas, and build instances for a zero-width band. The same early return covers a zero-height viewport and an empty tab list.
  In the same pass, everything the old sidebar carried that nothing needs is deleted rather than migrated: `height_px` (dead, grep-verified), `slot_y` (no caller outside its own tests), `PAD_X`, `BORDER_W`, the five colour consts (superseded by `SidebarTheme`/`SidebarMetrics`), the private `linear` (foundations D6 owns the surviving copy), the private `truncate` (superseded by D8), and the private `QUAD`/`Vertex`/`Uniforms`/`build_pipeline`/`uniform_entry`/`PREMULTIPLIED` copies (the crate-root twins are used instead, foundations D11). `RECT_WGSL` goes with the shape pipeline; `TEXT_WGSL` moves into `sidebar_row.rs` as the text pipeline's shader.
  This matters beyond tidiness: `sidebar-chrome-collapse` reads this code next, and leaving two colour vocabularies and two pipeline scaffolds in place invites it to pick the wrong one. Rejected: keeping the old constants "in case" (SD7 exists to remove exactly them, and `-D warnings` would flag the unused ones anyway); rejected: leaving `slot_y`/`height_px` as deprecated shims (nothing calls them, and `#[deprecated]` under `-D warnings` is a build failure the moment anything does).

## Constraints
- **No file outside the declared surface may be edited** — in particular `crates/app/src/terminal_app.rs`, `crates/app/src/input.rs`, `crates/app/src/settings.rs` and `crates/app/src/session.rs`. Everything they reference keeps its name, path and signature — **and every assertion in their existing tests keeps passing** (D1, D2, D4, D7).
- `crates/renderer/src/lib.rs` and `crates/app/src/main.rs` are **ordered-shared** with siblings: `lib.rs` gains only `pub mod sidebar_row;`, the `SidebarOverride` struct + field + `effective_sidebar()`, `sync_left_inset()` and its two call sites, the `set_left_inset` body change, `set_sidebar_collapsed` / `set_sidebar_width`, and the rewrite of one existing test; `main.rs` gains only `mod sidebar_model;` and one installation call.
- The test suite must pass identically under `cargo test --workspace` and `cargo test --workspace -- --test-threads=1` (D3's singleton).
- No new crate dependency of any kind (`Cargo.lock` edits are PreToolUse-blocked). `wgpu`, `bytemuck` and the existing atlas are sufficient.
- No GUI/widget/vector-graphics crate (`.claude/rules/no-gui-crates.md`); `renderer` must not import `winit`; `term-core` is untouched and `GridSnapshot` is unchanged (`.claude/rules/dependency-direction.md`, SD3).
- No `#[cfg(target_os = ...)]` branches (`.claude/rules/macos-only.md`).
- No `unwrap()` / `expect()` outside `#[cfg(test)]`; no `panic!` reachable from `record` — a cosmetic layer must never kill a live terminal session (D3's borrow discipline, D10's id join).
- No filesystem or process I/O on the render path, and no per-frame heap or GPU allocation in the steady state (SD4, D9, D10).
- `SidebarMetrics::scaled(scale)` is called **exactly once per frame** (foundations D6).
- The sidebar draws at `x = 0` and must **not** bake `left_inset` into its geometry, unlike `cursor_layer` and `image_layer`.
- Tabs must remain switchable by pointer for every card that is drawn — the redesign may not make clicking less accurate than it is today (D7).
- All three gates green: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

## Known Gaps
- **`tab-metadata`'s read API is not yet fixed.** It is being specified in parallel, and this feature consumes it through exactly one adapter (`SidebarMetaSource` in `sidebar_model.rs`, D10). If it exposes no cheap synchronous accessor + generation counter usable from the UI thread, that adapter — and only that adapter — must change. A human should reconcile the two specs before this feature is built.
- **The mock's *horizontal* gaps were never captured.** The token inventory records the card's column `gap: 6px` but not the mark→title or branch-icon→branch-name spacing. This spec reuses `card_gap` (6px) for both. A human with the mock open should confirm.
- **`skin.dim` is assigned by inference.** The mock lists it per skin without stating its element; D12 uses it as the idle border of an agent-backed card. Plausible and self-consistent, but not verified against the mock.
- **The title renders ~13% smaller than the mock at default settings** (26 vs 30 physical px, D5). The deliberate trade is sharpness and font-size responsiveness over absolute fidelity. If a human judges the title too small next to the mock, the escape hatch is a second `Atlas` at the title ppem — which needs a `Renderer::font()` accessor in `lib.rs` and ~1 MB more VRAM.
- **`TabBarModel.titles` and `Tabs::titles()` are now misnamed** — they carry `SidebarRow`s, not titles (D2). Renaming both is a one-line edit in `terminal_app.rs`, which belongs to `sidebar-interaction`.
- **`set_left_inset` silently ignores its argument** (D4). Correct here, a footgun in general. It stops being one when `sidebar-interaction` deletes the two remaining call sites, after which the method itself can go — `set_sidebar_collapsed` / `set_sidebar_width` are the real setters and survive it.
- **A runtime collapse/width outranks the config file for the rest of the session** (D4). Editing `[sidebar] collapsed` or `width_px` in the TOML after clicking the hide button does nothing until restart. Deliberate — the thing the user just clicked should outrank a file they edited for another reason — but a user could reasonably report it. The alternative (config wins) makes the sidebar reappear by itself on any unrelated save, which is worse.
- **There is a one-frame window at startup where `tab_at` uses the legacy formula against the new sidebar width** (D7). Between window creation and the first `record`, `layout` is `None`, so a click in that window is hit-tested with `sidebar_w = left_inset()` (the new, wider band) and the legacy `tab_h` — and can resolve to the wrong card. Bounded to a single frame and vanishingly unlikely, but it is the one place SD2's "one source of truth" is genuinely violated at runtime. The cheap fix, if it ever matters, is for `sync_left_inset` to publish an initial layout so `layout` is never `None` in a real `Renderer` — deliberately not done here, because it would put layout construction in `lib.rs`.
- **`TabModelHandle`'s singleton leaks between tests under `--test-threads=1`** (D3). Mitigated by an explicit `reset_shared_model()` that every singleton-touching test must call; a test that forgets it becomes order-dependent, and nothing enforces the call mechanically.
- **`skin_for` / `mark_for` may not be exhaustive-by-compiler.** D11 wants a catch-all-free `match` so a new agent breaks the build at the one site that must decide — but that only holds if `tab-metadata` exposes a closed enum. If it exposes a `#[non_exhaustive]` enum or a string, the fallback arm silently absorbs new agents and a new agent renders as a plain tab until someone notices.
- **Between this feature and `sidebar-interaction`, hover is inert and collapse is config-only.** `hovered` is never set and no hide button exists; `collapsed = true` in the TOML is the only way to collapse, and nothing draws a restore affordance until `sidebar-chrome-collapse` lands.
- **Cards past the viewport bottom are unreachable by pointer until `sidebar-chrome-collapse` adds scrolling** (D13). Today's sidebar has the same limitation but fits roughly three times as many tabs, so this is a temporary narrowing, not a new class of defect.
- **A config reload that changes only `theme.background` rebuilds the atlas without changing `ctx.cell` or `Atlas::generation()`**, so the text bind group would be stale until the next real change (D9). Narrow, and `apply_config` already documents that font-family swaps are not applied at all; fixing it properly means an atlas-swap counter in `atlas.rs`, which is outside this surface.
- **CJK and combining-character titles truncate at the wrong width** (D8) — a pre-existing property of the monospace-advance sidebar text path, not a regression. A correct fix routes the sidebar through `term-core`'s wide-char logic.
- **This feature is where `sidebar-foundations`' visual output is seen for the first time.** An icon-table or SDF transcription error from that feature surfaces here, at `/run-term`, and may need a fix in a file this feature does not own.
- **`TabModelHandle` becoming a per-thread singleton assumes one window per UI thread** (D3). True today and structurally true for winit's `run_app`, but a second window would silently share one sidebar model.

## Requirements

### R0: The redesign ships without touching a file it does not own

#### R0.1: No out-of-surface file is modified
- **Given**: the diff of this feature against its base commit
- **When**: the changed-file list is compared with the declared surface
- **Then**: it contains only `crates/renderer/src/tab_bar.rs`, `crates/renderer/src/sidebar_row.rs`, `crates/app/src/sidebar_model.rs`, `crates/app/src/tabs.rs`, `crates/renderer/src/lib.rs` and `crates/app/src/main.rs` — in particular `terminal_app.rs`, `input.rs`, `settings.rs` and `session.rs` are untouched

#### R0.2: All three gates are green
- **Given**: the completed feature
- **When**: `cargo build`, `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` are run
- **Then**: all three exit 0, with no new warnings and no `unwrap()` / `expect()` outside `#[cfg(test)]`

#### R0.3: The `lib.rs` and `main.rs` diffs stay inside their enumerated lists
- **Given**: the diffs to the two ordered-shared files
- **When**: they are reviewed
- **Then**: `lib.rs` contains only `pub mod sidebar_row;`, the `SidebarOverride` struct, the `sidebar_override` field on `Renderer`, `effective_sidebar()`, `sync_left_inset()` plus its two call sites (`assemble`, `apply_config`), the `set_left_inset` body change, the two new setters `set_sidebar_collapsed` / `set_sidebar_width`, and the rewrite of `set_left_inset_clamps_and_narrows_grid`; `main.rs` contains only `mod sidebar_model;` and one `set_meta_source` call

#### R0.4: The rest of `lib.rs`'s test suite still passes untouched
- **Given**: `draw_to_skips_reshape_on_unchanged_snapshot` (`lib.rs:1328`), which builds a 256px-wide headless renderer that now carries a 340px config-derived inset
- **When**: `cargo test --workspace` runs
- **Then**: it passes without modification, because `grid_dims` floors cols at ≥1 and the test asserts only `reshape_count` — and if it does need modification, that change is added to R0.3's enumerated list rather than made silently

#### R0.5: The terminal runs and the sidebar looks like the mock
- **Given**: the built binary launched via `/run-term` with three tabs, one inside a git repo and one running an AI agent CLI
- **When**: the sidebar is inspected
- **Then**: each tab is a rounded two-line card with visible padding and margins, the active card is brighter and bordered, the agent tab carries its gradient skin and mark, and the git tab shows a branch icon and branch name on its second line

### R1: The card renders the mock's geometry and tokens (D6, D12, SD1)

#### R1.1: Card geometry follows the mock's metrics
- **Given**: `SidebarMetrics::DEFAULT` at scale 1.0, `sidebar_w = 340`, a cell of `[8, 20]` and `show_branch = true`
- **When**: `layout(&geom, 3)` is evaluated
- **Then**: `card_w == 340 - 2*8 == 324`, `card_h == 2*(1 + 12) + 20 + 6 + round(20*0.8333) == 69`, `stride == card_h + 2*3 == 75`, and `card_rect(1)` is `[8, 3 + 75, 324, 69]`

#### R1.2: The card box uses the mock's radius and border width
- **Given**: a drawn card
- **When**: the emitted `SidebarShape` for its box is inspected
- **Then**: its radius is `card_radius` (13 logical px, scaled) and its border width is `border_w` (1 logical px, scaled), both routed through `SidebarShape::rounded_box` so foundations D9's clamps apply

#### R1.3: A plain idle card emits no background instance
- **Given**: a row with `AgentSkin::Generic` that is neither active nor hovered
- **When**: its shapes are built
- **Then**: no box instance is emitted for it — the panel background shows through unbroken — and no border instance either

#### R1.4: The selected plain card is brighter and bordered
- **Given**: three plain rows with `active == 1`
- **When**: shapes are built
- **Then**: row 1 gets a `card_bg_active` `#302c26` box and a `card_border_active` `#46413a` border, and rows 0 and 2 get neither

#### R1.5: An agent-backed card carries its gradient on every state
- **Given**: a row whose skin is `AgentSkin::Claude`, evaluated idle, hovered and selected
- **When**: its background and border are resolved
- **Then**: the background is `skin.bg`, `skin.hi`, `skin.hi` respectively and the border is `skin.dim`, `skin.dim`, `skin.border` — so an agent tab is always visibly skinned, never transparent

#### R1.6: Selection outranks hover
- **Given**: a row that is both `active` and `hovered`
- **When**: its state is resolved
- **Then**: it resolves to the selected row of the table, not the hovered one

#### R1.7: A stale active index highlights nothing
- **Given**: a model with 1 row and `active = 5`
- **When**: shapes are built
- **Then**: no card is drawn selected and no panic occurs — preserving the behaviour of `no_highlight_when_active_out_of_range`

#### R1.8: The panel and its right border are drawn
- **Given**: a sidebar of width `w` on a viewport of height `vh`
- **When**: chrome shapes are built
- **Then**: a `panel_bg` `#1e1b18` rect spans `[0, 0, w, vh]` and a `panel_border` `#2e2a24` rect spans `[w - border_w, 0, border_w, vh]`

### R2: Two text sizes from one atlas (D5)

#### R2.1: The title draws at the atlas's native size in synthesized bold
- **Given**: a card whose title is `"project"`
- **When**: its glyph instances are built
- **Then**: each quad's `rect` uses the `GlyphEntry`'s metrics unscaled, the pen advances by `ctx.cell[0]` per char, and the glyphs are fetched with `GlyphKey { bold: true, italic: false }`

#### R2.2: The branch line draws at the mock's ratio in the regular weight
- **Given**: a card whose branch is `"main"` and `meta_ratio = 12.5 / 15`
- **When**: its glyph instances are built
- **Then**: every `px_w`, `px_h`, `left` and `top` is multiplied by `meta_ratio`, the pen advances by `ctx.cell[0] * meta_ratio`, and glyphs are fetched with the plain key

#### R2.3: Both lines sit inside the card's padding box
- **Given**: any card at any scale factor with any `font.size_pt`
- **When**: the two baselines are computed
- **Then**: line 1's ascent box and line 2's descent box both lie within `[card_y + border_w + pad_y, card_y + card_h - border_w - pad_y]`, so no glyph crosses the card border

#### R2.4: Line spacing follows the mock's column gap
- **Given**: a two-line card
- **When**: the two line boxes are compared
- **Then**: the vertical distance between line 1's box bottom and line 2's box top is exactly `card_gap` (6 logical px, scaled)

#### R2.5: The sidebar's type scales with the user's font size
- **Given**: `font.size_pt` changed from 13 to 20 and `apply_config` applied
- **When**: the next frame is drawn
- **Then**: the title and branch text both grow proportionally and `card_h` grows with them — no text is clipped and no card overlaps its neighbour

### R3: Px-space truncation with an ellipsis (D8)

#### R3.1: A title that fits is drawn verbatim
- **Given**: `truncate_to_width("main", 8.0, 100.0, '…')`
- **When**: it is evaluated
- **Then**: it returns `"main"` with no ellipsis appended

#### R3.2: A title that overflows is elided
- **Given**: `truncate_to_width("a-very-long-directory-name", 8.0, 80.0, '…')`
- **When**: it is evaluated
- **Then**: it returns exactly 10 chars — 9 from the input plus `'…'` — so `chars().count() * 8.0 <= 80.0`

#### R3.3: Truncation never splits a codepoint
- **Given**: `truncate_to_width("café-très-long", 8.0, 40.0, '…')`
- **When**: it is evaluated
- **Then**: the result is valid UTF-8 with whole characters only, mirroring `truncate_is_char_safe`

#### R3.4: Degenerate widths yield an empty string, not a panic
- **Given**: `max_w = 0.0`, `max_w = -5.0` and `advance = 0.0`
- **When**: each is passed
- **Then**: each returns `""` with no division by zero and no panic

#### R3.5: The two lines truncate at their own widths
- **Given**: a card whose title and branch are both 40 chars, with `meta_ratio = 0.8333`
- **When**: both lines are truncated
- **Then**: the branch line keeps *more* characters than the title, because its advance is smaller — proving each line measured with its own advance

#### R3.6: A font without U+2026 falls back to three dots
- **Given**: an atlas whose `glyph_id('…')` returns 0 (`.notdef`)
- **When**: the frame's ellipsis is chosen
- **Then**: `'.'` repeated three times is used instead, and the choice is made once per frame rather than per string

### R4: Layout is pure, published, and exactly invertible (D7, SD2)

#### R4.1: `card_rect` and `card_at` round-trip for every card
- **Given**: a layout with 5 cards at an arbitrary scale, `list_top` and `scroll_px`
- **When**: `card_at` is called at the centre of each `card_rect(i)`
- **Then**: it returns `Some(i)` for every `i` — the redesign's replacement for `slot_y_and_tab_at_are_exact_inverses`

#### R4.2: A click in the inter-card margin still selects a card
- **Given**: two adjacent cards separated by `2 * card_margin_y`
- **When**: `card_at` is called at a point inside that margin
- **Then**: it returns the card whose stride band contains the point, not `None`

#### R4.3: Clicks outside the band and past the last card miss
- **Given**: a layout with 3 cards on a 540px viewport
- **When**: `card_at` is called at `x < 0`, at `x >= sidebar_w`, at `y < list_top`, and below the third card
- **Then**: each returns `None`

#### R4.4: The layer publishes the layout it actually drew
- **Given**: a `record` that drew 4 cards
- **When**: `SidebarModel.layout` is read afterwards
- **Then**: it is `Some(layout)` whose `count` is 4 and whose `stride` and `card_h` equal the values the draw used

#### R4.5: `tab_at` delegates to the published layout
- **Given**: a published layout and a point at the centre of card 2
- **When**: `tab_bar::tab_at(x, y, sidebar_w, tab_h, count)` is called with any `tab_h`
- **Then**: it returns `Some(2)`, equal to `layout.card_at(x, y)` — so `input::tab_target` hits the right card without any change to `input.rs`

#### R4.6: With no published layout, the shims reproduce today's behaviour exactly
- **Given**: a `SidebarModel` reset so `layout == None` (as in any `input.rs` unit test, where no `record` has ever run)
- **When**: `tab_bar::tab_height_px(20.0)` and `tab_bar::tab_at(40.0, 65.0, 160.0, 30.0, 3)` are called
- **Then**: they return `30.0` and `Some(2)` — the legacy `(cell_h * 1.5).round().max(1.0)` and `floor(y / tab_h)` results, so `input.rs:122-137`'s `tab_target_maps_click_to_tab_and_delegates_geometry` passes unmodified

#### R4.7: The out-of-surface input test is green
- **Given**: `crates/app/src/input.rs` unmodified
- **When**: `cargo test --workspace` runs
- **Then**: `tab_target_maps_click_to_tab_and_delegates_geometry` passes, including its `tab_target(...) == tab_bar::tab_at(...)` delegation assertion

### R5: The sidebar's width comes from config and the renderer owns it (D4, SD7)

#### R5.1: The inset equals the configured width times the scale factor
- **Given**: a `Config` with `[sidebar] width_px = 300` on a window at scale factor 2.0
- **When**: the renderer is constructed
- **Then**: `renderer.left_inset()` is `600.0` and `grid_size()` reports the columns that fit beside it

#### R5.2: A stale `set_left_inset` call cannot corrupt the width
- **Given**: a renderer whose config-derived inset is 600.0
- **When**: `set_left_inset(238.0)` is called, as `terminal_app.rs:133` and `settings.rs:28` still do
- **Then**: `left_inset()` is still `600.0`

#### R5.3: A live config reload re-fits the sidebar and the grid
- **Given**: a running renderer **on which no runtime setter has been called** (so no override claims the width) and an edit changing `[sidebar] width_px` from 340 to 260
- **When**: `apply_config` is called
- **Then**: `left_inset()` becomes `260 × scale` and `grid_size()` reports more columns, so the existing `settings::apply` → `resize_all` path re-fits every tab

#### R5.4: `collapsed = true` returns the whole band to the grid
- **Given**: a config with `[sidebar] collapsed = true`
- **When**: the inset is computed
- **Then**: `sidebar_width_px` returns `0.0`, `left_inset()` is `0.0`, and the grid spans the full window width

#### R5.5: The renderer exposes a runtime write path for collapse and width
- **Given**: a running renderer whose sidebar is expanded at 340 logical px
- **When**: `set_sidebar_collapsed(true)` is called, then `set_sidebar_collapsed(false)`, then `set_sidebar_width(260.0)`
- **Then**: `left_inset()` goes `0.0` → `340 × scale` → `260 × scale` and `grid_size()` tracks each change — so `sidebar-interaction` can implement its hide/restore accept-seed without editing `lib.rs`

#### R5.6: A runtime setter outranks a later config reload
- **Given**: a renderer on which `set_sidebar_collapsed(true)` was called
- **When**: `apply_config(&cfg)` runs with a `cfg` whose `[sidebar] collapsed` is `false` (a save that changed some unrelated key)
- **Then**: `left_inset()` is still `0.0` — the sidebar does not reappear on its own, because the override survives `apply_config`'s `self.sidebar = cfg.sidebar.clone()`

#### R5.7: The runtime width setter is clamped to the same band as the config
- **Given**: `set_sidebar_width(100000.0)`, `set_sidebar_width(10.0)` and `set_sidebar_width(f32::NAN)`
- **When**: each is called
- **Then**: the effective widths are `440 × scale`, `260 × scale` and the default `340 × scale` — foundations D2's band enforced on the runtime door it does not cover, with the NaN case falling through to `sidebar_width_px`'s non-finite rule (R5.8) since `f32::clamp` propagates a NaN receiver rather than panicking

#### R5.8: A degenerate configured width cannot reach the grid
- **Given**: a `SidebarConfig` built directly (bypassing `Config::load_from`'s clamp) with `width_px` of `-10.0`, `0.0` and `f32::NAN`
- **When**: `sidebar_width_px(&cfg, 2.0)` is evaluated
- **Then**: it returns `0.0`, `0.0` and the default `340 × 2.0` respectively — never a negative or non-finite inset, since `set_left_inset`'s `max(0.0)` no longer guards this path

#### R5.9: The layer draws nothing when the band is empty
- **Given**: `ctx.left_inset == 0.0`
- **When**: `record` runs
- **Then**: it returns before building any shape or text instance, records no pass, and creates no buffer

### R6: The model bus and the app-side assembly (D2, D3, D10)

#### R6.1: `sync_tab_model`'s existing call site compiles unchanged
- **Given**: `terminal_app.rs:219-222` verbatim — `TabBarModel { titles: self.tabs.titles(), active: self.tabs.active_index() }`
- **When**: the workspace is built
- **Then**: it compiles, because `Tabs::titles()` returns `Vec<SidebarRow>` and `TabBarModel.titles` is `Vec<SidebarRow>` with both field names unchanged

#### R6.2: Rows carry a stable id and a title
- **Given**: three open tabs, one whose cwd is `/Users/x/proj`
- **When**: `Tabs::titles()` is called
- **Then**: it returns three `SidebarRow`s whose `tab_id` matches each `Session.id.0` and whose `title` is the cwd basename or the `Tab N` fallback, exactly as today

#### R6.3: Every party reaches the same model, and tests can reset it
- **Given**: `reset_shared_model()`, then `TabBar::new()` on one thread, then `TabModelHandle::shared()` and `TabModelHandle::default()` on that same thread
- **When**: one of them writes and another reads
- **Then**: the write is observed — all three are clones of one per-thread `Rc` — and the suite passes identically under `cargo test` and `cargo test -- --test-threads=1`, because every singleton-touching test calls `reset_shared_model()` first

#### R6.4: Metadata is pulled only when a generation or the row revision changes
- **Given**: a `MetaSource` whose `generation()` is constant and whose `fill` counts its calls
- **When**: 100 frames are recorded with an unchanged row list, then `TabModelHandle::set()` pushes a new row list, then 100 more frames run
- **Then**: `fill` was called exactly twice — once initially and once when `rows_rev` changed — with no per-frame `Vec<SidebarRow>` comparison and no `RowMeta` allocation on the other 198 frames

#### R6.5: Reading, refreshing and publishing in one `record` never double-borrows
- **Given**: a `record` that copies the model out through `with`, refreshes meta through a `MetaSource` whose `fill` **itself calls `TabModelHandle::shared().with(...)`**, draws, then publishes its layout through `with_mut`
- **When**: it runs 100 times
- **Then**: no `RefCell` borrow panic occurs — because the `Rc<dyn MetaSource>` is cloned out and the borrow dropped before `fill` runs, and each subsequent borrow ends before the next begins

#### R6.6: A missing `MetaSource` degrades to plain cards
- **Given**: a model with rows but no `MetaSource` installed
- **When**: a frame is drawn
- **Then**: every row resolves to `RowMeta::default()` — no branch, no mark, `AgentSkin::Generic` — and the sidebar renders as plain titled cards

#### R6.7: Metadata is joined by id, never by index
- **Given**: rows `[id 1, id 3]` and a cache holding ids `[1, 2, 3]`
- **When**: `fill` runs
- **Then**: row 0 gets id 1's meta and row 1 gets id 3's — and a row whose id is absent gets `RowMeta::default()` rather than a neighbour's data or a panic

#### R6.8: `main.rs` installs the adapter exactly once
- **Given**: the built binary
- **When**: `main` runs
- **Then**: `renderer::sidebar_row::TabModelHandle::shared().set_meta_source(Rc::new(SidebarMetaSource::new()))` is called once before the event loop starts, and `mod sidebar_model;` is the only other addition

### R7: Agent identity maps to a skin and a mark (D11, foundations D13)

#### R7.1: Known agents map to their skins
- **Given**: a tab whose metadata reports Claude Code, and another reporting Antigravity
- **When**: `skin_for` runs
- **Then**: they yield `AgentSkin::Claude` and `AgentSkin::Antigravity`

#### R7.2: The mark reflects the agent's state
- **Given**: an agent reported as working, an agent reported as idle, and a tab with no agent
- **When**: `mark_for` runs
- **Then**: it yields `RowMark::Agent` (`✳`, `#e2d9c8`), `RowMark::Ready` (`◇`, `#7796b8`) and `RowMark::None` respectively

#### R7.3: An unrecognised agent degrades to a plain card
- **Given**: metadata reporting an agent this mapping does not know
- **When**: `skin_for` and `mark_for` run
- **Then**: they yield `AgentSkin::Generic` and `RowMark::None`, so the tab renders as an ordinary card rather than failing

#### R7.4: `show_agent_status = false` removes both the mark and the skin
- **Given**: a config with `show_agent_status = false` and a tab running Claude Code
- **When**: its row is resolved
- **Then**: the mark is `RowMark::None` and the skin is `AgentSkin::Generic` — the card is indistinguishable from a plain tab

#### R7.5: Marks are drawn as icons, never as font glyphs
- **Given**: a font containing neither U+2733 nor U+25C7
- **When**: a card with `RowMark::Agent` is drawn
- **Then**: `Icon::Asterisk` shapes are emitted and the glyph atlas is never consulted for the mark

#### R7.6: The branch line renders icon plus name, and is absent when disabled
- **Given**: a tab on branch `feature/x` with `show_git_branch = true`, and the same tab with it `false`
- **When**: each frame is drawn
- **Then**: the first draws `Icon::GitBranch` at `meta_px` followed by `feature/x` in `meta_fg` `#8f887a`; the second draws a one-line card with no second line reserved

#### R7.7: A branch-less tab keeps its reserved line
- **Given**: `show_git_branch = true` and a tab whose cwd is not in a repository
- **When**: its card is drawn
- **Then**: the card has the same height as every other card, with no icon and no text on the second line — so no neighbour shifts when a branch later appears

### R8: Pipelines, allocation, and the atlas (D9, D14)

#### R8.1: Both pipelines build lazily and are reused
- **Given**: a `TabBar` constructed with no device
- **When**: `record` runs twice with an unchanged `ctx.format`
- **Then**: the shape and text pipelines are created on the first call and reused on the second

#### R8.2: No GPU buffer is allocated per frame in the steady state
- **Given**: a layer that drew N shapes and M glyphs last frame
- **When**: it draws the same counts again
- **Then**: both instance uploads go through `upload_resident` with `queue.write_buffer`, and **no `create_buffer_init` appears on the per-frame path** — the once-per-pipeline quad and uniform buffers built inside `ensure` may still use it, exactly as every sibling layer does (`image_layer.rs:92`, `cursor_layer.rs:108`) and as foundations D3 permits

#### R8.3: An empty instance list records no pass
- **Given**: a frame whose shape list or text list is empty
- **When**: `draw` is called with it
- **Then**: it returns before recording a pass and creates no zero-size buffer

#### R8.4: The text bind group survives an atlas swap
- **Given**: a live `font.size_pt` change that makes `apply_config` replace the whole `Atlas`
- **When**: the next frame is drawn
- **Then**: the text bind group is rebuilt — because the key `(format, atlas.generation(), cell)` changed — and the glyphs sample the new texture rather than a dead one

#### R8.5: The WGSL compiles
- **Given**: a machine where `crate::headless()` yields a device
- **When**: the text pipeline is constructed
- **Then**: naga accepts the shader and the pipeline is created; the test skips cleanly when `headless()` returns `None`

#### R8.6: Metrics are scaled exactly once per frame
- **Given**: a `record` call
- **When**: its calls to `SidebarMetrics::scaled` are counted
- **Then**: there is exactly one, and every downstream metric derives from its result

### R9: The compatibility façade (D1)

#### R9.1: The module path and its items survive
- **Given**: `crates/renderer/src/tab_bar.rs`
- **When**: it is read
- **Then**: it exposes `TabBar`, `TabBar::new() -> (TabBar, TabModelHandle)`, `TabBarModel`, `TabModelHandle`, `TAB_BAR_Z`, `width_px`, `tab_height_px` and `tab_at` with their existing signatures, and nothing else

#### R9.2: The three out-of-surface callers compile untouched
- **Given**: `terminal_app.rs`, `input.rs` and `settings.rs` at their current contents
- **When**: `cargo build` runs
- **Then**: it succeeds with no edit to any of them

#### R9.3: Dead legacy geometry is gone
- **Given**: the workspace after this feature
- **When**: it is searched for `height_px`, `slot_y`, `PAD_X`, `BORDER_W`, `STRIP_BG`, `ACTIVE_BG`, `INACTIVE_FG` and a `tab_bar`-local `linear` or `truncate`
- **Then**: none is found — `SidebarTheme`, `SidebarMetrics` and D8's `truncate_to_width` are the surviving sources

#### R9.4: `tab_height_px` never returns nonsense
- **Given**: a published layout, and separately no published layout
- **When**: `tab_height_px(cell_h)` is called
- **Then**: it returns the published stride in the first case and today's `(cell_h * 1.5).round().max(1.0)` in the second — always positive, never zero, and byte-compatible with what `input.rs`'s test expects (R4.6)

### R10: Overflow is culled cleanly and the sibling boundary holds (D13)

#### R10.1: Only visible cards are drawn
- **Given**: 100 tabs on a viewport that fits 7 cards
- **When**: a frame is drawn
- **Then**: `visible_range()` covers 7–8 indices and instances are emitted only for those, so instance count is bounded by the viewport rather than by the tab count

#### R10.2: A click below the last visible card misses
- **Given**: 100 tabs and a click 10px above the viewport bottom, below the last drawn card
- **When**: `card_at` is called
- **Then**: it returns `None` rather than an index for a card nobody can see

#### R10.3: The scroll and header inputs exist and default to zero
- **Given**: `SidebarGeom`
- **When**: it is constructed by this feature
- **Then**: `list_top` and `scroll_px` are both `0.0`, and non-zero values shift `card_rect` and `card_at` together so the round-trip of R4.1 still holds

#### R10.4: A sibling can move the list without editing this feature's files
- **Given**: `TabModelHandle::shared().set_list_scroll(48.0, 120.0)` called from outside `sidebar_row.rs`
- **When**: the next frame is drawn
- **Then**: `record` copies both values from `SidebarModel` into `SidebarGeom`, every `card_rect` shifts by `48 - 120`, `visible_range()` narrows accordingly, and `card_at` inverts the shifted layout — so `sidebar-chrome-collapse` implements scrolling on its own declared surface

## Tasks

### T1: `sidebar_row.rs` skeleton — the model bus, the row/meta types, and the `tab_bar.rs` façade [infra]
- **Fulfills**: R4.6, R4.7, R6.1, R6.2, R6.3, R6.5, R6.7, R9.1, R9.2, R9.3, R9.4 (the fallback branch)
- **Depends on**: (none — but `sidebar-foundations` and `tab-metadata` must have landed)

Creates `crates/renderer/src/sidebar_row.rs` with `SidebarRow`, `RowMeta`, `RowMark`, `SidebarModel` (including `rows_rev` and the private `meta_source`), the `thread_local!` singleton behind `TabModelHandle::shared()` plus `#[cfg(test)] reset_shared_model()`, `TabBarModel`, and the `MetaSource` trait; rewrites `tab_bar.rs` into the façade of R9.1 and deletes everything R9.3 lists; adds `pub mod sidebar_row;` to `lib.rs` and changes `Tabs::titles()`'s return type in `crates/app/src/tabs.rs`. The `TabBar` layer is a stub whose `record` returns immediately, so the workspace stays green while the drawing lands in T3/T4. **The façade ships D7's legacy fallback formulas from the start** (five lines: `(cell_h * 1.5).round().max(1.0)` and `floor(y / tab_h)` with today's bounds checks) — it must, because until T2 publishes a layout there is nothing else for them to return, and `input.rs`'s test runs on every `cargo test` from this task onward. **Verify R6.1 and R4.7 first** — if `terminal_app.rs` fails to compile or `input.rs`'s test goes red at this point, every later task is built on sand.

### T2: Pure card layout, hit-test, and truncation [infra]
- **Fulfills**: R1.1, R3, R4.1, R4.2, R4.3, R4.5, R9.4 (the published-stride branch), R10.3
- **Depends on**: T1

Upgrades T1's legacy shims to delegate to the published layout when one exists (R4.5), keeping the fallback path for when one does not (R4.6/R4.7 stay green).

Fills in `SidebarGeom`, `SidebarLayout`, `layout()`, `card_rect`, `card_at`, `visible_range` and `truncate_to_width`, plus the `tab_at` / `tab_height_px` shims that read the published layout. All of it is pure and headlessly testable — this is the task that carries SD2, and R4.1 is the redesign's replacement for `slot_y_and_tab_at_are_exact_inverses`.

### T3: The text pipeline and two-size text [vertical]
- **Fulfills**: R2, R8.1 (text), R8.2 (text), R8.3 (text), R8.4, R8.5
- **Depends on**: T2

Adds `SidebarTextPipeline` + its inline WGSL on `crate::build_pipeline` / `crate::QUAD` / `crate::upload_resident` with the local `VERTEX_FRAGMENT` uniform entry, the composite atlas bind-group key, the native-ppem bold title, and the `meta_ratio`-scaled sub-line. R8.4 is a real bug fix over the code being replaced, not a port — write its test before the port so the old behaviour is demonstrably wrong first.

### T4: Card chrome — shapes, skins, marks, branch icon, panel and border [vertical]
- **Fulfills**: R1.2–R1.8, R4.4, R5.9, R7.5, R7.6, R7.7, R8.1 (shapes), R8.2 (shapes), R8.3 (shapes), R8.6, R10.1, R10.2, R10.4
- **Depends on**: T2  ← parallel with T3

Builds every shape through `SidebarShape`'s constructors and foundations' `SidebarShapePipeline`, resolves D12's state table, emits the mark and git-branch icons via `push_icon` into a reused sink, culls to `visible_range()`, owns `record`'s `ctx.left_inset <= 0.0` early return (R5.9 — `record` is stubbed in T1 and only becomes real here), copies `list_top`/`scroll_px` out of the model into `SidebarGeom` (R10.4), and publishes the drawn layout (R4.4). Parallel with T3: disjoint code paths (shapes vs text), disjoint pipelines, and both depend only on T2's geometry. This is the first time `sidebar-foundations`' icons and SDFs are seen on screen — budget time for fixing what that reveals.

### T5: The app-side view model — `sidebar_model.rs`, the metadata adapter, and the skin mapping [vertical]
- **Fulfills**: R6.4, R6.6, R6.8, R7.1–R7.4, R0.5
- **Depends on**: T3, T4

Creates `crates/app/src/sidebar_model.rs` with `SidebarMetaSource` (the single place naming `tab-metadata`'s API), `skin_for` and `mark_for`; adds `mod sidebar_model;` and the one `set_meta_source` call to `main.rs`. Depends on T3+T4 because the mapping's output is only observable once cards draw. If `tab-metadata`'s read API does not match, **this task is where it is reconciled** — and nowhere else (Known Gaps).

### T6: Renderer-owned sidebar width and the runtime setters [infra]
- **Fulfills**: R5.1–R5.8, R0.3, R0.4  *(R5.9 belongs to T4, which owns `record`)*
- **Depends on**: T2

Adds `sidebar_width_px(cfg, scale)` (with R5.8's defensive clamp) to `sidebar_row.rs`, and to `lib.rs` the `SidebarOverride` field, `effective_sidebar()`, `sync_left_inset()`, the `set_left_inset` body change and `set_sidebar_collapsed` / `set_sidebar_width` (the latter clamped to 260–440, R5.7); calls `sync_left_inset` from `assemble` and `apply_config` **without** clearing the override (R5.6); rewrites `set_left_inset_clamps_and_narrows_grid` to assert the new contract. Parallel with T3 and T4 in principle, but it also edits `lib.rs`, which T1 touched — serialise it after T2 and keep the diff to R0.3's list. **Landing this before T3/T4 would leave the terminal visibly broken** (a full-width band with nothing drawn in it), so it is ordered here deliberately. R0.4 belongs here because this is the task that can perturb the rest of `lib.rs`'s suite.

**On R0's placement**: R0.1–R0.4 are whole-feature properties every task must preserve, and R0.5 is the `/run-term` eyeball that is only meaningful once T3, T4, T5 and T6 have all landed. R0.3 and R0.4 are attached to T6 (the task that owns the `lib.rs` diff) and R0.5 to T5 (the last task); R0.1 and R0.2 are verified by every task per CLAUDE.md. This is not a disguised final-verify task — holistic verification belongs to the execute pipeline.

**DAG**: T1 → T2 → {T3 ∥ T4} → T5, with T6 branching off T2 and merging before T5. No cycles.

**Vertical-slice note**: T1, T2 and T6 are infrastructure (a shared model, pure geometry, a width contract) with no user-visible surface of their own, which the L4 rule permits. T3, T4 and T5 are genuine vertical slices — each one changes what the user sees, and T5 closes the loop from an app-side data source to a drawn pixel.

## External Dependencies

### Pre-work
- **Reconcile `tab-metadata`'s read API with D10** before T5. This feature needs a cheap, synchronous, `TabId`-keyed accessor plus a monotonic generation counter, callable from the UI thread. It is specified in parallel, and a mismatch is a one-adapter fix — but it should be a *known* one, not a surprise at build time.
- **Confirm two mock readings with the design open** (Known Gaps): the horizontal mark→title and icon→branch gaps, which the token inventory never captured and which this spec sets to `card_gap` (6px); and the element `skin.dim` paints, which D12 assigns to the idle border of an agent-backed card.
- `sidebar-foundations` must have landed — every shape, icon, token and the `FrameCtx.sidebar` / `FrameCtx.scale_factor` plumbing this feature consumes comes from it.

### Post-work
- **`sidebar-interaction` deletes the façade's reason to exist**: rename `TabBarModel.titles` → `rows` and `Tabs::titles()` → `rows()` at the one `terminal_app.rs` call site, drop the two `set_left_inset(width_px(...))` calls in `terminal_app.rs` and `settings.rs` (after which `set_left_inset` and its ignored parameter can go, while `set_sidebar_collapsed` / `set_sidebar_width` remain), and switch `input::tab_target` — and its test — from `tab_bar::tab_at` to `SidebarLayout::card_at`, at which point `crates/renderer/src/tab_bar.rs` and D7's legacy fallback can both be deleted outright.
- **`sidebar-interaction` calls `Renderer::set_sidebar_collapsed(bool)`** from its hide/restore button. That method is added by *this* feature (D4) precisely because `sidebar-interaction`'s declared surface does not include `crates/renderer/src/lib.rs`; without it, its accept-seed ("collapsing returns the reclaimed width to the terminal grid and expanding restores it") would be unimplementable on its own surface.
- **`sidebar-interaction` sets `SidebarModel.hovered`**, which this feature renders but never populates.
- **`sidebar-chrome-collapse` writes `SidebarModel.list_top` and `scroll_px`** via `TabModelHandle::set_list_scroll` — both fields and the setter are added by *this* feature (D3, D13) precisely because that sibling's declared surface does not include `sidebar_row.rs`. It adds clipping and lifts D13's temporary limit that cards past the viewport bottom are unreachable by pointer.
- **Its header layer should be registered at `z_order = 11`** so it records after this feature's `TAB_BAR_Z = 10` panel (`layers.sort_by_key` is stable, but an explicit order should not be left to registration sequence). **Note for `coherence-audit`:** `Renderer::add_layer` is called only from `terminal_app.rs` or, indirectly, via `Overlay::layer()` registered in `main.rs` — neither of which is `sidebar-chrome-collapse`'s surface. The registration line therefore belongs to `sidebar-interaction` (which owns both files) and should appear in *its* task list, not be assumed.
