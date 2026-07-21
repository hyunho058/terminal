# Spec: sidebar-foundations

## Meta
- **Created**: 2026-07-19
- **Type**: dev
- **Status**: ready
- **Approved by**: batch mode — the human approved the partition (features, declared surfaces, `depends_on` edges, SD1–SD8) at decompose's single gate; specify's L0/L2/L3/L4 prompts are bypassed per `pre-approved-batch: yes`
- **Approved at**: 2026-07-20
- **Execution**: NOT handed off. Batch mode's deliverable is this file; `decompose` → `build-order` owns execution.
- **Set**: sidebar-redesign (`specs/sidebar-redesign/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single human gate)
- **L2 review**: 3 rounds (initial + 2 retries — the skill's maximum). Rounds 1 and 2 returned NEEDS_FIX on source-verified defects and were fixed and re-reviewed. Round 3 returned one BLOCKING item (the `headless()` visibility gap, now fixed in D10) plus six non-blocking items, all of which were also fixed. **The round-3 fixes were applied without a further review pass, the retry budget being spent** — they are direct transcriptions of the reviewer's own prescribed remedies, not novel designs, but they are unverified. The cross-spec `coherence-audit` at decompose-end is the next independent check.

## Declared Surface
- feature-id: sidebar-foundations
- depends_on: []
- declared-surface:
  - crates/common/src/config.rs
  - config.example.toml
  - crates/renderer/src/sidebar_theme.rs
  - crates/renderer/src/sidebar_shapes.rs
  - crates/renderer/src/sidebar_shapes.wgsl
  - crates/renderer/src/sidebar_icons.rs
  - crates/renderer/src/lib.rs

## Goal
Give the sidebar redesign its foundation: a `[sidebar]` TOML section carrying the design mock's knobs and defaults, the mock's full set of design tokens (colours *and* metrics) resolved in one place, a GPU primitive able to draw the mock's rounded / bordered / gradient-filled surfaces, and the mock's icon set — plus the `lib.rs` plumbing that lets a draw layer actually reach the config. All of it consumed by later features in the set; none of it drawing anything on its own yet.

## Non-goals
- Drawing any sidebar pixel. This feature registers **no** `DrawLayer` and changes nothing on screen; `sidebar-row-redesign` and `sidebar-chrome-collapse` are the first consumers.
- Rewriting `crates/renderer/src/tab_bar.rs`. It keeps working, registered and unchanged; `sidebar-row-redesign` owns its replacement.
- Any per-tab data (git branch, agent status) — that is `tab-metadata`.
- Deciding *which* agent maps to which skin. This feature defines the `AgentSkin` vocabulary; `sidebar-row-redesign` owns the mapping (D13).
- *Consuming* `accent` on any sidebar surface. The key is defined and resolved here because SD7 names it a user-facing setting, but every element the mock tints with it is deferred under SD8 (D1).
- The search field, the mock's terminal-pane chrome (top-right icon row, footer bar, "Use agent" banner) and the agent welcome block (SD8).
- Text/glyph rasterisation. The existing `renderer::atlas` swash path stays the only text renderer; this feature adds shapes only.
- A general-purpose vector-graphics engine. The primitive covers exactly the shapes the mock's sidebar uses, nothing more.

## Confirmed Goal
Add a `[sidebar]` section to `common::config::Config` (`width_px`, `show_git_branch`, `show_agent_status`, `collapsed`, `accent`) whose defaults are the mock's own prop values, loading under the existing `#[serde(default, deny_unknown_fields)]` contract and clamped at load; expose `SidebarTheme` + `SidebarMetrics` resolving the mock's complete token set (≈15 colours, ≈15 metrics, 3 agent skins) as one source of truth, with metrics scaled by a typed conversion rather than a per-callsite convention; add one instanced wgpu pipeline type — built on the crate-root `build_pipeline`/`QUAD`/`upload_resident` helpers, not a private copy — that draws antialiased rounded boxes, capsules and rings with per-instance corner radius, border and up-to-3-stop linear gradient; express the mock's six sidebar icons (filter, plus, panel, git-branch, agent asterisk, ready diamond) as const data tables emitted through that primitive; and thread `SidebarConfig` + `scale_factor` through `Renderer`/`FrameCtx` so a future layer can reach them. Done when the config round-trips and clamps, the tokens resolve, every geometry / SDF / icon function is unit-tested headlessly, the WGSL compiles and its coverage cross-checks the Rust SDF under the repo's existing guarded-`headless()` idiom, and `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` are green with **no visual change** to the running terminal.

## Research

### The design source
- Mock is `Mac Terminal.dc.html` in Claude Design project `2ae889c6-09a6-402b-8675-9d5bafd3aa88` ("MAC 터미널 디자인", owner podo), imported read-only via the `DesignSync` MCP.
- The mock's own editable props fix three knobs: `accent` (default `#86b0a1`, alternates `#e0863f` / `#7796b8` / `#9db08a`), `showGitBranch` (default `true`), `sidebarWidth` (default **340**, range **260–440**, step 10, unit px).
- **Where the mock actually uses `accent`:** `agentNameStyle.color`, `promptStyle.color` (the `❯`), `footerModelStyle.color`. All three are terminal-pane surfaces, every one of them out of scope under SD8. No sidebar element is tinted by `accent`.
- Sidebar panel: `background:#1e1b18`, `borderRight:1px solid #2e2a24`, full height. Page background `#24211d`.
- Card (`rowStyle`): `padding:12px 15px`, `margin:3px 8px`, `borderRadius:13px`, `border:1px solid <border>`, `gap:6px`, `flexDirection:column`.
- Plain card: bg `transparent` / hover `#2a2722` / selected `#302c26`; border `transparent` unselected, `#46413a` selected.
- Agent skins are **3-stop 135° linear gradients** — `antigravity` bg `rgba(58,134,224,.14) → rgba(122,79,214,.13) @55% → rgba(224,69,58,.11)`, hi `.32/.27/.22`, border `rgba(122,79,214,.45)`, dim `rgba(160,150,220,.16)`; `claude` is 2-stop `rgba(255,157,92,.15) → rgba(214,82,31,.12)` (hi `.32/.26`), border `rgba(224,120,60,.5)`, dim `rgba(224,140,90,.18)`. **The mock defines exactly two skins** — there is no generic/third skin.
- Header: `padding:14px 12px 12px`, `gap:8px`; buttons `padding:6px`, `borderRadius:8px`, icon colour `#8f887a`, hover `{background:#2a2722, color:#c9c2b2}`.
- Text: title `15px/600` `#e8e2d2`; branch line `12.5px` `#8f887a`; mark `13px`.
- Marks: `agent` = `✳` (U+2733) `#e2d9c8`, `ready` = `◇` (U+25C7) `#7796b8`.
- Icons are **stroke-only**, `viewBox="0 0 24 24"`, `fill:none`, stroke-width 1.6–2.0, built from `<line>`, `<circle>`, `<rect rx>` and one `<path>` arc (git-branch: `M6 8.4v7.2M18 11v1.5a4 4 0 0 1-4 4H8.4`). The filter icon's three dots are the only *filled* sub-shapes (`fill:#1e1b18`).

### Existing config plumbing (the pattern to copy)
- `crates/common/src/config.rs:17-23` — `Config { font, theme, grid }` with `#[serde(default, deny_unknown_fields)]`; every sub-struct repeats the attribute pair (`:25-26`, `:35-36`, `:44-45`) and has **pub fields** (serde requires it).
- `crates/common/src/config.rs:35-42` — `ThemeConfig` stores colours as packed `u32` `0xRRGGBB`; the established colour representation.
- `crates/common/src/config.rs:51-82` — each sub-struct hand-writes `impl Default`; `Config` derives it.
- `crates/common/src/config.rs:103-112` — `load_from(&Path)` is the single parse entry point used by `load()`, tests and the exemplar. **This is the only place a load-time normalisation can live and be unavoidable.**
- `crates/common/src/config.rs:127-137` / `:139-154` — the two test precedents: "omitted keys fall back to defaults", and "omitted → default AND an explicit opt-in is still honoured".
- `config.example.toml:1-23` — documents every section with comments and states that unknown keys are rejected. **Observed inconsistency:** line 13 documents `ligatures = true` while `FontConfig::default()` is `false` (`config.rs:60`).
- `crates/app/src/settings.rs:23-34` — `settings::apply(cfg, renderer, tabs)` is the live-reload path: `renderer.apply_config(cfg)` → `set_left_inset(...)` → `tabs.resize_all(...)`.
- `tracing` is already a `common` dependency — a load-time `warn!` needs no new crate.
- TOML integer literals deserialize into `f32` under the workspace's `toml` + `serde`, so `width_px = 340` parses (verified against the workspace lock).

### Renderer plumbing — verified, including three gaps the first draft got wrong
- `crates/renderer/src/lib.rs:526-560` — **`apply_config(&mut self, cfg: &Config)` destructures four fields (`font.size_pt`, `font.ligatures`, `theme.background`) and then DROPS the `&Config`.** `Renderer` stores no `Config` (struct `:144-180`) and `FrameCtx` (`:113-131`) has no config field. **A `DrawLayer` therefore cannot see `[sidebar]` at all today** — the plumbing is real work, and it lives in `lib.rs`, this feature's surface (D10).
- `crates/renderer/src/lib.rs:163` — **`scale_factor: f64` is a private field with no public accessor**, and it is absent from `FrameCtx`. A layer cannot convert logical px to physical px today (D1, D10).
- `crates/renderer/src/lib.rs:1010-1011` — `cell_size_px(cfg, scale_factor)` computes `size_pt * scale_factor`: the precedent that **config lengths are logical units scaled at use**.
- `crates/renderer/src/lib.rs:1048-1060` — **`fn headless() -> Option<(wgpu::Device, wgpu::Queue)>` already exists** in `mod tests`, returning `None` (via `.ok()?`) when no adapter is available. `Renderer::new_headless` exists at `:243` "for benchmarks and tests", and several renderer tests create a real device and record real passes. **Guarded GPU tests are an established idiom here** — `docs/architecture.md:16` lists D10 as "`cargo test` + `insta` snapshots + **headless smoke**", which includes them rather than forbidding them (D5).
- `crates/renderer/src/lib.rs:113-131` — `FrameCtx { device, queue, encoder, view, viewport, cell, top_inset, left_inset, format, snapshot, atlas }`; `trait DrawLayer { z_order, record }`; z-order glyph=0 < tab bar=10 < image=20 < overlay=100.
- `crates/renderer/src/lib.rs:1110-1119` — `const _:` offset assertions on uniform structs: the established machine-check for GPU layout.
- `crates/renderer/src/tab_bar.rs:121` `QUAD`, `:114` `Vertex`, `:130` `Uniforms`, `:499` `PREMULTIPLIED`, `:512` `uniform_entry`, `:525` `build_pipeline` — **all module-private**. None are `pub`/`pub(crate)`, `tab_bar.rs` is not in this feature's declared surface, and `sidebar-row-redesign` deletes it. **This feature cannot reuse any of them** (D11).
- `crates/renderer/src/tab_bar.rs:155-175` — px→clip is `vec2(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0)`, top-left px origin, no inset applied (correct for a band at x=0).
- `crates/renderer/src/tab_bar.rs:578` — `fn linear(packed: u32, a: f32) -> [f32; 4]` is private and dies with the file; the surviving copy must live here (D6).
- `crates/renderer/src/tab_bar.rs:451-464` — the current sidebar does `create_buffer_init` **per frame**, and `terminal_app.rs:428-430` drives a **continuous** redraw loop (the winit#2640 workaround). Any per-frame allocation this feature's API forces on consumers is paid ~60×/second (D7, Tier 3).

### Constraints found in project docs
- `.claude/rules/no-gui-crates.md` — allowed render stack is exactly `wgpu`, `swash`, `bytemuck`, `raw-window-handle` (+ `winit`, `objc2` outside the renderer). Widget/vector-graphics toolkits prohibited.
- `.claude/rules/dependency-direction.md` — `renderer` depends on `common` + `term-core` only, never `winit`.
- `.claude/rules/macos-only.md` — no `#[cfg(target_os)]` branches.
- `CLAUDE.md` — `thiserror` error enums, no `unwrap()` outside tests; gates `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.
- `.claude/settings.json` — PostToolUse `rustfmt` on edited `*.rs`; PreToolUse blocks `Cargo.lock` edits.
- Existing deps cover everything needed — **no new crate required**. `pollster` is a *regular* dependency of `renderer` (`crates/renderer/Cargo.toml:12`, used in non-test code at `lib.rs:207`), so a `block_on` in a test needs no dev-dependency.
- **Crate-root render helpers in `lib.rs` — private, but reachable.** `Vertex` (`:58`), `QUAD` (`:101`), `upload_resident` (`:748`), `uniform_entry` (`:935`) and `build_pipeline` (`:949`) are module-private *at the crate root*, and Rust makes a crate-root private item visible in every descendant module — so `sidebar_shapes.rs` calls them as `crate::build_pipeline(...)` with **no visibility change and no out-of-surface edit**. `build_pipeline` already hardcodes vertex buffer 0 as the `Float32x2` corner, exactly the layout D3 needs. One caveat: `uniform_entry` is `ShaderStages::VERTEX` only, so a fragment stage that reads the viewport uniform needs its own descriptor (D11).
- `upload_resident`'s doc (`lib.rs:743-747`) states: *"An empty slice leaves the buffer untouched and returns 0 (the caller skips the draw), so no zero-size buffer is ever allocated."* — the codebase already treats a zero-size buffer as a real hazard.
- **No texture-readback precedent exists.** `copy_texture_to_buffer` and `map_async` appear nowhere in the workspace (grep-verified), so D5's readback is genuinely new machinery, unlike the device/pass half of that test.

## L2 Checkpoints

Complexity: **Medium (2 signals)** — 3+ independent subsystems (TOML config schema · WGSL/GPU pipeline · icon + token tables); data-schema design (the `[sidebar]` TOML shape *and* the GPU instance layout are both new contracts). No external resource loading, no state machine, no async resources, no multi-actor runtime interaction. (Consumers — user, draw path, sibling features, reload path — are consumers, not concurrent actors, and are not re-counted.)

Brownfield adjustment applied: Core 30% · Scope 20% · Error 25% · Data 15% · Implementation 10%.

### Core Behavior (30%) — 8/8 resolved
- [x] Config section fields + defaults → D1
- [x] Length-unit semantics for `width_px` → D1 (L1: `cell_size_px` scales by backing factor)
- [x] What the shape primitive can draw → D3
- [x] Gradient capability → D4
- [x] Token resolution entry point (colours **and** metrics) → D6
- [x] **The public API consumers program against** (lazy `ensure` construction, resident buffer write, pass record, ownership) → D3
- [x] **How config + scale_factor reach a draw layer** → D10 *(added by Tier-1: verified `apply_config` drops the `&Config`)*
- [x] **Logical→physical scaling is a typed conversion, not a per-callsite convention** → D6 *(added: SD2's draw/hit-test parity fails silently if one call site forgets to multiply, and it is invisible at 1× scale)*

### Scope Boundaries (20%) — 6/6 resolved
- [x] This feature draws nothing / registers no layer → D10
- [x] Out-of-scope surfaces (search, footer, banner, welcome block) → SD8
- [x] `tab_bar.rs` left untouched and still registered → D10
- [x] Platform + no-GUI-crate boundary → SD1, `macos-only`, `no-gui-crates`
- [x] **`accent` is defined and resolved here but consumed by no sidebar surface** → D1 *(SD7 names it a user-facing setting; every element the mock tints with it is deferred under SD8)*
- [x] **The agent→skin mapping seam belongs to `sidebar-row-redesign`, not here** → D13 *(added by Tier-1: `tab-metadata` is a root and cannot reference renderer types)*

### Error / Edge Cases (25%) — 9/9 resolved
- [x] Unknown `[sidebar]` key → D1 (`deny_unknown_fields`, fails loudly — existing contract)
- [x] Out-of-range `width_px`, and where the clamp is unbypassable → D2
- [x] Warning emitted once at load, not per frame → D2
- [x] Corner radius larger than the box → D9
- [x] Border wider than half the box → D9
- [x] Zero / negative / NaN instance extent → D9
- [x] **Zero-length gradient direction vector** (normalise → NaN) → D9 *(added by Tier-2 on D4)*
- [x] **Gradient mid-stop at 0.0 / 1.0 / outside [0,1]** (divide-by-zero in two-segment interpolation) → D9 *(added by Tier-2 on D4)*
- [x] **Zero instances to draw** — every shape dropped by D9, or a collapsed sidebar; a zero-size buffer is a wgpu error → D3 *(added by Tier-2 on D9: tightening the constructors made an empty slice newly reachable)*

### Data Model (15%) — 6/6 resolved
- [x] `[sidebar]` field names, types, defaults → D1
- [x] GPU instance struct layout → D3
- [x] **Attribute location → field map and formats** (the `kind`/`_pad` pair must be `Uint32x2`, not a float attribute) → D3
- [x] Colour representation (packed `u32` in, linear RGBA out) → D6
- [x] **Metric tokens (radius, paddings, margins, gap, stroke widths) have an owner** → D6 *(added: SD2 pushes layout into two consumer features, which would otherwise each define their own constants and drift)*
- [x] Icon data shape and emit signature → D7

### Implementation (10%) — 6/6 resolved
- [x] serde `default` + `deny_unknown_fields` + hand-written `impl Default` → L1 `config.rs:17-82` (`assumed: true`)
- [x] `#[repr(C)]` + `bytemuck::{Pod, Zeroable}` + `const _:` offset assertions → L1 `tab_bar.rs:115-153`, `lib.rs:1110-1119` (`assumed: true`)
- [x] Headless pure-function tests in `#[cfg(test)] mod tests` → L1 `tab_bar.rs:588-683` (`assumed: true`)
- [x] **Pipeline scaffolding is REUSED from the crate root, not duplicated** → D11 *(corrected twice: first wrongly auto-resolved as "reuse `tab_bar`'s", then wrongly decided as "duplicate them" — the helpers exist at the crate root in `lib.rs`, which is in surface, and are visible to descendant modules)*
- [x] **Instance upload uses the resident grow-only buffer, not per-frame `create_buffer_init`** → D3 *(added: `tab_bar.rs:451-464`'s per-frame allocation is the anti-pattern; `upload_resident` (`lib.rs:748`) is the codebase's fix)*
- [x] **WGSL lives in a separate file pulled in with `include_str!`, not an inline const** → D11 *(the declared surface lists `sidebar_shapes.wgsl`)*

**Composite: 1.00** (35/35 resolved; every dimension 1.00 ≥ 0.60 floor). Unknown/unknowns: 0 pending.

> Scoring honesty note: this table reached 1.00 twice before an adversarial review found real gaps both times (the config never reaching a layer; the pipeline helpers being reusable after all; an empty instance slice becoming reachable *because* D9 was tightened). A 1.00 here means "no checkpoint I can currently name is open", not "no gap exists" — the L2-reviewer, not this table, is the gate.

### Unknown/Unknown Detection

**Tier 1 (Actor).** Actors: the *user editing TOML* (D1, D2, D12 ✓); the *renderer draw path* consuming the primitive (D3, D4, D5, D11 ✓); the *two downstream sibling features* consuming tokens + icons (D6, D7, D10 ✓); the *live config-reload path* `settings::apply` (initially 0 decisions) → **+1 Core checkpoint**: how does `[sidebar]` reach a layer at all? Investigating showed `apply_config` *drops* the `&Config` and `scale_factor` has no accessor → resolved by **D10**, which now owns that plumbing. And the *sibling `tab-metadata` feature* as a producer of agent identity (0 decisions) → **+1 Scope checkpoint** → resolved by **D13**.

**Tier 2 (Implication).**
- D3 (three kinds, one pipeline) → implies a `kind` discriminant *and* a stated public API and attribute map → both now in D3.
- D4 (per-instance gradient) → implies a direction representation → D4; and implies **two new NaN-capable inputs** (zero-length direction, degenerate mid) → **+2 Error/Edge checkpoints** → D9.
- D5 (antialiased SDF) → implies "how is shader maths verified without a GPU?" → investigating showed the repo already has a guarded `headless()` helper → D5 now uses it instead of rejecting it.
- D6 (token resolution) → implies **metric** tokens need an owner too, or two consumers duplicate them → **+1 Data checkpoint** → D6.
- D7 (icon tables) → implies a coordinate space (24×24, D7) *and* an emit signature whose allocation behaviour every consumer inherits → Tier 3 below.
- D1 (`accent` field) → implies naming what `accent` tints; the mock tints only out-of-scope surfaces → **+1 Scope checkpoint** → D1 keeps the field (SD7 names it a setting set-wide; `deny_unknown_fields` makes late addition breaking) and states plainly that no sidebar element consumes it yet.
- D6 (logical-px metrics) → implies a scaling *mechanism*, or two consumer features multiply ~15 tokens by hand at every use site and SD2's draw/hit-test parity fails invisibly at 1× → **+1 Core checkpoint** → `SidebarMetrics::scaled`.
- D9 (constructors drop degenerate instances) → implies an all-dropped instance list is now reachable → **+1 Error/Edge checkpoint** → D3's empty-slice early return.

**Tier 3 (Pair Tension).** **Runs** — SD4 is an explicit performance/reliability constraint, which qualifies.
- **D7 × SD4.** A `-> Vec<SidebarShape>` return allocates once per icon per frame on a continuously-redrawing loop (`terminal_app.rs:428-430`), in the codebase whose G1 idle-CPU debt is *why* SD4 exists. The allocating signature is decided here even though this feature has no runtime — the cost lands on both consumers. → D7 uses a `&mut Vec<SidebarShape>` sink.
- **D3 × D10.** D3's "one buffer, one draw call" only holds within a single `DrawLayer`; the two consumers are separate layers and D10 forbids shared state in `lib.rs`. → D3's claim is narrowed honestly to "one pipeline *type*, one shader, one instance format", with each layer owning its own instance.
- **D9 × D4.** Covered above (+2 checkpoints).
- **D2 × SD4.** A per-access clamp warning would fire every frame. → D2 moves the clamp and the warning to load time.

### Inversion Probe
*"What could make this fail even if every requirement is met correctly?"* — The primitive could pass all of its own tests and still be **unable to express the mock**, because this feature has no visual consumer and its acceptance is self-referential. Mitigated three ways: D3/D4/D7 are derived from an enumerated inventory of every surface the mock's sidebar draws (R6.1 asserts that inventory is covered); D5's guarded-`headless()` readback means the *shipped WGSL* is executed and cross-checked, not just its Rust twin; and D6 now owns the metric tokens too, so a consumer cannot silently invent different geometry. Residual: nothing here proves the result *looks* right — discharged by `sidebar-row-redesign` and `sidebar-chrome-collapse` via `/run-term`.

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

### D1: `[sidebar]` config section — five fields, mock defaults, lengths in logical px
- **Status**: resolved
- **Rationale**: `SidebarConfig { width_px: f32 = 340.0, show_git_branch: bool = true, show_agent_status: bool = true, collapsed: bool = false, accent: u32 = 0x86B0A1 }`, added as `Config.sidebar` with the same `#[serde(default, deny_unknown_fields)]` pair and pub fields every sibling carries (`config.rs:17-49`), plus a hand-written `impl Default` (`:51-82`). `width_px`, `show_git_branch` and `accent` come straight from the mock's own `data-props` — the designer already decided these are settings, which is SD7's argument. `show_agent_status` is the symmetric partner to `show_git_branch` (SD8 makes agent status a first-class new feature; a user wanting one toggle will want the other). `collapsed` persists the sidebar's hidden state across restarts, which is otherwise lost every launch. `accent` is a packed `u32` `0xRRGGBB` matching `ThemeConfig` (`config.rs:35-42`) rather than a hex string, so it needs no parser and TOML's `0x86B0A1` literal reads naturally.
  **`width_px` is a *logical* px value multiplied by the window scale factor at the point of use**, exactly as `font.size_pt` is (`lib.rs:1010-1011`) — otherwise 340 renders half-size on every retina Mac and the mock's number stops meaning what the mock says. The mechanism is not assumed: `scale_factor` is private with no accessor (`lib.rs:163`), so **D10 exposes it** and **D6 makes the conversion typed** rather than a convention.
  **On `accent` specifically.** An earlier draft dropped it, reasoning that the mock tints only three things with it — `agentNameStyle`, `promptStyle` (the `❯`) and `footerModelStyle` — all three deferred under SD8, so the key would change nothing on screen. That reasoning was **overruled**, for two reasons. First, it contradicted **SD7**, an inherited shared decision that names `accent` among "the natural user-facing settings" and that four sibling specs inherit *verbatim* — dropping the field here would silently invalidate a line those siblings are entitled to rely on, and reconciling only against this feature's accept-seed would never have reached them. Second, and independently: under `#[serde(default, deny_unknown_fields)]`, **a user who writes `accent = …` before the key exists gets a hard `CommonError::ConfigParse` and a terminal that will not start** — and they plausibly will, since the mock's props advertise it and sibling specs cite SD7. Adding the field now costs one defaulted line and *widens* the accepted input set, so it cannot break any config that loads today; the asymmetry runs entirely against omitting it. So `accent` is defined, parsed, clamped-free and resolved into `SidebarTheme` (D6) — with the honest statement that **no sidebar element consumes it in this feature**; its consumers are the deferred terminal-pane surfaces. Rejected: hex-string colours (a parser and an error path for no benefit); rejected: physical px (breaks the mock's numbers on every retina display); rejected: width in columns like today's `width_px(cell_w) = cell_w * 14.0` (the mock's width is a fixed px value independent of font size); rejected: dropping `accent` (contradicts SD7 set-wide, and `deny_unknown_fields` makes late addition a breaking change); rejected: inventing a sidebar binding for `accent` so it "does something" (the mock specifies no accent-tinted sidebar element — `mark_ready_fg` `#7796b8` merely *coincides* with one of the mock's accent alternates, and treating that as intent would be fabricating design).

### D2: `width_px` is clamped to 260–440 **at load**, warned once, and never observable unclamped
- **Status**: resolved
- **Rationale**: The mock's own slider bounds the width to 260–440, and outside that band the design breaks down concretely: below ~260 a two-line card cannot fit a title plus a branch line without permanent ellipsis; above ~440 the sidebar eats more terminal columns than a laptop grid can spare. But a bad number in a *cosmetic* field must never stop the terminal starting — that would make a typo unrecoverable without another terminal. So the clamp happens **inside `Config::load_from` (`config.rs:103-112`), the single parse entry point used by `load()`, tests and the exemplar**, mutating the value before it is ever returned. That makes it unbypassable: the field stays `pub` (serde requires it), but no consumer can read an unclamped value and `Serialize` round-trips the clamped one. The single `tracing::warn!` naming requested-vs-effective is emitted there too — at load and reload only, never per access. This matters because SD4 records that the app redraws continuously (`terminal_app.rs:428-430`): a clamp-on-accessor would emit ~60 warnings/second. **Testing note:** the two precedent tests cited in Research (`config.rs:127-137`, `:139-154`) both call `toml::from_str::<Config>` directly, which **bypasses `load_from` and therefore bypasses the clamp** — so a clamp test cannot follow them; it must write a temp file and go through `load_from`. That same fact bounds the "never observable unclamped" claim honestly: every *production* path funnels through `load_from` (`main.rs:28` → `Config::load`, and `watch.rs:74` `try_reload`), but `toml::from_str::<Config>` stays an unclamped door for any future caller that skips it. Rejected: hard rejection via `CommonError::ConfigParse` (a cosmetic typo bricking startup is a worse failure than a clamped sidebar); rejected: clamping in a `width_px()` accessor (bypassable through the pub field, and the warning would spam the redraw loop); rejected: a private field + newtype (fights serde and diverges from all three sibling structs for one field); rejected: no clamp (`width_px = 100000` reduces the grid to one column, which reads as a crash).

### D3: One instanced pipeline **type** draws three shape kinds, with an explicit public API and attribute map
- **Status**: resolved
- **Rationale**: Every surface the mock's sidebar draws decomposes into three primitives — **rounded box** (panel, cards, panel-icon outline, collapsed restore button), **capsule** (a thick segment with round caps: every SVG `<line>`, every straight path run, dividers, and the 1px right border as a degenerate case) and **ring** (a stroked circle: git-branch nodes, filter dots) — so one shader with `kind: u32` and a `switch` in the fragment stage covers the whole surface inventory with one instance format and one place holding the px→clip transform.
  **Public API** (what the two consumers program against). A `DrawLayer` has no `wgpu::Device` until `record(ctx)` runs — layers are constructed device-free (`TabBar::new()` returns `gpu: None`), and this crate already contains **four hand-rolled copies** of the same lazy-init dance (`tab_bar.rs:242`, `image_layer.rs:92`, `overlay_layer.rs:139`, `cursor_layer.rs:108`). So the API publishes the lazy form and neither consumer reinvents it:
  - `fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self` — the primitive constructor. It stays public because D5's pipeline-build test needs a pipeline from a bare device: going through `ensure` would force a test to assemble a twelve-field `FrameCtx` including a font-backed `Atlas` and a `GridSnapshot`, purely to ask "does the WGSL compile".
  - `fn ensure<'a>(slot: &'a mut Option<Self>, ctx: &FrameCtx<'_>) -> &'a mut Self` — the lazy wrapper over `new`: builds on first use, rebuilds on a target-format change; the layer owns only an `Option<SidebarShapePipeline>` field.
  - `fn draw(&mut self, ctx: &mut FrameCtx<'_>, shapes: &[SidebarShape])` — uploads instances, updates the viewport uniform from `ctx.viewport`, and records its own `LoadOp::Load` pass.
  - **`draw` returns immediately when `shapes` is empty.** This is newly reachable *because* D9 tightened the constructors: a layer whose instances are all dropped — or a collapsed sidebar (D1's `collapsed = true`) — passes `&[]`, and a zero-size wgpu buffer is an error. `upload_resident` already encodes the contract (`lib.rs:743-747`: an empty slice "leaves the buffer untouched and returns 0 (the caller skips the draw)"), so `draw` skips the pass when it returns 0.
  - **Instances go through `crate::upload_resident` (`lib.rs:748`) — a grow-only buffer reused across frames — not `create_buffer_init` per frame.** `tab_bar.rs:451-464` does the latter, which this spec's own Research cites as a cost paid ~60×/second on the continuous redraw loop; reproducing it here would re-create at the GPU-allocation level exactly what D7's Tier-3 analysis eliminates at the `Vec` level, one order of magnitude larger.

  **Each consumer `DrawLayer` owns its own `SidebarShapePipeline`** — its own resident buffer, uniform buffer and bind group.
  **Honest scope of the "one pipeline" claim**: it means one pipeline *type*, one shader source and one instance format — **not** one shared buffer across the sidebar. The two consumers are separate `DrawLayer`s and D10 forbids putting shared render state in `lib.rs`, so there will be two pipeline instances at runtime. That is accepted: pipeline creation is once-per-format (the `ensure_gpu` idiom, `tab_bar.rs:242`), and the alternative — a shared owner in `Renderer` — is exactly the `lib.rs` surface both consumers would then collide on.
  **Instance layout** (`#[repr(C)]`, `Pod`, `Zeroable`, with `const _:` offset assertions in the `lib.rs:1110-1119` style), attribute locations starting at 1 because location 0 is the quad corner (`tab_bar.rs:285`):

  | loc | field | format |
  |-----|-------|--------|
  | 1 | `rect: [f32;4]` (x, y, w, h px, top-left origin) | `Float32x4` |
  | 2 | `radius: f32`, `border_w: f32` | `Float32x2` |
  | 3 | `kind: u32`, `_pad: u32` | `Uint32x2` |
  | 4 | `fill0: [f32;4]` | `Float32x4` |
  | 5 | `fill1: [f32;4]` | `Float32x4` |
  | 6 | `fill2: [f32;4]` | `Float32x4` |
  | 7 | `border_color: [f32;4]` | `Float32x4` |
  | 8 | `grad: [f32;4]` (dir_x, dir_y, mid, _) | `Float32x4` |

  `kind` is `0 = RoundedBox`, `1 = Capsule`, `2 = Ring`. It must ride a `Uint32x2` attribute — packing an integer discriminant into a float attribute would round-trip through `f32` and is not what WGSL's `u32` input expects. Rejected: three separate pipelines (three buffers, three passes, three copies of the transform, for shapes sharing 90% of their fragment maths); rejected: CPU tessellation (a 13px corner needs enough segments to stay smooth, and antialiasing would then need MSAA the renderer does not use); rejected: an arc/bezier kind for the git-branch elbow (D7 approximates it with the three kinds already present).

### D4: The gradient is per-instance, up to three stops, with a direction vector and a mid-stop position
- **Status**: resolved
- **Rationale**: The mock's antigravity skin is a genuine 3-stop 135° gradient (`rgba(58,134,224,.14) → rgba(122,79,214,.13)` at 55% `→ rgba(224,69,58,.11)`); claude's is 2-stop at 120°. `fill0`/`fill1`/`fill2` plus `grad = [dir_x, dir_y, mid, _]` covers both: a 2-stop gradient sets `fill2 == fill1`, a flat fill sets all three equal — one code path for flat, 2-stop and 3-stop with no branching in the caller. The extra 12 floats per instance are irrelevant here: the sidebar emits tens of instances per frame, not the thousands the cell grid does. `dir` is a unit vector in the instance's local box space and `mid ∈ (0,1)`; both are normalised and clamped by D9's constructors, because both are new NaN paths. Rejected: 2 stops only (flattens the antigravity skin the mock deliberately gives three colours — the one visual that distinguishes agents at a glance); rejected: a gradient texture lookup (an extra bind group and an upload to encode six numbers); rejected: a global gradient uniform (different cards need different skins in the same draw).

### D5: Edges are antialiased by an SDF specified in Rust, transcribed to WGSL, and cross-checked on a guarded headless device
- **Status**: resolved
- **Rationale**: A 13px radius and a 1px border look wrong without antialiasing, and the renderer uses no MSAA — so coverage comes from a signed-distance field in the fragment stage (`coverage = clamp(0.5 - d / fwidth(d), 0.0, 1.0)`). Verification is in **two layers**, because either alone is insufficient:
  1. **`rounded_box_sdf`, `capsule_sdf`, `ring_sdf` are `pub fn` in `sidebar_shapes.rs` with table-driven unit tests** (inside / outside / exactly-on-edge / degenerate). These are the *specification*; `sidebar_shapes.wgsl` transcribes them line-for-line, each WGSL function carrying a doc comment naming its Rust twin.
  2. **A guarded headless GPU test executes the shipped WGSL**, in two tiers of decreasing precedent — the implementer should price them separately:
     - **Pipeline-build smoke (fully precedented, cheap).** `fn headless() -> Option<(Device, Queue)>` at `lib.rs:1048-1060` returns `None` when no adapter exists, `Renderer::new_headless` exists at `:243` "for benchmarks and tests", several renderer tests already record real passes, and `docs/architecture.md:16` lists "headless smoke" under D10. Building the pipeline runs the WGSL through naga, so **any syntax or type error fails at test time instead of two features later**. This tier is the must-have.
     - **Four-point coverage readback (NEW machinery, no precedent here).** `copy_texture_to_buffer` and `map_async` appear **nowhere in the workspace** (grep-verified), so this tier introduces a `RENDER_ATTACHMENT | COPY_SRC` offscreen texture, `COPY_BYTES_PER_ROW_ALIGNMENT` row padding, and a `map_async` + device-poll round trip against the wgpu 30 API. It renders one instance of each kind and **compares the readback against the Rust twin evaluated at the same sample points**, with a generous ±0.15 tolerance to absorb adapter rasterisation and `fwidth` differences.
       That comparison — not a monotonicity check — is the point. Asserting only "well-inside ~opaque, well-outside ~transparent, on-edge in between" is satisfied by *any* monotone SDF, **including a wrong one**: transcribe the radius as `r * 0.5`, drop the border term, or swap two components of the capsule projection, and every such sample still passes, because they were chosen away from the boundary. That would leave the tier catching only gross breakage and compile errors — which tier 1 already catches — while the Rust↔WGSL transcription error it exists to detect slips through. Since the Rust twin is already linked into the test binary, comparing against its values costs nothing extra. At least one sample must sit **just inside a large-radius corner arc**, where a radius error displaces the boundary by whole pixels and a mid-arc sample is the only thing that sees it.
       This is still the tier most likely to be flaky, and the one whose cost the earlier draft hid behind "the repo already has this idiom"; it does not, for this half.
  Both tiers skip when `headless()` yields `None`. An earlier draft rejected (2) outright on the false premise that GPU tests are forbidden here; they are not — but only the first tier is genuinely precedented, and saying otherwise would understate the work. Rejected: no antialiasing (the mock's whole look is soft rounded cards; aliased corners read as a bug); rejected: MSAA on the surface (a whole-renderer change to smooth one layer, and it would not fix 1px-border ringing); rejected: the Rust mirror alone (the shipped artifact would never execute in this feature, and D10 gives it no runtime either); rejected: the GPU test alone (it skips on adapterless CI, so the maths would then be unverified there — hence both layers).

### D6: `SidebarTheme` + `SidebarMetrics` own the mock's complete token set — colours **and** metrics
- **Status**: resolved
- **Rationale**: SD7 makes appearance config-driven, but only four values are genuine user knobs; every other token is *design*, not settings. Two structs, both `const`-constructible:
  - **`SidebarTheme`** — the ~15 colours: `panel_bg` `#1e1b18`, `panel_border` `#2e2a24`, `card_bg` (transparent), `card_bg_hover` `#2a2722`, `card_bg_active` `#302c26`, `card_border_active` `#46413a`, `title_fg` `#e8e2d2`, `meta_fg` `#8f887a`, `icon_fg` `#8f887a`, `icon_fg_hover` `#c9c2b2`, `icon_bg_hover` `#2a2722`, `mark_agent_fg` `#e2d9c8`, `mark_ready_fg` `#7796b8`, plus the skins — **and `accent`**, which is stored so the value has a home but is read by nothing in this feature (D1); the deferred terminal-pane surfaces are its consumers.
  - **`SidebarMetrics`** — the metric tokens SD2 would otherwise have two consumer features each redefining and drifting on: `card_radius` 13, `card_pad_x` 15, `card_pad_y` 12, `card_margin_x` 8, `card_margin_y` 3, `card_gap` 6, `border_w` 1, `header_pad` (14, 12, 12), `header_gap` 8, `button_pad` 6, `button_radius` 8, `icon_stroke_w` 1.7, `mark_px` 13, `title_px` 15, `meta_px` 12.5. `SidebarMetrics::DEFAULT` holds them **in logical px**, and **`fn scaled(&self, scale: f64) -> SidebarMetrics` is the only way to get physical px** — every field multiplied once, in one place. This is **centralisation, not a type-level guarantee** — `scaled` returns the same type it consumes, so nothing stops it being applied twice or a logical value reaching a physical slot. It is still worth doing, because it turns ~15 tokens × N call sites × 2 features into one multiplication site, and because double-scaling fails *loudly* (everything twice the size) whereas the failure it prevents fails silently: SD2 requires the app's hit-testing and the renderer's drawing to agree exactly, two separate consumer features each multiply ~15 tokens at many use sites, and **a single missed multiplication is a draw/hit-test mismatch that is completely invisible at 1× scale** — i.e. it would pass every test on a non-retina display and mis-click on every real Mac. Cheap to enforce here; unfixable later without touching both consumer features.
  - **`AgentSkin`** is an enum with `Claude`, `Antigravity` and `Generic`, each resolving four states (`bg`, `hi`, `border`, `dim`) from the mock's values. **`Generic` is defined, not invented**: the mock has no third skin, so `Generic` resolves to the *plain card* treatment already in `SidebarTheme` (transparent / `#2a2722` / `#302c26` / `#46413a`) — an unrecognised agent looks like a normal tab, which is the correct conservative default.
  Colours are stored packed `u32` (matching `ThemeConfig` `config.rs:35-42` and `tab_bar.rs:58-62`) and converted at the draw boundary by `pub fn linear(packed: u32, alpha: f32) -> [f32; 4]` living here — the existing one is private to `tab_bar.rs:578`, which `sidebar-row-redesign` deletes, so this feature must own the surviving copy.
  **Two constructors, each with real inputs.** `SidebarTheme::DEFAULT` is a `const` holding every mock literal. `SidebarTheme::from_config(cfg: &SidebarConfig) -> SidebarTheme` starts from `DEFAULT` and overlays the one config-derived colour, `accent` (D1) — so the argument is genuinely used. The **layout toggles `show_git_branch` / `show_agent_status` are deliberately NOT stored on the theme**: they are not colours, and copying them here would create a second source of truth alongside `FrameCtx.sidebar`, which consumers read directly. (An earlier draft had `from_config` reading only those booleans, making it a constructor that ignored its argument while the struct was also advertised as `const`-constructible — incoherent on both counts.) Rejected: exposing all ~15 colours and ~15 metrics as TOML (a config surface larger than the feature, and `deny_unknown_fields` makes every key permanent); rejected: loose `const`s like `tab_bar.rs:58-62` (that is the state SD7 exists to fix, and two consumers would each need a copy); rejected: colours here but metrics in the consumers (the drift SD2 warns about, split across two features that cannot see each other).

### D7: Icons are const stroke-geometry tables in a 24×24 box, emitted through a caller-owned sink
- **Status**: resolved
- **Rationale**: The mock's icons are stroke-only SVGs on a `0 0 24 24` viewBox, so each is a short const list of `{kind, coords, stroke_w, filled}` entries in that same space. **Six icons** are needed — `Filter` (3 capsules + 3 filled rings), `Plus` (2 capsules), `Panel` (rounded-box outline + capsule), `GitBranch` (3 rings + 1 capsule + the `a4 4 0 0 1` elbow approximated by 2 capsules and a short ring segment), `Asterisk` and `Diamond` (D8). **`Search` is excluded**: its only consumer is the header search field, which SD8 defers — and unlike the filter button (a real button with a real hover state, drawn but inert), the search icon is the ornament of a text input that will not exist. The emit signature is a **sink**, not a return: `pub fn push_icon(out: &mut Vec<SidebarShape>, icon: Icon, x: f32, y: f32, size: f32, color: u32)`. This matters because the signature decided here is paid by both consumers on a continuously-redrawing loop (`terminal_app.rs:428-430`) in the codebase whose G1 idle-CPU debt is why SD4 exists — a `-> Vec` would allocate per icon per frame forever. Keeping the geometry as data means a wrong icon is a wrong number in a table, checkable by a test asserting every entry stays inside the 24×24 box and every icon is non-empty. Rejected: build-time SVG rasterisation (needs an SVG parser + rasteriser, against `no-gui-crates`); rejected: PNGs in the atlas (a decoder dependency, and fixed-resolution icons blur at the retina scale factors vectors handle for free); rejected: an icon font (same dependency problem plus a licence question); rejected: `-> Vec<SidebarShape>` (per-frame allocation, see above).

### D8: The `✳` and `◇` marks are drawn as icons, not as font glyphs
- **Status**: resolved
- **Rationale**: The mock writes them as literal characters, but the terminal's font is whatever the user set in `font.family` (default Menlo, `config.rs:54`). U+2733 EIGHT SPOKED ASTERISK is absent from many monospace faces, so routing the marks through `atlas::glyph_id` risks a tofu box — or nothing — on exactly the indicator that tells the user an agent is running. Drawing them as icons (asterisk = 4 crossed capsules, diamond = 4 capsules forming an outlined rhombus) makes the agent/ready indicator **font-independent**, which for a status indicator matters more than typographic fidelity, and costs nothing extra because D7's table mechanism already exists. Rejected: font glyphs with a tofu fallback (needs a glyph-coverage query the atlas does not expose, and the fallback would be these same icons anyway); rejected: bundling a font containing them (a new asset, a licence, and it would override the user's `font.family` for one character).

### D9: Degenerate geometry is normalised in the constructors, never in the shader
- **Status**: resolved
- **Rationale**: Bad geometry either inverts the SDF or propagates NaN into a fullscreen artefact, and none of it is observable in a shader without a readback. All of it is cheap to normalise on the CPU where it is unit-testable — so `SidebarShape`'s constructors (`rounded_box`, `capsule`, `ring`) are the **only** way to build an instance, and they:
  - clamp `radius` to `min(w,h) / 2`;
  - clamp `border_w` to `min(w,h) / 2`;
  - return `None` for any instance whose `w` or `h` is `<= 0.0` or non-finite — dropped before it reaches the buffer;
  - **replace a zero-length or non-finite gradient direction with the mock's default 135° unit vector** (normalising a zero vector is a divide-by-zero → NaN → precisely the fullscreen artefact this rule exists to prevent);
  - **clamp the gradient `mid` into `[0.05, 0.95]`** — at exactly 0.0 or 1.0 one of the two interpolation segments has zero width and divides by zero, and a value outside `[0,1]` inverts the stop order.
  Rejected: clamping in WGSL (untestable, and every frame pays for a mistake the CPU could fix once); rejected: `debug_assert!` (release builds would still render garbage, and a cosmetic layer must never panic a terminal session); rejected: public struct-literal construction (it would bypass every clamp above).

### D10: `lib.rs` gains three `pub mod` lines **and the config/scale-factor plumbing**; the feature stays visually inert
- **Status**: resolved
- **Rationale**: The first draft claimed `[sidebar]` would "arrive automatically" via `settings::apply` → `apply_config`. **That is false**: `apply_config` (`lib.rs:526-560`) destructures four font/theme fields and drops the `&Config`; `Renderer` stores no `Config` (`:144-180`) and `FrameCtx` has no config field (`:113-131`). Likewise `scale_factor` is private with no accessor (`:163`) and absent from `FrameCtx`, so D1's "logical px scaled at use" would have no mechanism. Both gaps live in `lib.rs` — this feature's declared surface — and if left unassigned, **both** consumers would have to add the same `FrameCtx` fields, which is exactly the collision an ordered-shared surface must avoid. So this feature owns the plumbing, minimally:
  - `pub mod sidebar_theme; pub mod sidebar_shapes; pub mod sidebar_icons;`
  - a `sidebar: SidebarConfig` field on `Renderer`, set in `assemble` and refreshed in `apply_config` (one added line: `self.sidebar = cfg.sidebar.clone();`);
  - `pub fn scale_factor(&self) -> f64` on `Renderer`;
  - two new `FrameCtx` fields — `pub sidebar: &'a SidebarConfig` and `pub scale_factor: f64` — populated where `FrameCtx` is built (`lib.rs:721-733`);
  - **`headless()` is hoisted out of `mod tests` to a `#[cfg(test)] pub(crate) fn headless() -> Option<(wgpu::Device, wgpu::Queue)>` at the crate root**, with the existing `mod tests` calling `crate::headless()`. This is required, not cosmetic: `headless` currently sits inside `#[cfg(test)] mod tests` (`lib.rs:1046`, fn at `:1050`), and **a private module's contents are invisible to a sibling module** — `crate::sidebar_shapes::tests` could not call it, so D5's tests would be unable to obtain a device at all. Making the *fn* `pub(crate)` is not enough while the enclosing module stays private; it has to move. The readback tier additionally needs a `Queue`, and `Renderer` exposes `device()` (`:462`) and `surface_format()` (`:467`) but **no `queue()` accessor** — hoisting `headless`, which returns both, avoids adding one. Being `#[cfg(test)]`, it costs the shipped binary nothing. Doing it here rather than in a consumer is deliberate: both dependent features will want a device in tests, and this is the ordered-shared surface where an unplanned edit would otherwise collide with two dependents.
  Everything else in `lib.rs` is untouched. **No layer is registered and `tab_bar` keeps rendering the current sidebar**, so the terminal looks and behaves identically after this feature ships — the primitive and tokens are built and tested but never emitted. Adding two `FrameCtx` fields is source-compatible for existing layers (they are struct-literal constructed in one place inside `lib.rs`). Rejected: leaving the plumbing to the consumers (guaranteed collision on a shared surface, and it would silently make D1 inert); rejected: registering a layer that draws the new panel now (it would double-draw over `tab_bar`'s strip and make the terminal visibly worse for two features' duration); rejected: deleting `tab_bar.rs` here (it is `sidebar-row-redesign`'s surface, and removing it leaves no sidebar at all mid-build).

### D11: `sidebar_shapes.rs` REUSES the crate-root render helpers; the WGSL is a separate file via `include_str!`
- **Status**: resolved
- **Rationale**: Two earlier drafts got this wrong in opposite directions — first assuming `tab_bar.rs`'s helpers could be reused, then concluding they must be duplicated. Both missed the actual fact: **the same helpers exist at the crate root in `lib.rs`, which IS this feature's declared surface** — `Vertex` (`:58`), `QUAD` (`:101`), `upload_resident` (`:748`), `uniform_entry` (`:935`), `build_pipeline` (`:949`). They are module-private, but **Rust makes a crate-root private item visible in every descendant module**, so `sidebar_shapes.rs` calls them as `crate::build_pipeline(...)`, `crate::QUAD`, `crate::upload_resident(...)` with **zero visibility changes and zero out-of-surface edits**. `build_pipeline` even already hardcodes vertex buffer 0 as the `Float32x2` corner — exactly the layout D3 specifies — and already carries `#[allow(clippy::too_many_arguments)]`.
  **One genuine exception:** `crate::uniform_entry` (`lib.rs:935`) declares `ShaderStages::VERTEX` only, while D5's SDF needs the viewport uniform in the **fragment** stage. That one `BindGroupLayoutEntry` is therefore written locally with `VERTEX_FRAGMENT`; the other four items reuse cleanly. (`tab_bar.rs:512` has its own `VERTEX_FRAGMENT` copy for the same reason — evidence this is the normal divergence, not a special case.)
  The WGSL lives in `crates/renderer/src/sidebar_shapes.wgsl` (it is in the declared surface) pulled in with `include_str!` — matching `cells.wgsl` / `glyph.wgsl` rather than `tab_bar.rs`'s inline `const &str`, because this shader is substantially larger (three SDFs plus gradient logic) and benefits from syntax highlighting and stable line numbers in naga's error messages. Rejected: duplicating the scaffolding (~80 lines of copy-paste creating a maintenance debt this feature would be *choosing*, not inheriting — and the reason given for it, that the helpers are unreachable, is simply false); rejected: making `tab_bar`'s copies `pub(crate)` (edits a file outside the declared surface, to export code `sidebar-row-redesign` deletes); rejected: an inline WGSL const (a ~150-line string literal with meaningless naga offsets).

### D12: `config.example.toml` documents the new section to the same standard as the existing ones
- **Status**: resolved
- **Rationale**: The example file is in this feature's declared surface and is the only user-facing documentation of the config format; a new section that ships undocumented reproduces exactly the doc/behaviour drift already visible at `config.example.toml:13`. It gains a `[sidebar]` block stating each field's default, the unit and meaning of `width_px` (logical px, scaled by the display's backing factor — the same wording the `[font]` comment uses), the 260–440 clamp band with a note that out-of-range values are clamped and warned rather than rejected, and — because honesty in the example file is cheaper than a bug report — an explicit note that **`accent` is reserved and currently affects no sidebar element** (D1). The pre-existing `ligatures` mismatch is **not** silently fixed here — it is a separate question about which side is wrong, recorded in Known Gaps. Rejected: leaving the section undocumented (a `deny_unknown_fields` config the user cannot discover); rejected: opportunistically fixing the `ligatures` line (an unrelated behaviour question smuggled into a sidebar feature).

### D13: This feature defines the `AgentSkin` vocabulary; `sidebar-row-redesign` owns the mapping to it
- **Status**: resolved
- **Rationale**: D6 needs a skin enum, but the seam has a direction problem: per the manifest `tab-metadata` is a **root** (`depends_on: []`) owning `crates/app/src/tabmeta/**`, so it may be built before this feature exists and it cannot reference `renderer::sidebar_theme::AgentSkin` — nor should an app-side metadata provider know about renderer types (SD3). So the contract is explicit: **`tab-metadata` emits its own app-side agent identity** (its own enum/id, its own vocabulary, decided in its own spec); **`sidebar-row-redesign` — the only feature that `depends_on` both — owns the mapping** from that identity to `AgentSkin`. This feature's sole obligation is that `AgentSkin` is exhaustive and total: `Claude`, `Antigravity`, and `Generic` as the fallback for any identity the mapping does not recognise (D6 defines `Generic`'s four states as the plain-card treatment). That way neither root feature depends on the other's vocabulary, and an unrecognised agent degrades to a normal-looking tab instead of failing to render. Rejected: putting the skin enum in `common` so both could share it (a colour concept in the UI/GPU-free config crate); rejected: `tab-metadata` emitting a renderer enum (inverts the dependency direction and breaks SD3); rejected: a stringly-typed seam (loses exhaustiveness at the mapping site, which is the one place a new agent must be handled).

## Constraints
- No new crate dependency of any kind. `wgpu`, `bytemuck`, `serde`, `toml`, `tracing` and `pollster` (a *regular* dependency of `renderer`, `Cargo.toml:12`) are already in the workspace and are sufficient; `Cargo.lock` edits are blocked by a PreToolUse hook.
- No GUI/widget/vector-graphics crate (`.claude/rules/no-gui-crates.md`). The render stack stays `wgpu` + `swash` + `bytemuck` + `raw-window-handle`.
- `renderer` must not import `winit`, and nothing here may touch `term-core` (`.claude/rules/dependency-direction.md`).
- No `#[cfg(target_os = ...)]` branches (`.claude/rules/macos-only.md`).
- No `unwrap()` outside `#[cfg(test)]` (CLAUDE.md).
- Existing `~/.config/mac-terminal/config.toml` files carrying only `[font]`/`[theme]`/`[grid]` must keep loading unchanged — the new section is additive and defaulted.
- The terminal's on-screen appearance and behaviour must be **unchanged** before and after this feature (D10): no layer registered, no `left_inset` change, no `tab_bar` change.
- GPU-touching tests use the guarded `headless()` helper — **hoisted to the crate root by D10 so sibling modules can reach it** — and **skip cleanly when no adapter is available**; they must never be required for a green run on adapterless CI. All non-GPU logic stays testable with no device at all.
- All three gates green: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

## Known Gaps
- **Icon fidelity is unverifiable here.** D7's tables are checked for being non-empty and in-bounds, not for looking like the mock. First visual confirmation is `sidebar-chrome-collapse` via `/run-term`.
- **The Rust↔WGSL SDF transcription is cross-checked only where an adapter exists.** D5's guarded GPU test executes the shipped shader, but skips when `headless()` returns `None`; on an adapterless machine only the Rust specification is verified. Residual drift would surface at `sidebar-row-redesign`.
- **D5's readback tier has no precedent in this workspace.** `copy_texture_to_buffer` / `map_async` are used nowhere today, so the coverage-readback test is new machinery (row-padding alignment, device poll, wgpu 30 API) and is the likeliest thing in this feature to be adapter-flaky. If it proves unstable, the pipeline-build smoke tier alone still catches WGSL compile errors and should be kept.
- **`config.example.toml:13` documents `ligatures = true` while the code default is `false`** (`config.rs:60`, deliberate per the `ligatures_default_off` regression test). Pre-existing, inside this feature's declared surface, unrelated to the sidebar goal — flagged rather than silently fixed (D12) so the human decides which side is wrong.
- **The 260–440 clamp band is inherited from the mock's slider, not measured.** D2's bounds are reasoned (title+branch legibility below, grid starvation above) but not checked against a real font at a real window size. Revisit if the clamp warning fires in normal use.
- **`accent` ships as a config key that nothing on screen reads yet** (D1). It is defined because SD7 names it a user-facing setting and because `deny_unknown_fields` makes adding a key later a breaking change — but until the deferred terminal-pane chrome lands, setting it changes nothing. A user could reasonably file that as a bug. Worth a comment in `config.example.toml` (D12) saying so.
- **The claim that the mock tints only `agentNameStyle` / `promptStyle` / `footerModelStyle` with `accent` is not verifiable from this repo** — the mock is not checked in, and it was read through the `DesignSync` MCP during spec authoring. The claim is load-bearing for "no sidebar element consumes accent". Note that `mark_ready_fg` `#7796b8` is byte-identical to one of the mock's listed accent alternates; this spec treats that as coincidence rather than intent. A human with the mock open should confirm before `sidebar-row-redesign` hardcodes that colour.

## Requirements

### R0: The foundation exists, is tested, and changes nothing on screen

#### R0.1: The whole feature is visually inert
- **Given**: the terminal built at the commit before this feature, and at the commit after it
- **When**: both are launched with the same config and the same tabs opened
- **Then**: the sidebar, grid origin and every rendered pixel are the same — no layer was registered, `left_inset` is unchanged, and `tab_bar` still draws the sidebar

#### R0.2: All three gates are green
- **Given**: the completed feature
- **When**: `cargo build`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings` are run
- **Then**: all three exit 0, with no new warnings and no `unwrap()` outside `#[cfg(test)]`

#### R0.3: Non-GPU logic is testable with no adapter
- **Given**: a machine where `headless()` returns `None`
- **When**: `cargo test --workspace` is run
- **Then**: every geometry, SDF, icon, theme and config test still runs and passes; only the two GPU tiers skip, and the run is green

### R1: `[sidebar]` config section (D1, D12)

#### R1.1: An absent `[sidebar]` section yields the mock's defaults
- **Given**: a `config.toml` containing only `[font]`, `[theme]` and `[grid]`
- **When**: `Config::load_from` parses it
- **Then**: `cfg.sidebar` equals `SidebarConfig::default()` — `width_px = 340.0`, `show_git_branch = true`, `show_agent_status = true`, `collapsed = false`, `accent = 0x86B0A1` — and the file still loads without error

#### R1.2: A partial `[sidebar]` section fills the rest from defaults
- **Given**: a `config.toml` whose `[sidebar]` sets only `show_git_branch = false`
- **When**: `Config::load_from` parses it
- **Then**: `show_git_branch` is `false` and `width_px` is `340.0`, `show_agent_status` is `true`, `collapsed` is `false`, `accent` is `0x86B0A1`

#### R1.3: An unknown key in `[sidebar]` fails loudly
- **Given**: a `config.toml` whose `[sidebar]` contains `widht_px = 300` (a typo)
- **When**: `Config::load_from` parses it
- **Then**: it returns `Err(CommonError::ConfigParse { .. })` naming the file, rather than silently ignoring the key

#### R1.4: `accent` accepts a TOML hex literal
- **Given**: a `config.toml` whose `[sidebar]` sets `accent = 0xE0863F`
- **When**: `Config::load_from` parses it
- **Then**: `cfg.sidebar.accent == 0x00E0_863F`, with no string parsing involved

#### R1.5: `config.example.toml` documents the section, including that `accent` is inert
- **Given**: the shipped `config.example.toml`
- **When**: a user reads it
- **Then**: it contains a `[sidebar]` block giving every field's default, stating `width_px` is logical px scaled by the display backing factor, stating the 260–440 clamp band and that out-of-range values are clamped-and-warned rather than rejected, and stating explicitly that `accent` currently affects no sidebar element

#### R1.6: Copying the example file produces a config that loads
- **Given**: `config.example.toml` copied verbatim to the config path
- **When**: `Config::load_from` parses it
- **Then**: it returns `Ok`, proving no documented key is misspelled against `deny_unknown_fields`

### R2: `width_px` is clamped at load and warned once (D2)

#### R2.1: A too-small width is clamped up
- **Given**: a `config.toml` on disk whose `[sidebar]` sets `width_px = 10.0`
- **When**: `Config::load_from` reads that file
- **Then**: the returned `cfg.sidebar.width_px` is `260.0`, and exactly one `tracing::warn!` naming both the requested `10` and the effective `260` was emitted

#### R2.2: A too-large width is clamped down
- **Given**: a `config.toml` on disk whose `[sidebar]` sets `width_px = 100000.0`
- **When**: `Config::load_from` reads that file
- **Then**: the returned `cfg.sidebar.width_px` is `440.0` and the terminal is still able to start

#### R2.3: An in-band width passes through silently
- **Given**: a `config.toml` on disk whose `[sidebar]` sets `width_px = 300.0`
- **When**: `Config::load_from` reads that file
- **Then**: `width_px` is exactly `300.0` and no warning is emitted

#### R2.4: The clamp is not bypassable on any production path
- **Given**: the two production parse paths — startup (`main.rs:28` → `Config::load`) and live reload (`watch.rs:74` → `try_reload`)
- **When**: either loads a file with an out-of-band `width_px`
- **Then**: both observe the clamped value, because both funnel through `load_from`

#### R2.5: The warning does not repeat per frame
- **Given**: a clamped config and a running terminal drawing frames continuously
- **When**: 100 frames are rendered
- **Then**: no further clamp warning is emitted — the warning is bound to load/reload, not to reading the value

### R3: The shape primitive and its pipeline API (D3, D11)

#### R3.1: The three kinds cover the mock's surface inventory
- **Given**: the enumerated list of every surface the mock's sidebar draws (panel, panel right border, card, card border, collapsed restore button, header button backgrounds, and all six icons' sub-shapes)
- **When**: each is expressed using only `RoundedBox`, `Capsule` and `Ring`
- **Then**: every one is expressible, with no surface left requiring a fourth kind

#### R3.2: The instance layout matches the shader's expectations
- **Given**: the `SidebarShape` struct
- **When**: the `const _:` offset assertions and `size_of` checks are compiled
- **Then**: they pass, `kind`/`_pad` occupy a `Uint32x2` attribute at location 3, and the struct is `Pod` + `Zeroable` with `#[repr(C)]`

#### R3.3: The pipeline is built from the crate-root helpers, not a private copy
- **Given**: `sidebar_shapes.rs`
- **When**: its pipeline construction is read
- **Then**: it calls `crate::build_pipeline`, `crate::QUAD`, `crate::Vertex` and `crate::upload_resident`, and the only locally-defined descriptor is the `VERTEX_FRAGMENT` uniform entry that `crate::uniform_entry`'s `VERTEX`-only visibility cannot supply

#### R3.4: A layer constructs the pipeline lazily on first record
- **Given**: a `DrawLayer` holding `Option<SidebarShapePipeline>` set to `None`, built with no device
- **When**: `SidebarShapePipeline::ensure(&mut slot, ctx)` is called during `record`
- **Then**: the pipeline is created and returned, and a second call with an unchanged `ctx.format` reuses it without rebuilding

#### R3.5: A target-format change rebuilds the pipeline
- **Given**: a `SidebarShapePipeline` built for format A
- **When**: `ensure` is called with a `ctx.format` of B
- **Then**: the pipeline is rebuilt for B rather than reused

#### R3.6: Instances go into a resident buffer that grows but is not reallocated per frame
- **Given**: a pipeline that drew 10 instances last frame
- **When**: it draws 10 instances again this frame
- **Then**: `upload_resident` reuses the existing buffer via `queue.write_buffer` with no new allocation; drawing 500 instances grows it once, and dropping back to 10 does not shrink or reallocate it

#### R3.7: An empty instance list draws nothing and allocates nothing
- **Given**: a layer whose shapes were all dropped by D9's constructors, or a sidebar with `collapsed = true`
- **When**: `draw(ctx, &[])` is called
- **Then**: it returns before recording a pass, no zero-size buffer is created, and no wgpu validation error is raised

### R4: Up-to-3-stop per-instance gradients (D4)

#### R4.1: Three distinct stops reproduce the antigravity skin
- **Given**: `fill0`/`fill1`/`fill2` set to the mock's three antigravity colours with `mid = 0.55`
- **When**: the fill is evaluated along the gradient direction at t = 0.0, 0.55 and 1.0
- **Then**: it equals `fill0`, `fill1` and `fill2` respectively, interpolating linearly between them

#### R4.2: Equal second and third stops degenerate to a 2-stop gradient
- **Given**: `fill2 == fill1` and `mid = 0.5`
- **When**: the fill is evaluated across the full span
- **Then**: the result is a plain two-stop ramp from `fill0` to `fill1`, with no visible seam at the mid point

#### R4.3: Three equal stops degenerate to a flat fill
- **Given**: `fill0 == fill1 == fill2`
- **When**: the fill is evaluated anywhere in the box
- **Then**: every sample is that single colour, so a caller needing a flat fill uses the same code path with no branch

### R5: Antialiased SDF edges, specified in Rust and verified against the shipped shader (D5)

#### R5.1: Each SDF returns the correct sign and magnitude
- **Given**: a table of points for each of `rounded_box_sdf`, `capsule_sdf` and `ring_sdf` — well inside, well outside, exactly on the edge, and in a corner arc
- **When**: each function is evaluated
- **Then**: inside points are negative, outside positive, on-edge points within an epsilon of zero, and corner-arc distances match the analytic circle distance

#### R5.2: The WGSL compiles
- **Given**: a machine where `headless()` yields a device
- **When**: the pipeline-build smoke test constructs `SidebarShapePipeline::new(device, format)`
- **Then**: naga accepts `sidebar_shapes.wgsl` and the pipeline is created, so any syntax or type error fails here rather than in a later feature

#### R5.3: The shipped shader agrees with the Rust specification
- **Given**: a machine where `headless()` yields a device
- **When**: one instance of each kind is rendered offscreen and coverage is read back at sample points that include at least one just inside a large-radius corner arc
- **Then**: each sampled coverage is within ±0.15 of the Rust twin evaluated at the same point — so a transcription error such as a halved radius or a dropped border term fails the test

#### R5.4: Both GPU tiers skip cleanly without an adapter
- **Given**: a machine where `headless()` returns `None`
- **When**: the test suite runs
- **Then**: R5.2 and R5.3 skip without failing, and R5.1 still runs

#### R5.5: `headless()` is reachable from a sibling module
- **Given**: the test module inside `sidebar_shapes.rs`
- **When**: it calls `crate::headless()`
- **Then**: it compiles — because D10 hoisted the helper out of `lib.rs`'s private `mod tests` to a `#[cfg(test)] pub(crate) fn` at the crate root

### R6: Design tokens and the agent-skin vocabulary (D6, D13)

#### R6.1: Every mock colour token has exactly one home
- **Given**: the mock's colour inventory in Research
- **When**: `SidebarTheme::DEFAULT` is compared against it
- **Then**: every listed colour is present with the mock's exact value, and no sidebar colour literal appears anywhere else in the feature

#### R6.2: `from_config` overlays the config-derived colour
- **Given**: a `SidebarConfig` with `accent = 0xE0863F`
- **When**: `SidebarTheme::from_config(&cfg)` is called
- **Then**: the result equals `DEFAULT` except that `accent` is `0xE0863F`, and the layout toggles are absent from the theme entirely

#### R6.3: Packed colours convert to linear RGBA
- **Given**: `linear(0x86B0A1, 1.0)`
- **When**: it is evaluated
- **Then**: it returns the sRGB→linear conversion of those three channels with alpha 1.0, matching the semantics of the `tab_bar.rs:578` function it replaces

#### R6.4: Metrics carry the mock's values in logical px
- **Given**: `SidebarMetrics::DEFAULT`
- **When**: its fields are read
- **Then**: `card_radius` is 13.0, `card_pad_x` 15.0, `card_pad_y` 12.0, `card_margin_x` 8.0, `card_margin_y` 3.0, `card_gap` 6.0 and `border_w` 1.0 — the mock's numbers, unscaled

#### R6.5: `scaled` multiplies every metric exactly once
- **Given**: `SidebarMetrics::DEFAULT` and a scale factor of 2.0
- **When**: `scaled(2.0)` is called
- **Then**: every field is exactly doubled, with no field left unscaled — asserted field-by-field, so adding a metric without scaling it fails the test

#### R6.6: `AgentSkin` is total, and unknown agents degrade to a plain card
- **Given**: the `AgentSkin` enum
- **When**: `Claude`, `Antigravity` and `Generic` are each resolved to their four states
- **Then**: `Claude` and `Antigravity` return the mock's skin gradients and borders, and `Generic` returns the plain-card treatment (`transparent` / `#2a2722` / `#302c26` / `#46413a`) so an unrecognised agent renders as a normal tab rather than failing

#### R6.7: The skin vocabulary is renderer-side only
- **Given**: the `tab-metadata` feature's declared surface
- **When**: it is searched for a reference to `AgentSkin`
- **Then**: there is none — `tab-metadata` emits its own app-side agent identity, and only `sidebar-row-redesign` maps it to a skin

### R7: The icon set (D7, D8)

#### R7.1: Six icons exist and none is empty
- **Given**: the `Icon` enum
- **When**: each of `Filter`, `Plus`, `Panel`, `GitBranch`, `Asterisk` and `Diamond` is expanded
- **Then**: each produces at least one shape, and `Search` is absent because its only consumer is deferred

#### R7.2: Every icon stays inside its 24×24 box
- **Given**: each icon's const table
- **When**: every entry's extents (including stroke width) are computed
- **Then**: all lie within `[0, 24] × [0, 24]`, so no icon bleeds outside its allotted square at any scale

#### R7.3: Emitting an icon appends to a caller-owned buffer
- **Given**: a `Vec<SidebarShape>` reused across frames
- **When**: `push_icon(&mut out, Icon::GitBranch, x, y, size, color)` is called
- **Then**: the shapes are appended to the existing allocation, positioned and scaled to `(x, y, size)`, with no `Vec` returned and no per-call allocation once the buffer has grown

#### R7.4: The status marks render without font support
- **Given**: a font that contains neither U+2733 nor U+25C7
- **When**: `Icon::Asterisk` and `Icon::Diamond` are emitted
- **Then**: both produce their full shape sets, because neither consults the glyph atlas

### R8: Degenerate geometry is normalised before it reaches the GPU (D9)

#### R8.1: An oversized corner radius is clamped
- **Given**: a request for a 20×10 rounded box with `radius = 50.0`
- **When**: `SidebarShape::rounded_box` builds it
- **Then**: the stored radius is `5.0` (`min(w,h)/2`), so the SDF cannot invert

#### R8.2: An oversized border is clamped
- **Given**: a 20×10 box with `border_w = 40.0`
- **When**: the constructor builds it
- **Then**: the stored `border_w` is `5.0`

#### R8.3: A zero, negative or non-finite extent is dropped
- **Given**: requests with `w = 0.0`, `h = -5.0`, and `w = f32::NAN`
- **When**: each is passed to a constructor
- **Then**: each returns `None` and never reaches the instance buffer

#### R8.4: A degenerate gradient direction is replaced, not normalised
- **Given**: a shape whose gradient direction is `[0.0, 0.0]` or contains a NaN
- **When**: the constructor builds it
- **Then**: the direction is replaced with the mock's default 135° unit vector, so no divide-by-zero produces a NaN that propagates across the frame

#### R8.5: A degenerate mid-stop is clamped into range
- **Given**: shapes with `mid` of `0.0`, `1.0` and `-3.0`
- **When**: the constructors build them
- **Then**: each stored `mid` lies in `[0.05, 0.95]`, so neither interpolation segment has zero width

#### R8.6: The constructors are the only way to build an instance
- **Given**: `SidebarShape`
- **When**: a caller outside `sidebar_shapes.rs` attempts struct-literal construction
- **Then**: it does not compile, because at least one field is private — so no caller can bypass R8.1–R8.5

### R9: Renderer plumbing, minimal and additive (D10)

#### R9.1: A draw layer can read the sidebar config
- **Given**: a `Config` whose `[sidebar]` sets `width_px = 400.0`
- **When**: a `DrawLayer`'s `record` reads `ctx.sidebar.width_px`
- **Then**: it observes `400.0` — because `Renderer` now stores `SidebarConfig` and `FrameCtx` carries a reference to it

#### R9.2: A draw layer can convert logical px to physical
- **Given**: a window at scale factor 2.0
- **When**: a layer reads `ctx.scale_factor`
- **Then**: it observes `2.0`, so `SidebarMetrics::scaled(ctx.scale_factor)` yields physical px

#### R9.3: A live config reload updates the stored sidebar config
- **Given**: a running terminal and an edit to `[sidebar]` in the config file
- **When**: the watcher fires and `settings::apply` calls `renderer.apply_config(&cfg)`
- **Then**: the `Renderer`'s stored `SidebarConfig` reflects the new values on the next frame

#### R9.4: Existing layers are unaffected by the new `FrameCtx` fields
- **Given**: `CursorLayer`, `ImageLayer`, `OverlayLayer` and `TabBar`
- **When**: the two fields are added to `FrameCtx`
- **Then**: all four compile unchanged, because `FrameCtx` is struct-literal constructed only inside `lib.rs` and external layers only read fields

#### R9.5: `lib.rs` gains nothing beyond the enumerated changes
- **Given**: the diff to `crates/renderer/src/lib.rs`
- **When**: it is reviewed
- **Then**: it contains only the three `pub mod` lines, the `Renderer.sidebar` field and its `apply_config` refresh, `scale_factor()`, the two `FrameCtx` fields, and the `headless()` hoist — keeping the ordered-shared surface small for the two dependent features

## Tasks

### T1: `[sidebar]` config section, load-time clamp, and example documentation [infra]
- **Fulfills**: R1, R2
- **Depends on**: (none)

Adds `SidebarConfig` to `crates/common/src/config.rs` with the sibling `#[serde(default, deny_unknown_fields)]` shape and a hand-written `impl Default`; wires the clamp and its single `tracing::warn!` into `Config::load_from`; documents the section in `config.example.toml`. Tests must write a temp file and go through `load_from` — the existing `toml::from_str` precedents bypass the clamp — using the hand-rolled temp-dir helper idiom (`common/src/watch.rs:94-100`), since no `tempfile` dev-dependency exists and `Cargo.lock` is hook-blocked.

### T2: Renderer plumbing, module stubs, and the `headless()` hoist [infra]
- **Fulfills**: R9
- **Depends on**: T1

Adds the three `pub mod` lines with minimal compiling stubs behind them, the `Renderer.sidebar` field plus its one-line `apply_config` refresh, `pub fn scale_factor()`, the two `FrameCtx` fields, and hoists `headless()` out of `mod tests` to a `#[cfg(test)] pub(crate) fn` at the crate root. Depends on T1 only because `SidebarConfig` must exist to be stored. Keep the diff to exactly R9.5's list.

### T3: `SidebarTheme`, `SidebarMetrics`, `AgentSkin`, and `linear()` [infra]
- **Fulfills**: R6
- **Depends on**: T2

Fills `sidebar_theme.rs`. Note R6.5 asserts field-by-field so that a metric added later without scaling fails the test, and R6.1 requires no sidebar colour literal to survive anywhere else in the feature.

### T4: The shape primitive — instance layout, WGSL, pipeline, gradients, SDFs, degenerate handling [infra]
- **Fulfills**: R3, R4, R5, R8
- **Depends on**: T2  ← parallel with T3

Fills `sidebar_shapes.rs` + `sidebar_shapes.wgsl`. The largest task and the one carrying the feature's real risk: the Rust SDFs are the specification, the WGSL transcribes them, and R5.3's readback is what proves the two agree. Build on `crate::build_pipeline` / `crate::QUAD` / `crate::upload_resident`; the only local descriptor is the `VERTEX_FRAGMENT` uniform entry. Consider landing R5.2 (compile smoke) before R5.3 (readback), since the readback machinery has no precedent in this workspace and R5.2 alone already catches WGSL errors.

### T5: The six icon tables [infra]
- **Fulfills**: R7, R0
- **Depends on**: T4

Fills `sidebar_icons.rs`. Depends on T4 because every icon is expressed in `SidebarShape`s built by T4's constructors.

**On R0's placement**: R0 is a whole-feature property, not a unit of work. R0.2 (the three gates) is run by *every* task per CLAUDE.md, and R0.1 (visual inertness) is most at risk in **T2**, the only task touching shared render state — but neither is finally provable until all five tasks have landed, so R0 is attached to T5 as the last task rather than duplicated across all five. It is not a disguised final-verify task: holistic verification belongs to the execute pipeline.

**DAG**: T1 → T2 → {T3 ∥ T4} → T5. No cycles. T3 and T4 are genuinely parallel — disjoint files, no shared interface, and both depend only on T2's stubs and plumbing.

**Horizontal-split justification**: every task here is BE-only in vertical-slice terms, which the L4 rule permits for *pure infrastructure* — this feature deliberately has no user-visible surface (D10, R0.1). The vertical slices arrive in `sidebar-row-redesign` and `sidebar-chrome-collapse`, which consume all five of these tasks' outputs.

## External Dependencies

### Pre-work
- A human should confirm, against the Claude Design mock, that no *sidebar* element is tinted with `accent` (Known Gaps). The mock is not checked into this repo, so the spec's claim rests on the import performed during authoring. This blocks nothing in T1–T5 but should settle before `sidebar-row-redesign` hardcodes `mark_ready_fg`.

### Post-work
- None. The feature is visually inert by design; the first `/run-term` eyeball happens in `sidebar-row-redesign`.
