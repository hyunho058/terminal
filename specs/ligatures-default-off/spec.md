# Spec: ligatures-default-off

## Meta
- **Created**: 2026-07-19
- **Type**: dev
- **Status**: drafting
- **Set**: vt-underline-and-ligature-fixes (via decompose, `mode: batch`)

## Declared Surface
- feature-id: ligatures-default-off
- depends_on: []
- declared-surface:
  - crates/common/src/config.rs

## Goal
Stop the "ghost space" that appears inside words (e.g. `first` → `fi rst`) with the
default configuration. The default font is Menlo (`config.rs:54`), which ships
*typographic* f-ligatures (`fi`, `fl`, `ffi`, …). With `font.ligatures` defaulting to
`true` (`config.rs:56`), swash shaping in the renderer collapses `f`+`i` into a single
glyph that the renderer draws only at the cluster's lead cell
(`renderer/src/lib.rs:850`), leaving the second monospace cell blank — the ghost
space. Flip the default `FontConfig::default().ligatures` from `true` to `false` so
the default, out-of-the-box terminal renders words correctly on its monospace grid.
Add a regression test asserting the default is `false`.

## Non-goals
- Fixing the renderer's lead-cell-only ligature draw (`renderer/src/lib.rs:850`). That
  is a real renderer limitation, but it lives in a different crate and is out of this
  feature's declared surface. This feature only stops it from being triggered *by
  default*; a monospace-safe multi-cell-advance fix is a separate renderer feature.
- Removing programming-ligature support. Users can still opt in with
  `font.ligatures = true` in `config.toml` — the *default* changes, the capability does
  not. `!=` / `=>` ligatures remain available for anyone who wants them (ideally with a
  ligature-safe monospace font).
- Changing the default font family away from Menlo — a larger, more opinionated change
  with its own risks; out of scope.

## Confirmed Goal
`crates/common/src/config.rs::FontConfig::default()` returns `ligatures: false` instead
of `ligatures: true`, so `Config::default()` (and any `config.toml` that omits the
`font.ligatures` key) yields ligatures-off. An explicit `font.ligatures = true` in a
user's config still enables ligatures (serde `#[serde(default)]` only supplies the
default on absence). A regression test in config.rs's existing `#[cfg(test)] mod tests`
asserts `FontConfig::default().ligatures == false` and that an explicit `= true` TOML
still parses to `true`. No other crate changes.

## Research
- Change site: `FontConfig::default()` sets `ligatures: true`
  (`crates/common/src/config.rs:56`); the default family is `"Menlo"` (`config.rs:54`).
- The field is documented as "programming ligatures (e.g. `!=`, `=>`)"
  (`crates/common/src/config.rs:31-32`) and is `#[serde(default)]` on `FontConfig`
  (`config.rs:26`), so an omitted key falls back to `default()`.
- The renderer already consumes the flag and is correct in isolation: it reads
  `cfg.font.ligatures` (`renderer/src/lib.rs:428,498`), threads it into
  `build_glyph_instances` where `let liga: u16 = if ligatures { 1 } else { 0 }`
  (`renderer/src/lib.rs:804`) gates the swash ligature features. When on, a collapsed
  cluster's glyphs are positioned from the lead cell's pen (`renderer/src/lib.rs:850`),
  leaving trailing covered cells blank — the ghost space.
- Existing config tests do NOT assert a `true` ligatures default:
  `defaults_are_sane` (columns/rows/size only, `config.rs:116`) and
  `partial_toml_fills_defaults` (family/rows fill, `config.rs:122`). So flipping the
  default breaks neither.
- `crates/common/src/watch.rs:102-111` loads a TOML that sets `ligatures = false`
  *explicitly* and asserts `!cfg.font.ligatures` — an explicit-value test, unaffected
  by the default change.
- No insta/snapshot test depends on the ligatures default: term-core grid snapshots do
  no shaping, and the renderer's glyph-count test computes both on/off explicitly
  (`renderer/src/lib.rs:1098-1113`).
