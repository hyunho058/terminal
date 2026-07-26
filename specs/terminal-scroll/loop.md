# loop.md — Loop Contract

> Supervisor document for **terminal-scroll** (wiring the mouse wheel to
> `term-core`'s scrollback viewport). Verification gate for an implementation that
> is **already complete** — the loop starts by verifying, not by redoing work.

## Goal
Prove the shipped implementation satisfies every requirement and constraint in
`specs/terminal-scroll/spec.md` (12 requirements / 44 GWT sub-requirements, C1–C8),
without violating that spec or the project's hard rules
(`.claude/rules/dependency-direction.md`, `no-gui-crates`, `macos-only`,
`zsh-local-only`).

## Scope & non-goals
- **In:** `crates/app/src/input.rs`, `crates/app/src/seam.rs`,
  `crates/app/src/terminal_app.rs` — plus tests for code already in scope.
- **Out:** `crates/term-core`, `crates/renderer`, `crates/common` (C1 — these must
  stay byte-identical); `crates/app/src/clipboard.rs` and every other registered
  handler (C7); the spec itself (changing the bar to fit the code is the fence).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| typecheck | N/A — `cargo build`/`clippy` typecheck; there is no separate command in a Rust workspace |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| core-crate blast radius | `== 0` lines changed | `git diff --stat crates/term-core crates/renderer crates/common` (C1, R11.4) |
| sibling-handler blast radius | `== 0` lines changed | `git diff --stat crates/app/src/clipboard.rs` (C7, R11.3) |
| new-dependency count | `== 0` | `git diff crates/*/Cargo.toml Cargo.toml` (C4) |
| `unwrap()` outside tests | `== 0` | grep the three changed files outside `#[cfg(test)]` (C6) |
| `#[cfg(target_os` added | `== 0` | grep the diff (C4, `macos-only` rule) |
| sub-requirements with located evidence | `== 44 / 44` | each of R0–R11's sub-reqs mapped to a test name or a `file:line` in the dispatch path |
| app test suite | all pass, `≥ 169` | `cargo test -p app` |

> Performance numbers (120fps, ≤8ms input→screen) are deliberately **not** gates
> here: `docs/domain.md` invariant 4 and spec SD7 state they remain goals until the
> `performance-harness` feature confirms them.

## Gate 3 — Qualitative  (score + grounds + action)

Scored by a **separate checker subagent** (maker ≠ checker). Items the checker
cannot ground from the diff + spec are marked `self-graded`.

- **Event-loop requirement fidelity** — R0, R1.1, R4, R7, R9.4, R9.5 live in the
  winit dispatch where no unit test can reach them. Does the code actually do what
  each GWT says, read line by line?
- **Spec alignment** — do the 12 implemented behaviours match the approved
  decisions D1–D12, including the ones that were adversarially revised (D3's
  reset rule, D4's write boundary, D6's latch, D12's pre-drain sample)?
- **Architecture fit** — does it match the codebase's idiom (pure logic in
  `input.rs`, thin dispatch in the arm, decisions referenced in doc comments,
  `redraw()` vs bare `request_redraw()`)?
- **Test honesty** — do the new tests assert the behaviour the requirement
  describes, or do they assert something weaker that would pass anyway?
- **T5 deviation judgment** — the spec's T5 named `crates/term-core/tests/` as a
  touched path, but C1/R11.4 require that crate's diff to be empty. The engine
  contract tests were placed in `crates/app` instead. Is that the right call, and
  is it recorded honestly?

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/format/type errors, missing tests for code
already in scope, doc/comment corrections, local renames, behaviour-preserving
refactors that keep Gate 1 green.

**🛑 STOP — call a human (escalate):**
- DB schema changes · data-loss-capable migrations · auth/permission changes ·
  payment/secret/crypto changes (none of these exist in this feature, but the fence
  stands).
- **Any edit to `crates/term-core`, `crates/renderer`, or `crates/common`** — that
  is C1 and the `dependency-direction` rule; "the fix needs a term-core change" is
  an escalation, not an auto-fix.
- **Any change that conflicts with the approved spec** — including relaxing a
  requirement, weakening a GWT, or editing `spec.md` to match the code.
- Adding a dependency, a GUI crate, or a `#[cfg(target_os)]` branch.

## Lessons

### L1 — per-tab actions must carry their tab id
- cause: escalation
- source: human-approved
- form: gate3-item
- status: active
- check: "Does every deferred, latched, or cross-tab action carry the `TabId` it belongs to, rather than being replayed on whichever tab happens to be active?"
- note: the drag latch stored bare bools and replayed them on `tabs.active()`, so a background tab's deferred snap was spent on the dragged tab — the active tab was yanked to live having earned nothing while the real one drifted past `SNAP_FLOOR` into the permanently-disarmed state the latch exists to prevent. Fixing it surfaced the same class again one layer down (a floor re-tested on release against the *dragged* tab's offset), which is why this is stated over "every deferred action", not just the latch.
