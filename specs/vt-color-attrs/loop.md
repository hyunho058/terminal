# loop.md — Loop Contract: vt-color-attrs

> Supervisor document for the vt-color-attrs feature. Approved in one batch by
> /build-order (post-pr2-backlog). Spec: specs/vt-color-attrs/spec.md.

- pre-approved-unattended: yes

## Goal
Parse full SGR color + attributes in `term-core` (256-color `38;5`, truecolor
`38;2` incl. colon T.416 form, bold/dim/italic/underline/reverse + their clears,
`39`/`49` default-reset) and render them; keep `Cell`'s `PartialEq` DERIVED and all
attributes inside `Cell` so the sibling idle-cpu snapshot cache stays correct. Must
not violate specs/vt-color-attrs/spec.md (SD1–SD4, D5–D11, R0–R7).

## Scope & non-goals
- In: `crates/term-core/src/parser.rs`, `crates/term-core/src/grid.rs`,
  `crates/renderer/src/lib.rs`, `crates/renderer/src/glyph.rs`,
  `crates/renderer/src/atlas.rs`.
- Out: `tab_bar.rs`, `overlay_layer.rs`, `snapshot.rs`, `terminal_app.rs`,
  everything else. Blink/conceal/strikethrough SGR and per-run ligature reshaping
  are non-goals (Known Gaps).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --check` |
| tests | `cargo test --workspace` (incl. `insta` grid snapshots — accept new snapshots via `/insta-review`) |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| SGR parse coverage | R1–R4 + R7 each has a passing test | `term-core` unit/insta tests: `38;5;n`, `48;5;n`, ramp bands (R1); `38;2;r;g;b`, colon `38:2::r:g:b` skips the T.416 slot (R2.3); attr set 1/2/3/4/7 + clears 22/23/24/27 (R3); 39/49 per-channel reset (R4); unknown/truncated code safety (R7) |
| Cell equality intact | `Cell` still `#[derive(PartialEq, Eq)]`; attrs inside `Cell` | a test asserting two snapshots differing only in one cell's attrs compare unequal (R6.1); code review confirms the derive (R6.2) |
| atlas u16 entry preserved | `tab_bar.rs`/`overlay_layer.rs` compile unchanged | `cargo build` (workspace) with those files unedited (R5.5) |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — `Color(u32)` reused (no enum churn, D6); `CellAttrs(u8)` inside `Cell` (D5); `get_or_insert(u16)` kept as a wrapper over `get_or_insert_styled(GlyphKey)` (D10); reverse/dim/underline applied at draw time (D9).
- **Spec alignment** — 256 + truecolor (both forms) + all 5 attrs + 39/49 render correctly; SD2 cache-key-completeness honored.
- **Surface discipline** — no edit outside the 5 declared files; siblings' DrawLayers untouched.
- **Naming / readability** — `CellAttrs` masks and the `GlyphKey` are clear at call sites.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the SGR rewrite, `CellAttrs`/`ansi_256`/pen
changes, the instance-builder attribute reads, the styled atlas path + swash
synthesis, in-scope tests, insta snapshot updates, lint/fmt.

**🛑 STOP — call a human (escalate):**
- Any change that would require editing `tab_bar.rs` or `overlay_layer.rs` (out of
  surface — the atlas `u16` wrapper exists precisely to avoid this).
- A hand-written `Cell` `PartialEq`, or storing a render-affecting attribute outside
  `Cell`/`GridSnapshot` (breaks idle-cpu's cache — SD2).
- Pulling a GPU/winit type into `term-core` (SD1).
- Anything conflicting with this contract's Goal or specs/vt-color-attrs/spec.md.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop
in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- A fresh contract has none. -->