- Build/test/lint: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings` (CLAUDE.md). `common` is
  already UI/GPU-free (`config.rs:5`); this change keeps it so.

## Decisions

### SD1: Each fix stays inside its owning crate; the dependency direction (common ← term-core ← renderer ← app) and term-core's GPU-free invariant are preserved.
- **Status**: resolved (inherited — Shared Decision)
- **Rationale**: This feature is confined to `crates/common/src/config.rs`. The
  renderer's lead-cell ligature draw is the visible symptom, but the corrective change
  is a config default in `common`; the renderer is left byte-unchanged. Rejected:
  fixing the symptom in the renderer — it entangles two crates and adds shaping
  complexity while leaving the misleading default on.

### SD2: No new dependencies; reuse the existing vte / serde stack and mirror the code idiom already present at each change site.
- **Status**: resolved (inherited — Shared Decision)
- **Rationale**: This is a one-field flip in an existing `#[serde(default)]
  FontConfig::default()`; the regression test reuses config.rs's existing `mod tests`
  pattern. No dependency is added. Rejected: a font-shaping abstraction — over-engineering.

### SD3: Every fix ships with a regression test in the owning crate's existing test convention, and the set must pass `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
- **Status**: resolved (inherited — Shared Decision)
- **Rationale**: The regression test asserts `FontConfig::default().ligatures == false`
  in config.rs's `#[cfg(test)] mod tests`, next to `defaults_are_sane`. Rejected: an
  end-to-end render test of `first` — it would live in the renderer crate (outside this
  feature's surface) and depend on font availability.

### D1: Flip `FontConfig::default().ligatures` from `true` to `false`.
- **Status**: resolved
- **Rationale**: The default font, Menlo, carries typographic f-ligatures that, under
  ligature-on shaping, collapse `f`+`i` into one glyph the renderer draws only at the
  lead cell (`renderer/src/lib.rs:850`) — leaving the second monospace cell blank and
  producing `fi rst`. A monospace terminal's cell-grid integrity outranks the
  programming-ligature nicety, so the default must be off for a correct out-of-the-box
  render. The capability is retained as opt-in (`font.ligatures = true`). Rejected:
  advancing the cursor across a collapsed ligature's cells in the renderer — crosses the
  crate boundary (SD1), adds shaping complexity, and glyph-collapse is font-dependent
  and fragile; the bug report specifies the config-default fix. Rejected: changing the
  default font — bigger, more opinionated, its own risks, out of scope.

### D2: Add a regression test in config.rs's existing `#[cfg(test)] mod tests` locking the default to `false` and proving explicit opt-in still works.
- **Status**: resolved
- **Rationale**: `FontConfig::default().ligatures == false` locks the default against a
  silent future re-enable; a second assertion that a TOML with `ligatures = true` parses
  to `true` proves the flip did not break serde opt-in (the real regression risk of
  "just change the default"). Both sit next to the existing default tests. Rejected: a
  negative-only test (default is false) — it would pass even if serde opt-in were
  accidentally broken.

## Constraints
- An explicit `font.ligatures = true` under `[font]` in a user's `config.toml` must
  still yield `ligatures == true` — only the DEFAULT (absence) behavior changes. serde
  `#[serde(default)]` already guarantees this; the test asserts it.
- Change is confined to `crates/common/src/config.rs`; no new dependency; `common`
  stays UI/GPU-free (SD1, SD2).
- All existing tests stay green: `defaults_are_sane`, `partial_toml_fills_defaults`
  (config.rs), and the explicit-value default-fill test in `watch.rs` — none assert a
  `true` ligatures default, so the flip is non-breaking.
- Must pass `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` (SD3).

## Known Gaps
- **Renderer ghost-space remains for opt-in ligature users.** This fix stops the ghost
  space *by default* only; it does not repair the renderer's lead-cell-only draw of a
  collapsed multi-cell ligature (`renderer/src/lib.rs:850`). A user who re-enables
  `font.ligatures = true` with Menlo (or any font with wide typographic ligatures) will
  still see `fi rst`. A monospace-preserving renderer fix (split a ligature's advance
  across its covered cells, or reject ligatures that don't preserve cell advance) is a
  separate feature in the renderer crate, outside this feature's declared surface.

## Requirements

### R0: The default configuration disables ligatures so the default (Menlo) terminal renders words without ghost spaces.

#### R0.1: Default config is ligatures-off
- **Given**: No config file, or a `config.toml` that omits `font.ligatures`
- **When**: `Config::default()` is constructed (or such a TOML is parsed)
- **Then**: `config.font.ligatures == false`

### R1: `FontConfig` defaults ligatures to `false` (the fix).

#### R1.1: FontConfig default is false
- **Given**: The `common` crate
- **When**: `FontConfig::default()` is constructed
- **Then**: `.ligatures == false`

#### R1.2: Omitted key falls back to false via serde
- **Given**: A TOML string with a `[font]` table that has no `ligatures` key
- **When**: It is parsed with `toml::from_str::<Config>`
- **Then**: `config.font.ligatures == false`

### R2: Explicit opt-in is preserved (the default flip does not override a set value).

#### R2.1: Explicit `ligatures = true` still enables ligatures
- **Given**: A TOML with `ligatures = true` under `[font]`
- **When**: It is parsed with `toml::from_str::<Config>`
- **Then**: `config.font.ligatures == true`

### R3: No other default or existing test is disturbed by the flip.

#### R3.1: Other defaults unchanged
- **Given**: `Config::default()`
- **When**: It is constructed
- **Then**: `font.family == "Menlo"`, `font.size_pt > 0.0`, `grid.columns > 0`,
  `grid.rows > 0` — i.e. only `ligatures` changed, and `defaults_are_sane` /
  `partial_toml_fills_defaults` remain green

## Tasks

### T1: Flip the ligatures default to `false` [config]
- **Fulfills**: R0, R1
- **Depends on**: (none)
- In `FontConfig::default()` (`crates/common/src/config.rs:52-58`), change
  `ligatures: true` to `ligatures: false`. No other line changes.

### T2: Add regression tests to config.rs `mod tests` [test]
- **Fulfills**: R1, R2, R3
- **Depends on**: T1  (same file — serialized)
- In config.rs's existing `#[cfg(test)] mod tests`, add: assert
  `FontConfig::default().ligatures == false` and `Config::default().font.ligatures ==
  false` (R1.1/R0.1); assert a TOML omitting the key parses to `false` (R1.2); assert a
  TOML with `ligatures = true` parses to `true` (R2.1); confirm `defaults_are_sane`
  still passes (R3.1). Run `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings`.

## External Dependencies

### Pre-work
- (none)

### Post-work
- (none)
