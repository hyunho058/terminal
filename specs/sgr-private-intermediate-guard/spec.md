# Spec: sgr-private-intermediate-guard

## Meta
- **Created**: 2026-07-19
- **Type**: dev
- **Status**: drafting
- **Set**: vt-underline-and-ligature-fixes (via decompose, `mode: batch`)

## Declared Surface
- feature-id: sgr-private-intermediate-guard
- depends_on: []
- declared-surface:
  - crates/term-core/src/parser.rs

## Goal
Stop the whole terminal screen from showing a persistent underline. The CSI `m`
dispatch in `crates/term-core/src/parser.rs` misparses the XTMODKEYS private
sequence `ESC[>4;2m` as SGR `4` (underline) + `2` (dim): because the `'m'` arm
(parser.rs:322) unconditionally calls `handle_sgr`, the private-marker byte `>` is
ignored and the params `4;2` set UNDERLINE and DIM on the pen — which then never
clear, painting every subsequent cell underlined. Guard the `'m'` path so that when
the CSI carries a private-marker intermediate (`>`, `<`, or `=`) it is ignored
instead of treated as SGR, mirroring the existing `?` private-mode guard at
parser.rs:261. Add a regression test proving `ESC[>4;2m` sets no underline.

## Non-goals
- Implementing XTMODKEYS (`CSI > Pp ; Pv m`) semantics — the fix ignores the
  sequence, it does not act on it. Programs that send it simply get no modifyKeys
  state change (matching current behavior for every other unimplemented private CSI).
- Touching any other crate. The renderer's underline pass (`renderer/src/lib.rs`)
  is correct; the bug is purely in the parser's dispatch, so the renderer is left
  byte-unchanged.
- Reworking `handle_sgr` or the SGR code table — the SGR handler itself is correct;
  only the gate that decides *whether* a `CSI ... m` is SGR at all changes.

## Confirmed Goal
`crates/term-core/src/parser.rs::csi_dispatch` gains an early guard — the same shape
as the existing `?` private-mode guard — that ignores any CSI whose first
intermediate is a private marker `>`/`<`/`=`, so `ESC[>4;2m` (and `ESC[<...m` /
`ESC[=...m`) no longer reach `handle_sgr` and no longer set UNDERLINE/DIM. Normal
SGR (`ESC[4m`, `ESC[1;4;7m`) and the existing `?`-marker `h`/`l` alt-screen path are
unaffected. A regression test in parser.rs's `#[cfg(test)] mod tests` proves both the
fix (`ESC[>4;2m` → no UNDERLINE) and the positive control (`ESC[4m` → UNDERLINE).

## Research
- The CSI `'m'` arm calls `handle_sgr(params)` unconditionally — no
  intermediate/private-marker check (`crates/term-core/src/parser.rs:322`).
- An early private-mode guard already exists and is the pattern to mirror:
  `if intermediates.first() == Some(&b'?') && matches!(action, 'h' | 'l') { … return; }`
  — it sits before `match action` and handles alt-screen enter/leave
  (`crates/term-core/src/parser.rs:261-266`). This proves this `vte` version delivers
  the private-marker byte (`?`, and by extension `>`/`<`/`=`) through the
  `intermediates` slice.
- `handle_sgr` maps `4 → CellAttrs::UNDERLINE` and `2 → CellAttrs::DIM`, and these
  persist on the pen until `24`/`22` clear them
  (`crates/term-core/src/parser.rs:163,162,170,168`).
- The test module already has the ideal helper: `sgr_cell(seq)` feeds `seq` then
  prints `X` and returns `cell(0,0)` — exactly the shape a
  "does this attr get set?" assertion needs (`crates/term-core/src/parser.rs:486`).
  Existing SGR attr tests use it (`sgr_attributes_set_and_clear`, parser.rs:533).
