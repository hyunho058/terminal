# loop.md — Loop Contract: sgr-private-intermediate-guard

> Supervisor contract for the "sgr-private-intermediate-guard" feature. Approved in
> batch by build-order (batch: vt-underline-and-ligature-fixes) for an unattended run.
> Copy of loop's contract template, seeded from specs/sgr-private-intermediate-guard/spec.md.

- pre-approved-unattended: yes
- spec: specs/sgr-private-intermediate-guard/spec.md

## Goal
Guard `csi_dispatch` in `crates/term-core/src/parser.rs` so a CSI carrying a private
intermediate `>`/`<`/`=` (e.g. XTMODKEYS `ESC[>4;2m`) is ignored instead of dispatched
as SGR — so it no longer sets a persistent UNDERLINE (or DIM). Must not violate
`specs/sgr-private-intermediate-guard/spec.md`.

## Scope & non-goals
- In: `crates/term-core/src/parser.rs` — the early guard in `csi_dispatch` + a
  regression test in the file's `#[cfg(test)] mod tests`.
- Out: any other crate or file; implementing XTMODKEYS semantics; reworking
  `handle_sgr` or the SGR code table; the renderer's underline pass.

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
| workspace test failures | `= 0` | `cargo test --workspace` |
| regression test for this bug | `≥ 1` present & green | new test in parser.rs `mod tests`: `sgr_cell(b"\x1b[>4;2mX")` has no `UNDERLINE`/`DIM`, plus positive control `sgr_cell(b"\x1b[4mX")` HAS `UNDERLINE` |
| test coverage % | N/A — no coverage tooling configured in this repo | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — mirrors the existing `?` private-mode guard (`parser.rs:261`):
  an early check before `match action`, keyed on `intermediates.first()`.
- **Naming / readability** — the guard is self-evident at the call site.
- **Spec alignment** — matches `## Decisions` D1/D2/D3: ignores `>`/`<`/`=`, leaves the
  `?` `h`/`l` alt-screen path and all normal SGR intact.
- **Surface containment** — only `crates/term-core/src/parser.rs` changes; `term-core`
  stays GPU-free (no wgpu/winit reference introduced).

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the guard, the regression test, lint/type/format,
in-scope test additions, behaviour-preserving cleanups.

**🛑 STOP — call a human (escalate):**
- DB schema / data-loss / auth / payment / security changes (not expected here).
- Any change needing a **new dependency**, or touching a file **outside**
  `crates/term-core/src/parser.rs`.
- Any change that would break the `?` `h`/`l` guard or over-suppress normal SGR
  (i.e. the positive-control test `ESC[4m` → UNDERLINE would fail).
- Anything conflicting with this contract's Goal or the spec.

## Exit criteria
Exit only when **every** gate passes and the Evidence Report is emitted. Hitting a 🛑
boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- fresh contract — none -->
