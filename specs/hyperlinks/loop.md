# loop.md — Loop Contract

> Supervisor document for the **hyperlinks** feature (⌘-click a URL to open it).
> This is a **verification** run: the implementation (T1–T5) is already complete,
> so iteration 1's WORK step is a no-op and VERIFY is effectively the first action.
> The contract must not be satisfied by changing the spec — `specs/hyperlinks/spec.md`
> is approved and is the fixed bar.

## Goal
Prove the implementation of `specs/hyperlinks/spec.md` satisfies its `## Requirements`
— R0–R6, **56 sub-requirements**, each stated as Given/When/Then — without violating
its `## Constraints`, and report honestly on anything it does not cover.

## Scope & non-goals
- **In:** `crates/term-core/src/links.rs`, `crates/renderer/src/link_layer.rs`,
  `crates/app/src/hyperlink.rs`, and the wiring edits in
  `crates/app/src/terminal_app.rs`, `crates/{term-core,renderer}/src/lib.rs`,
  `crates/term-core/src/snapshot.rs`. Adding missing tests for these is in scope.
- **Out:** the spec itself (approved — a gate failure is fixed in code, never by
  lowering the spec); any feature not named in `specs/hyperlinks/spec.md`; the
  rendered-appearance judgement at real DPI, which the spec assigns to a human
  (`## External Dependencies` → Post-work: `/run-term`).

---

## Gate 1 — Pass/Fail  (100% required, binary)

Mirrors the project's own CI (`.github/workflows`), so passing here means passing CI.

| Check | Command |
|-------|---------|
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| build (release) | `cargo build --workspace --release` |
| typecheck | N/A — `cargo build`/`clippy` are the type check in Rust; no separate command exists |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| Sub-requirements with a mapped, passing test or an explicit non-testable rationale | `= 56 / 56` | Trace each `#### R*` in spec.md to a test name or to a stated human-check/structural rationale |
| Sub-requirements deferred to the human eyeball check | `≤ 1` | Only R3.1's real-DPI appearance may be deferred (spec Post-work) |
| New third-party dependencies | `= 0` | `git diff main -- Cargo.toml crates/*/Cargo.toml Cargo.lock` is empty |
| `unwrap()`/`expect()` outside `#[cfg(test)]` in new files | `= 0` | scan the three new files above their `#[cfg(test)]` line |
| Existing test files modified | `= 0` | `git diff main --stat` over `crates/**/tests/` and existing `mod tests` |
| Link scans while ⌘ is up, over 200 snapshots | `= 0` | the `streaming_output_with_cmd_up_costs_no_scans` assertion |
| `term-core` GPU/window imports | `= 0` | grep `wgpu|winit|raw-window-handle` in term-core |

## Gate 3 — Qualitative  (score + grounds + action)

Scored by a **separate checker subagent** (maker ≠ checker), given only the diff,
this contract, and the spec.

- **Spec alignment** — does the code do what D1–D25 and R0–R6 actually say, including
  the reviewer-carried items (pointer-inside gate, non-empty authority, one span,
  underline-only visual, the R4.8 assertion)?
- **The D19/D20 invariant** — is "what is underlined is exactly what opens" true in the
  code at the moment of the open, not merely by construction?
- **Architecture fit** — dependency direction (`common ← term-core ← renderer ← app`),
  the `PreeditLayer` handle pattern, the `DrawLayer` seam, no GUI crates.
- **Idle-CPU discipline** — are the scan triggers genuinely edge-triggered, and does the
  hover redraw avoid `snapshot_dirty`?
- **Naming / readability** — does the new code read like the surrounding code?

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact, a comparison>
  action: <change that raises it | "none — meets bar" + why>
```

Bar: every item ≥ 7/10, and no item may pass on a bare score.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/format/type errors, missing tests for the
in-scope code above, doc/comment corrections, local renames, behaviour-preserving
refactors that keep every Gate-1 command green.

**🛑 STOP — call a human (escalate):**
- DB schema changes · data-loss-capable migrations · auth/permission changes ·
  payment/secrets/crypto changes (none of these exist in this feature — listed so the
  fence is unambiguous)
- **Anything conflicting with `specs/hyperlinks/spec.md`** — in particular, weakening
  D2's scheme allow-list, D3's authority rejection, or the D19/D20 open-what-is-lit
  invariant to make a gate pass
- Adding a third-party dependency (the spec forbids it: D16, R5.3)
- Editing the spec's Requirements to match the implementation

## Exit criteria
Every gate passes **and** the Evidence Report is emitted. A 🛑 boundary ends the loop
in escalation, not completion. R3.1's real-DPI appearance is reported as a human
check, never as a passed gate.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Each lesson is a mistake this loop already made, hardened so it can't recur.
> Phase 0 loads `status: active` lessons. The human owns this section.

### L1 — Validate over the untrimmed run, not the trimmed candidate
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "When a rule trims or truncates a candidate, verify that no validation downstream reads the trimmed text where the spec defines the check over the untrimmed one. Name the slice each check reads."
- note: `is_valid` asked "did the hostile cut land inside the authority?" of the trimmed URL, so D4's punctuation trim ate the very `?` that had closed the authority and `https://example.com?│` was rejected outright — a false negative on the exact shape D3's own example is about.

### L2 — One arming path per guard flag, and only the trigger with first-hand evidence
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "For every boolean that guards a gesture or a side effect: does it have exactly one setter, is that setter the trigger carrying first-hand evidence for what the flag asserts, and can any other path repopulate the state it protects?"
- note: three separate defects from one flag — `on_edge` re-armed from a snapshot edge (undoing the focus-loss disarm), `CursorLeft` shared the disarm (killing ⌘-hover on re-entry), and `resolve` refilled `last_cell` from a stale `cursor_cell()` while the pointer was outside (the same dead window, one event later).

