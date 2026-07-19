# loop.md — Loop Contract: ligatures-default-off

> Supervisor contract for the "ligatures-default-off" feature. Approved in batch by
> build-order (batch: vt-underline-and-ligature-fixes) for an unattended run. Copy of
> loop's contract template, seeded from specs/ligatures-default-off/spec.md.

- pre-approved-unattended: yes
- spec: specs/ligatures-default-off/spec.md

## Goal
Flip `FontConfig::default().ligatures` from `true` to `false` in
`crates/common/src/config.rs`, so the default (Menlo) terminal renders words without
the `fi` ghost space — while an explicit `font.ligatures = true` in a user config still
enables ligatures. Must not violate `specs/ligatures-default-off/spec.md`.

## Scope & non-goals
- In: `crates/common/src/config.rs` — the one-field default flip + regression tests in
  the file's `#[cfg(test)] mod tests`.
- Out: any other crate or file; fixing the renderer's lead-cell ligature draw; changing
  the default font family; removing ligature support.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| typecheck | N/A — Rust; type-checking is performed by `cargo build` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| clippy warnings | `= 0` | clippy exits non-zero under `-D warnings` |
| workspace test failures | `= 0` | `cargo test --workspace` (incl. existing config.rs / watch.rs tests) |
| regression test for this bug | `≥ 1` present & green | new test in config.rs `mod tests`: `FontConfig::default().ligatures == false`, and a TOML with `ligatures = true` still parses to `true` |
| test coverage % | N/A — no coverage tooling configured in this repo | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — a one-field change to the existing `#[serde(default)]
  FontConfig::default()`; no new pattern introduced.
- **Naming / readability** — the flipped default reads correctly against the field's
  doc comment (programming ligatures, opt-in).
- **Spec alignment** — matches `## Decisions` D1/D2: default off, explicit opt-in
  preserved, other defaults untouched.
- **Surface containment** — only `crates/common/src/config.rs` changes; `common` stays
  UI/GPU-free; the renderer is byte-unchanged.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the default flip, the regression tests,
lint/type/format, in-scope test additions.

**🛑 STOP — call a human (escalate):**
- DB schema / data-loss / auth / payment / security changes (not expected here).
- Any change needing a **new dependency**, or touching a file **outside**
  `crates/common/src/config.rs`.
- Any change that would break an existing test (`defaults_are_sane`,
  `partial_toml_fills_defaults`, or the watch.rs default-fill test) — that signals the
  flip disturbed more than the default and must be reviewed.
- Anything conflicting with this contract's Goal or the spec.

## Exit criteria
Exit only when **every** gate passes and the Evidence Report is emitted. Hitting a 🛑
boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- fresh contract — none -->
