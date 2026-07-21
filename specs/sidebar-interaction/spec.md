# Spec: sidebar-interaction

## Meta
- **Created**: 2026-07-20
- **Type**: dev
- **Status**: ready
- **Approved by**: batch mode — the human approved the partition (features, declared surfaces, `depends_on` edges, SD1–SD8) at decompose's single gate; specify's L0/L2/L3/L4 prompts are bypassed per `pre-approved-batch: yes`
- **Approved at**: 2026-07-20
- **Execution**: NOT handed off. Batch mode's deliverable is this file; `decompose` → `build-order` owns execution.
- **Set**: sidebar-redesign (`specs/sidebar-redesign/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single human gate)
- **L2 review**: 3 rounds (initial + 2 retries — the skill's maximum). Round 1 returned NEEDS_FIX because the three sibling specs, absent when the first draft was written, had landed mid-review and contradicted every assumed signature. Round 2 returned NEEDS_FIX for the same structural reason a second time: the siblings were **rewritten again** during the review, partly *in response to this spec's own earlier draft* (`sidebar-chrome-collapse` D2/D7/D10 and `sidebar-row-redesign` D4 each cite this file and overrule their own earlier decisions to match it). L2 was re-derived against the current sibling revisions both times. **Round 3's fixes were applied and re-reviewed within the retry budget; anything found after that is recorded in Known Gaps rather than silently patched.** The durable answer to the churn — restoring the escalation rule that both siblings quote as binding, and framing every sibling signature as revision-dated rather than frozen — is **D13**, and it is the most important decision in this spec.

## Declared Surface
- feature-id: sidebar-interaction
- depends_on: [sidebar-row-redesign, sidebar-chrome-collapse]
- declared-surface:
  - crates/app/src/input.rs
  - crates/app/src/terminal_app.rs
  - crates/app/src/settings.rs
  - crates/app/src/main.rs

## Goal
Make the redesigned sidebar *respond*, and close out the wiring the whole set has been deferring to this feature. Every pointer event the mock implies — clicking a card, clicking the header's new-tab and hide buttons, clicking the collapsed restore button, hovering, scrolling the card list — becomes a real app-side event path. Collapsing must give the reclaimed width back to the terminal grid and every PTY, and expanding must take it back, without ever mis-sizing the shell. `sidebar-chrome-collapse`'s layer, which nobody has registered, gets registered. `tab-metadata`'s worker, which nobody has fed, gets its roster. And the four migration hand-offs `sidebar-row-redesign` parks on this feature get done, so the legacy `tab_bar.rs` façade can finally be deleted.

## Non-goals
- **Drawing anything.** This feature registers a layer but implements none, computes no layout, and defines no colour. `sidebar-row-redesign` and `sidebar-chrome-collapse` own every pixel; this feature decides *which region an event lands in* — by calling their functions — and *what the app does about it*.
- **Reimplementing layout maths on the app side.** Forbidden by SD2 and treated as a hard constraint. The one boundary case — assembling a `SidebarGeom` out of values each read from an accessor or from a sibling's own function — is argued explicitly in D3 and contains no arithmetic.
- **Any behaviour for the filter button beyond swallowing its press.** `sidebar-chrome-collapse` D2 (current revision) makes `SidebarHit::Filter` a real hit region with **no hover visual**, precisely so the press is consumed rather than leaking to the grid. There is nothing more to wire.
- **The search field**, the mock's terminal-pane chrome, and the "Use agent" banner (SD8).
- **A keyboard chord for collapse** (e.g. Cmd+B). `input::tab_action` is a user-facing chord table; adding a binding is a product decision the manifest does not make.
- **Drag, press-and-release semantics, double-click, tab reordering, drag-resize of the sidebar, and a per-card close button.** None exists today (`ElementState::Released` appears nowhere in the workspace) and none is in the accept-seed (D2). Note `Renderer::set_sidebar_width` ships in `sidebar-row-redesign` D4 as the backing for a future drag-resize; this feature does not call it.
- **Deleting `crates/renderer/src/tab_bar.rs` or `Renderer::set_left_inset`.** This feature removes their last callers, which is what *unblocks* the deletions; the deletions themselves are in `lib.rs` / `tab_bar.rs`, out of surface (D4).
- **Writing to `config.toml`.** No code in this workspace writes a config file, and this feature does not become the first (D5).
- **Handling `ScaleFactorChanged`.** Pre-existing gap; the fix needs a `Renderer` setter in `lib.rs`, out of surface. Known Gaps.

## Confirmed Goal
Wire the redesigned sidebar's pointer input through `crates/app/src/input.rs` (pure, window-free intent functions that delegate **all** geometry to `sidebar_layout::hit_test`) and `crates/app/src/terminal_app.rs` (thin winit arms), so that: a left press on a card switches to that tab through the existing `do_tab_action(Jump)` path; a press on the header's new-tab button opens a tab; a press on the hide button collapses the sidebar and a press on the floating restore button expands it; a press on the filter or on sidebar dead space is swallowed; pointer motion resolves **one** `hit_test` and pushes both the hovered card index and the hovered chrome region into the shared `SidebarModel`, region-debounced, with no syscall and no grid re-clone; and a wheel over the sidebar (in either `LineDelta` or `PixelDelta` units) moves an app-owned physical-px scroll offset through `sidebar_layout::clamp_scroll` and publishes it. Collapse and expand call `Renderer::set_sidebar_collapsed(bool)` — the setter `sidebar-row-redesign` D4 added for exactly this — followed by a single shared `settings::refit` helper that all **five** grid-refit paths (startup, config reload, collapse, expand, window resize) route through, so the shell's column count can never be read before the inset changes. This feature also registers `SidebarChromeLayer`, calls `scroll_to_reveal` on every tab action, pushes `tab-metadata`'s `(TabId, pid)` roster from `do_tab_action`, and performs `sidebar-row-redesign`'s four parked migrations (`titles`→`rows`, dropping both dead `set_left_inset` calls, retiring `tab_target` in favour of `hit_test`, and populating `SidebarModel.hovered`). Done when every decision is expressed as a pure function unit-tested without a window, a shell or an `EventLoopProxy`, and `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` are green.

## Research

### The four files this feature owns — verified line by line

**`crates/app/src/input.rs`** — the SD2 pattern to extend, and the file whose test the whole set has been protecting.
- `TabAction { New, CloseActive, Next, Prev, Jump(usize) }` (`:11-19`); `tab_action(key, mods) -> Option<TabAction>` (`:26-42`).
- `tab_target(cursor, cell, sidebar_w, count) -> Option<usize>` (`:52-60`) delegates the geometry to `tab_bar::tab_at`; its doc states the rule — *"the hit-test math lives in exactly one place (SD2), the same functions the sidebar draws with."*
- `tab_target_maps_click_to_tab_and_delegates_geometry` (`:122-137`) is the test idiom **and a live constraint on two sibling features**: `sidebar-row-redesign` D7 keeps a legacy fallback in `tab_at` specifically so this assertion stays green across the two features between it and this one. Retiring it is this feature's job (D4).
- **`renderer` is a plain library dependency of `app`**, so any renderer pure function is callable from an `app` unit test with no GPU and no window.

**`crates/app/src/terminal_app.rs`** — the event loop.
- `Running` (`:86-113`): `last_cursor: (f64,f64)` (`:96`), `tab_model: TabModelHandle`, `occluded`, `last_snapshot`, `snapshot_dirty`, `config_rx`.
- `redraw()` (`:208-215`) sets `snapshot_dirty` then `window.request_redraw()`; its doc reserves the bare call for the idle continuous frame, *"which must NOT re-snapshot."*
- `sync_tab_model()` (`:218-223`) — `tab_model.set(TabBarModel { titles: tabs.titles(), active: tabs.active_index() })`. **`Tabs::titles()` (`tabs.rs:138-151`) calls `s.cwd()` per tab → `cwd_of_pid` → `libc::proc_pidinfo` (`session.rs:142-166`): one syscall per tab per call.**
- `do_tab_action` (`:227-250`) is the **single shared path for chords and clicks**: match → `sync_tab_model()` → `overlays.notify_active_changed(cwd)` → `redraw()`. Its `New` arm is `if let Err(e) = self.tabs.open(cols, rows)` (`:230-233`) — it **discards the `Ok(TabId)`** (D9 needs it). It returns `false` at `:236-239` when the last tab closed, before any of the tail work runs.
- Startup (`:130-138`): `tab_bar::width_px(...)` → `set_left_inset(bar_w)` → `TabBar::new()` → `add_layer` → `CursorLayer`. `Tabs::new` is at `:158`. `grid_size()` is read at `:153`, after the inset is set.
- `CursorMoved` (`:353-357`) stores the raw physical position and does nothing else.
- `MouseInput { Pressed, Left }` (`:359-376`) → `tab_target(...)` → `do_tab_action(Jump(i))`; `None` → silent no-op; the returned `bool` is **discarded** at `:374`.
- `Resized` (`:378-383`) → `renderer.resize()` → `grid_size()` → `resize_all` → `redraw()`. **A fifth refit path that does not share code with `settings::apply`** (D6).
- `RedrawRequested` (`:406-431`): re-snapshots only when `snapshot_dirty`, then unconditionally `request_redraw()` unless occluded — the **continuous redraw** (winit#2640, `:421-427`).
- `UserEvent` (`:33-43`): `PtyOutput(TabId)`, `ConfigReloaded`, `OverlayPending`. `ConfigReloaded` (`:322-332`) drains to the newest config, calls `settings::apply`, and **discards the `Config`** (`self.cfg` is read only at `:127`, `:154`, `:155`, inside `start()`). `OverlayPending` (`:333-338`) calls `overlays.poll_all()` and `redraw()`s iff it returns `true` — `OverlayHost::poll_all` (`seam.rs:169-175`) polls **every** registered overlay regardless of visibility while `dispatch` (`:154-161`) skips invisible ones, which is the hook `tab-metadata` D3 uses.

**`crates/app/src/settings.rs`** — 34 lines. `apply(cfg, renderer, tabs)` (`:23-34`): `apply_config` → `set_left_inset(tab_bar::width_px(cell_w))` (`:28`) → `grid_size()` → `resize_all` → `set_default_colors_all`. The comment at `:25-27` states the ordering rule: *"keep the left tab sidebar in step **before** reading `grid_size`."*

**`crates/app/src/main.rs`** — 52 lines. `mod` list `:10-17`; `proxy` created `:39`, cloned into `App::new` `:40`, **moved** into `AiMonitorOverlay::new(proxy)` at `:44`.

### What the app can reach on `Tabs` / `Session` (this bounds D9)
`Tabs` exposes `len`, `active_index`, `active() -> &Session`, `active_mut`, `by_id_mut(TabId)`, `open(cols,rows) -> Result<TabId>`, `close_active() -> bool`, `next/prev/jump`, `resize_all`, `titles`, `active_cwd` (`tabs.rs:46-156`). **`sessions` is private; there is no iterator and no index→id accessor.** `Session` has `pub id: TabId` and `pub pty: Pty` (`session.rs:23-28`); `Pty::child_pid() -> Option<u32>` is `pub` (`term-core/src/pty.rs:63`). `close_active()` clears and returns `false` for the last tab (`tabs.rs:88-96`).

### Gaps in today's pointer handling (grep-verified, 0 hits each across `crates/`)
`MouseWheel`, `MouseScrollDelta`, `CursorLeft`, `CursorEntered`, `Focused`, `ElementState::Released`, `ScaleFactorChanged`. No hover state, no wheel handling, no drag, no close-button region, no double-click; grid-area presses are ignored entirely.

### Renderer facts this feature depends on
- `set_left_inset` (`lib.rs:508-511`), `left_inset()` (`:514-516`), `grid_size()` (`:471-479`) → `grid_dims(..., left_inset)` (`:1021-1033`) whose `cols` floors at 1 (`:1029`). `add_layer` at `:453`.
- `resize(w,h)` (`:562-572`) sets `config.width/height`, calls `surface.configure`, invalidates `last_snapshot`, re-writes uniforms; it **never** touches `left_inset` or `scale_factor`.
- `apply_config` (`:526-560`) re-derives cell metrics and ppem and **rebuilds the glyph `Atlas` and bind group** (`:545-560`).
- **The registered-layer record loop (`lib.rs:716-738`) sits OUTSIDE the snapshot-equality branch (`:646-673`) and runs unconditionally every frame** — verified directly, and independently re-verified by `sidebar-chrome-collapse` D10. This is what makes D7's redraw policy sound.
- `scale_factor` (`:163`) is private, captured once at `:433`, never updated; `sidebar-foundations` D10 adds `pub fn scale_factor(&self) -> f64`.
- `tab_bar::width_px` (`tab_bar.rs:77-79`) is called from exactly two places, **both in this feature's surface**: `terminal_app.rs:132` and `settings.rs:28`.

### The sibling contracts — read from the current spec revisions, and dated (D13)
> Every signature below is quoted from a sibling spec **as of 2026-07-20**, after two rounds of mutual reconciliation. Three of them changed shape *during this spec's own review*, and two of them changed *because of* this spec's earlier draft. They are recorded as revision-dated facts, not frozen ones; **D13 is the rule that governs a mismatch.**

**`sidebar-chrome-collapse`** (`sidebar_layout.rs`, `sidebar_header.rs`):
- **D7** — the module owns **chrome geometry only**, over the sibling's type: every function takes `&SidebarLayout`. Published: `header_h(scale) -> f32`, `header_rect(l)`, `button_rect(hit, l) -> Option<Rect>`, `search_slot_rect(l)`, `list_viewport(l)`, `max_scroll(l)`, `clamp_scroll(scroll, l)`, `scroll_to_reveal(i, scroll, l)`, `restore_button_rect(l)`, `chrome_hit(x, y, l)`, **`hit_test(x, y, l: &SidebarLayout) -> Option<SidebarHit>`**. Its earlier bespoke `SidebarLayoutInput`, `row_rect`, `row_at`, `row_stride`, `content_height` and `visible_rows` are **deleted**; the card stride is the public field `l.stride`. `SidebarHit { Card(usize), NewTab, Filter, Hide, Restore, Panel }` is adopted **verbatim from this spec's earlier draft**, and its D7 reconciliation table cites this feature's escalation rule as the mechanism.
- **D9** — `hit_test` is an ordered cascade: (1) collapsed (`l.geom.sidebar_w <= 0.0`) → `Restore` if inside `restore_button_rect(l)`, else `None`; (2) outside the band → `None`; (3) inside `header_rect` → the button variant or `Panel`, **never falling through to a card**; (4) else `l.card_at(x,y).map(Card).unwrap_or(Panel)`. `Panel` exists so this feature can *swallow* the press — it cites this spec's `Some(Inert)` mapping by name.
- **D2** — `Filter` **is** a hit region (overruling its own earlier draft, on this spec's argument) with no hover visual.
- **D3** — collapsed ⇔ `sidebar_w == 0`; no boolean crosses the seam. It names the `set_left_inset` contradiction between the two other specs and records the fix in its own Known Gaps.
- **D6** — the scroll offset is **this feature's `f32`**, physical px, mutated on `MouseWheel`; `clamp_scroll` is total and NaN-safe; **`scroll_to_reveal` must be called by this feature on tab change** or a chord-switch highlights a card the user cannot see — *"a correctness bug, not a nicety"* — and it hands over the risk that *"a caller can forget to call it"*.
- **D10** — **the app resolves hover once with `hit_test` and pushes the resolved `Option<SidebarHit>`**; the layer highlights `NewTab`/`Hide`/`Restore`. This overrules its earlier raw-cursor handle, explicitly on the ground that this feature already calls `hit_test` for clicks.
- **D11** — the `SidebarChromeHandle` is **deleted**; chrome state rides the shared `SidebarModel` via `TabModelHandle::shared()`.
- **D14** — `SidebarChromeLayer` ships **unregistered**: *"Registration … happens in `crates/app/src/terminal_app.rs`, which is `sidebar-interaction`'s surface … the first frame it draws is in `sidebar-interaction`."* `SIDEBAR_CHROME_Z = 30`.

**`sidebar-row-redesign`** (`tab_bar.rs`, `sidebar_row.rs`, `sidebar_model.rs`, `tabs.rs`):
- **D3** — `TabModelHandle::shared()` is a per-UI-thread singleton over one `SidebarModel { rows, active, rows_rev, hovered: Option<usize>, list_top: f32, scroll_px: f32, layout: Option<SidebarLayout>, meta_source }`. The handle's API is `set(TabBarModel)`, `set_meta_source`, **`set_hovered(Option<usize>)`**, **`set_list_scroll(list_top, scroll_px)`**, `layout() -> Option<SidebarLayout>`, and `with`/`with_mut` as **the only borrow entry points** — each takes and releases the borrow inside the call, so the borrow discipline is structural rather than advisory. `hovered`, `list_top` and `scroll_px` are declared there *now* precisely because the features that write them cannot edit that file.
- **D4** — `sidebar_row::sidebar_width_px(cfg, scale) -> f32` (0.0 when collapsed); `Renderer` gains a `SidebarOverride { collapsed: Option<bool>, width_px: Option<f32> }` consulted **before** the config, `sync_left_inset()`, `set_left_inset(_requested_px)` as a **compat no-op**, and — **added for this feature and "not optional"** — **`pub fn set_sidebar_collapsed(&mut self, c: bool)`** and `set_sidebar_width(logical_px)`. The override **survives `apply_config`**: *"a runtime setter wins over the file for the rest of the session."* Its rejected list includes, by name, *"leaving runtime collapse to a `Config` round-trip through `settings::apply`."*
- **D7** — `SidebarGeom { sidebar_w, viewport_h, cell, scale, show_branch, list_top, scroll_px }` (*"every input, all physical px"*), `SidebarLayout { geom, count, stride, card_h, … }`, `pub fn layout(geom: &SidebarGeom, count: usize) -> SidebarLayout`, and `card_rect(i)` / `card_at(x,y)` / `visible_range()`. **The shims degrade to the legacy formula rather than to `None`** before the first frame — chosen specifically to keep `input.rs:122-137` green until this feature retires it.
- **D13** — `record` copies `list_top`/`scroll_px` out of the model into `SidebarGeom`, so a sibling that writes them moves the drawn *and* hit-tested geometry together.
- **Post-work** parks four migrations on this feature: rename `TabBarModel.titles`→`rows` and `Tabs::titles()`→`rows()` at the one `terminal_app.rs` call site; drop both `set_left_inset(width_px(...))` calls; switch `input::tab_target` **and its test** off `tab_bar::tab_at`; and **populate `SidebarModel.hovered`**, *"which this feature renders but never populates."* It also states plainly: *"`sidebar-interaction` calls `Renderer::set_sidebar_collapsed(bool)` from its hide/restore button."*

**`tab-metadata`** (`crates/app/src/tabmeta/**`, `ai-monitor`):
- **D2** — `TabMetaHandle::spawn(wake)`, `set_roster(&[TabPid])` with `TabPid { id: TabId, pid: u32 }`, `set_active(Option<TabId>)`, `set_enabled`, `generation()`, `sync(&mut TabMetaSnapshot) -> bool`; reached through `tabmeta::install(handle)` from `main.rs` and `tabmeta::global()`. It states the roster *"must come from the feature that owns `tabs.rs`"* while noting `Tabs::new` is constructed in `terminal_app.rs`, *"in neither this feature's surface nor `sidebar-row-redesign`'s (it belongs to `sidebar-interaction`)"* — **so the push is left here.**
- **D3** — the wake rides the **existing** `UserEvent::OverlayPending` through a permanently-invisible `TabMetaPump` registered from `main.rs`. It **rejects adding a `UserEvent` variant by name**, *"because it would collide with `sidebar-interaction`, which owns `terminal_app.rs`."* It also claims the `main.rs:44` `proxy.clone()` edit as *"a one-token edit inside the declared surface"*, and it lands **before** this feature in the manifest order.
- **D4** — publication is change-only, so `poll_all()` returns `true` only on a real change, and `D14` requires the worker to tolerate dead pids without panicking.

### Project constraints
- Gates: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`. No `unwrap()` outside tests. `crates/app` is a **binary** crate, so `pub` does **not** exempt an uncalled item from `dead_code` under `-D warnings` (D4).
- `.claude/rules/`: `no-gui-crates`, `macos-only`, `dependency-direction`, `zsh-local-only`.
- **PreToolUse blocks `Cargo.lock` edits → no new crate dependency.** winit 0.30.13 (`crates/app/Cargo.toml:26`) supplies every event type needed.
- Test idiom: pure free functions unit-tested without a window or a shell (`input.rs:76-137`, `tabs.rs:159-211`, `seam.rs:199-255`). **Nothing in the repo constructs an `EventLoopProxy` in a test** (13 occurrences, all production).

## L2 Checkpoints

Complexity: **Complex**, counted honestly rather than inflated — **3 strong signals + 1 soft**:
- *3+ independent subsystems* (strong): winit pointer handling · renderer inset/grid coordination + PTY re-fit · the `tab-metadata` roster loop · the shared-model publication channel.
- *State machine / multi-stage flow* (strong): collapsed ⇄ expanded crosses three owners (renderer override → uniforms → grid dims → N PTYs), with scroll and hover as further live state.
- *Multi-actor interaction* (strong): the user's pointer, the metadata worker and the config-watcher thread all feed one sidebar; a reload can race a collapse click.
- *Async resources* (**soft**): this feature spawns nothing and owns no worker; it feeds and consumes an existing wake path.

It lands in **Complex** on a fourth signal that dominates all of them and is not on the standard list: this is the **integration point of four features**, and its correctness is defined almost entirely by contracts written in three other spec files — two of which changed shape twice during this spec's own review. Brownfield adjustment: Core 30% · Scope 20% · Error 25% · Data 15% · Implementation 10%. 6–8 checkpoints per dimension.

### Core Behavior (30%) — 8/8 resolved
- [x] Which region a press maps to, and who owns that geometry → D1
- [x] Press vs. release, and which mouse buttons act → D2
- [x] **How the app obtains a `SidebarLayout` at all** — including when nothing has been published → D3
- [x] What collapse must do, and by which API → D6
- [x] What "trigger a redraw" means under a continuous redraw loop → D7
- [x] When hover updates, on which channel, and what it costs → D8
- [x] Wheel → scroll-offset delta, in both winit unit systems, and how the offset reaches the drawn cards → D10
- [x] **Who pushes `tab-metadata`'s roster**, given no feature can iterate `Tabs` → D9

### Scope Boundaries (20%) — 8/8 resolved
- [x] This feature draws nothing and computes no layout → D1, D3
- [x] The filter is a hit region with no hover; the press is swallowed → D1, D2
- [x] Grid-area presses stay ignored → D2
- [x] No drag / double-click / close button / reorder / drag-resize → D2
- [x] No config writing → D5
- [x] **No new `UserEvent` variant** → D9 (`tab-metadata` D3 designed around not having one, to avoid colliding here)
- [x] **Registering `SidebarChromeLayer`, and the four parked migrations, are this feature's job** → D4
- [x] `ScaleFactorChanged` out of scope, and why → Non-goals, Known Gaps

### Error / Edge Cases (25%) — 10/10 resolved
- [x] A press on no region vs. on sidebar dead space → D2
- [x] A press while collapsed, where `left_inset == 0` → D1, D3
- [x] **A session that STARTS collapsed**, where the row layer never publishes a layout → D3 *(the app constructs the layout rather than reading it — without this, `[sidebar] collapsed = true` is an unrecoverable state)*
- [x] Hover stale when the pointer exits the window → D8 (`CursorLeft`)
- [x] **Hover stale when focus is lost with the pointer still inside** (Cmd-Tab fires no `CursorLeft`) → D8 (`Focused(false)`)
- [x] Hover stale when the layout moves under a stationary pointer → D8
- [x] Scroll offset out of range after tabs close **or after a window resize** → D10, D6
- [x] A wheel event when the list already fits → D10 (`max_scroll == 0`)
- [x] **A `tabs.open()` that fails, and a `child_pid()` that is `None`** → D9
- [x] Closing the last tab from a click path that discards the exit bool → D2

### Data Model (15%) — 7/7 resolved
- [x] **Where the live `collapsed` value lives — and that the app stores no copy** → D5, D6 (`Renderer`'s override is the single source; the app reads `left_inset() <= 0.0`)
- [x] The action vocabulary a region maps to → D1 (`SidebarAction`)
- [x] Scroll offset units, owner, sign, and publication → D10
- [x] Hover's two published values, both from **one** `hit_test` → D8
- [x] The roster payload and how it stays in step with `Tabs` → D9
- [x] Which `SidebarConfig` the app reads when building a `SidebarGeom` → D3, D5
- [x] **`SidebarGeom` field provenance** — every field an accessor or a sibling function, none computed → D3

### Implementation (10%) — 7/7 resolved
- [x] Pure free functions in `input.rs`, thin winit arms → D11
- [x] No `EventLoopProxy`/`Window`/GPU/PTY in any test → D11
- [x] Reuse `do_tab_action` for every tab mutation → D1
- [x] **One shared grid-refit helper for all five refit paths** → D6
- [x] **Borrow discipline on the shared singleton** → D8 (the sibling's `with`/`with_mut` make it structural)
- [x] **`tab_target`'s retirement must not leave dead code under `-D warnings`** → D4 *(added: `crates/app` is a bin crate, so an uncalled `pub fn` still fails the clippy gate)*
- [x] **A rule for when a sibling signature does not match this spec** → D13

**Composite: 1.00** (40/40 resolved; every dimension ≥ 0.60). Unknown/unknowns: 0 pending.

> Scoring honesty note. This table read 1.00 twice on premises that were false within the hour, both times because a sibling spec changed underneath it. 1.00 means "no checkpoint I can currently name is open" — **it does not mean the sibling signatures quoted above are still current.** D13 exists because that is a standing condition of this feature, not a one-off accident, and the residual cross-spec items are left in Known Gaps for `coherence-audit` rather than scored away.

### Unknown/Unknown Detection

**Tier 1 (Actor).** Actors: the *user's pointer* (D1, D2, D8, D10 ✓); the *terminal grid / PTYs* (D6 ✓); the *metadata worker* (D9 ✓); the *config-watcher thread* (D5 ✓); the *chrome layer*, which nobody registers (0 decisions) → **+1 Scope checkpoint** → **D4**; the *`Tabs` collection* as a roster source that cannot be iterated (0 decisions) → **+1 Core checkpoint** → **D9**; the *row layer* as the publisher of `layout`, which does not publish while collapsed (0 decisions) → **+1 Error checkpoint** → **D3**; the *three sibling specs themselves*, as mutable inputs (0 decisions) → **+1 Implementation checkpoint** → **D13**.

**Tier 2 (Implication).**
- D1 (delegate geometry) → implies obtaining a `SidebarLayout`; reading chrome-collapse D7 showed every function now takes one, and reading row-redesign D14 showed none is published while collapsed → **+1 Core, +1 Error checkpoint** → **D3**.
- D3 (construct the geom) → implies the app needs `show_git_branch`, i.e. a live `SidebarConfig` → **+1 Data checkpoint** → D3/D5.
- D6 (`set_sidebar_collapsed`) → implies the app should store **no** `collapsed` copy, because the renderer's override already wins over the file → **deleted** an entire earlier decision (a live-`Config` + `disk_collapsed` edge-trigger apparatus) as redundant and as a second source of truth.
- D8 (push hover) → implies a single resolution serving two published fields, and implies borrow discipline; reading row-redesign D3 showed `with`/`with_mut` already make it structural → **+1 Data, +1 Implementation checkpoint**.
- D10 (wheel → offset) → implies re-clamping whenever `max_scroll` changes, which a **window resize** does → **+1 Error checkpoint** → D6's shared refit; and implies the offset must reach the drawn cards, not just the hit-test → D10's `set_list_scroll` publication.
- D4 (retire `tab_target`) → implies a dead-code failure under `-D warnings` in a bin crate → **+1 Implementation checkpoint**.
- Reading tab-metadata D3 → **deleted** a `UserEvent::TabMetaUpdated` variant an earlier draft added, which that spec rejects by name.
- Reading chrome-collapse D2 (current) → **deleted** an earlier claim that the filter has no hit region, and restored the `Filter` arm.

**Tier 3 (Pair Tension).** **Runs** — SD4 is an explicit performance constraint; D6 is a multi-step cross-owner mutation; D13 is a reliability constraint on the seam itself.
- **D8 × SD4.** Hover via `sync_tab_model` would issue N `proc_pidinfo` syscalls per pointer event at 60–120 Hz. → D8 writes model fields directly and never calls `titles()`/`rows()`.
- **D9 × SD4.** A per-frame roster push would re-walk every tab. → D9 pushes only from `do_tab_action` and `start()`.
- **D6 × D5.** A collapse and a config reload both change the inset. → the renderer's override makes the runtime value win by construction, so there is nothing for the app to arbitrate — the earlier edge-trigger machinery was the tension, and removing it removed the tension.
- **D6 × `Resized`.** Both mutate grid dims; `Resized` was a fifth refit path outside any shared helper, and it is the path that changes `max_scroll`. → `settings::refit`.
- **D3 × D6.** The app constructs a `SidebarGeom` whose `sidebar_w` is `left_inset()`, while `left_inset` is derived from the override the app just set. Ordering matters: the geom must be built **after** `set_sidebar_collapsed`, or the hit-test and the re-clamp run against the pre-toggle width. → D6 fixes the step order and D3 states that `sidebar_w` is always read live.
- **D9 × D2.** Closing the *last* tab makes `do_tab_action` return `false` at `:236-239` **before** the tail work — so no roster push happens on the way out, which is correct (the process is exiting) but must be stated, since the shadow roster would otherwise look leaked.
- **D10 × D8.** Both write the shared model on high-frequency events. → both go through the sibling's `with_mut`/setters, which release the borrow inside the call.

### Inversion Probe
*"What could make this fail even if every requirement is met correctly?"* — **A sibling spec changing again.** That is not hypothetical here: it happened twice during this spec's review, and twice the change was *toward* this spec rather than away from it (chrome-collapse adopted `SidebarHit` and the `Panel`→swallow rule verbatim; row-redesign added `set_sidebar_collapsed` after this spec's earlier draft showed the accept-seed was otherwise unimplementable). The mitigation is **D13**, which is why it is a decision and not a footnote: signatures are dated, the escalation rule is restored and binding, and every app-side function is a thin delegate so an adaptation is one line per call site. Residual, stated: if a sibling changes a *semantic* rather than a signature — e.g. if `hit_test` stopped returning `Panel` for dead space — D2's swallow rule would silently become a leak, and no compile error would say so. That class is what `coherence-audit` exists for.

*Second probe — "the renderer's override wins over the file for the session; does that also mean…?"* — Yes: after a runtime collapse, editing `[sidebar] collapsed` in the TOML does nothing until restart. `sidebar-row-redesign` D4 makes that choice deliberately and records it; this feature inherits it rather than re-litigating it, and repeats it in Known Gaps so it is visible from here too.

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

### D1: Every pointer event resolves through ONE `sidebar_layout::hit_test` call; the app owns only the region→action table
- **Status**: resolved
- **Rationale**: SD2's point is that drawing and hit-testing cannot diverge, and this feature is the only place hit-testing happens. So the app performs **no** geometry: `input.rs` gains pure functions shaped exactly like the existing `tab_target` (`input.rs:52-60` — cast `f64`→`f32`, gather inputs, delegate):

  ```rust
  /// What the app does about a pointer press. Distinct from the renderer's
  /// *region* enum: several regions map to one action, and two map to "consumed".
  pub enum SidebarAction { Switch(usize), NewTab, Collapse, Expand, Inert }

  pub fn sidebar_press(cursor: (f64, f64), l: &SidebarLayout) -> Option<SidebarAction>;
  pub fn sidebar_hover(cursor: (f64, f64), l: &SidebarLayout) -> Option<SidebarHit>;   // D8
  pub fn wheel_scroll(delta: WheelDelta, scroll: f32, l: &SidebarLayout) -> f32;        // D10
  ```

  **There is deliberately no app-side dispatch *order*.** chrome-collapse D9 defines `hit_test` as an ordered cascade whose non-fall-through property — *"a card scrolled under the header returns `Panel`, not `Card`"* — is a stated correctness requirement with its own test. Re-deriving any part of it here would create a second answer to one question (SD2's failure mode) and would re-open the invisible-but-clickable bug D9 closes. Note also that `collapsed` and `scroll` are **not** parameters: chrome-collapse D3 derives collapsed from `l.geom.sidebar_w <= 0.0` and rejects a boolean parameter by name (*"a caller could pass `true` with `sidebar_w > 0` and nothing would detect it"*), and `scroll` lives in `l.geom.scroll_px`. Both live in the layout, so the app cannot desynchronise them from what was drawn.
  The app's table is a total `match` with no fall-through:

  | `SidebarHit` | action | why |
  |---|---|---|
  | `Card(i)` | `Switch(i)` | routed through the existing `do_tab_action(Jump(i))` (`terminal_app.rs:368-375`) so clicks and Cmd+N take the identical path |
  | `NewTab` | `NewTab` | routed through `do_tab_action(TabAction::New)` — same reason |
  | `Hide` | `Collapse` | D6 |
  | `Restore` | `Expand` | D6 |
  | `Filter` | `Inert` | chrome-collapse D2: a real hit region with no hover visual, whose press must be **swallowed** so it cannot leak to a future grid handler |
  | `Panel` | `Inert` | chrome-collapse D9 returns `Panel` for sidebar dead space *specifically* so this feature can swallow it |
  | `None` | `None` | not the sidebar's business (D2) |

  **The hit-test is consulted even when `left_inset == 0`.** Collapsed, the only live region is the restore button, which floats *over the terminal* (chrome-collapse D4 accepts that it occludes ~15 cells). A guard like today's `x < sidebar_w` (`tab_bar.rs:107-113`) would make it permanently unclickable — the sidebar could be hidden but never brought back. `hit_test`'s own step 1 handles the collapsed case; the app must simply not short-circuit before calling it.
  Rejected: an app-side region table with its own coordinates (SD2 violation, and it drifts the moment a metric changes); rejected: calling `chrome_hit` and `card_at` separately from the app (that *is* re-deriving D9's cascade, and it loses the header-occlusion rule); rejected: a mouse analogue of the `seam.rs` `InputHandler` registry (`seam.rs:112-135`) — that seam exists so *other* features can extend input without editing `terminal_app.rs`, but this file is this feature's own surface and no sibling needs mouse dispatch.

### D2: Left button, on **press**, with a total mapping — inert regions are consumed, everything else keeps today's behaviour
- **Status**: resolved
- **Rationale**: Four sub-decisions bundled as one policy about *how a press is treated*. They are individually revisable — which the checkpoint table cannot express, and this sentence therefore does.
  - **Press, not release.** The existing click path acts on `ElementState::Pressed` (`terminal_app.rs:359-363`); `Released` is matched nowhere in the workspace. Rejected: conventional press-in/release-out semantics — it needs a `pressed_region` field, a `Released` arm, and a **drawn pressed state** to be worth anything, and chrome-collapse D10 specifies only a *hover* treatment. Shipping the state machine without the visual feedback is strictly worse than press-to-act: it delays every action by a release with nothing on screen explaining the delay. Deferred, Known Gaps.
  - **Left button only.** Right and middle presses fall through unchanged (today: ignored). A context menu is a native macOS affordance and a separate feature.
  - **`Filter` and `Panel` are swallowed; `None` falls through.** Both siblings depend on this: chrome-collapse D2 restored the `Filter` hit region *because* this feature swallows it (*"would let a press on the filter icon fall through to whatever future feature handles grid clicks"*), and D9 created `Panel` for the same reason. `None` means the pointer was over the grid; today that is a no-op (`terminal_app.rs:374` drops a `None`) and this feature does not change it, but it is now a named branch where a future terminal-selection feature hooks in.
  - **A click that closes the last tab must exit the app.** `do_tab_action` returns `false` in exactly that case (`:236-239`), and **today's click arm discards the bool** (`:374`) — harmless only because no click can reach `CloseActive`. This feature adds no close button, so the case stays unreachable *now*; but the arm is rewritten here, so it propagates the bool to `event_loop.exit()` exactly as the keyboard arm does (`:401-403`). Rejected: leaving the discard (a latent bug that fires silently the day a close button lands).

### D3: The app **constructs** the `SidebarLayout` from the siblings' own functions — it never reads the published one, and never computes geometry
- **Status**: resolved
- **Rationale**: chrome-collapse D7 makes every function take `&SidebarLayout`, and row-redesign D3 publishes one into the shared model after each frame — so reading `TabModelHandle::shared().layout()` looks like the obvious source, and it is the strictly stronger SD2 form (the same *values* that were drawn, not merely the same formula). **It is nevertheless the wrong choice here, because of one case that breaks it outright:** row-redesign D14 makes the row layer's `record` return immediately when `ctx.left_inset <= 0.0`, so **while the sidebar is collapsed no layout is ever published**. A session started with `[sidebar] collapsed = true` (foundations D1) would then have `layout() == None`, no hit-test, and therefore **no reachable restore button** — collapse would be an unrecoverable state fixable only by editing the config file. That is precisely the failure D1 says it exists to prevent, arrived at from the other direction.
  So the app calls the sibling's own pure constructor, `sidebar_row::layout(&geom, count)`, with a `SidebarGeom` assembled — with **no arithmetic** — from values that are each an accessor or another sibling function:

  | `SidebarGeom` field | source | arithmetic? |
  |---|---|---|
  | `sidebar_w` | `renderer.left_inset()` (`lib.rs:514`), read **live** after any toggle | none |
  | `viewport_h` | `window.inner_size().height` | none |
  | `cell` | `renderer.cell_size()` (`lib.rs:457`) | none |
  | `scale` | `renderer.scale_factor()` (foundations D10) | none |
  | `show_branch` | `Running.sidebar_cfg.show_git_branch` (D5) | none |
  | `list_top` | `sidebar_layout::header_h(scale)` — **the same function the chrome layer uses** to write the field | none |
  | `scroll_px` | `Running.scroll`, the value this feature also publishes (D10) | none |
  | `count` | `TabModelHandle::shared().with(\|m\| m.rows.len())` — the layer's own input, not `tabs.len()` | none |

  Every field is byte-identical to what the layer will use, because each comes from the same accessor or the same function, and because this feature is itself the writer of `scroll_px`. The result is "same formula **and** same inputs", which is materially stronger than "same formula" and weaker than "same values" only by one frame's staleness in `count` — and `count` is read from the model the layer reads, updated inside `do_tab_action` before any pointer event can arrive.
  **`sidebar_w` must be read live, after a toggle.** D6's step order depends on it: building the geom before `set_sidebar_collapsed` would hit-test and re-clamp against the pre-toggle width.
  Two consequences worth stating. (1) The app now calls `sidebar_row::layout` on every pointer event; it is a pure function over a handful of `f32`s (row-redesign D7 calls it once per frame in the layer), so the cost is a rounding error next to the `hit_test` it feeds. (2) The published `SidebarModel.layout` is left to its existing consumer — `tab_bar::tab_at`'s shim — and this feature does not read it, which also means this feature does not depend on whether the shim's legacy fallback survives.
  Rejected: reading the published layout (`None` while collapsed — the unrecoverable state above — and `None` before the first frame); rejected: reading the published layout with a constructed fallback (two sources for one value, differing exactly in the cases that matter, which is the drift SD2 forbids in its most confusing form); rejected: constructing a bespoke input struct (chrome-collapse D7 deleted its `SidebarLayoutInput` for precisely this reason); rejected: computing `card_h`/`stride` in the app (that is row-redesign D6's card-height formula — a textbook SD2 violation).

### D4: This feature registers `SidebarChromeLayer` and performs `sidebar-row-redesign`'s four parked migrations
- **Status**: resolved
- **Rationale**: Five pieces of wiring only this feature can do, four of them explicitly parked here by a sibling's Post-work section.
  1. **Register the chrome layer.** chrome-collapse D14 ships `SidebarChromeLayer` constructible, tested and **registered by nobody**: *"the first frame it draws is in `sidebar-interaction`."* So `start()` (`terminal_app.rs:130-138`) gains one `add_layer` call (`lib.rs:453`) beside the existing `TabBar` and `CursorLayer`. **Until this lands, the whole set's header, hide button and restore button are dark** — which is why registration is a requirement here, not a footnote.
  2. **Drop both dead `set_left_inset` calls** (`terminal_app.rs:132-134`, `settings.rs:28`). row-redesign D4 turns the method into a compat no-op and names this feature as the one that removes its callers. Leaving them would leave two calls that look authoritative, compute a stale `cell_w × 14`, and do nothing — the most misleading state possible for the next reader. Deleting the startup one is safe because D4 moves the startup inset into `assemble`.
  3. **Rename at the one call site**: `TabBarModel.titles` → `rows` and `Tabs::titles()` → `rows()` in `sync_tab_model` (`terminal_app.rs:218-223`).
  4. **Retire `input::tab_target` and its test.** row-redesign Post-work asks for it to be switched off `tab_bar::tab_at`; this spec goes one step further and **deletes** it, because `sidebar_press` (D1) supersedes it completely — one ordered `hit_test` covering cards *and* chrome *and* dead space, where `tab_target` covers only cards and only with the legacy formula. Its test is replaced by `sidebar_press`'s. This is **required, not tidy**: `crates/app` is a binary crate, so a `pub fn` with no caller still trips `dead_code` under `-D warnings`, which is a declared gate. Deleting it also removes the last caller of `tab_bar::tab_at`, which is what unblocks deleting `tab_bar.rs` and row-redesign D7's legacy fallback (both out of surface — recorded as follow-ups, not done here).
  5. **Populate `SidebarModel.hovered`** — row-redesign Post-work: *"which this feature renders but never populates."* Discharged by D8.
  Rejected: registering the chrome layer from `lib.rs` (`add_layer` is deliberately caller-side — `lib.rs:453`, *"no edit to this file is required to add a layer"*); rejected: leaving the dead `set_left_inset` calls as "harmless" (harmless to execution, actively harmful to comprehension, and a future reader restoring the old semantics would silently reintroduce a wrong width); rejected: keeping `tab_target` as a thin card-only helper (a second pointer entry point that skips D9's ordering, and the clippy gate would still need a caller); rejected: deleting `tab_bar.rs` here (out of surface).

### D5: The app stores **no** `collapsed` value; the renderer's override is the single source of truth, and nothing is written to disk
- **Status**: resolved
- **Rationale**: An earlier draft of this spec built a whole apparatus for this — a live `Config` on `Running`, a `disk_collapsed` shadow, and an edge-triggered reload rule — to stop a config save from undoing a user's click. **All of it is redundant**, because `sidebar-row-redesign` D4 solves the same problem one layer down and better: `Renderer` holds a `SidebarOverride { collapsed, width_px }` consulted *before* the config, `apply_config` leaves it untouched, and *"a runtime setter wins over the file for the rest of the session."* Reimplementing that in the app would be a second source of truth for one boolean — the exact defect the app-side version was invented to prevent. So it is deleted, and the rules are:
  - **The live value is `renderer.left_inset() <= 0.0`.** That is chrome-collapse D3's rule verbatim (*"collapsed ⇔ `sidebar_w == 0`"*), so the app, the chrome layer and `grid_dims` all read one number and cannot disagree. The toggle in D6 reads it to decide which way to flip; nothing is cached.
  - **The app holds `Running.sidebar_cfg: SidebarConfig`** — refreshed by `settings::apply` on every reload — for exactly one field, `show_git_branch`, which D3 needs to build a `SidebarGeom` and which has no runtime override. `collapsed` is deliberately *not* read from it.
  - **Nothing is written to `config.toml`.** `Config` derives `Serialize` (`config.rs:17`) but nothing in the workspace serializes it (`fs::write` appears only in test fixtures — `watch.rs:108/119/132/143`, `clipboard.rs:152/153` — and image-paste's temp PNG at `clipboard.rs:60`). Three reasons not to be the first: a durable write needs temp-file + atomic-rename + an error path for one boolean; `ConfigWatcher` watches the **parent directory** and matches by **file name** (`watch.rs:44-56`), so an app-initiated save is indistinguishable from a user edit and every toggle would deliver a `ConfigReloaded` → a redundant `apply_config` + full `resize_all`; and a `toml::to_string` round-trip drops the comments and key order of a file users are told to copy from a comment-heavy `config.example.toml`. Note that chrome-collapse D3 lists *"persisting `collapsed`"* among this feature's obligations — this decision discharges it as **explicitly not persisting**, with reasons, rather than dropping it silently.
  The inherited consequence, repeated here so it is visible from this side too: after a runtime collapse, editing `[sidebar] collapsed` in the TOML does nothing until restart. row-redesign D4 chooses that deliberately (*"the thing the user just clicked should outrank a file they edited for another reason"*); this feature does not re-litigate it. Rejected: the app-side live-`Config` + edge-trigger apparatus (redundant with the renderer override, and a second source of truth); rejected: a `Running.collapsed: bool` (a third copy — the renderer's override, the inset, and the app's bool, any two of which can disagree); rejected: writing `config.toml`; rejected: a separate `state.toml` (a second config format, load path and error case for one boolean — the right follow-up only if session-scoped collapse proves annoying, Known Gaps).

### D6: Collapse calls `Renderer::set_sidebar_collapsed`; ALL FIVE grid-refit paths share one helper
- **Status**: resolved
- **Rationale**: **The API exists for this call.** `sidebar-row-redesign` D4 adds `pub fn set_sidebar_collapsed(&mut self, c: bool)` and states it is *"not optional"*, because neutering `set_left_inset` would otherwise remove the only public write path to the inset and leave this feature's accept-seed unimplementable on a surface that excludes `lib.rs`; its Post-work says outright, *"`sidebar-interaction` calls `Renderer::set_sidebar_collapsed(bool)` from its hide/restore button."* It also **rejects by name** the alternative an earlier draft of this spec chose — *"leaving runtime collapse to a `Config` round-trip through `settings::apply`"* — which would have rebuilt the glyph atlas on every toggle (`lib.rs:545-560`) and routed a button press through a debounced file-watch path. That draft is withdrawn. The sequence:

  ```
  toggle(collapsed):
    1. renderer.set_sidebar_collapsed(collapsed)   // sets the override, then sync_left_inset()
    2. settings::refit(&renderer, &mut tabs)       // the shared helper: grid_size() AFTER (1), then resize_all
    3. let l = self.sidebar_layout();              // built from the LIVE left_inset (D3)
    4. self.scroll = clamp_scroll(self.scroll, &l); publish it (D10)
    5. refresh_hover();                            // the layout moved under a stationary pointer (D8)
    6. redraw();                                   // grid geometry changed → the snapshot IS stale (D7)
  ```

  Step 1 before step 2 is the whole ballgame: `grid_size()` (`lib.rs:471-479`) reads `grid_dims(..., self.left_inset)` (`:1021-1033`), so reading it first yields the *previous* column count and every PTY gets sized wrong — the classic symptom being a prompt that redraws over itself. Step 3 after step 1 for the same reason (D3).
  **The shared refit helper answers this decision's own steelman.** The real invariant is not "call these steps in this order"; it is *"`left_inset` and every PTY's column count agree"*. Encoding it as a procedure to remember is weak, and the workspace already proves it: `Resized` (`terminal_app.rs:378-383`) is a **fifth** refit path duplicating `grid_size()` → `resize_all()` entirely outside `settings::apply`. So `settings.rs` — this feature's surface — gains

  ```rust
  /// The ONLY place the grid is re-fitted. Reads grid_size ITSELF, after any inset
  /// change, and resizes every tab — background tabs included.
  pub fn refit(renderer: &Renderer, tabs: &mut Tabs) -> (u16, u16)
  ```

  and **all five** paths call it: startup, config reload (from inside `apply`), collapse, expand and `Resized`. The ordering rule the codebase documents at `settings.rs:25-27` then has one enforcement point instead of five conventions, because `refit` reads `grid_size()` itself and cannot be handed a stale value. `resize_all` (not the active tab) is non-negotiable: a background tab left at the old width renders wrapped garbage the moment you switch to it, which is why `Resized` already does it.
  **Two edge cases re-checked.** *Window narrower than the sidebar*: `grid_dims` floors cols at 1 (`lib.rs:1029`), degrading to a one-column grid rather than a zero-size PTY; foundations D2's 260–440 clamp makes it unreachable at normal window sizes. *`Resized` recomputing the inset*: under row-redesign D4 the inset is `width_px × scale × !collapsed`, containing no window dimension, so `Resized` still need not touch it — but it **must** call `refit` and re-clamp the scroll offset, because it is the primary event that shrinks `list_viewport.h` and therefore `max_scroll`.
  Rejected: `set_left_inset(0.0)` as chrome-collapse D3 instructs (a no-op under row-redesign D4 — the collapse would silently not happen; that spec records the contradiction in its own Known Gaps and this decision resolves it by using the setter that exists); rejected: the `Config` round-trip (rejected by name upstream; atlas rebuild per toggle); rejected: resizing only the active tab (background corruption); rejected: an animated width (each intermediate frame would `resize_all` every PTY — dozens of `ioctl`s and `SIGWINCH`s per toggle, and zsh redraws its prompt on every one); rejected: leaving `Resized` outside the shared helper because "it works today" (it is the path that changes `max_scroll`, and it is the living proof that a remembered procedure drifts).

### D7: Two-tier redraw policy — grid-affecting changes call `redraw()`, sidebar-only changes call `window.request_redraw()`
- **Status**: resolved
- **Rationale**: Under the continuous redraw loop (`terminal_app.rs:421-430`) a frame is *always* in flight, so "trigger a redraw" cannot mean "schedule a frame". What `redraw()` adds over the bare call is `snapshot_dirty = true` (`:208-215`), forcing a full re-clone of the active grid (`:412-415`). The file's own doc already draws this line.
  The load-bearing verification: **the registered-layer record loop (`lib.rs:716-738`) sits OUTSIDE the snapshot-equality branch (`:646-673`) and runs unconditionally every frame**, so a layer re-reads its model and redraws on the next frame even on a snapshot cache hit. Checked directly, and independently re-verified by chrome-collapse D10 (*"a finding that removes work, verified twice"*). Had the loop been inside the branch, this decision would be wrong and every hover would need a grid re-clone.
  - **`redraw()`** — tab switch, tab open/close, collapse/expand, config reload, resize. Unchanged from today.
  - **`window.request_redraw()`** — hover change and scroll change (D8, D10). The grid is byte-identical; re-cloning it per trackpad event is the exact idle-CPU debt G1 records as already paid down once.
  - **Neither** — the `tab-metadata` wake, handled entirely by the existing `OverlayPending` arm (`:333-338`), which is not this feature's code (D9).
  The bare call is kept rather than relying on the continuous loop because it is idempotent, winit coalesces it, and it keeps these paths correct if the workaround is reverted — which `:421-427` frames as a workaround, not a design. Rejected: routing everything through `redraw()`; rejected: doing nothing on sidebar-only changes (silently couples correctness to a documented workaround); rejected: a third `redraw_chrome()` helper (a third name for `window.request_redraw()` adds vocabulary, not clarity).

### D8: ONE `hit_test` per motion feeds BOTH published hover fields, region-debounced, with no syscall
- **Status**: resolved
- **Rationale**: The naive implementation — hit-test on `CursorMoved` and call `sync_tab_model()` — issues **one `proc_pidinfo` syscall per tab per pointer event** (`sync_tab_model` → `Tabs::rows()` → `Session::cwd()` → `cwd_of_pid`) at trackpad rates of 60–120 Hz: an SD4 violation in the file SD4 cites. Nothing here touches it.
  Both siblings converged on the app resolving hover and pushing the answer — chrome-collapse D10 (*"the app resolves hover once with `hit_test` and pushes the resolved `Option<SidebarHit>`"*, overruling its own earlier raw-cursor design **because** this feature already calls `hit_test` for clicks) and row-redesign Post-work (*"`sidebar-interaction` sets `SidebarModel.hovered`, which this feature renders but never populates"*). So one resolution serves two published values:

  ```
  on CursorMoved(p):
    last_cursor = p
    let hit = sidebar_hover(p, &self.sidebar_layout());     // ONE hit_test
    if hit == self.last_hit { return }                       // region debounce
    self.last_hit = hit;
    handle.set_hovered(match hit { Some(Card(i)) => Some(i), _ => None });
    handle.set_hovered_chrome(hit);                          // see D13 — one-line sibling gap
    window.request_redraw();                                 // D7, not redraw()
  ```

  **The debounce is on the region, not the pixel.** Moving 300 px inside one card produces **zero** model writes; only crossing a boundary does anything. `SidebarHit` needs `PartialEq` — a derive on a plain enum.
  **Borrow discipline is structural, not advisory.** row-redesign D3 makes `with`/`with_mut` *"the ONLY borrow entry points, each taking and releasing the borrow inside the call so no caller can hold one across another"*, and requires no borrow be held across a call into the renderer. This feature obeys by construction: `sidebar_layout()` takes its short borrow to read `rows.len()` and **drops it before `hit_test` runs**, and each setter is a separate call.
  **Hover must be cleared and re-derived, not only set.** Three stale-highlight bugs:
  - **The pointer leaves the window** → `WindowEvent::CursorLeft` (handled nowhere today) → clear both fields.
  - **Focus is lost with the pointer still physically inside** — Cmd-Tab does exactly this and fires **no** `CursorLeft` → `WindowEvent::Focused(false)` → same clear. Without it the highlight stays lit under another app's window.
  - **The layout moves under a stationary pointer** — collapse, expand, scroll, tab open/close, resize and reload all change which region sits under an unmoved cursor. Click **Hide** and the pointer is now over the terminal, yet the hide button stays highlighted under a sidebar that is gone. One `refresh_hover()` re-runs the resolution at `last_cursor` and is called at the end of every layout-changing path.
  Rejected: hovering via `sync_tab_model()` (the syscall storm); rejected: pushing a raw cursor and letting the layer resolve (chrome-collapse D10 withdrew that design — it would be a second resolution every frame of a question the app already answered per event); rejected: two independent hit-tests for the two fields (two answers to one question, in the one function SD2 exists to make singular); rejected: `CursorEntered`/`CursorLeft` only (no per-card resolution).

### D9: The roster is pushed from `do_tab_action` via a shadow id list; no `UserEvent` variant, no wake handler
- **Status**: resolved
- **Rationale**: `tab-metadata` D2 says the roster *"must come from the feature that owns `tabs.rs`"* — but `Tabs` is constructed at `terminal_app.rs:158`, and that spec itself notes the file *"belongs to `sidebar-interaction`"*. Meanwhile `sidebar-row-redesign`, which owns `tabs.rs`, never mentions `set_roster`. **The push is unassigned, and this is the only feature that can reach both `Tabs` and `tabmeta::global()`.** It is claimed here.
  **The obstruction and the in-surface answer.** `Tabs.sessions` is private and there is **no iterator and no index→id accessor** (`tabs.rs:46-156`) — the roster cannot be read off. But `open()` returns the new `TabId`, `active() -> &Session` is public, and `Session.id` / `Session.pty` are public with `Pty::child_pid()` public. So `Running` keeps `roster: Vec<TabPid>` maintained by exactly two operations mirroring exactly two `Tabs` mutations, both inside `do_tab_action` — the provably single mutation path for chords *and* clicks (D1):
  - `TabAction::New` → **capture** the `Ok(TabId)` that `:230-233` currently discards → `by_id_mut(id)` → `child_pid()`; push `(id, pid)`. On `Err` the tab does not exist, so the roster is unchanged. On `child_pid() == None` the entry is **skipped**: `TabPid.pid` is `u32` while `child_pid()` is `Option<u32>`, and a tab with no pid is a tab the worker could not walk anyway. It is picked up on the next push; `tab-metadata` D14 requires the worker to tolerate exactly this without panicking.
  - `TabAction::CloseActive` → read `tabs.active().id` **before** closing → remove that entry. When `close_active()` returns `false` (the last tab, `tabs.rs:88-96`), `do_tab_action` returns at `:236-239` **before** any tail work — so no push happens on the way out, which is correct because the process is exiting.
  `Next`/`Prev`/`Jump` do not change the roster; they change the active id. `set_active(Some(tabs.active().id))` and `set_roster(&self.roster)` are called on **every** action uniformly, so a conditional cannot get out of sync. Order within the `Vec` is irrelevant — `tab-metadata` keys by `TabId`.
  A shadow copy of another structure's membership is a real smell, named as one. It is bounded by three facts: it is mutated in one function, in two branches, each adjacent to the `Tabs` call it mirrors; a desync is self-healing (`set_roster` replaces the whole roster, so a wrong entry lasts one tick and a closed tab's metadata simply disappears); and no other path mutates the tab set. The **correct** fix — `Tabs::roster()` or constructor injection, which would also let `tab-metadata`'s `OnceLock` locator be deleted as its own D2 anticipates — needs `tabs.rs`, out of surface. Follow-up, Known Gaps.
  **No `UserEvent` variant and no wake handler.** An earlier draft added `UserEvent::TabMetaUpdated`; `tab-metadata` D3 **rejects that by name** — *"it would collide with `sidebar-interaction`, which owns `terminal_app.rs`"* — and rides the existing `OverlayPending` path through a permanently-invisible `TabMetaPump` registered from `main.rs`. `terminal_app.rs:333-338` already calls `poll_all()` and `redraw()`s iff it returns `true`, and D4 makes publication change-only so it fires only on a real change. **This feature therefore writes no wake code**, and the earlier decision is deleted rather than merged.
  This feature also does **not** read metadata: row-redesign D10 has the row layer pull it through a `MetaSource` installed from `main.rs` by row-redesign itself. This feature's entire relationship with `tab-metadata` is the roster and the active id.
  Rejected: a per-frame or per-`CursorMoved` push (SD4); rejected: pushing from the individual `open`/`close`/`jump` call sites (scattered — the reason `do_tab_action` exists); rejected: adding a `UserEvent` variant (rejected upstream by name); rejected: adding `Tabs::roster()` (out of surface — the reason for the shadow list); rejected: deleting the `OnceLock` locator now (same reason).

### D10: Both wheel unit systems; an app-owned physical-px offset that is **published**, clamped, and revealed on tab change
- **Status**: resolved
- **Rationale**: chrome-collapse D6 assigns the offset here unambiguously: *"`sidebar-interaction` owns the `f32`, mutates it on `MouseWheel`."* So `Running.scroll: f32`, physical px.
  **It must be published, not just held.** The offset feeds two places: the hit-test (via `SidebarGeom.scroll_px`, D3) **and the drawn cards** — row-redesign D13 has the row layer copy `scroll_px` out of the shared model into its `SidebarGeom` rather than hardcoding `0.0`, via `TabModelHandle::set_list_scroll(list_top, scroll_px)`. Without the publish, a wheel would move the hit-test while the cards stayed put — a silent, maximally confusing divergence. The setter takes both fields; this feature passes `sidebar_layout::header_h(scale)` for `list_top`, which is byte-identical to what the chrome layer writes (chrome-collapse D11), so the two writers cannot clobber each other with different values.
  **Both unit systems.** winit 0.30 delivers `LineDelta(f32,f32)` for a wheel and `PixelDelta(PhysicalPosition<f64>)` for a trackpad; handling one leaves half the input devices dead.
  - `PixelDelta(p)` — `p.y` is already **physical** px, the space the layout lives in (SD2). Used directly.
  - `LineDelta(_, y)` — abstract lines. For a list of cards the meaningful line is **one card**, so it multiplies by `l.stride` — a public field on the sibling's `SidebarLayout` (chrome-collapse D7 deleted its `row_stride` function in favour of it). One notch advances the list by exactly one card, which is what every list UI does, and the number stays on the renderer side.
  **Sign.** The offset is "how far the list has scrolled down" (`card_at` adds it to `y`), and winit reports positive `y` for content moving down, so `new = clamp_scroll(scroll − delta_y, &l)`. macOS's natural-scrolling preference is already baked into the OS-supplied delta; inverting again would double-apply it. This is the one thing in the spec that **cannot be verified without hardware** — a hypothesis with a behavioural acceptance criterion ("the cards follow the fingers"), checked via `/run-term`, and flipping it is one character. Known Gaps.
  **Gating** is by `hit_test`, not by `x < left_inset` — same reason as D1: collapsed there is no band, and a wheel over the terminal must not move an invisible list. A wheel resolving to `None` falls through to today's behaviour (nothing; the terminal has scrollback but no wheel binding — a future feature, and this is where it hooks in).
  **Clamping is the sibling's** (`clamp_scroll`, total and NaN-safe) and must be re-applied whenever `max_scroll` changes, not only on a wheel: **closing tabs** shrinks the content and **resizing the window** shrinks `list_viewport.h`, and both leave a stored offset past the new maximum, scrolling the list into empty space. So `do_tab_action`, `Resized` and the collapse toggle all re-clamp. A wheel when the content already fits clamps to the same value → no change → no publish, with no special case.
  **`scroll_to_reveal` on tab change.** chrome-collapse D6 requires this feature to call `scroll_to_reveal(active, scroll, &l)` on tab change, calling its absence *"a correctness bug, not a nicety"* — a `Cmd+9` to a tab below the fold would otherwise highlight a card the user cannot see — and it explicitly hands over the risk that *"a caller can forget to call it"*. So the call lives in `do_tab_action`, the single path both chords and clicks take, which is the one place it cannot be forgotten. It runs on **every** tab action including `New`, since a newly-opened tab is appended and made active and is therefore the one most likely to be below the fold.
  Rejected: pixel-only (breaks every wheel mouse); rejected: line-only with a hardcoded px-per-line (breaks trackpad smoothness and hardcodes layout in `app`); rejected: clamping in `app` (needs `content_height` — an SD2 violation); rejected: holding the offset without publishing it (hit-test and cards diverge); rejected: applying reveal per frame (chrome-collapse D6 rejects it — a user who scrolls away is snapped back 60×/second); rejected: a sub-pixel accumulator (`f32` px already resolves finer than a physical pixel).

### D11: Everything decidable is a pure function in `input.rs`; the winit arms are adapters with no logic
- **Status**: resolved
- **Rationale**: **Nothing in this repository constructs an `EventLoopProxy` in a test** — `Running` cannot be built without a window, a GPU and a live zsh, so anything placed there is permanently untested. The codebase's answer is already visible: `input.rs:52-60` extracts the pointer→intent translation as a free function *specifically* so `input.rs:122-137` can test it with a literal cursor tuple; `tabs.rs:159-185` does the same for index arithmetic; `seam.rs:199-255` hand-rolls trait fakes rather than pull in a mocking crate.
  - **`input.rs`** holds `SidebarAction`, `WheelDelta` (a small app-side enum over winit's two units, so the translation is testable without constructing a winit event), `sidebar_press`, `sidebar_hover`, `wheel_scroll`, and the roster-diff helpers. All pure, all callable with a **literal `SidebarGeom`** fed through `sidebar_row::layout` — which row-redesign D7 publishes as a pure `pub fn` over a plain struct, and chrome-collapse D7 declares its own module *"pure, state-free, GPU-free"*. No device, no window.
  - **Tests assert equality against the sibling's function** — the `tab_target_maps_click_to_tab_and_delegates_geometry` pattern (`input.rs:131-136`), carried forward to `sidebar_press` vs `sidebar_layout::hit_test`. That equality is what proves the app added no geometry of its own (SD2), the single most important property of this feature.
  - **`terminal_app.rs`** arms destructure the winit event, call one pure function, and `match`. The logic that must live there is D6's sequencing and D8/D10's model writes, neither constructible headlessly. That limit is stated rather than hidden: R2's PTY-consistency criteria are verified by inspection plus `/run-term`, not by `cargo test` — which is exactly why D6 collapses all five refit paths into one helper, so there is one place to get it wrong rather than five.
  Rejected: a headless winit event loop (no precedent, needs a display); rejected: a trait abstraction over `Renderer`/`Tabs` to make `Running` mockable (a large indirection across three files to test a short call sequence, against a codebase that consistently extracts the pure part instead); rejected: integration tests that spawn the app (`zsh-local-only`, no headless display).

### D12: `main.rs` gains nothing from this feature — `tab-metadata` already owns its only needed edit
- **Status**: resolved
- **Rationale**: `main.rs:44` moves `proxy` into `AiMonitorOverlay::new(proxy)`, so anything registered afterwards needing a proxy fails to compile. An earlier draft of this spec claimed that one-token `proxy.clone()` edit. **`tab-metadata` D3 already claims it** — *"that line becomes `proxy.clone()` and this feature's `spawn` takes the original — a one-token edit inside the declared surface"* — and `tab-metadata` lands **before** this feature in the manifest order. `main.rs` is ordered-shared, so the correct behaviour for the later feature is to **verify, not repeat**: if the clone is there, this feature changes nothing; if `tab-metadata` shipped differently, this feature makes the edit then, because by that point it is the last writer and the compile error is unambiguous. Duplicating a claimed edit on an ordered-shared file is how ordered-shared surfaces produce conflicts.
  Beyond that, `input`, `settings` and `terminal_app` are already declared (`:10-17`) and `sidebar_model`/`tabmeta` are declared by their own features, so this feature's `main.rs` diff is expected to be **empty** — the correct outcome for a file four features touch. Rejected: a defensive extra `proxy.clone()` (guarding against a sibling's edit that is already specified); rejected: dropping `main.rs` from the declared surface (the partition is the human's, approved at decompose's gate; narrowing it unilaterally is not this spec's call, and a compile-time need could still arise).

### D13: Sibling signatures are **revision-dated**, and a mismatch is adapted here — never worked around, never reimplemented
- **Status**: resolved
- **Rationale**: This is the most important decision in the spec, and it exists because an earlier draft **deleted** it and was wrong twice within one review as a result. The three sibling specs this feature programs against are being written concurrently; two of them changed shape *during* this spec's review, and — crucially — **two changed toward this spec**: `sidebar-chrome-collapse` D7's reconciliation table cites this feature's escalation rule as *"binding and favourable"* and adopts `SidebarHit` verbatim; its D2 restores the `Filter` hit region on this spec's argument; its D10 withdraws its raw-cursor handle because this feature already calls `hit_test`; and `sidebar-row-redesign` D4 adds `set_sidebar_collapsed` after this spec's draft showed the accept-seed was otherwise unimplementable. **The rule is load-bearing infrastructure for the whole set, not boilerplate.** It has three parts:
  1. **Every sibling signature quoted in this spec is dated** (Research, "read from the current spec revisions") and is a *best-known* fact, not a frozen one. A reader at implementation time must re-read the sibling spec, not this quotation.
  2. **On a mismatch, the implementer adapts THIS feature's call sites** to the real signature. Every app-side function is a thin delegate precisely so an adaptation is one line each — that property is designed in (D1, D3, D11), not incidental.
  3. **If a needed function or field does not exist at all, it is added to the sibling's file as a human decision — never reimplemented in `crates/app/**`.** `sidebar_layout.rs`, `sidebar_header.rs`, `sidebar_row.rs`, `tab_bar.rs`, `sidebar_model.rs`, `tabs.rs`, `tabmeta/**`, `lib.rs` and `crates/common/**` are outside this surface; editing them here is the overlap the partition exists to prevent, and duplicating them in `app` is the drift SD2 exists to prevent.
  **The one known instance of part 3, sized exactly.** chrome-collapse D10/D11 require the app to publish a resolved chrome region into the shared model, but `sidebar-row-redesign` D3's current `SidebarModel` declares `hovered: Option<usize>` and **no `hovered_chrome: Option<SidebarHit>`**, and its handle exposes `set_hovered` but no `set_hovered_chrome`. The diff is **one field and one setter in `sidebar_row.rs`** — a file belonging to `sidebar-row-redesign`, which lands first and whose D3 states the governing principle itself: *"Every field a sibling needs is declared HERE, now … a field 'the sibling adds later' is a field the sibling cannot add at all without a declared-surface overlap."* By that principle the field belongs in `sidebar-row-redesign`, and the safest sequencing is to fold it in there. Escalated, not worked around; recorded in Known Gaps and Pre-work. **Until it exists, D8 publishes only `hovered` and the chrome buttons have no hover highlight** — a cosmetic gap, not a functional one, so this feature is not blocked by it.
  Rejected: hard-asserting the sibling signatures as frozen facts (exactly what the round-2 draft did, and it was stale within hours); rejected: designing against no assumption at all (the spec would then have no testable requirements); rejected: defining a local copy of any missing function "temporarily" (temporary geometry duplication is permanent drift, and SD2 names it); rejected: waiting for the siblings to stabilise (batch mode runs them concurrently by design, and the `depends_on` edges already sequence the build, which is where the real signatures land).

## Constraints
- **No new crate dependency.** `Cargo.lock` edits are hook-blocked; winit 0.30.13 supplies every event type needed.
- **No layout maths in `crates/app/**`** (SD2). Every distance, region, stride and clamp comes from a `renderer` call. The one boundary case — assembling a `SidebarGeom` — contains no arithmetic (D3).
- **No edits outside the declared surface.** `sidebar_layout.rs`, `sidebar_header.rs`, `sidebar_row.rs`, `tab_bar.rs`, `sidebar_model.rs`, `tabs.rs`, `tabmeta/**`, `lib.rs`, `crates/common/**` and `crates/term-core/**` are siblings' or frozen — a needed change there is an **escalation** (D13).
- **No `UserEvent` variant may be added** — `tab-metadata` D3 designed around not having one, specifically to avoid colliding here.
- **Borrow discipline on `TabModelHandle::shared()`**: only through the sibling's `with`/`with_mut`/setters, never held across a call into `renderer` (row-redesign D3's hard rule; this feature is a writer on a pointer-rate path).
- **No per-frame and no per-`CursorMoved` I/O** (SD4). `Tabs::rows()` — one `proc_pidinfo` per tab per call — may be reached only from tab-action paths, never from pointer-motion or redraw paths.
- **No dead code.** `crates/app` is a binary crate, so an uncalled `pub fn` fails `-D warnings`; anything this feature supersedes must be deleted, not orphaned (D4).
- `term-core` untouched; `renderer` never learns about `winit`; no `#[cfg(target_os)]`; zsh-local-only; no `unwrap()` outside `#[cfg(test)]`.
- **No test may construct an `EventLoopProxy`, a `Window`, a GPU device or a PTY** (D11).
- Existing behaviour preserved where not deliberately changed: keyboard chords, PTY encoding, the overlay/router dispatch order, and the ignore-by-default treatment of grid-area presses.
- All three gates green: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

## Known Gaps
- **The sibling signatures quoted here are dated 2026-07-20 and changed twice during this spec's review** (D13). Re-read the sibling specs before implementing; adapt call sites, escalate missing items. This is a standing condition of this feature, not a one-off.
- **`SidebarModel` has no `hovered_chrome` field and the handle has no `set_hovered_chrome`** (D13). `sidebar-chrome-collapse` D10/D11 require it; `sidebar-row-redesign` D3 declares every other sibling-written field but not this one. One field + one setter in `sidebar_row.rs`, which by that spec's own stated principle belongs there. Until it lands, the header/restore buttons have no hover highlight — cosmetic, not blocking. **Pre-work.**
- **CROSS-SPEC: `Renderer::set_left_inset` has two readings.** `sidebar-chrome-collapse` D3 tells this feature to call `set_left_inset(0.0)`; `sidebar-row-redesign` D4 makes that a compat no-op and supplies `set_sidebar_collapsed` instead. D6 uses the setter that exists, which satisfies chrome-collapse's *observable* contract (`left_inset` becomes 0). Both sibling specs already record the contradiction; a human should close it so the prose stops disagreeing.
- **CROSS-SPEC: z-order.** `sidebar-chrome-collapse` D4 fixes `SIDEBAR_CHROME_Z = 30` (above `IMAGE_Z = 20`, so the restore button is not buried by a pasted image); `sidebar-row-redesign`'s Post-work asks it to register at `z_order = 11`. This feature only calls `add_layer` with whatever the layer reports, but the two specs disagree and 11 would reintroduce the failure D4 exists to prevent.
- **The wheel sign (D10) is a hypothesis** verifiable only on hardware via `/run-term`.
- **`Running.roster` shadows `Tabs`' membership** (D9) because `Tabs` exposes no iterator and `tabs.rs` is out of surface. Self-healing and confined to one function; the correct fix is `Tabs::roster()`, which would also let `tab-metadata`'s `OnceLock` locator be deleted, as its own D2 anticipates.
- **`scroll_to_reveal` is a call a caller can forget** — chrome-collapse D6 says so and hands the risk over. D10 mitigates by putting it in `do_tab_action`; nothing enforces it.
- **After a runtime collapse, editing `[sidebar] collapsed` in the TOML does nothing until restart** — inherited from `sidebar-row-redesign` D4's override precedence (D5), deliberate there, repeated here so it is visible from this side.
- **`collapsed` does not survive a restart unless the user edits the config by hand** (D5). A separate `state.toml` — rejected as disproportionate for one boolean — is the follow-up if it proves annoying.
- **`ScaleFactorChanged` is unhandled**, so moving between a retina and a non-retina display leaves `Renderer::scale_factor` (`lib.rs:163`, set once at `:433`) stale — the sidebar width *and* the cell metrics drift. Pre-existing (`cell_size_px` has the same exposure); the fix needs a `lib.rs` setter.
- **`App.cfg` is stale after the first config reload** (`terminal_app.rs:322-332` discards the reloaded `Config`). D5 sidesteps it for the sidebar by keeping `Running.sidebar_cfg` refreshed in `settings::apply`; whether `App.cfg` should be updated wholesale is a separate human decision.
- **D6's call sequence is verified by inspection and `/run-term`, not by `cargo test`** (D11). A regression reordering the inset change and the `grid_size()` read would compile, pass every unit test, and mis-size the shell. Mitigated by the single `refit` helper; not eliminated.
- **No press/release semantics, drag, double-click, close button, reorder or drag-resize** (D2). Press-to-act is correct only while there is no drawn pressed state.
- **`Tabs::rows()` re-walks each tab's cwd that `tab-metadata` has already resolved.** Deduping needs `tabs.rs`; `sidebar-row-redesign` D2 keeps the existing derivation deliberately.
- **A config reload racing a collapse click is resolved by "last event on the UI thread wins"** — correct, but incidental: it follows from the single-threaded event loop, not from a deliberate lock. The renderer override (D5) makes the outcome stable either way.

## Requirements

### R0: The feature integrates the set, and the gates are green

#### R0.1: The chrome layer draws for the first time
- **Given**: `sidebar-chrome-collapse` has landed with `SidebarChromeLayer` registered by nobody
- **When**: the terminal is launched after this feature
- **Then**: `start()` has called `renderer.add_layer(Box::new(SidebarChromeLayer::new()))`, and the header with its new-tab, filter and hide buttons is visible above the card list

#### R0.2: All three gates pass
- **Given**: the completed feature
- **When**: `cargo build`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings` are run
- **Then**: all three exit 0, with no `unwrap()` outside `#[cfg(test)]` and no `dead_code` warning from a superseded item

#### R0.3: Keyboard behaviour is unchanged
- **Given**: the terminal after this feature
- **When**: Cmd+T, Cmd+W, Cmd+[, Cmd+], Cmd+1..9 and ordinary typing are exercised
- **Then**: each behaves exactly as before — the overlay → router → chord → `encode_key` dispatch order (`terminal_app.rs:254-292`) is untouched and no keystroke is diverted by any pointer path

### R1: Click routing (D1, D2)

#### R1.1: A press on a card switches to that tab
- **Given**: three open tabs with tab 0 active, and the pointer over the second card
- **When**: the left button is pressed
- **Then**: `hit_test` returns `Card(1)`, the app calls `do_tab_action(TabAction::Jump(1))`, and tab 1 becomes active — the identical path Cmd+2 takes

#### R1.2: A press on the new-tab button opens a tab
- **Given**: one open tab and the pointer over the header's new-tab button
- **When**: the left button is pressed
- **Then**: `hit_test` returns `NewTab`, `do_tab_action(TabAction::New)` runs, and a second tab exists and is active

#### R1.3: A press on the filter button is swallowed
- **Given**: the pointer over the header's filter icon
- **When**: the left button is pressed
- **Then**: `hit_test` returns `Filter`, `sidebar_press` returns `Some(SidebarAction::Inert)`, no tab changes, no collapse occurs, and the press is **not** forwarded to any grid handler

#### R1.4: A press on sidebar dead space is swallowed
- **Given**: the pointer inside the sidebar but below the last card
- **When**: the left button is pressed
- **Then**: `hit_test` returns `Panel`, `sidebar_press` returns `Some(SidebarAction::Inert)`, and nothing changes

#### R1.5: A press over the terminal grid falls through
- **Given**: the pointer to the right of the sidebar band
- **When**: the left button is pressed
- **Then**: `hit_test` returns `None`, `sidebar_press` returns `None`, and the app takes today's no-op path

#### R1.6: The app adds no geometry of its own
- **Given**: any cursor point, any `SidebarLayout`
- **When**: `sidebar_press` is evaluated and compared against `sidebar_layout::hit_test` at the same point
- **Then**: the region `sidebar_press` acted on is exactly the one `hit_test` returned, asserted by a unit test that calls both — the `input.rs:131-136` pattern carried forward

#### R1.7: Non-left buttons and releases do nothing
- **Given**: the pointer over a card
- **When**: the right button is pressed, or the left button is released
- **Then**: no tab switch, no collapse, no model write — only `ElementState::Pressed` with `MouseButton::Left` is acted on

#### R1.8: A click that would close the last tab exits the app
- **Given**: one open tab and a click path that reaches `TabAction::CloseActive`
- **When**: `do_tab_action` returns `false`
- **Then**: the click arm propagates it to `event_loop.exit()` rather than discarding it, exactly as the keyboard arm at `terminal_app.rs:401-403` does

### R2: Collapse and expand keep the grid and every PTY consistent (D6)

#### R2.1: The hide button collapses and returns the width to the grid
- **Given**: an expanded sidebar with `left_inset > 0` and a grid of N columns
- **When**: the hide button is pressed
- **Then**: `renderer.set_sidebar_collapsed(true)` runs, `left_inset()` becomes `0.0`, and the grid reports more than N columns

#### R2.2: The restore button expands and gives the width back
- **Given**: a collapsed sidebar drawing only the floating restore button
- **When**: the restore button is pressed
- **Then**: `set_sidebar_collapsed(false)` runs, `left_inset()` returns to `width_px × scale_factor`, and the grid returns to its previous column count

#### R2.3: Every PTY is re-fitted, background tabs included
- **Given**: three open tabs with tab 0 active
- **When**: the sidebar is collapsed
- **Then**: `resize_all(cols, rows)` is called with the **new** dimensions, so switching to tab 2 afterwards shows correctly-wrapped output rather than text wrapped at the old column count

#### R2.4: The grid size is never read before the inset changes
- **Given**: the collapse path
- **When**: its call order is inspected
- **Then**: `set_sidebar_collapsed` precedes `settings::refit`, and `refit` calls `renderer.grid_size()` itself rather than accepting a caller-supplied value — so a stale column count cannot be passed in

#### R2.5: A session started collapsed can be expanded by clicking
- **Given**: `config.toml` with `[sidebar] collapsed = true`, so the row layer publishes no `SidebarLayout` (`tab_bar.rs` early-returns on `left_inset <= 0.0`)
- **When**: the user clicks the floating restore button
- **Then**: the app's **constructed** layout (D3) yields `sidebar_w == 0`, `hit_test` takes its collapsed branch, returns `Restore`, and the sidebar expands — collapse is never an unrecoverable state

#### R2.6: A window narrower than the sidebar does not produce a zero-size PTY
- **Given**: a window resized narrower than `width_px × scale`
- **When**: `refit` runs
- **Then**: `grid_dims` floors the column count at 1 (`lib.rs:1029`) and every PTY is resized to a valid, non-zero size

### R3: One shared re-fit helper serves all five paths (D6)

#### R3.1: `settings::refit` is the only re-fit site
- **Given**: the completed feature
- **When**: `crates/app/src` is searched for `resize_all`
- **Then**: the only call is inside `settings::refit`, and `refit` is called from exactly five places — startup, `settings::apply`, collapse, expand, and the `Resized` arm

#### R3.2: A window resize routes through the helper
- **Given**: a running terminal
- **When**: the window is resized
- **Then**: `renderer.resize()` runs, then `settings::refit`, then the scroll offset is re-clamped and hover re-derived — replacing the ad-hoc `grid_size()` → `resize_all()` pair at `terminal_app.rs:378-383`

#### R3.3: A config reload still re-fits correctly
- **Given**: a running terminal and an edited `config.toml` that changes `font.size_pt`
- **When**: the watcher delivers it and `settings::apply` runs
- **Then**: `apply_config` runs first, `refit` reads the new `grid_size()`, every PTY is resized, and `Running.sidebar_cfg` is refreshed — with no `set_left_inset` call anywhere in the path

#### R3.4: A runtime collapse survives an unrelated config save
- **Given**: the user has collapsed the sidebar by clicking, and `config.toml` still says `collapsed = false`
- **When**: the user saves an unrelated change (a theme colour) and `apply_config` runs
- **Then**: the sidebar stays collapsed, because `Renderer`'s override is consulted before the config (row-redesign D4) and the app stores no competing copy

### R4: Hover (D8)

#### R4.1: Moving over a card publishes its index
- **Given**: the pointer outside the sidebar
- **When**: it moves onto the second card
- **Then**: one `hit_test` runs, `SidebarModel.hovered` becomes `Some(1)`, and `window.request_redraw()` — not `redraw()` — is called, so `snapshot_dirty` stays false

#### R4.2: Moving within one card publishes nothing
- **Given**: the pointer already over card 1
- **When**: it moves 300 px while staying inside card 1's stride band
- **Then**: the resolved region is unchanged, so **no** model write and no redraw request occur

#### R4.3: Hover never performs I/O
- **Given**: three open tabs
- **When**: 1000 `CursorMoved` events are processed
- **Then**: `Tabs::rows()` is not called and no `proc_pidinfo` syscall is issued on this feature's behalf

#### R4.4: The pointer leaving the window clears the highlight
- **Given**: the pointer over the hide button with the highlight lit
- **When**: `WindowEvent::CursorLeft` arrives
- **Then**: both hover fields are cleared to `None`

#### R4.5: Losing focus clears the highlight even with the pointer inside
- **Given**: the pointer over a card, highlight lit, and the user presses Cmd+Tab (no `CursorLeft` is emitted)
- **When**: `WindowEvent::Focused(false)` arrives
- **Then**: both hover fields are cleared, so no highlight is left lit under another application's window

#### R4.6: A layout change under a stationary pointer re-derives the hover
- **Given**: the pointer over the hide button and the sidebar expanded
- **When**: the hide button is pressed and the sidebar collapses, with the pointer not moving
- **Then**: `refresh_hover()` re-runs `hit_test` at `last_cursor`, the hide button's highlight clears, and the region now under the pointer (the terminal → `None`) is published

#### R4.7: The card hover and the chrome hover come from one resolution
- **Given**: any cursor point
- **When**: hover is updated
- **Then**: exactly one `hit_test` call produces both published values — `hovered` from a `Card(i)` result and the chrome region from the same result — so the card highlight and the button highlight can never disagree about where the pointer is

### R5: Scroll (D10)

#### R5.1: A trackpad scroll moves the list by pixels
- **Given**: more tabs than fit, with `scroll == 0`
- **When**: a `PixelDelta` event with `y = -40.0` arrives over the sidebar
- **Then**: `scroll` becomes `clamp_scroll(40.0, &l)`, and the value is published via `set_list_scroll` so the drawn cards move by the same amount the hit-test does

#### R5.2: A wheel scroll moves the list by whole cards
- **Given**: more tabs than fit and a layout whose `stride` is 70 px
- **When**: a `LineDelta` event with `y = -1.0` arrives over the sidebar
- **Then**: `scroll` advances by exactly one `l.stride`, not by a cell height or a hardcoded constant

#### R5.3: Scrolling is clamped at both ends
- **Given**: a list scrolled to `max_scroll`
- **When**: further scroll-down events arrive
- **Then**: `clamp_scroll` pins the offset at `max_scroll`, with no overscroll and no rubber-band; scrolling up past 0 pins at 0

#### R5.4: A wheel over a list that fits does nothing
- **Given**: two tabs in a viewport that fits ten
- **When**: any wheel event arrives over the sidebar
- **Then**: `max_scroll == 0`, the clamped result equals the current offset, and no model write occurs

#### R5.5: A wheel over the terminal does not move the list
- **Given**: the pointer over the terminal grid, and a collapsed sidebar in a second case
- **When**: a wheel event arrives
- **Then**: `hit_test` returns `None`, the offset is unchanged, and the event falls through to today's behaviour

#### R5.6: Closing a tab re-clamps a now-out-of-range offset
- **Given**: eight tabs scrolled to the bottom
- **When**: four tabs are closed
- **Then**: `do_tab_action` re-clamps the offset against the new `max_scroll` and publishes it, so the list does not display empty space below the last card

#### R5.7: Shrinking the window re-clamps the offset
- **Given**: a list scrolled to the bottom
- **When**: the window is made shorter, reducing `list_viewport.h` and therefore `max_scroll`
- **Then**: the `Resized` path re-clamps and publishes, leaving the last card flush with the viewport bottom

#### R5.8: A chord switch to an off-screen tab scrolls it into view
- **Given**: twenty tabs, scrolled to the top, and tab 18 below the fold
- **When**: the user presses Cmd+Shift+] repeatedly until tab 18 is active
- **Then**: `do_tab_action` calls `scroll_to_reveal(18, scroll, &l)` and publishes the result, so the active card is fully visible rather than highlighted off-screen

#### R5.9: Reveal does not fight a user who scrolled away
- **Given**: the active tab is visible and the user has scrolled to a different part of the list
- **When**: frames are rendered continuously with no tab action
- **Then**: the offset is unchanged — `scroll_to_reveal` runs only on a tab action, never per frame

### R6: Driving `tab-metadata` (D9)

#### R6.1: Opening a tab pushes the new roster
- **Given**: two tabs, with `tabmeta::global()` installed
- **When**: a third tab is opened by chord or by clicking the new-tab button
- **Then**: `set_roster` is called with three `TabPid` entries and `set_active` with the new tab's id, from `do_tab_action` — the single path both triggers take

#### R6.2: Closing a tab removes it from the roster
- **Given**: three tabs with tab 1 active
- **When**: tab 1 is closed
- **Then**: the entry for tab 1's `TabId` is removed **before** the close, and `set_roster` is called with the remaining two

#### R6.3: Switching tabs updates only the active id
- **Given**: three tabs
- **When**: the user jumps to tab 2
- **Then**: `set_active(Some(id_of_2))` is called and the roster's membership is unchanged

#### R6.4: A tab with no resolvable pid is skipped, not faked
- **Given**: a newly opened session whose `Pty::child_pid()` returns `None`
- **When**: the roster is built
- **Then**: no `TabPid` is emitted for it (there is no `u32` to emit), the push proceeds with the remaining tabs, and the entry appears on the next push once a pid exists

#### R6.5: A failed `open` leaves the roster unchanged
- **Given**: `Tabs::open` returning `Err`
- **When**: `do_tab_action(New)` handles it
- **Then**: the warning is logged as today, no `TabPid` is pushed, and the roster still matches the live tab set

#### R6.6: Closing the last tab pushes nothing
- **Given**: one open tab
- **When**: it is closed and `do_tab_action` returns `false` at `terminal_app.rs:236-239`
- **Then**: no roster push occurs — the app is exiting, and the early return is before the tail work

#### R6.7: No roster work happens per frame or per pointer event
- **Given**: a running terminal
- **When**: 1000 frames are rendered and 1000 `CursorMoved` events are processed with no tab action
- **Then**: `set_roster` and `set_active` are not called

#### R6.8: The metadata wake path is untouched
- **Given**: `tab-metadata`'s worker publishing a change
- **When**: it wakes the loop
- **Then**: the existing `UserEvent::OverlayPending` arm (`terminal_app.rs:333-338`) handles it, no new `UserEvent` variant exists, and this feature contributed no wake-handling code

### R7: The parked migrations are completed (D4)

#### R7.1: Both dead `set_left_inset` calls are gone
- **Given**: the completed feature
- **When**: `crates/app/src` is searched for `set_left_inset`
- **Then**: there are no matches — `terminal_app.rs:132-134` and `settings.rs:28` are removed, and the startup inset comes from `Renderer::assemble`

#### R7.2: The model call site uses the renamed API
- **Given**: `sidebar-row-redesign` renamed `TabBarModel.titles` → `rows` and `Tabs::titles()` → `rows()`
- **When**: `sync_tab_model` (`terminal_app.rs:218-223`) is compiled
- **Then**: it uses the new names and the workspace builds

#### R7.3: `tab_target` is deleted, not orphaned
- **Given**: `sidebar_press` supersedes it
- **When**: the feature is built with `-D warnings`
- **Then**: `input::tab_target` and its test no longer exist, `tab_bar::tab_at` has no caller in `crates/app`, and no `dead_code` warning is emitted

#### R7.4: `SidebarModel.hovered` is populated
- **Given**: `sidebar-row-redesign` renders `hovered` but never writes it
- **When**: the pointer moves over a card
- **Then**: `hovered` is `Some(i)` and the card draws its hover background — the field is no longer permanently `None`

### R8: Purity and testability (D3, D11, D13)

#### R8.1: Every pointer decision is unit-testable with no window
- **Given**: a literal `SidebarGeom` passed through `sidebar_row::layout`
- **When**: `sidebar_press`, `sidebar_hover` and `wheel_scroll` are called from a `#[cfg(test)]` module
- **Then**: they run with no `Window`, no `EventLoopProxy`, no GPU device and no PTY, following the `input.rs:122-137` idiom

#### R8.2: The winit arms contain no decision logic
- **Given**: the `MouseInput`, `CursorMoved`, `MouseWheel`, `CursorLeft` and `Focused` arms
- **When**: they are read
- **Then**: each destructures the event, calls one pure `input.rs` function, and matches the result — every branch condition lives in the pure function

#### R8.3: The layout is assembled from sibling functions only
- **Given**: the app's `SidebarGeom` construction
- **When**: it is read
- **Then**: every field is a `Renderer` accessor, a `Window` value, `sidebar_layout::header_h`, `Running.sidebar_cfg`, `Running.scroll`, or the shared model's `rows.len()` — and no arithmetic appears

#### R8.4: A missing sibling item is escalated, not reimplemented
- **Given**: a sibling function or field this spec cites that does not exist at implementation time (e.g. `set_hovered_chrome`)
- **When**: the implementer reaches it
- **Then**: the call site is adapted to the real signature, or the item is added to the sibling's own file as a human decision — no equivalent is defined anywhere under `crates/app/**`

## Tasks

> This feature's four files are small and every behavioural slice touches `terminal_app.rs`, so the L4 file-overlap rule serialises almost everything. The DAG is stated honestly as a near-linear chain rather than dressed up as wide; only T1 and T2 are genuinely independent.

### T1: Pure pointer-intent layer in `input.rs` [infra]
- **Fulfills**: R1.6, R8.1, R8.2 (the pure half), R7.3 (deletion)
- **Depends on**: (none)
Adds `SidebarAction`, `WheelDelta`, `sidebar_press`, `sidebar_hover`, `wheel_scroll` and the roster-diff helper, each delegating to `sidebar_layout::hit_test` / `clamp_scroll` / `l.stride`; deletes `tab_target` and its test and replaces them with equality-against-`hit_test` tests over literal `SidebarGeom`s. No wiring — nothing in `terminal_app.rs` calls these yet, so the deletion is sequenced with T3 to keep the tree compiling (keep `tab_target` until T3 lands if the gate would otherwise break, then delete in T3).

### T2: `settings::refit` and the sidebar config mirror [infra]
- **Fulfills**: R3.1, R3.3, R2.4, R2.6
- **Depends on**: (none — different file from T1)
Extracts `pub fn refit(renderer, tabs) -> (u16, u16)` in `settings.rs`, reading `grid_size()` itself and calling `resize_all`; rewrites `apply` to `apply_config` → `refit` → `set_default_colors_all` with the `set_left_inset` line **deleted** (R7.1); adds `Running.sidebar_cfg: SidebarConfig` refreshed there. `Resized` is repointed at `refit` in T3.

### T3: Click routing + layer registration + the parked migrations [vertical]
- **Fulfills**: R0.1, R0.3, R1.1–R1.8, R7.1, R7.2, R7.3, R8.2, R8.3
- **Depends on**: T1, T2
Registers `SidebarChromeLayer` in `start()`; adds `Running::sidebar_layout()` assembling the `SidebarGeom` (R8.3); rewrites the `MouseInput` arm to `sidebar_press` + the action match, propagating `do_tab_action`'s bool to `event_loop.exit()`; repoints `Resized` at `settings::refit`; performs the `titles`→`rows` rename and completes `tab_target`'s deletion. First runnable slice: cards and the new-tab button work.

### T4: Collapse and expand [vertical]
- **Fulfills**: R2.1, R2.2, R2.3, R2.5, R3.2, R3.4
- **Depends on**: T3
Wires `SidebarAction::Collapse`/`Expand` to `renderer.set_sidebar_collapsed(bool)` followed by `settings::refit`, in that order, then the re-clamp / refresh-hover / redraw tail. Verifies R2.5 by launching with `collapsed = true` and clicking restore. `/run-term` is the acceptance instrument for R2.3 (switch to a background tab and check the wrap).

### T5: Hover [vertical]
- **Fulfills**: R4.1–R4.7, R7.4
- **Depends on**: T4
Adds `Running.last_hit`, `refresh_hover()`, the `CursorMoved` / `CursorLeft` / `Focused(false)` arms, and the two model writes from one `hit_test`; calls `refresh_hover()` at the end of every layout-changing path (collapse, expand, resize, reload, tab action). Uses `window.request_redraw()`, never `redraw()`. If `set_hovered_chrome` does not exist yet (D13), publishes `hovered` only and records the gap.

### T6: Scroll and reveal [vertical]
- **Fulfills**: R5.1–R5.9
- **Depends on**: T5
Adds `Running.scroll`, the `MouseWheel` arm handling both delta units, the `set_list_scroll` publication, the re-clamp on tab action / resize / collapse, and the `scroll_to_reveal` call in `do_tab_action`. The wheel sign is confirmed via `/run-term` before the task closes.

### T7: `tab-metadata` roster driving [vertical]
- **Fulfills**: R6.1–R6.8
- **Depends on**: T6
Adds `Running.roster: Vec<TabPid>`, captures the `Ok(TabId)` from `tabs.open()`, reads `tabs.active().id` before a close, and calls `tabmeta::global()`'s `set_roster` / `set_active` from `do_tab_action` and once at `start()`. Verifies no `UserEvent` variant was added and that the `OverlayPending` arm is untouched.

## External Dependencies

### Pre-work
- **`sidebar-row-redesign` and `sidebar-chrome-collapse` must have landed** — this feature calls their functions and registers their layer; the `depends_on` edges are real.
- **Add `hovered_chrome: Option<SidebarHit>` + `set_hovered_chrome` to `SidebarModel` / `TabModelHandle`** in `crates/renderer/src/sidebar_row.rs` (D13, Known Gaps). One field, one setter; by `sidebar-row-redesign` D3's own stated principle it belongs in that feature. Without it the chrome buttons have no hover highlight (cosmetic, non-blocking).
- **Re-read all three sibling specs before T1** and reconcile every signature quoted in this spec's Research section against their current revisions (D13). Two of them changed shape twice during this spec's authoring.
- **Resolve the two cross-spec conflicts** recorded in Known Gaps — `set_left_inset`'s two readings, and `SIDEBAR_CHROME_Z = 30` vs the suggested `z_order = 11`. Neither blocks this feature's code, but the second one determines whether a pasted inline image can bury the restore button.

### Post-work
- **`crates/renderer/src/tab_bar.rs` and `sidebar-row-redesign` D7's legacy `tab_at`/`tab_height_px` fallback can now be deleted** — this feature removed their last caller (R7.3). Out of surface here.
- **`Renderer::set_left_inset` and its ignored parameter can now be deleted** — this feature removed both callers (R7.1). `set_sidebar_collapsed` / `set_sidebar_width` remain. Out of surface here.
- **Replace `Running.roster` with `Tabs::roster()`** and inject the `TabMetaHandle` into `Tabs` directly, after which `tab-metadata`'s `OnceLock` locator can be deleted, as its D2 anticipates. Needs `tabs.rs`.
- **`/run-term`** to confirm the wheel direction (D10), that a collapse does not mis-wrap a background tab (R2.3), and that hover feels right — none of these is checkable by `cargo test`.
