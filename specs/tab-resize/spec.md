# Spec: tab-resize

## Meta
- **Created**: 2026-07-29
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-29

## Goal
Add a function to edit the size of the tab (the left tab sidebar) — let the user
change how wide the tab panel is, by dragging, and have that size stick.

## Non-goals
- Per-tab **card height** / row density (the card stride stays `sidebar-row-redesign`'s).
- Terminal **font or cell** size changes (that is `settings`' `font.size_pt`).
- A settings-panel **slider or GUI widget** for the width (`no-gui-crates` — the drag
  affordance is drawn on the cell grid / sidebar layer, nothing else).
- Changing the existing **260–440 logical-px band** (`SIDEBAR_WIDTH_MIN`/`MAX`).
- Redesigning collapse/restore. *(Scope note: three decisions do touch shipped collapse
  behaviour — D16 adds **persistence** to the Hide/Restore button so it and the drag
  cannot disagree about `SidebarConfig::collapsed`; D15a changes when a runtime override
  yields to the file; D17a adds a third toggle entry point. What stays untouched is the
  collapse **surface**: its geometry, chrome, and hit regions.)*
- **Double-click-to-reset** and **keyboard-chord *resize*** (stepping the width from the
  keyboard) — considered and explicitly deferred to follow-up specs (L2 round 2). D17a
  adds a single **toggle** chord, which is a recovery affordance for D16, not the deferred
  resize feature.
- Resizing anything other than the sidebar (no split panes, no window chrome).

## Confirmed Goal
Wire a **drag-to-resize** affordance onto the tab sidebar's right edge: press on the
edge, drag horizontally, and the sidebar width follows the pointer live — clamped to the
existing `SIDEBAR_WIDTH_MIN..SIDEBAR_WIDTH_MAX` band — with the terminal grid reflowing
to the reclaimed/ceded columns. On release the chosen width **persists to
`config.toml`** so it survives a restart.

The renderer half already exists and is unused: `Renderer::set_sidebar_width()`
(`crates/renderer/src/lib.rs:586`) is documented as *"the public backing for a
drag-resize"* and has no caller in `app/`. This feature supplies the missing seam:
hit region → drag state machine → setter → persistence.

**Done when:** dragging the sidebar edge visibly resizes the tab panel, the grid
reflows, the width clamps at both ends, and the value is still there after a restart.

## Research

### The half-built seam (the feature's whole reason to be small)
- `Renderer::set_sidebar_width(logical_px)` already exists, clamps to
  `SIDEBAR_WIDTH_MIN..MAX`, and its doc comment names itself *"the public backing for a
  drag-resize, which does not go through `Config::load_from`'s clamp"*
  (`crates/renderer/src/lib.rs:583-592`). **It has zero callers outside its own unit
  test** (`crates/renderer/src/lib.rs:1523-1525`) — grep for `set_sidebar_width` across
  `crates/app/` returns nothing.
- The width path behind it is complete: `sidebar_override.width_px: Option<f32>`
  (`renderer/src/lib.rs:163`) outranks the config file for the session
  (`effective_sidebar`, `lib.rs:556-565`); `sync_left_inset` (`lib.rs:568-573`) is the
  single site that re-derives `left_inset` and pushes uniforms; `sidebar_width_px`
  (`sidebar_row.rs:633-647`) turns logical px + scale into physical, returning `0.0`
  when collapsed and defending against non-finite input.
- The **collapse** feature is the exact structural precedent to copy:
  `SidebarAction::Collapse/Expand` (`app/src/input.rs:57-58`) →
  `TerminalAppInner::toggle_sidebar` (`app/src/terminal_app.rs:1107-1113`) →
  `renderer.set_sidebar_collapsed` → `settings::refit` → `clamp_scroll` → redraw. A
  resize is the same chain with a continuous value instead of a boolean.

### Where the drag has to be intercepted
- Pointer events are handled in ONE place: `TerminalApp::window_event`
  (`app/src/terminal_app.rs`, from `:1665`; the match runs past `Resized` `:1838` and
  `Occluded` `:1849`) — `CursorMoved` (`:1676`), `MouseInput` Pressed (`:1701`), Released
  `if inner.dragging` (`:1786`).
- There is already a drag state machine on `TerminalAppInner`: `dragging`, `drag_anchor`,
  `drag_moved`, `drag_latch` (`terminal_app.rs:173-176`) — owned by **text selection**
  (selection-input). A sidebar-edge drag must not be confused with it: today a press
  that resolves to no sidebar region and lands on a real grid cell arms selection
  (`terminal_app.rs:1770-1783`).
- All press routing goes through ONE ordered hit-test, and the app is forbidden from
  doing its own geometry (SD2, stated in `sidebar_layout.rs:1-20`):
  `sidebar_press` (`app/src/input.rs:83-100`) → `sidebar_layout::hit_test`
  (`renderer/src/sidebar_layout.rs:547-603`) → `SidebarRegion` → `SidebarAction`.
- **The obstacle:** `hit_test` step 2 (`sidebar_layout.rs:574-577`) returns `None` for
  any `p[0] >= inp.sidebar_w`. A grab gutter that straddles the edge would be half-dead;
  a gutter wholly inside the band (`[sidebar_w - g, sidebar_w)`) fits the existing
  ordering but must resolve **before** the header (`:579`), band (`:588`) and list
  (`:598`) arms, or the topmost rows of the gutter would open the menu / switch tabs.
- `hit_test` is also consulted by the wheel router (`terminal_app.rs:1823`) and hover
  (`sidebar_hover`, `input.rs:105-120`) — a new region leaks into both and must be
  handled there (hover → no card highlight; wheel → **superseded by D13a**, which decides
  the band scrolls the list like `Panel`).

### Persistence — the genuinely new capability
- **Nothing in the workspace ever writes `config.toml`.** `crates/common/src/config.rs`
  has `load`/`load_from`/`normalize` only; the only `std::fs::write` calls are in test
  helpers (`config.rs:247-252`). There is no `save`, no `toml::to_string`.
- `Config::path()` (`config.rs:144-149`) resolves the config file via
  `directories::ProjectDirs`. **On macOS that is
  `~/Library/Application Support/mac-terminal/config.toml`, NOT `~/.config/...`** — the
  latter is claimed by `docs/architecture.md` D9 and by two sibling specs, and is wrong
  (confirmed empirically during hand-verification: the app created the file under
  `~/Library/Application Support/`).
- A `ConfigWatcher` (`crates/common/src/watch.rs:32-70`) watches that file's **parent
  directory**, matching by file *name*, and reloads on any change — so a self-write will
  fire our own watcher and round-trip a fresh `Config` back through
  `settings::apply` (`app/src/settings.rs:37-50`). Note `apply_config` re-derives the
  inset from the **file** value while `sidebar_override` still holds the dragged value —
  `effective_sidebar` means the override wins, so this is survivable but must be stated.
- The collapse feature set the precedent of *not* persisting: `SidebarConfig::collapsed`
  is documented "Persisted collapsed state" (`config.rs:84-85`) yet `toggle_sidebar`
  only sets the runtime override. So "persist on release" is a deliberate extension, not
  a convention already in place.

### Refit, clamping, and the visual affordance
- `settings::refit` (`app/src/settings.rs:27-34`) is documented as **the one place** the
  grid is re-fitted after any inset change (D6) — resizing every tab, not just the
  active one. Every resize step must go through it, and a per-pixel drag therefore
  re-runs `Pty::resize` on all tabs (a real cost worth a decision).
- Band constants live in `common`: `SIDEBAR_WIDTH_MIN = 260.0`,
  `SIDEBAR_WIDTH_MAX = 440.0` (`config.rs:90-95`), already applied at load
  (`normalize`, `config.rs:183-197`) and in `set_sidebar_width`.
- A 1px right border already separates the sidebar from the grid:
  `SidebarTheme::panel_border` `#2e2a24` (`sidebar_theme.rs:25-26, 69`) — the natural
  visual anchor for the grab affordance, no new layer strictly required.
- The app **never sets a mouse cursor shape**: no `CursorIcon` / `set_cursor` anywhere in
  `crates/app/` (the two `set_cursor` hits at `terminal_app.rs:1124,1136` are the
  chrome layer's *pointer-position* handle, not a cursor icon). A `ColResize` cursor is
  therefore new ground — `winit` is an allowed dependency (`no-gui-crates` exception).

### Conventions this must satisfy
- `crates/app/` and `crates/renderer/` have **no `tests/` directory** — every test is an
  in-file `#[cfg(test)] mod tests` (e.g. `sidebar_layout.rs:605+`, `input.rs`,
  `settings.rs:63+`). Pure, GPU-free functions are the testable unit.
- `dependency-direction`: geometry belongs in `renderer::sidebar_layout` (pure, no wgpu),
  the drag state machine in `app`, the band constants in `common`. `term-core` is
  untouched by this feature.
- Build/test/lint: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`; no `unwrap()` outside tests.

## L2 Complexity

**Medium (4 signals)** — top of the band.

| Signal | Evidence |
|--------|----------|
| 3+ independent subsystems | `app` event loop (drag state) + `renderer::sidebar_layout` (geometry) + `common` config write + `term-core` PTY/grid resize reached via `settings::refit` — four |
| State machine | press → armed → dragging → (collapse ⇄ expand, D12) → release, coexisting with the *existing* selection drag on the same three winit events |
| Async resources | the `ConfigWatcher` notify thread round-trips our own write back through `settings::apply` |
| **Compat / migration** | D16 changes the behaviour of an already-shipped control (Hide/Restore gains persistence), D15a changes shipped override semantics, and D17a adds a chord — this feature edits live surfaces, it does not only add one |

*Not counted:* data-schema design (`SidebarConfig::width_px`/`collapsed` already exist),
multi-actor (single local user), external resource loading (a local config file is not a
network/CDN/DB dependency).

Brownfield weights applied: Core 30%, Scope 20%, Error/Edge 25%, Data 15%,
Implementation 10% — Implementation auto-resolved from existing patterns (SD2's one
hit-test, D6's one refit site, winit's allowed status).

**Final checkpoint scores** (after the L2-reviewer's two rounds forced Error/Edge open
again): Core 6/6, Scope 4/4, Error/Edge 12/12, Data 7/7, Implementation 3/3 —
composite 1.00, 0 unknowns. The Error/Edge count grew from 8 to 12 because the reviewer
was right that pointer capture (D17) had never been asked about.

## Decisions

### D1: Live per-frame resize — the sidebar and the grid follow the pointer continuously
- **Status**: resolved
- **Rationale**: Matches every native macOS resize (Finder, VS Code, iTerm2): you judge
  the result while dragging, not after. Rejected *ghost-line-then-commit* (cheapest, but
  the whole point of a hand-rolled cell renderer is that we can afford the real thing)
  and rejected *live sidebar + coalesced grid* (smoother on paper, but it desynchronises
  the band from the grid it is supposed to be pushing, and the seam is visible). The
  known cost — `settings::refit` → `Pty::resize` on **every** tab per pointer move — is
  accepted here and paid down by D8's no-op guard, not by degrading the feel.

### D2: The dragged width persists to the platform config file on release
- **Status**: resolved
- **Rationale**: A width you have to re-drag every launch is not a setting, it is a
  gesture. The file already owns `sidebar.width_px`, so writing it keeps ONE source of
  truth. Rejected a *sidecar state file* (a second source of truth to reconcile against
  the same field, and the watcher would have to learn about it) and rejected
  *session-only* (the existing collapse toggle's behaviour — which is precisely the gap
  this feature exists to close). Write happens on **release**, never per-frame, so the
  drag never touches the filesystem. Fidelity and failure handling → D9/D10.

### D3: The grab region is resolved by `sidebar_layout::hit_test`, like every other press
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: SD2 — "resolve **every** pointer event through `hit_test`… the app
  performs no geometry" (`sidebar_layout.rs:5-8`, `terminal_app.rs:1706-1708`). A new
  `SidebarRegion` variant keeps hit-testing and drawing reading from the same layout
  module, so the grab band and the drawn edge can never drift apart. Rejected an
  app-side `if x > sidebar_w - 4` check on principle: that is the exact drift this
  codebase's one-hit-test rule was written to prevent.

### D4: No grab region while collapsed
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: `hit_test` short-circuits to the restore button alone when collapsed
  (`sidebar_layout.rs:553-558`), and a zero-width band has no edge to grab. Restore
  first, then resize.

### D5: The runtime override outranks the config file for the session — **amended by D15a**
- **Status**: assumed (auto-resolved from L1), superseded in part
- **Rationale**: Already true — `effective_sidebar` applies `sidebar_override`'s `Some`
  fields over the file (`renderer/src/lib.rs:556-565`), documented as "a click survives a
  `config.toml` save". This is what makes D2's self-write safe: the reload cannot yank
  the width out from under a just-finished drag.
- **Amendment (D15a):** "for the session" is no longer unconditional for the two
  persisted fields. A reload carrying a value that is *not* our armed echo clears the
  corresponding override. Read D5 and D15a together; D5 alone states the superseded rule.

### D6: Drag state lives on `TerminalAppInner`; every width change goes through `settings::refit`
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: `refit` is documented as THE one place the grid is re-fitted after any
  inset change (`app/src/settings.rs:14-17`), and `toggle_sidebar`
  (`terminal_app.rs:1107-1113`) is the load-bearing ordering precedent: set the override
  first, then refit, then re-clamp scroll, then redraw. Resize is that same chain with a
  continuous value.

### D7: Persisted units are logical px
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: `SidebarConfig::width_px` is defined as logical px scaled at the point
  of use (`config.rs:77-79`). Persisting physical px would make the file display-dependent
  — dragging on an external monitor would resize the sidebar on the laptop panel.

### D8: The config write is surgical + atomic — one line replaced, temp file renamed into place
- **Status**: resolved
- **Rationale**: TOML was chosen because the file is hand-editable (D9 in
  `docs/architecture.md`); a `toml::to_string(&Config)` round-trip would silently delete
  every comment and reorder every key, quietly converting a human's file into a machine's.
  So: locate the `width_px` line under `[sidebar]` and replace only its value, inserting a
  `[sidebar]` table when absent. Rejected the *full serde round-trip* (destroys comments)
  and rejected *appending an override block* (produces duplicate keys, which TOML and
  `deny_unknown_fields` reject). Atomic write-to-temp + `rename` because the
  `ConfigWatcher` matches by filename over the parent directory
  (`watch.rs:44-55`) and would otherwise be free to parse a half-written file.
- **The temp file must sit in the same directory** (`rename` is only atomic within a
  filesystem) and **must not be named `config.toml`** — the watcher matches by file *name*
  over that very directory, so a same-named temp would trip it twice. Use a distinct name
  (e.g. `config.toml.tmp`).

### D9: Absolute pointer mapping with a grab offset fixed at press; overshoot is not accumulated
- **Status**: resolved
- **Rationale**: On press, record `grab_offset = pointer_x − current_edge_x`; thereafter
  the drag's ONE piece of geometric state is the **raw** value
  `raw = pointer_x − grab_offset`, and the width handed to `set_sidebar_width` is
  `clamp(raw, MIN, MAX)`. The edge therefore stays welded to
  the point you grabbed, and pushing past 440 pins the width without the edge re-anchoring
  under the pointer. Rejected *re-anchoring on reverse* (the edge teleports to the cursor,
  which is exactly the jump this model exists to avoid) and rejected *rubber-banding*
  (the grid would have to reflow to a width the config's own clamp forbids).
  **Honest consequence, stated so no one "fixes" it later:** overshooting by 300px means
  the pointer must travel those 300px back before the edge moves again — the edge resumes
  when the pointer returns to where the edge actually is. That is the same behaviour as
  Finder and VS Code, and it is deliberate, not a missing feature.
- **`grab_offset` is fixed at press and never re-derived from the live edge.** This is
  load-bearing, not incidental: it is what lets D12 collapse the sidebar mid-gesture and
  still re-expand, because at `width = 0` there is no edge to measure an offset against —
  `raw` is the only state the drag needs, and it keeps advancing with the pointer while
  the sidebar is shut. Re-deriving the offset from the current edge would look like a
  tidy-up and would silently break re-expansion.

### D9a: The clamp is applied at the setter; the collapse/expand thresholds read the RAW value
- **Status**: resolved
- **Rationale**: Correcting an outright contradiction between D9 and D12 caught by the L2
  reviewer: `clamp(raw, 260, 440)` can never be below 260, so a threshold test at 220 on
  the clamped width would be **dead code that never fires**. The two quantities are
  distinct and must stay distinct — `raw` drives the D12 threshold comparisons (collapse
  when `raw < 220`, re-expand when `raw ≥ 260`), while `clamp(raw, …)` is what reaches
  `set_sidebar_width` and therefore what is ever drawn or persisted. The 260 in "clamp at
  260" and the 260 in "re-expand past 260" are the same number naming two different
  things; this decision exists so nobody collapses them back together.

### D10: The grab region is a 6 logical-px band ending at the edge, wholly inside the sidebar
- **Status**: resolved
- **Rationale**: `[sidebar_w − 6·scale, sidebar_w)` keeps `hit_test`'s existing step-2
  bound (`p[0] >= inp.sidebar_w → None`, `sidebar_layout.rs:575`) untouched — the grid
  loses no pixels to the sidebar's hit-test, and no other feature's routing changes. The
  band must resolve **before** the header / waiting-band / list arms, or its top rows
  would open the card menu instead of starting a resize. Rejected the *straddling ±4px
  band* (easier to hit, but it forces points outside the sidebar to resolve to a sidebar
  region — a change to the one ordered hit-test that every sibling feature depends on).
  The existing 1px `panel_border` (`sidebar_theme.rs:25-26`) is the visual anchor; no new
  draw path. Rejected a *drawn grip on hover* as scope for a follow-up.
- **Exact arm position (L2 reviewer):** **immediately after step 2** — i.e. after step 1
  (collapsed → Restore only, `:553-558`), after step 1.5 (the open card menu, `:559-573`),
  and after step 2's viewport bail (`:574-577`), but before step 3 (header, `:579`).
  "After step 2" is load-bearing, not pedantry: placed above it, a point in the gutter's
  column with `y < 0` or `y >= viewport[1]` would resolve to the grab region instead of
  being rejected as outside the window. Step 1.5 is not optional to respect: that popup is
  right-aligned to a card and, by its own comment, "may extend past `sidebar_w` over the
  terminal" — so it overlaps the grab band by construction. An arm inserted above it
  would turn the right 6px of every open card menu into a resize handle, breaking
  sticky-tab-title R3.1's shipped menu activation.

### D11: Hover over the band shows a `ColResize` cursor
- **Status**: resolved
- **Rationale**: The only discoverability affordance, and the reason D10 needs no drawn
  handle. New ground — `crates/app/` sets no cursor icon today — via winit's
  `Window::set_cursor`, an allowed dependency (`no-gui-crates` explicit exception). The
  cursor must be restored to the default when the pointer leaves the band, including on
  `CursorLeft` (except mid-drag, D17).
- **Where the region comes from (L2 reviewer):** `sidebar_hover` (`input.rs:105-120`)
  collapses every region to `Option<usize>` card index, so it cannot report "on the grab
  band". Resolve this **without** a second `hit_test` call — `refresh_hover`'s own comment
  ("one hit_test feeds both hover fields", `terminal_app.rs:1677`) is the rule, and
  calling it twice per move is exactly the drift D3/SD2 forbids. So: call `hit_test` once
  in `refresh_hover` and map that single `Option<SidebarRegion>` through **two** pure
  helpers — the existing card-index mapping and a new cursor-icon mapping. `sidebar_hover`
  keeps its signature and its existing test assertions (`input.rs:960-975`) intact.

### D12: Crossing ~220px collapses mid-drag; dragging back past 260px re-expands, with hysteresis
- **Status**: resolved
- **Rationale**: The gesture stays live across the mode change — a mis-drag costs nothing.
  The two thresholds are deliberately different (collapse at `MIN − 40`, re-expand at
  `MIN`) because a single threshold flickers collapse/expand every frame on ordinary hand
  jitter, and under D1 each flicker is a full-window grid reflow. Rejected *collapse is
  final for the gesture* (a mis-drag then costs a trip to the Restore button) and rejected
  *collapse only on release* (no mid-drag feedback — the ghost-commit feel D1 rejected).

### D13: `settings::refit` runs only when the derived `(cols, rows)` actually changes
- **Status**: resolved
- **Rationale**: This is a correctness guard, not an optimisation. `Grid::resize`
  (`grid.rs:1283-1300`) unconditionally reallocates every tab's cell vector **and sets
  `self.selection = None`** — so an unguarded per-frame refit would clear the user's
  selection on every pixel of drag travel and fire a `TIOCSWINSZ` (`pty.rs:153-162`) at
  zsh on every pointer move, on every tab. The sidebar band and `left_inset` still update
  every frame, so D1's feel is intact; the grid simply steps one column at a time, which
  is the finest granularity a cell terminal actually has. Rejected *refit every frame*
  (selection loss + SIGWINCH storm) and *refit only on release* (visible lag between the
  band and the text it is pushing).
- **The guard bounds the rate, not the total (L2 reviewer).** A full 260→440 drag still
  crosses ~20 column boundaries, and `refit` → `tabs.resize_all` (`tabs.rs:152-158`)
  resizes **every** tab per crossing — with 10 tabs open that is ~200 `TIOCSWINSZ` calls
  per gesture, and any background full-screen TUI repaints on each. Accepted: it is two
  orders of magnitude below the unguarded per-pixel path, and each of those resizes is a
  real column change the tab genuinely needs. Recorded so the multiplier is a known
  quantity rather than a surprise.
- **The guard lives at the drag call site, not inside `refit` (L2 reviewer).** Putting it
  inside `settings::refit` would change behaviour for startup, `Resized`, and config
  reload — all of which share that function — including silently stopping a no-op window
  resize from clearing the selection, a sibling behaviour change this feature has no
  mandate to make. `refit` stays the one refit site (D6); the drag simply decides whether
  to call it, by comparing the `(cols, rows)` it would produce against the live pair.

### D13a: The grab band scrolls the sidebar list on a wheel event, exactly like `Panel`
- **Status**: resolved
- **Rationale**: The wheel arm routes on `hit_test(...).is_some()`
  (`terminal_app.rs:1824-1835`), so a new region defaults to scrolling the list. That
  default is also the right answer: the band is a 6px strip lying *over* the card list,
  and a dead stripe there would make the list stop scrolling under a pointer the user
  cannot see is "on" anything. The alternative — falling through to the grid — would mean
  scrolling the terminal from inside the sidebar, which is worse. Recorded because L1's
  Research asserted "wheel → not a scroll" without deciding it; that assertion was wrong
  and is superseded here.

### D14: A failed config write warns to the log and keeps the width; it never reverts and never blocks
- **Status**: resolved
- **Rationale**: `create_dir_all` on the config directory first, since a fresh install has
  no file at all (`Config::load` returns defaults **without** creating one,
  `config.rs:151-158`), and write a minimal file containing a `[sidebar]` block when the
  table is absent. A genuine failure (read-only FS) is `tracing::warn!`-ed and otherwise
  silent — the session keeps the dragged width, it just will not survive a restart.
  Rejected *reverting on write failure* (punishes the user for a filesystem problem by
  undoing a gesture that visibly succeeded) and rejected an *on-screen notice* (this
  feature would have to define a toast surface that does not exist — a disproportionate
  addition for a cosmetic setting). Consistent with `normalize`'s existing philosophy: a
  cosmetic problem warns, it never fails the terminal (`config.rs:179-182`).
- **D14 × D15a, resolved (L2 reviewer):** these two contradict unless the interaction is
  stated. After a *failed* write the file still holds the old pair while the override
  holds the dragged one — so the next reload for **any** unrelated reason (the watcher
  fires on any change to that filename, e.g. a `font.size_pt` edit) would deliver a
  non-echo pair, and D15a would clear the override and snap the sidebar back mid-session,
  breaking the promise this decision just made. Therefore: **a failed write arms nothing
  and marks the persisted fields as override-authoritative**, so D15a does not fire for
  `width_px`/`collapsed` until the next *successful* write. The session keeps the width,
  as promised, and unrelated config reloads still work for every other field.

### D15: Our own write's echo is suppressed by **exact value match, latched**; any other value is a real edit
- **Status**: resolved
- **Rationale**: The atomic rename of D8 **is** a change event, so the round-trip is
  guaranteed rather than incidental — and `settings::apply` → `renderer.apply_config`
  rebuilds the entire glyph atlas and invalidates the glyph cache
  (`renderer/src/lib.rs:605-612`). Paying that on every drag release, for a change we
  already applied, is absurd. Rejected *letting it round-trip* (correct — `effective_sidebar`
  means nothing visibly moves — but a full atlas rebuild per release) and rejected
  *early-outing inside `apply_config`* (the better global fix, genuinely tempting, but it
  edits a shared function on behalf of every reload path while this feature is the only
  one with evidence; recorded as a Known Gap).
- The predicate is **exact**, and the L2 reviewer's three ambiguities are closed here:
  - **Scope:** the suppressed value is the `(width_px, collapsed)` **pair**, not just the
    float — D16 put `collapsed` on the same persistence path, so a width-only predicate
    would make every Hide/Restore click pay the atlas rebuild this decision exists to
    avoid.
  - **Predicate:** suppress only when the delivered `Config`'s pair **equals exactly what
    we just wrote**. Not "differs only in that field" — under that reading a deliberate
    hand edit to `sidebar.width_px` alone would be dropped forever, killing live reload
    for the one field this feature owns.
  - **Cardinality: latched, not one-shot.** Hold the *last value written* and suppress
    **every** delivery whose pair equals it, until a differing pair arrives (which is
    also D15a's override-clearing event). A one-shot "consume the first match" rule was
    written first and is wrong: macOS FSEvents can deliver several events for one
    rename, and the second identical delivery is **not** harmless — it runs
    `apply_config`, i.e. exactly the atlas rebuild this decision exists to prevent
    (`lib.rs:614-648`), *and* D15a would then read it as a human edit and clear the
    override after every single drag.
  - **Float equality is on a re-parsed value, and the writer must round-trip exactly.**
    The comparison happens after the value has been through `f32` → decimal → `f32` and
    through `normalize` (`config.rs:179-197`). Format with `{}` (Rust's shortest
    round-trip representation for `f32`), never a fixed precision like `{:.1}`, and
    compare against the *parsed* config value — a formatting choice that loses bits turns
    every echo into a spurious "human edit".

### D15a: A reload carrying a *different* value is a real human edit — it wins and clears the override
- **Status**: resolved
- **Rationale**: The L2 reviewer found the malign direction D5 never stated: `sidebar_override`
  is sticky, so after the first drag `effective_sidebar` (`lib.rs:556-565`) would ignore the
  file's `width_px`/`collapsed` **for the rest of the session** — making these the only two
  fields in `config.toml` that stop live-reloading, while the next drag silently overwrites
  the user's edit on disk. The spec asserts "`config.toml` remains hand-editable" as a hard
  Constraint; that has to mean more than comment preservation. So a delivered value that is
  *not* the armed echo **clears the corresponding override field** and takes effect live.
  This is the same distinction D15's predicate draws — "my echo" vs "a real edit" — decided
  once and referenced from both. Rejected *override always wins for the session* (today's
  shipped rule, defensible before this feature because nothing persisted and the override
  was `None` until you clicked — D2 makes the anomaly permanent and visible) and rejected
  *dropping the override for width entirely* (one path, but the width would visibly
  round-trip through the filesystem, and a D14 write failure would then lose the drag).
- **Which fields clear:** clear **both** persisted override fields (`width_px` and
  `collapsed`) on a non-echo delivery, not only the differing one. The two readings
  converge in practice — the armed value equals what the override holds — but an
  implementer must not have to derive that, and "the file is now authoritative for the
  pair" is the simpler invariant to test.
- **Exception — D14 (see the D14 × D15a tension below):** when the last write *failed*,
  the override is deliberately out of step with the file, and D15a must not fire for the
  affected fields. Arming is therefore conditional on a **successful** write.

### D16: `collapsed` persists too — and the existing Hide/Restore button routes through the same path
- **Status**: resolved
- **Rationale**: Once a drag can collapse the sidebar (D12) and a drag persists (D2),
  leaving the Hide button session-only means two controls that do the identical thing
  remember differently — the field is *already* documented as "Persisted collapsed state"
  (`config.rs:84-85`) while `toggle_sidebar` only sets the runtime override
  (`terminal_app.rs:1107-1113`). One field, one persistence path, both entry points.
  Rejected *persisting drag-collapse only* (ships the disagreement) and rejected
  *width-only persistence* (narrowest, fully consistent with today — but then dragging the
  sidebar shut silently un-shuts it at next launch, which reads as a bug).
- **Steelman, and the answer (L2 reviewer):** the strongest objection is not the one
  rejected above. Today a hidden sidebar is recoverable **by restarting** — the restart
  *is* the escape hatch, and D16 removes it, precisely while D12 makes an accidental
  collapse a one-flick gesture. Recovery would then rest entirely on a small
  fixed-position button (`restore_button_rect`, `sidebar_layout.rs:517`) that was designed
  for a state you could always escape by relaunching, not as the sole cross-restart
  recovery path. This is a real cost, and it is answered by D17a rather than waved away.

### D17: A resize drag owns the pointer — disjoint state from the selection drag, its own release arm
- **Status**: resolved
- **Rationale**: The L2 reviewer's day-one blocker: as the code stands the Released arm is
  guarded `if inner.dragging` (`terminal_app.rs:1786-1790`), and `dragging` /
  `drag_anchor` / `drag_moved` / `drag_latch` (`:173-176`) all belong to **text
  selection** — so a release ending a resize would match no arm at all and **D2's
  write-on-release would never fire**. Therefore:
  - A new, **disjoint** `resize_drag: Option<ResizeDrag>` on `TerminalAppInner` holding
    `grab_offset` and the armed collapse/expand hysteresis state. Not a reuse of
    `dragging`: the two gestures have different release semantics (selection keeps a
    selection; resize commits a config write), and overloading one flag is how they would
    come to corrupt each other.
  - **Invariant: `resize_drag.is_some()` ⇒ `!dragging`.** No single press can arm both
    (a grab-band press returns `Some(SidebarAction::…)` from `sidebar_press`, and the
    selection arm is the `else if let Some(cell)` branch, `terminal_app.rs:1770-1783`),
    but a *lost* `Released` can: nothing clears `dragging` on `Focused(false)` today
    (`:1857-1868`), so a stale `dragging: true` plus a later grab-band press would arm
    both. Assert the invariant when arming a resize.
  - **Arm shape: a new `Released` arm placed *before* the existing one**, guarded on
    `resize_drag.is_some()` — not a `||`-widened guard on the existing arm. Rust runs
    exactly one match arm, so with both flags somehow set, a widened guard would swallow
    the selection release and leave `dragging` stuck true forever. Two arms, two
    independent guards, each clearing only its own state.
  - While a resize is in flight, `CursorMoved` **skips `refresh_hover`** (`:1119, 1677`).
    Hover must not be re-resolved mid-gesture: once D12 collapses the sidebar, `hit_test`
    short-circuits to Restore-only (`sidebar_layout.rs:553-558`), which would strip the
    hover state the drag is standing on. This is the one deliberate exception to "resolve
    every pointer event through `hit_test`" — a captured pointer is not a hit-test
    question, and the exception is confined to the in-flight window.
  - `CursorLeft` does **not** end the drag and does **not** restore the cursor icon
    (overriding D11 for the duration): macOS keeps delivering held-button moves outside
    the window, so tracking continues and the clamp handles the excursion.
  - **Focus loss commits and *ends* the drag** — it clears `resize_drag`, so the eventual
    `Released` finds nothing armed and does not write a second time. The codebase already
    notes that Cmd-Tab fires no `CursorLeft` (`terminal_app.rs:1135`), so focus loss is
    the only signal that a gesture was abandoned; committing what is on screen beats both
    silently reverting and leaving a drag armed across an app switch.

### D17a: One keyboard chord toggles the sidebar — the recovery path D16 owes
- **Status**: resolved
- **Rationale**: D16 makes a hidden sidebar survive a restart, so hiding it needs a
  recovery that does not depend on finding a 28px button. A single toggle chord answers
  the steelman directly and is reachable with no pointer at all. Deliberately **only a
  toggle** — the keyboard *resize* idea rejected in L2 round 2 stays rejected; this is the
  minimum affordance that makes D16 safe, not the reopening of a deferred feature.
- **The chord is a third entry point and routes through the SAME persistence path** as
  the Hide button and the drag (D16). A chord that toggled without persisting would
  recreate precisely the disagreement D16 exists to remove.
- The chord must not collide with the existing chord table (`tab_action`,
  `input.rs:27-42` — Cmd+T/W/[/]/1-9) **nor** with anything `encode_key` forwards to zsh;
  a chord that silently stops reaching the shell is worse than no chord.

## Constraints

- **`term-core` stays GPU-free and window-free** (`dependency-direction`). This feature
  touches `term-core` not at all; the only engine contact is the existing
  `Pty::resize`/`Grid::resize` reached through `settings::refit`.
- **No GUI/widget crate** (`no-gui-crates`). The grab affordance is a hit region plus a
  winit `CursorIcon` — winit is the named exception. No slider, no widget.
- **All pointer geometry lives in `renderer::sidebar_layout`** (SD2). `crates/app/` must
  not compute a rect, an edge, or a threshold; it owns the drag *state machine* only.
- **`hit_test` stays the single ordered router**, with exactly one stated exception.
  Hover (`sidebar_hover`), the wheel router (`terminal_app.rs:1823`), and presses
  (`sidebar_press`) all consume it — a new region must be handled in all three, not just
  the press path. The sole exception is D17's captured pointer: while a resize drag is in
  flight, `CursorMoved` skips hover resolution. No other path may add an exception.
- **`settings::refit` stays the only grid re-fit site** (D6, `settings.rs:14-17`).
- **The 260–440 band is unbypassable.** `Config::load_from`'s `normalize` clamps the file
  value and `set_sidebar_width` clamps the runtime value; the drag must not open a third
  path that writes an out-of-band width.
- **`config.toml` remains hand-editable.** Comments, key order, and unknown-to-us keys
  survive every write this feature performs.
- **No `unwrap()` outside tests**; `cargo clippy --workspace --all-targets -- -D warnings`
  must stay clean.
- **macOS ARM64 only** — no `#[cfg(target_os)]` branches for the cursor or the file write.
- Tests live in in-file `#[cfg(test)] mod tests` blocks; neither `crates/app/` nor
  `crates/renderer/` has a `tests/` directory, and the pure functions (band geometry,
  width mapping, threshold/hysteresis, TOML line rewrite) must be testable without a GPU
  or a PTY.

## Requirements

### R0: Dragging the sidebar's right edge resizes the tab panel, and the size sticks

#### R0.1: End-to-end drag resizes the panel and reflows the grid
- **Given**: The terminal is running with the sidebar open at its 340px default
- **When**: The user presses on the sidebar's right edge, drags 60 logical px right, and releases
- **Then**: The sidebar is drawn 400px wide, `Renderer::left_inset()` reports `400 × scale`,
  and the terminal grid has re-fitted to the reduced column count

#### R0.2: The dragged width survives a restart
- **Given**: A drag has completed at 400px and the config write succeeded
- **When**: The app is relaunched and `Config::load()` runs
- **Then**: `cfg.sidebar.width_px == 400.0` and the sidebar is drawn at 400px on the first frame

---

### R1: The grab region is a layout-owned hit region (D3, D4, D10)

#### R1.1: A point inside the 6px band resolves to the grab region
- **Given**: `SidebarLayoutInput` with `sidebar_w = 340 × scale` and `collapsed = false`
- **When**: `hit_test` is called with `x` in `[sidebar_w − 6·scale, sidebar_w)` at any `y` inside the viewport
- **Then**: It returns the new grab `SidebarRegion` variant

#### R1.2: A point just left of the band still resolves to whatever is under it
- **Given**: The same input, with a card list published
- **When**: `hit_test` is called at `x = sidebar_w − 7·scale` over a card row
- **Then**: It returns `SidebarRegion::Card(i)` — the band steals no pixel beyond its 6

#### R1.3: The band never extends past the sidebar into the grid
- **Given**: The same input
- **When**: `hit_test` is called at `x = sidebar_w` (the first grid pixel)
- **Then**: It returns `None`, exactly as before this feature

#### R1.4: An open card menu outranks the band
- **Given**: A card menu is open whose popup overlaps `[sidebar_w − 6·scale, sidebar_w)`
- **When**: `hit_test` is called at a point inside both the popup and the band
- **Then**: It returns `MenuItem`/`Panel` from step 1.5 — never the grab region

#### R1.5: The band is bounded by the viewport test
- **Given**: The same input with `viewport = [W, H]`
- **When**: `hit_test` is called at `x` inside the band but `y = H + 10`
- **Then**: It returns `None` — the arm sits after step 2's bail, not before it

#### R1.6: No grab region while collapsed
- **Given**: `collapsed = true`
- **When**: `hit_test` is called anywhere outside the restore button
- **Then**: It returns `None`; only `Chrome(Restore)` is live

#### R1.7: The app routes the region without computing geometry
- **Given**: A press whose `hit_test` result is the grab region
- **When**: `sidebar_press` maps it
- **Then**: It yields the new resize `SidebarAction`, and `crates/app/` contains no rect,
  edge, or threshold arithmetic of its own

---

### R2: Hover over the band shows a resize cursor (D11)

#### R2.1: Entering the band sets `ColResize`
- **Given**: The pointer is over a card, cursor at default
- **When**: `CursorMoved` delivers a position inside the grab band
- **Then**: `Window::set_cursor(CursorIcon::ColResize)` is called

#### R2.2: Leaving the band restores the default cursor
- **Given**: The cursor is `ColResize` from R2.1 and no drag is in flight
- **When**: `CursorMoved` delivers a position outside the band
- **Then**: The cursor is set back to the default icon

#### R2.3: Hover resolution costs exactly one `hit_test`
- **Given**: Any `CursorMoved` event
- **When**: `refresh_hover` runs
- **Then**: `hit_test` is invoked once, and both the card-hover index and the cursor icon
  are derived from that single result

#### R2.4: `CursorLeft` restores the default cursor when idle
- **Given**: The cursor is `ColResize` and `resize_drag` is `None`
- **When**: `CursorLeft` fires
- **Then**: The default cursor is restored

---

### R3: Pointer-to-width mapping (D9, D9a)

#### R3.1: The width tracks the pointer through a fixed grab offset
- **Given**: A drag armed at `pointer_x = 345`, `edge_x = 340`, so `grab_offset = 5`
- **When**: The pointer moves to `x = 405`
- **Then**: `raw == 400.0` and the width applied is `400.0`

#### R3.2: The applied width is clamped at both ends
- **Given**: The same drag
- **When**: `raw` evaluates to `700.0`, then to `100.0`
- **Then**: The applied widths are `440.0` and `260.0` respectively

#### R3.3: Overshoot is not accumulated
- **Given**: The pointer has run to `raw = 700` (width pinned at 440)
- **When**: The pointer moves back left to `raw = 500`
- **Then**: The width is still `440.0` — it only moves once `raw` re-enters the band

#### R3.4: The grab offset is never re-derived mid-drag
- **Given**: A drag in flight whose sidebar has collapsed to width 0
- **When**: Further `CursorMoved` events arrive
- **Then**: `raw` continues to advance from the offset captured at press; no code path
  reads a live edge position to recompute it

---

### R4: Collapse and re-expand within one gesture (D12, D9a)

#### R4.1: Crossing the collapse threshold collapses mid-drag
- **Given**: A drag in flight with the sidebar open
- **When**: `raw` falls below 220
- **Then**: `set_sidebar_collapsed(true)` is called, `left_inset` becomes `0.0`, and the
  grid re-fits to the full window width

#### R4.2: Dragging back re-expands within the same gesture
- **Given**: The drag from R4.1, currently collapsed, button still down
- **When**: `raw` rises back to 260 or above
- **Then**: `set_sidebar_collapsed(false)` is called and the width resumes tracking `raw`

#### R4.3: The two thresholds differ, so jitter cannot flicker the mode
- **Given**: A drag hovering at `raw = 240` (between the thresholds)
- **When**: `raw` oscillates by ±15 across several events without leaving 220..260
- **Then**: No collapse or expand call is made at all

#### R4.4: The thresholds read the raw value, not the clamped width
- **Given**: `raw = 200`
- **When**: The threshold comparison runs
- **Then**: It compares against `200`, not against `clamp(200, 260, 440) == 260` — the
  collapse fires

---

### R5: Live resize with a change-guarded refit (D1, D6, D13)

#### R5.1: Sub-cell travel moves the band without touching the grid
- **Given**: A drag in flight where a 3px width change yields the same `(cols, rows)`
- **When**: The `CursorMoved` is processed
- **Then**: `left_inset` and the drawn band update, and `settings::refit` is **not** called —
  so no `Grid::resize`, no `Pty::resize`, and any active selection survives

#### R5.2: Crossing a column boundary re-fits every tab
- **Given**: Three tabs open and a drag whose next width changes `cols` by 1
- **When**: The `CursorMoved` is processed
- **Then**: `settings::refit` runs once and all three tabs are resized to the new `(cols, rows)`

#### R5.3: The guard lives at the drag call site
- **Given**: A window `Resized` event that produces an unchanged `(cols, rows)`
- **When**: That event is handled
- **Then**: `settings::refit` still runs exactly as it does today — the guard has not
  changed `refit`'s own behavior for any non-drag caller

#### R5.4: Every width change reaches the grid through the one refit site
- **Given**: Any path that changes the sidebar width (drag, collapse, expand, reload)
- **When**: The width changes
- **Then**: The grid is re-fitted via `settings::refit` and by no other route

---

### R6: The new region behaves sanely in the non-press routers (D13a, D11)

#### R6.1: A wheel event over the band scrolls the card list
- **Given**: The pointer rests inside the grab band and the list is scrollable
- **When**: A wheel event is delivered
- **Then**: The sidebar list scroll offset changes, exactly as it would over `Panel`

#### R6.2: The band does not highlight a card
- **Given**: The pointer moves from a card into the band directly to its right
- **When**: `sidebar_hover` resolves
- **Then**: It returns `None` for the card index — no card shows a hover highlight

---

### R7: A resize drag owns the pointer (D17)

#### R7.1: A press on the band arms the resize and not a selection
- **Given**: The pointer is inside the band over a real grid row's y-position
- **When**: Left-press fires
- **Then**: `resize_drag` becomes `Some`, and `dragging` (selection) stays `false`

#### R7.2: The disjointness invariant holds when arming
- **Given**: A stale `dragging == true` (a `Released` was lost across focus change)
- **When**: A grab-band press arms a resize
- **Then**: The invariant `resize_drag.is_some() ⇒ !dragging` is upheld

#### R7.3: Release commits from anywhere, including over the grid
- **Given**: A resize drag in flight, pointer currently 200px into the terminal grid
- **When**: Left-release fires
- **Then**: The dedicated resize `Released` arm runs (ahead of the selection-guarded arm),
  the width is committed, and `resize_drag` is cleared

#### R7.4: Hover is not re-resolved mid-drag
- **Given**: A resize drag in flight that has collapsed the sidebar
- **When**: `CursorMoved` fires
- **Then**: `refresh_hover` is skipped, so the collapsed `hit_test` short-circuit cannot
  strip the state the drag depends on

#### R7.5: Leaving the window does not end the drag
- **Given**: A resize drag in flight
- **When**: `CursorLeft` fires and the pointer keeps moving outside the window
- **Then**: `resize_drag` stays `Some`, the width keeps tracking (clamped), and the cursor
  icon is not reset

#### R7.6: Focus loss commits and ends the drag
- **Given**: A resize drag in flight at width 400
- **When**: `Focused(false)` fires
- **Then**: 400 is committed and persisted, `resize_drag` becomes `None`, and a later
  `Released` performs no second write

#### R7.7: A selection drag is unaffected
- **Given**: No resize in flight
- **When**: The user presses on a grid cell and drags
- **Then**: Selection behaves exactly as it does today, and its `Released` arm still runs

---

### R8: Persisting the width to `config.toml` (D2, D7, D8, D14)

#### R8.1: Release writes the width in logical px
- **Given**: A drag released at 400 logical px on a 2× display
- **When**: The write runs
- **Then**: The file contains `width_px = 400` — the logical value, not `800`

#### R8.2: The write preserves comments, key order, and unknown keys
- **Given**: A `config.toml` with comments above `[sidebar]`, a custom key order, and
  `width_px = 340`
- **When**: 400 is persisted
- **Then**: Only that value changes; every comment, blank line, and other key survives, and
  every *untouched* line is byte-identical
- **Known deviation** (found in verification): on the two lines actually rewritten, the
  spacing around `=` is normalised — `width_px=340.0` becomes `width_px = 400.0`. The
  indentation, the trailing comment, and its original gap are preserved; the inter-token
  spacing is not. "Byte-identical" holds for every line the writer does not touch.

#### R8.3: A missing `[sidebar]` table is inserted
- **Given**: A `config.toml` with a `[grid]` table and no `[sidebar]` table
- **When**: 400 is persisted
- **Then**: A `[sidebar]` table containing `width_px = 400` is added and the file still parses

#### R8.4: A missing config file is created
- **Given**: No config directory at all (`~/Library/Application Support/mac-terminal/`)
- **When**: 400 is persisted
- **Then**: `create_dir_all` runs, a minimal file with a `[sidebar]` block is written, and
  `Config::load_from` on it returns `width_px == 400.0`

#### R8.5: The write is atomic and does not self-trip the watcher by name
- **Given**: A persist in progress
- **When**: The temp file is created
- **Then**: It lives in the config directory under a name that is **not** `config.toml`,
  and the final step is a `rename` over the target

#### R8.6: A failed write warns and keeps the width
- **Given**: The config directory is read-only
- **When**: A drag is released at 400
- **Then**: A `tracing::warn!` is emitted, the sidebar stays at 400 for the session, the
  terminal keeps running, and no panic or `unwrap` is reached

#### R8.7: A written value round-trips exactly
- **Given**: A width whose `f32` needs full precision to reproduce
- **When**: It is written and re-parsed via `Config::load_from`
- **Then**: The parsed `f32` equals the written one bit-for-bit

---

### R9: Echo suppression and file/override precedence (D5, D15, D15a, D14)

#### R9.1: Our own write does not rebuild the atlas
- **Given**: A successful persist of `(400.0, false)` has armed the latch
- **When**: The watcher delivers a `Config` whose sidebar pair equals `(400.0, false)`
- **Then**: `settings::apply` is not invoked — no atlas rebuild, no refit, no visible change

#### R9.2: Repeated identical deliveries stay suppressed
- **Given**: The armed latch from R9.1
- **When**: FSEvents delivers the same matching `Config` a second and third time
- **Then**: Every one of them is suppressed — the latch is not consumed by the first

#### R9.3: A hand edit takes effect live and clears the override
- **Given**: The armed latch from R9.1 and a session that has already dragged
- **When**: The user hand-edits `width_px` to `300` and the watcher delivers it
- **Then**: Both persisted override fields are cleared, `settings::apply` runs, and the
  sidebar is drawn at 300

#### R9.4: A partial-match delivery counts as a real edit
- **Given**: The latch armed on `(400.0, false)`
- **When**: A delivery carries `(400.0, true)`
- **Then**: It is treated as a human edit, not an echo — the override clears and the
  collapsed state is applied

#### R9.5: After a failed write, an unrelated reload does not snap the sidebar back
- **Given**: A drag to 400 whose write failed (R8.6), file still holding 340
- **When**: The user edits `font.size_pt` and the watcher delivers the reloaded config
- **Then**: The font change applies, the sidebar **stays at 400**, and the width/collapsed
  override is not cleared

#### R9.6: The override still outranks the file for an unrelated save
- **Given**: A session that has dragged to 400, latch armed
- **When**: A config save arrives that changes only `theme.background`
- **Then**: The sidebar remains 400 — the drag is not undone by an unrelated save

---

### R10: Collapsed state persists across every entry point (D16, D17a)

#### R10.1: Drag-collapse persists
- **Given**: A drag that crosses the collapse threshold and is released
- **When**: The write runs, then the app relaunches
- **Then**: `cfg.sidebar.collapsed == true` and the sidebar starts hidden

#### R10.2: The Hide button persists identically
- **Given**: The sidebar is open
- **When**: The user clicks Hide, then relaunches
- **Then**: The sidebar starts hidden — the button and the drag agree, both routed through
  the same persistence path

#### R10.3: Restore persists too
- **Given**: A persisted collapsed sidebar
- **When**: The user clicks Restore and relaunches
- **Then**: The sidebar starts open at its persisted width

#### R10.4: A chord toggles the sidebar
- **Given**: The sidebar is hidden and the terminal has focus
- **When**: The user presses the toggle chord
- **Then**: The sidebar is restored, the grid re-fits, and the new state is persisted

#### R10.5: The chord does not shadow shell input
- **Given**: The chosen chord
- **When**: The key table and `encode_key` are inspected
- **Then**: It collides with no existing `tab_action` chord and with no byte sequence
  `encode_key` forwards to zsh

---

## Coverage map (decision → requirement)

| Decision | Covered by |
|---|---|
| D1 live per-frame resize | R0.1, R5.1, R5.2 |
| D2 persist on release | R0.2, R8.1, R7.3 |
| D3 hit_test owns routing | R1.1, R1.7 |
| D4 no region while collapsed | R1.6 |
| D5 override outranks file | R9.6 (amended by R9.3) |
| D6 one refit site | R5.4 |
| D7 logical px | R8.1 |
| D8 surgical + atomic write | R8.2, R8.3, R8.5 |
| D9 grab offset / no accumulation | R3.1, R3.3, R3.4 |
| D9a raw vs clamped | R3.2, R4.4 |
| D10 6px band, arm position | R1.1–R1.5 |
| D11 ColResize cursor | R2.1–R2.4, R6.2 |
| D12 collapse hysteresis | R4.1–R4.3 |
| D13 refit guard | R5.1, R5.2, R5.3 |
| D13a wheel over band | R6.1 |
| D14 write failure | R8.4, R8.6, R9.5 |
| D15 latched echo suppression | R9.1, R9.2, R8.7 |
| D15a real edit wins | R9.3, R9.4 |
| D16 collapsed persists | R10.1, R10.2, R10.3 |
| D17 pointer capture | R7.1–R7.7 |
| D17a toggle chord | R10.4, R10.5 |

No orphan decisions. Boundary check: every renderer-side geometry sub-req (R1, R6.2) has
its app-side routing counterpart (R1.7, R7); every `common`-side write sub-req (R8) has
its app-side trigger (R7.3, R7.6) and its watcher-side consumer (R9).

## Tasks

> **Note on the DAG's shape.** This feature is concentrated in two files —
> `crates/app/src/terminal_app.rs` (the single `window_event` match) and
> `crates/app/src/input.rs`. Only T1 and T2 are genuinely independent; from T4 onward the
> dependencies are **file-overlap serialization**, not interface dependencies. They are
> labelled so, because a parallel executor that ignores them produces merge conflicts, not
> a faster build.

### T1: Grab-band geometry + `hit_test` arm [infra — renderer only]
- **Fulfills**: R1.1, R1.2, R1.3, R1.4, R1.5, R1.6
- **Depends on**: (none) — parallel with T2
- **Files**: `crates/renderer/src/sidebar_layout.rs`
- Add the grab `SidebarRegion` variant and a pure `grab_band_rect(inp)`; insert the arm
  **immediately after step 2's viewport bail** (D10). Unit-test the ordering against the
  menu popup (R1.4) and the viewport bail (R1.5) — both are in-file `#[cfg(test)]`.

### T2: Surgical atomic config writer [infra — common only]
- **Fulfills**: R8.2, R8.3, R8.4, R8.5, R8.7
- **Depends on**: (none) — parallel with T1 and T3
- **Files**: `crates/common/src/config.rs`
- A `save_sidebar(path, width_px, collapsed)` that rewrites only the target lines,
  inserts a `[sidebar]` table when absent, `create_dir_all`s, writes `config.toml.tmp` in
  the same directory and `rename`s over the target. Format floats with `{}` for exact
  round-trip (R8.7). Tests use a `tempdir`, no GPU or PTY.

### T3: Route the new region through press, hover, cursor, and wheel [vertical]
- **Fulfills**: R1.7, R2.1, R2.2, R2.3, R2.4, R6.1, R6.2
- **Depends on**: T1 (consumes the region variant) — parallel with T2
- **Files**: `crates/app/src/input.rs`, `crates/app/src/terminal_app.rs`
- Add the resize `SidebarAction`, map it in `sidebar_press`, and add a pure
  region→`CursorIcon` helper that `refresh_hover` feeds from its **existing single**
  `hit_test` call (R2.3). Confirm the wheel router scrolls the list over the band (R6.1)
  and that `sidebar_hover` returns `None` for it (R6.2).

### T4: Resize drag state machine + pointer capture [vertical]
- **Fulfills**: R3.1, R3.2, R3.3, R3.4, R7.1, R7.2, R7.3, R7.4, R7.5, R7.7, R0.1 (partial)
- **Depends on**: T3 (consumes the action) — **and** file overlap on `input.rs`/`terminal_app.rs`
- **Files**: `crates/app/src/terminal_app.rs`, `crates/app/src/input.rs`
- `resize_drag: Option<ResizeDrag>` disjoint from `dragging`; a **new preceding**
  `Released` arm; the pure `raw`/`clamp` mapping in `input.rs` (testable without a
  window); hover skipped mid-drag; `CursorLeft` non-terminating. Assert
  `resize_drag.is_some() ⇒ !dragging` when arming (R7.2).

### T5: Collapse ⇄ re-expand hysteresis mid-drag [vertical]
- **Fulfills**: R4.1, R4.2, R4.3, R4.4
- **Depends on**: T4 (needs the drag state) — logic dependency
- **Files**: `crates/app/src/terminal_app.rs`, `crates/app/src/input.rs`
- A pure threshold function over `raw` + current mode (the whole hysteresis table is
  unit-testable), wired to `set_sidebar_collapsed`. R4.4 is the regression test for the
  raw-vs-clamped contradiction D9a fixed.

### T6: `(cols, rows)` refit guard at the drag call site [vertical]
- **Fulfills**: R5.1, R5.2, R5.3, R5.4, R0.1 (completes)
- **Depends on**: T5 — **file-overlap serialization only** (no logic dependency)
- **Files**: `crates/app/src/terminal_app.rs`
- Compare the prospective `(cols, rows)` against the live pair before calling
  `settings::refit`. R5.3 must assert `refit`'s behavior is **unchanged** for startup,
  `Resized`, and config reload — the guard does not move into `settings.rs`.

### T7: Commit + persist on release and focus loss [vertical]
- **Fulfills**: R8.1, R8.6, R7.6, R0.2
- **Depends on**: T2 (the writer), T6 (file overlap on `terminal_app.rs`)
- **Files**: `crates/app/src/terminal_app.rs`
- Call `save_sidebar` from the resize `Released` arm and from `Focused(false)`, clearing
  `resize_drag` in both (R7.6) so no second write occurs. A failed write warns and keeps
  the width (R8.6) — no `unwrap`, no revert.

### T8: Latched echo suppression + override precedence [vertical]
- **Fulfills**: R9.1, R9.2, R9.3, R9.4, R9.5, R9.6
- **Depends on**: T7 (arming happens at the write site)
- **Files**: `crates/app/src/terminal_app.rs`, `crates/renderer/src/lib.rs`
- Latch the last successfully-written `(width_px, collapsed)`; suppress every equal
  delivery; on a differing delivery clear **both** persisted override fields (needs a
  small `clear_sidebar_override` on `Renderer`) and let `settings::apply` run. A failed
  write arms nothing and marks the fields override-authoritative (R9.5).

### T9: Collapsed persistence across all three entry points + toggle chord [vertical]
- **Fulfills**: R10.1, R10.2, R10.3, R10.4, R10.5
- **Depends on**: T8 (file overlap + the same persistence path)
- **Files**: `crates/app/src/terminal_app.rs`, `crates/app/src/input.rs`
- Route `toggle_sidebar` (both existing button paths) and the new chord through the same
  persist call. Verify the chosen chord collides with neither `tab_action` nor anything
  `encode_key` forwards to zsh (R10.5).

**DAG check**: no cycles. Critical path `T1 → T3 → T4 → T5 → T6 → T7 → T8 → T9`; T2 runs
concurrently with T1/T3 and joins at T7. Every requirement R0–R10 is fulfilled by at
least one task.

## External Dependencies

### Pre-work
- **Choose the D17a toggle chord.** Requires reading the live chord table
  (`input.rs:27-42`) and the `encode_key` byte mappings; it is a lookup, not a design
  decision, but T9 is blocked without it.

### Post-work
- **Eyeball it** — `/run-term`, then drag: check the cursor changes on the band, the grid
  reflows column-by-column, collapse/re-expand does not flicker, and the width survives a
  relaunch. None of R0.1, R2.1, or R4.3 is convincing from a unit test alone.
- **Tune the judgement constants** (Known Gaps): the 220/260 hysteresis gap and the 6px
  band width are unmeasured; adjust by feel after the first real session.
- **Consider documenting** the new `sidebar.width_px` / `collapsed` write-back behavior
  wherever the config is described for users, since these become the first fields the app
  writes rather than only reads.

## Known Gaps

- **`apply_config` no-op early-out not taken.** D15 suppresses the echo locally rather
  than fixing `renderer::apply_config` to skip an unchanged `Config` — which would help
  every reload path. Deliberately deferred as another feature's surface; worth a
  follow-up spec if the atlas rebuild shows up in another reload path.
- **A drag-collapse remembers 260, not the pre-drag width.** `clamp` floors every raw
  value below 260 at 260, while D12's collapse only fires below 220 — so a continuous drag
  from 400 writes 260 throughout the [220, 260) hysteresis band before collapsing, and a
  later Restore brings the sidebar back at 260. This is the honest consequence of the
  pointer having really been dragged to the minimum on the way down (the Hide button does
  the same if you drag to 260 first), but it is a behavioural choice, not an accident:
  preserving the pre-drag width would need the gesture to latch it at press and restore it
  on collapse. Found by the Gate-3 checker during verification.
- **`save_sidebar` is an unsynchronised read-modify-write.** A hand-edit saved between our
  `read_to_string` and our `rename` is silently clobbered. Inherent to D8's surgical
  rewrite; the window is milliseconds and the loser is one edit, not the file.
- **Hysteresis thresholds (220 / 260) are a judgement, not a measurement.** They were
  chosen to be jitter-proof under D1's per-frame reflow; no trackpad jitter data was
  collected. Expect to tune the 40px gap by feel.
- **Grab-band width (6 logical px) is unvalidated against a trackpad.** It is inside the
  sidebar only (D10), so it is strictly less forgiving than a straddling band; if it
  proves hard to hit, widening it inward is a one-constant change, but widening it
  *outward* would require reopening D10's hit_test bound.
- ~~**D17a's chord is unassigned.**~~ **Resolved at execution pre-work: `Cmd+Shift+S`.**
  Free against the full live inventory — `tab_action`'s Cmd+T/W/[/]/1-9
  (`input.rs:36-41`), Cmd+C (`copy.rs:23`), Cmd+V (`clipboard.rs:83`), Cmd+Shift+M
  (`overlay.rs:79-85`), Cmd+Shift+R (`rename.rs:81-87`) — and consistent with the
  `Cmd+Shift+<letter>` family both existing chrome toggles use. Must be intercepted
  **before** `encode_key`: that function swallows `super` only for its `key_seq` table,
  so a `Key::Character` chord otherwise reaches zsh through the text fallback
  (`input.rs:365-370, 384`).
- **The `(cols,rows)` refit guard bounds rate, not total.** ~20 column crossings per full
  drag × every open tab (D13). Accepted, unmeasured.
- **No performance number is attached to the per-frame path.** `docs/domain.md` invariant
  4 says performance is judged by numbers, but the project's budgets are still explicitly
  goals, not gates (G1 in `docs/architecture.md`) — so D13's guard is justified by the
  *correctness* argument (selection loss), with the SIGWINCH saving unquantified.