- Build/test/lint: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings` (CLAUDE.md Conventions).
- Rules honored: `term-core` stays GPU-free / headless-testable
  (`.claude/rules/dependency-direction.md`); no new deps; macOS-only.

## Decisions

### SD1: Each fix stays inside its owning crate; the dependency direction (common ← term-core ← renderer ← app) and term-core's GPU-free invariant are preserved.
- **Status**: resolved (inherited — Shared Decision)
- **Rationale**: The two bugs in this set live in different crates and are fixed
  independently. This feature is confined to `crates/term-core/src/parser.rs`; it
  adds no wgpu/winit reference and reaches into no sibling crate. Rejected: fixing
  the underline symptom in the renderer — the parser is where the misparse happens,
  and the renderer's underline pass is correct.

### SD2: No new dependencies; reuse the existing vte / serde stack and mirror the code idiom already present at each change site.
- **Status**: resolved (inherited — Shared Decision)
- **Rationale**: The guard mirrors the existing private-mode guard at parser.rs:261
  (`intermediates.first() == Some(&b'?')`), extended to the SGR-relevant private
  markers `>`/`<`/`=`. No parsing crate is added. Rejected: a richer VT library —
  over-engineering for a one-arm guard.

### SD3: Every fix ships with a regression test in the owning crate's existing test convention, and the set must pass `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
- **Status**: resolved (inherited — Shared Decision)
- **Rationale**: The regression test goes in parser.rs's existing `#[cfg(test)] mod
  tests` and uses the existing `sgr_cell` helper. Rejected: an insta snapshot — this
  is a boolean attr assertion, not a grid-shape change.

### D1: Guard the SGR path against private-marker intermediates `>`/`<`/`=`; ignore such a CSI instead of treating it as SGR.
- **Status**: resolved
- **Rationale**: XTMODKEYS (`CSI > Pp ; Pv m`) and the reset/query forms
  (`CSI < … m`, `CSI = … m`) share the final byte `m` with SGR. Without a guard,
  `vte` hands the numeric params (`4;2`) to `handle_sgr`, which sets UNDERLINE (4) and
  DIM (2) and never clears them — every later cell is drawn underlined/dim. Ignoring
  the sequence matches how the terminal already treats other unimplemented private
  CSIs (they fall through to `_ => {}`). Rejected: guarding only `>` — `<` and `=`
  are equally private markers that must never be interpreted as SGR; rejected:
  implementing XTMODKEYS — out of scope (non-goal), and unnecessary to stop the bug.

### D2: Place the guard as an early check before `match action`, mirroring the existing `?` guard, keyed on `intermediates.first()` being `>`/`<`/`=`.
- **Status**: resolved
- **Rationale**: The existing `?` guard already sits before the `match action` and
  returns; putting the new guard in the same place makes a private-marker CSI a
  silent no-op for `m` (and defensively for any other final byte carrying `>`/`<`/`=`
  that we don't implement), exactly as xterm treats unknown private sequences. This is
  the "same pattern" the bug report calls for. Rejected: adding the check inside the
  `'m'` arm only — it fixes this bug but is less consistent than a single early guard
  and would still let other private-marker finals reach `_ => {}` unguarded. The
  behavior for `ESC[>4;2m` is identical either way; the early-guard placement is the
  cleaner mirror of the established idiom.

### D3: The regression test lives in parser.rs's `#[cfg(test)] mod tests`, uses `sgr_cell`, and includes a positive control.
- **Status**: resolved
- **Rationale**: `sgr_cell(b"\x1b[>4;2mX")` returns `cell(0,0)`; asserting its attrs
  contain neither UNDERLINE nor DIM proves the fix. A positive control
  (`sgr_cell(b"\x1b[4mX")` DOES contain UNDERLINE) proves the guard does not
  over-suppress legitimate SGR — the real failure mode of a too-broad guard.
  Rejected: testing only the negative case — a guard that swallows *all* `m` would
  pass a negative-only test while breaking every color/attr in the terminal.

## Constraints
- The existing `?`-marker `h`/`l` guard (alt-screen enter/leave, parser.rs:261-266)
  must remain byte-for-behavior unchanged — only `>`/`<`/`=` markers are newly ignored.
- Normal SGR — no intermediate — must be completely unaffected: `ESC[4m` still sets
  UNDERLINE, `ESC[1;4;7m` still sets bold+underline+reverse, colors still apply.
- Change is confined to `crates/term-core/src/parser.rs`; no new dependency;
  `term-core` remains GPU-free (SD1, SD2).
- Must pass `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` (SD3).

## Known Gaps
- **vte delivery channel of `>`/`<`/`=` — CONFIRMED, not a gap for the current pin.**
  Verified against `vte` 0.15.0 source (L2-reviewer): `advance_csi_entry` collects
  bytes `0x3C..=0x3F` (`< = > ?`) into the `intermediates` buffer via `action_collect`,
  so `ESC[>4;2m` dispatches as `csi_dispatch(params=[[4],[2]], intermediates=[b'>'],
  'm')` — `intermediates.first() == Some(&b'>')` holds. The only residual gap is
  forward-looking: a future `vte` major bump *could* change this channel, in which case
  the guard's *read* must adapt — but the behavioral invariant (R0.1/R1.1: `ESC[>4;2m`
  sets no UNDERLINE) is what the regression test locks, independent of the field read.

## Requirements

### R0: `ESC[>4;2m` no longer sets a persistent underline; the CSI `m` handler ignores `>`/`<`/`=` private-marker sequences while normal SGR is unchanged.

#### R0.1: XTMODKEYS does not set underline
- **Given**: A fresh `Grid` and `Processor`
- **When**: `\x1b[>4;2m` is advanced, then a character is printed
- **Then**: That cell's `attrs` contain neither `CellAttrs::UNDERLINE` nor
  `CellAttrs::DIM`

### R1: Private-marker CSI `m` sequences are ignored (the fix).

#### R1.1: `>` marker — XTMODKEYS ignored
- **Given**: The `sgr_cell` test helper (advances a sequence, prints `X`, returns cell 0,0)
- **When**: `sgr_cell(b"\x1b[>4;2mX")` is evaluated
- **Then**: The returned cell's `attrs` does NOT contain `CellAttrs::UNDERLINE`
  (and does NOT contain `CellAttrs::DIM`)

#### R1.2: `=` and `<` markers — also ignored (boundary)
- **Given**: The `sgr_cell` helper
- **When**: `sgr_cell(b"\x1b[=4mX")` and `sgr_cell(b"\x1b[<4mX")` are evaluated
- **Then**: Neither returned cell's `attrs` contains `CellAttrs::UNDERLINE` — the
  guard covers all three private markers `>`/`<`/`=`, not just `>`

### R2: Normal SGR and the existing `?` guard are unaffected (positive controls / no-regression).

#### R2.1: Plain SGR underline still sets underline
- **Given**: The `sgr_cell` helper
- **When**: `sgr_cell(b"\x1b[4mX")` is evaluated (SGR `4`, no intermediate)
- **Then**: The returned cell's `attrs` DOES contain `CellAttrs::UNDERLINE`

#### R2.2: Multi-attribute SGR still applies every attribute
- **Given**: The `sgr_cell` helper
- **When**: `sgr_cell(b"\x1b[1;4;7mX")` is evaluated
- **Then**: The returned cell's `attrs` contains `BOLD`, `UNDERLINE`, and `REVERSE`
  (existing `sgr_attributes_set_and_clear` behavior remains green)

#### R2.3: Existing `?`-marker alt-screen path unchanged
- **Given**: A `Grid` and `Processor`
- **When**: `\x1b[?1049h` then `\x1b[?1049l` are advanced
- **Then**: The grid enters and leaves the alt screen exactly as before — the new
  guard does not intercept the `?`-marker `h`/`l` path (parser.rs:261-266)

## Tasks

### T1: Add the private-marker guard to `csi_dispatch` [core]
- **Fulfills**: R0, R1, R2.3
- **Depends on**: (none)
- Add an early guard in `csi_dispatch` (before `match action`), mirroring the `?`
  guard at parser.rs:261: when `intermediates.first()` is `Some(&b'>')`,
  `Some(&b'<')`, or `Some(&b'=')`, `return` without dispatching (the sequence is
  ignored). Leave the existing `?` `h`/`l` guard and all `match action` arms intact.

### T2: Add regression tests to parser.rs `mod tests` [test]
- **Fulfills**: R1, R2
- **Depends on**: T1  (same file — serialized)
- Add a test using `sgr_cell`: assert `\x1b[>4;2mX` yields a cell with no UNDERLINE
  and no DIM (R1.1); assert `\x1b[=4mX` / `\x1b[<4mX` yield no UNDERLINE (R1.2);
  assert `\x1b[4mX` DOES set UNDERLINE (R2.1, positive control); assert
  `\x1b[1;4;7mX` sets BOLD+UNDERLINE+REVERSE (R2.2). Run `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings`.

## External Dependencies

### Pre-work
- (none)

### Post-work
- (none)
